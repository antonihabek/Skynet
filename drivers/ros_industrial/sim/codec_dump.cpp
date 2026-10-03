// ROS-Industrial's own `simple_message` library, asked to encode and to decode the messages this driver speaks (DRY-COVERAGE-PLAN.md item 12.E.6).
//
//   codec_dump encode            prints `<name> <hex of the whole frame>` for each message, made by the maker's `toByteArray` and `SmplMsgConnection::sendMsg`'s own steps
//   codec_dump decode <hex>      reads one frame with the maker's `SimpleMessage::init` and the typed message's `unload`, and prints what it found, one `key value` per line
//
// Built by `layout_check.py` and `sim/run-sim.sh` from the maker's own sources (a `git clone` of `ros-industrial/industrial_core`, not vendored here) without ROS: the library compiles its logging out
// when neither `ROS` nor `STDIOLOG` is defined. The build's flags decide what an int and a real are (`-DBYTE_SWAPPING`, `-DFLOAT64`), which is the controller's own choice.
//
// Nothing here is this driver's code. It is the maker's code and a few lines of glue, and the point of it is that the two never share a line.

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include "simple_message/byte_array.h"
#include "simple_message/joint_data.h"
#include "simple_message/joint_feedback.h"
#include "simple_message/joint_traj_pt.h"
#include "simple_message/messages/joint_feedback_message.h"
#include "simple_message/messages/joint_message.h"
#include "simple_message/messages/joint_traj_pt_message.h"
#include "simple_message/messages/robot_status_message.h"
#include "simple_message/ping_message.h"
#include "simple_message/robot_status.h"
#include "simple_message/simple_message.h"

using industrial::byte_array::ByteArray;
using industrial::joint_data::JointData;
using industrial::simple_message::SimpleMessage;
namespace st = industrial::shared_types;

// What `SmplMsgConnection::sendMsg` does: the message's bytes, with their length in front.
static std::string frame_hex(SimpleMessage& message)
{
  ByteArray msg_data, send_buffer;
  message.toByteArray(msg_data);
  send_buffer.load((int)msg_data.getBufferSize());
  send_buffer.load(msg_data);
  std::vector<char> raw;
  send_buffer.copyTo(raw);
  std::string out;
  char two[3];
  for (char c : raw)
  {
    snprintf(two, sizeof two, "%02x", (unsigned char)c);
    out += two;
  }
  return out;
}

static JointData joints(const double (&values)[6])
{
  JointData data;
  for (int i = 0; i < 6; i++)
    data.setJoint(i, (st::shared_real)values[i]);
  return data;
}

static int encode()
{
  const double positions[6] = {0.1, -0.2, 0.3, -0.4, 0.5, -0.6};
  const double velocities[6] = {0.01, 0.02, -0.03, 0.04, -0.05, 0.06};
  const double accelerations[6] = {1.0, -1.0, 0.5, -0.5, 0.25, -0.25};
  // A fresh SimpleMessage for every case: `SimpleMessage::init` with an empty body leaves the body a message already had, so a ping made on a message that carried a status carries the status.

  {
    SimpleMessage message;
    JointData p = joints(positions);
    industrial::joint_traj_pt::JointTrajPt point;
    point.init(3, p, 0.25, 0.5);
    industrial::joint_traj_pt_message::JointTrajPtMessage m;
    m.init(point);
    m.toRequest(message);
    printf("traj_pt_request %s\n", frame_hex(message).c_str());
    m.toReply(message, industrial::simple_message::ReplyTypes::SUCCESS);
    printf("traj_pt_reply_success %s\n", frame_hex(message).c_str());
    m.toReply(message, industrial::simple_message::ReplyTypes::FAILURE);
    printf("traj_pt_reply_failure %s\n", frame_hex(message).c_str());
  }
  {
    SimpleMessage message;
    JointData p = joints(positions);
    industrial::joint_traj_pt::JointTrajPt point;
    point.init(industrial::joint_traj_pt::SpecialSeqValues::STOP_TRAJECTORY, p, 0.0, 0.0);
    industrial::joint_traj_pt_message::JointTrajPtMessage m;
    m.init(point);
    m.toRequest(message);
    printf("traj_pt_stop %s\n", frame_hex(message).c_str());
  }
  {
    SimpleMessage message;
    JointData p = joints(positions), v = joints(velocities), a = joints(accelerations);
    industrial::joint_feedback::JointFeedback feedback;
    feedback.init(7, industrial::joint_feedback::ValidFieldTypes::TIME | industrial::joint_feedback::ValidFieldTypes::POSITION | industrial::joint_feedback::ValidFieldTypes::VELOCITY,
                  12.5, p, v, a);
    industrial::joint_feedback_message::JointFeedbackMessage m;
    m.init(feedback);
    m.toTopic(message);
    printf("feedback %s\n", frame_hex(message).c_str());
  }
  {
    SimpleMessage message;
    JointData p = joints(positions);
    industrial::joint_message::JointMessage m;
    m.init(0, p);
    m.toTopic(message);
    printf("joint %s\n", frame_hex(message).c_str());
  }
  {
    SimpleMessage message;
    industrial::robot_status::RobotStatus status;
    status.init(industrial::robot_status::TriState(1), industrial::robot_status::TriState(0), 4021, industrial::robot_status::TriState(0),
                industrial::robot_status::TriState(1), industrial::robot_status::RobotMode(2), industrial::robot_status::TriState(1));
    industrial::robot_status_message::RobotStatusMessage m;
    m.init(status);
    m.toTopic(message);
    printf("status %s\n", frame_hex(message).c_str());
  }
  {
    SimpleMessage message;
    industrial::ping_message::PingMessage m;
    m.init();
    m.toRequest(message);
    printf("ping_request %s\n", frame_hex(message).c_str());
    m.toReply(message, industrial::simple_message::ReplyTypes::SUCCESS);
    printf("ping_reply %s\n", frame_hex(message).c_str());
  }
  return 0;
}

static bool from_hex(const char* text, std::vector<char>& out)
{
  size_t n = strlen(text);
  if (n % 2)
    return false;
  for (size_t i = 0; i < n; i += 2)
  {
    unsigned value;
    if (sscanf(text + i, "%2x", &value) != 1)
      return false;
    out.push_back((char)value);
  }
  return true;
}

static void print_joints(const char* name, JointData& data, int count)
{
  printf("%s", name);
  for (int i = 0; i < count; i++)
    printf(" %.17g", (double)data.getJoint(i));
  printf("\n");
}

// A whole frame in, including its length prefix, as `SmplMsgConnection::receiveMsg` takes one: the length first, then a message of that many bytes.
static int decode(const char* text)
{
  std::vector<char> raw;
  if (!from_hex(text, raw) || raw.size() < sizeof(int))
  {
    printf("error not hex\n");
    return 1;
  }
  ByteArray length_buffer, msg_buffer;
  length_buffer.init(raw.data(), (st::shared_int)sizeof(int));
  int length = 0;
  if (!length_buffer.unload(length))
  {
    printf("error length\n");
    return 1;
  }
  printf("length %d\n", length);
  if (length < 0 || (size_t)length + sizeof(int) != raw.size())
  {
    printf("error length_mismatch\n");
    return 1;
  }
  msg_buffer.init(raw.data() + sizeof(int), length);
  SimpleMessage message;
  if (!message.init(msg_buffer))
  {
    printf("error invalid_message\n");
    return 1;
  }
  printf("type %d\ncomm %d\nreply %d\n", message.getMessageType(), message.getCommType(), message.getReplyCode());
  ByteArray body;
  body.copyFrom(message.getData());
  switch (message.getMessageType())
  {
    case industrial::simple_message::StandardMsgTypes::JOINT_TRAJ_PT:
    {
      industrial::joint_traj_pt::JointTrajPt point;
      if (!point.unload(&body))
      {
        printf("error body\n");
        return 1;
      }
      JointData p;
      point.getJointPosition(p);
      printf("seq %d\n", point.getSequence());
      print_joints("joints", p, 6);
      printf("velocity %.17g\nduration %.17g\n", (double)point.getVelocity(), (double)point.getDuration());
      break;
    }
    case industrial::simple_message::StandardMsgTypes::JOINT_FEEDBACK:
    {
      industrial::joint_feedback::JointFeedback feedback;
      if (!feedback.unload(&body))
      {
        printf("error body\n");
        return 1;
      }
      JointData p, v, a;
      st::shared_real time = 0;
      // The maker's class keeps its valid-fields mask private and answers per field, so the mask is rebuilt from the answers.
      int valid = 0;
      valid |= feedback.getTime(time) ? industrial::joint_feedback::ValidFieldTypes::TIME : 0;
      valid |= feedback.getPositions(p) ? industrial::joint_feedback::ValidFieldTypes::POSITION : 0;
      valid |= feedback.getVelocities(v) ? industrial::joint_feedback::ValidFieldTypes::VELOCITY : 0;
      valid |= feedback.getAccelerations(a) ? industrial::joint_feedback::ValidFieldTypes::ACCELERATION : 0;
      printf("robot_id %d\nvalid %d\ntime %.17g\n", feedback.getRobotID(), valid, (double)time);
      print_joints("positions", p, 6);
      print_joints("velocities", v, 6);
      print_joints("accelerations", a, 6);
      break;
    }
    case industrial::simple_message::StandardMsgTypes::STATUS:
    {
      industrial::robot_status::RobotStatus status;
      if (!status.unload(&body))
      {
        printf("error body\n");
        return 1;
      }
      printf("status %d %d %d %d %d %d %d\n", (int)status.getDrivesPowered(), (int)status.getEStopped(), status.getErrorCode(), (int)status.getInError(), (int)status.getInMotion(),
             (int)status.getMode(), (int)status.getMotionPossible());
      break;
    }
    case industrial::simple_message::StandardMsgTypes::JOINT:
    {
      industrial::joint_message::JointMessage m;
      if (!m.init(message))
      {
        printf("error body\n");
        return 1;
      }
      print_joints("joints", m.getJoints(), 6);
      break;
    }
    default:
      printf("body %zu\n", (size_t)body.getBufferSize());
  }
  return 0;
}

int main(int argc, char** argv)
{
  if (argc >= 2 && !strcmp(argv[1], "encode"))
    return encode();
  if (argc >= 3 && !strcmp(argv[1], "decode"))
    return decode(argv[2]);
  fprintf(stderr, "usage: codec_dump encode | codec_dump decode <hex of one frame>\n");
  return 2;
}
