// A controller's ROS-Industrial server, built from ROS-Industrial's own `simple_message` library (DRY-COVERAGE-PLAN.md item 12.E.6): the substrate of `live.py`.
//
//   rosi_server --motion 11000 --state 11002 --control 11099 --log /tmp/rosi.log [--joints 6] [--speed 3.0] [--queue 8] [--feedback-hz 50]
//
// What is the maker's: the sockets (`TcpServer`), the framing, the codecs, the message manager that dispatches a received message to its handler, its ping handler, and the reconnect
// its default comms fault handler does when a client goes. What is written here: the two handlers' bodies and an arm that moves — a trajectory point is queued if its sequence number is
// the next one, a stop flushes the queue, a point outside a joint's range is acknowledged and then faults the controller — which is the behaviour `controller.py` has and for the same
// reasons: the protocol's header says a server "should" stop on a stop trajectory, and says nothing of a sequence gap or a full queue, so those are this file's choice (FAILURE),
// marked here and in the README, and not any controller's measured behaviour.
//
// A third port takes one-line commands from the test (`fault N`, `clear`, `estop on|off`, `drives on|off`, `positions`, `quit`), so that the live block can make this controller
// misbehave the way real ones do. Everything the motion server receives is appended to the log as one JSON object per line, *decoded by the maker's code*: that is the live block's
// second opinion on what the driver put on the wire.

#include <arpa/inet.h>
#include <signal.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "simple_message/joint_data.h"
#include "simple_message/joint_feedback.h"
#include "simple_message/joint_traj_pt.h"
#include "simple_message/message_handler.h"
#include "simple_message/message_manager.h"
#include "simple_message/messages/joint_feedback_message.h"
#include "simple_message/messages/joint_traj_pt_message.h"
#include "simple_message/messages/robot_status_message.h"
#include "simple_message/robot_status.h"
#include "simple_message/simple_message.h"
#include "simple_message/socket/tcp_server.h"

using namespace industrial;
using simple_message::SimpleMessage;
namespace rs = industrial::robot_status;

// `JointData::MAX_NUM_JOINTS`, which the maker's class keeps private: a message always carries ten joints.
static const int NJ = 10;
static const double RANGE[6][2] = {{-2.9, 2.9}, {-1.9, 1.9}, {-2.5, 2.5}, {-3.1, 3.1}, {-2.0, 2.0}, {-6.2, 6.2}};

static int joint_count = 6;
static double max_speed = 3.0;
static size_t queue_depth = 8;
static double feedback_hz = 50.0;
static FILE* log_file = nullptr;

struct Point
{
  int seq;
  double joints[NJ];
  double velocity, duration;
};

// The arm, and everything about it that a handler and a publisher thread both touch.
static std::mutex arm_mutex;
static double positions[10], velocities[10];
static std::vector<Point> queue;
static bool moving = false;
static double seg_from[10], seg_to[10], seg_started = 0, seg_length = 0;
static int expected_seq = 0;
static int error_code = 0;
static bool e_stopped = false, drives_on = true;
static std::atomic<bool> quitting(false);

static double now()
{
  return std::chrono::duration<double>(std::chrono::steady_clock::now().time_since_epoch()).count();
}

static void log_line(const std::string& line)
{
  if (!log_file)
    return;
  fprintf(log_file, "%s\n", line.c_str());
  fflush(log_file);
}

static void flush_locked()
{
  queue.clear();
  moving = false;
  for (int i = 0; i < 10; i++)
    velocities[i] = 0.0;
  expected_seq = 0;
}

// The arm moves towards each queued point in turn, in a straight line in joint space, taking the longer of the point's duration and what its velocity ratio allows.
static void physics()
{
  double last = now();
  while (!quitting)
  {
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
    std::lock_guard<std::mutex> lock(arm_mutex);
    double t = now();
    if (!moving && !queue.empty())
    {
      Point p = queue.front();
      queue.erase(queue.begin());
      bool inside = true;
      for (int i = 0; i < joint_count; i++)
        if (p.joints[i] < RANGE[i][0] || p.joints[i] > RANGE[i][1])
          inside = false;
      if (!inside)
      {
        error_code = 4021;  // acknowledged already; the fault comes when the controller tries to go there
        flush_locked();
      }
      else
      {
        double travel = 0.0;
        for (int i = 0; i < joint_count; i++)
          travel = std::max(travel, std::fabs(p.joints[i] - positions[i]) / (std::max(p.velocity, 1e-3) * max_speed));
        for (int i = 0; i < 10; i++)
        {
          seg_from[i] = positions[i];
          seg_to[i] = i < joint_count ? p.joints[i] : positions[i];
        }
        seg_length = std::max(p.duration, travel);
        seg_started = t;
        moving = true;
      }
    }
    if (!moving)
    {
      for (int i = 0; i < 10; i++)
        velocities[i] = 0.0;
      last = t;
      continue;
    }
    double done = seg_length > 0 ? std::min(1.0, (t - seg_started) / seg_length) : 1.0;
    double span = std::max(t - last, 1e-3);
    for (int i = 0; i < joint_count; i++)
    {
      double before = positions[i];
      positions[i] = seg_from[i] + (seg_to[i] - seg_from[i]) * done;
      velocities[i] = (positions[i] - before) / span;
    }
    last = t;
    if (done >= 1.0)
      moving = false;
  }
}

static std::string joints_json(const double* v, int count)
{
  std::string out = "[";
  char buf[48];
  for (int i = 0; i < count; i++)
  {
    snprintf(buf, sizeof buf, "%s%.9g", i ? "," : "", v[i]);
    out += buf;
  }
  return out + "]";
}

class TrajPtHandler : public message_handler::MessageHandler
{
public:
  bool init(smpl_msg_connection::SmplMsgConnection* connection)
  {
    return MessageHandler::init(simple_message::StandardMsgTypes::JOINT_TRAJ_PT, connection);
  }

private:
  bool internalCB(SimpleMessage& in)
  {
    joint_traj_pt_message::JointTrajPtMessage message;
    SimpleMessage reply;
    if (!message.init(in))
    {
      message.toReply(reply, simple_message::ReplyTypes::FAILURE);
      return this->getConnection()->sendMsg(reply);
    }
    Point p;
    p.seq = message.point_.getSequence();
    joint_data::JointData data;
    message.point_.getJointPosition(data);
    for (int i = 0; i < NJ; i++)
      p.joints[i] = data.getJoint(i);
    p.velocity = message.point_.getVelocity();
    p.duration = message.point_.getDuration();
    char buf[400];
    snprintf(buf, sizeof buf, "{\"event\":\"point\",\"seq\":%d,\"joints\":%s,\"velocity\":%.9g,\"duration\":%.9g}", p.seq, joints_json(p.joints, joint_count).c_str(), p.velocity, p.duration);
    log_line(buf);

    simple_message::ReplyType result = simple_message::ReplyTypes::SUCCESS;
    {
      std::lock_guard<std::mutex> lock(arm_mutex);
      if (p.seq == joint_traj_pt::SpecialSeqValues::STOP_TRAJECTORY)
      {
        flush_locked();
        log_line("{\"event\":\"stop\"}");
      }
      else if (error_code || e_stopped || !drives_on)
        result = simple_message::ReplyTypes::FAILURE;
      else
      {
        if (p.seq == 0)
        {
          queue.clear();
          moving = false;
          expected_seq = 0;
        }
        if (p.seq != expected_seq || queue.size() >= queue_depth)
          result = simple_message::ReplyTypes::FAILURE;
        else
        {
          queue.push_back(p);
          expected_seq++;
        }
      }
    }
    message.toReply(reply, result);
    return this->getConnection()->sendMsg(reply);
  }
};

static void serve_motion(int port)
{
  tcp_server::TcpServer server;
  message_manager::MessageManager manager;
  TrajPtHandler handler;
  if (!server.init(port))
  {
    fprintf(stderr, "rosi_server: the motion server could not listen on %d\n", port);
    exit(2);
  }
  server.makeConnect();
  manager.init(&server);
  handler.init(&server);
  manager.add(&handler);
  manager.spin();
}

static void serve_state(int port)
{
  tcp_server::TcpServer server;
  if (!server.init(port))
  {
    fprintf(stderr, "rosi_server: the state server could not listen on %d\n", port);
    exit(2);
  }
  double last_status = 0.0;
  server.makeConnect();
  while (!quitting)
  {
    std::this_thread::sleep_for(std::chrono::microseconds((int)(1e6 / feedback_hz)));
    double p[10], v[10], zero[10] = {0};
    bool in_motion, estop, drives;
    int code;
    {
      std::lock_guard<std::mutex> lock(arm_mutex);
      memcpy(p, positions, sizeof p);
      memcpy(v, velocities, sizeof v);
      in_motion = moving || !queue.empty();
      estop = e_stopped;
      drives = drives_on;
      code = error_code;
    }
    joint_data::JointData jp, jv, ja;
    for (int i = 0; i < NJ; i++)
    {
      jp.setJoint(i, p[i]);
      jv.setJoint(i, v[i]);
      ja.setJoint(i, zero[i]);
    }
    joint_feedback::JointFeedback feedback;
    feedback.init(0, joint_feedback::ValidFieldTypes::TIME | joint_feedback::ValidFieldTypes::POSITION | joint_feedback::ValidFieldTypes::VELOCITY, std::fmod(now(), 1e6), jp, jv, ja);
    joint_feedback_message::JointFeedbackMessage fm;
    fm.init(feedback);
    SimpleMessage out;
    fm.toTopic(out);
    bool ok = server.sendMsg(out);
    if (ok && now() - last_status > 0.1)
    {
      last_status = now();
      rs::RobotStatus status;
      status.init(rs::TriState(drives ? 1 : 0), rs::TriState(estop ? 1 : 0), code, rs::TriState(code ? 1 : 0), rs::TriState(in_motion ? 1 : 0), rs::RobotMode(2),
                  rs::TriState((code || estop || !drives) ? 0 : 1));
      robot_status_message::RobotStatusMessage sm;
      sm.init(status);
      SimpleMessage sout;
      sm.toTopic(sout);
      ok = server.sendMsg(sout);
    }
    if (!ok)
    {
      // The client went. Wait for the next one, as the maker's own fault handler does for the motion server.
      server.makeConnect();
    }
  }
}

// The test's own port: one line in, one line out.
static void serve_control(int port)
{
  int listener = socket(AF_INET, SOCK_STREAM, 0);
  int on = 1;
  setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
  sockaddr_in address;
  memset(&address, 0, sizeof address);
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  address.sin_port = htons(port);
  if (bind(listener, (sockaddr*)&address, sizeof address) || listen(listener, 4))
  {
    fprintf(stderr, "rosi_server: the control port %d could not be opened\n", port);
    exit(2);
  }
  while (!quitting)
  {
    int client = accept(listener, nullptr, nullptr);
    if (client < 0)
      continue;
    std::string buffer;
    char chunk[256];
    ssize_t n;
    while ((n = read(client, chunk, sizeof chunk)) > 0)
    {
      buffer.append(chunk, n);
      size_t eol;
      while ((eol = buffer.find('\n')) != std::string::npos)
      {
        std::string line = buffer.substr(0, eol);
        buffer.erase(0, eol + 1);
        std::string reply = "ok\n";
        {
          std::lock_guard<std::mutex> lock(arm_mutex);
          if (line.compare(0, 6, "fault ") == 0)
          {
            error_code = atoi(line.c_str() + 6);
            flush_locked();
          }
          else if (line == "clear")
            error_code = 0;
          else if (line == "estop on")
          {
            e_stopped = true;
            flush_locked();
          }
          else if (line == "estop off")
            e_stopped = false;
          else if (line == "drives off")
          {
            drives_on = false;
            flush_locked();
          }
          else if (line == "drives on")
            drives_on = true;
          else if (line.compare(0, 6, "place ") == 0)
          {
            double v[10] = {0};
            sscanf(line.c_str() + 6, "%lf %lf %lf %lf %lf %lf", v, v + 1, v + 2, v + 3, v + 4, v + 5);
            for (int i = 0; i < 10; i++)
              positions[i] = v[i];
          }
          else if (line == "positions")
          {
            char buf[300];
            snprintf(buf, sizeof buf, "%s %d\n", joints_json(positions, joint_count).c_str(), (moving || !queue.empty()) ? 1 : 0);
            reply = buf;
          }
          else if (line == "quit")
            quitting = true;
          else
            reply = "unknown\n";
        }
        if (write(client, reply.c_str(), reply.size()) < 0)
          break;
      }
    }
    close(client);
  }
}

int main(int argc, char** argv)
{
  int motion = 11000, state = 11002, control = 11099;
  const char* log_path = nullptr;
  for (int i = 1; i + 1 < argc; i += 2)
  {
    std::string key = argv[i];
    if (key == "--motion")
      motion = atoi(argv[i + 1]);
    else if (key == "--state")
      state = atoi(argv[i + 1]);
    else if (key == "--control")
      control = atoi(argv[i + 1]);
    else if (key == "--log")
      log_path = argv[i + 1];
    else if (key == "--joints")
      joint_count = atoi(argv[i + 1]);
    else if (key == "--speed")
      max_speed = atof(argv[i + 1]);
    else if (key == "--queue")
      queue_depth = (size_t)atoi(argv[i + 1]);
    else if (key == "--feedback-hz")
      feedback_hz = atof(argv[i + 1]);
    else
    {
      fprintf(stderr, "rosi_server: unknown option %s\n", argv[i]);
      return 2;
    }
  }
  // The maker's socket sends with plain `send()` and a Linux process dies of SIGPIPE when it writes to a client that has gone, which is the first thing a controller's server meets
  // when the driver closes a session (found by the live block: the server exited with status 141 between two sessions). A controller's RTOS has no such signal; this process says so.
  signal(SIGPIPE, SIG_IGN);
  if (log_path)
    log_file = fopen(log_path, "w");
  std::thread(physics).detach();
  std::thread(serve_state, state).detach();
  std::thread(serve_control, control).detach();
  std::thread(serve_motion, motion).detach();
  while (!quitting)
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
  return 0;
}
