/*
 * skynet-x1's own suite: the discovery, the impedance mapping, the clamp and every refusal.
 *
 * Linked against `x1_bridge.cpp` rather than dlopening it, so a refusal can be asserted
 * directly through the vtable. It runs against `fake_x1` on DDS domain 42 — the vendor's
 * message types on both sides — which the caller starts; `run-check.sh` is what starts it.
 * The sim is the motion substrate and deliberately not this suite's: a clamping check should
 * not depend on MuJoCo stepping, and an estop check should not wait on a render thread.
 *
 * What makes this worth more than a list of return codes is the **substrate's own report**,
 * the unitree shape: the driver's job here is not only to return SKR_OK but to name every
 * joint on the wire (the sim's `WriteMotorCmd` defaults a missing joint to index 0 — a
 * partial command drives silent joints from the first joint's setpoint), to clamp to the
 * MJCF, and to hold masked joints rather than omit them. Those are things only the thing on
 * the other end can see, so the fake counts them and `run-check.sh` reads the count. This
 * suite is the other half: a spy on `/joint_cmd` reads the exact message the driver
 * published — the fake says *whether* something was wrong, and this says *what was in the
 * message*.
 *
 * Usage: selftest <domain> <model.xml>
 */

#include "skynet_rhal.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <signal.h>
#include <sys/types.h>
#include <unistd.h>

#include <utility>
#include <vector>

#include "rclcpp/rclcpp.hpp"
#include "sensor_msgs/msg/joint_state.hpp"
#include "my_ros2_proto/msg/joint_command.hpp"

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

/* What the driver says on stderr while the suite runs something: descriptor 2 is pointed at a temporary file until `finish()`, which puts it back and returns what was written.
 * Two refusals can share a code and not a reason, and the reason is what the operator acts on. Always finished on the path that started it. */
class StderrCapture {
 public:
  StderrCapture() {
    std::fflush(stderr);
    saved_ = ::dup(STDERR_FILENO);
    file_ = std::tmpfile();
    ::dup2(::fileno(file_), STDERR_FILENO);
  }

  std::string finish() {
    std::fflush(stderr);
    ::dup2(saved_, STDERR_FILENO);
    ::close(saved_);
    std::string text;
    std::rewind(file_);
    char chunk[4096];
    std::size_t got = 0;
    while ((got = std::fread(chunk, 1, sizeof chunk, file_)) > 0) text.append(chunk, got);
    std::fclose(file_);
    return text;
  }

 private:
  int saved_ = -1;
  std::FILE *file_ = nullptr;
};

/* The fake's four joints, in the order it publishes them — real X1 names from the sim's MJCF,
 * which is also what the driver discovers and clamps to. */
const char *const kJoints[] = {
    "lumbar_yaw_joint",
    "left_shoulder_pitch_joint",
    "left_elbow_pitch_joint",
    "left_knee_pitch_joint",
};
const double kStarts[] = {0.15, -0.4, 0.9, -0.25};
const double kVelocities[] = {0.01, -0.02, 0.03, -0.04};
const double kEfforts[] = {0.5, -1.0, 1.5, -2.0};

int CHECKS = 0;
std::vector<std::string> FAILURES;

void check(bool held, const std::string &what) {
  CHECKS += 1;
  if (held) {
    std::printf("  ok    %s\n", what.c_str());
  } else {
    FAILURES.push_back(what);
    std::printf("  FAIL  %s\n", what.c_str());
  }
}

void section(const char *title) { std::printf("\n%s\n", title); }

/* A listener on a topic, so the suite can read the exact message the driver published rather
 * than infer it from a return code. One background thread spins; the suite's waits poll the
 * counter with a deadline, the way discovery demands. */
template <typename Msg>
class Spy {
 public:
  Spy(const std::string &topic) {
    node_ = std::make_shared<rclcpp::Node>("x1_selftest_spy");
    subscriber_ = node_->create_subscription<Msg>(
        topic, 10, [this](typename Msg::SharedPtr msg) {
          std::lock_guard<std::mutex> guard(lock_);
          last_ = *msg;
          seen_ += 1;
        });
    thread_ = std::thread([this] {
      rclcpp::executors::SingleThreadedExecutor executor;
      executor.add_node(node_);
      while (rclcpp::ok() && spinning_) executor.spin_some(std::chrono::milliseconds(5));
    });
  }

  ~Spy() {
    spinning_ = false;
    if (thread_.joinable()) thread_.join();
  }

  bool wait_for(std::uint64_t count, double seconds = 5.0) {
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::duration<double>(seconds);
    while (std::chrono::steady_clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (seen_ >= count) return true;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    return false;
  }

  Msg last() {
    std::lock_guard<std::mutex> guard(lock_);
    return last_;
  }

  std::uint64_t seen() {
    std::lock_guard<std::mutex> guard(lock_);
    return seen_;
  }

 private:
  std::mutex lock_;
  std::uint64_t seen_ = 0;
  Msg last_{};
  std::shared_ptr<rclcpp::Node> node_;
  typename rclcpp::Subscription<Msg>::SharedPtr subscriber_;
  std::atomic<bool> spinning_{true};
  std::thread thread_;
};

/* The sim's own travel for one joint, scanned out of the MJCF the same bounded way the driver
 * does — the range on the joint's own element, so a later joint's range cannot match. The suite
 * reads the file rather than hardcoding figures, so a vendor model revision moves both sides
 * together; what is pinned is that the MJCF reached the descriptor and the wire, not the
 * numbers themselves. */
bool mjcf_range(const std::string &xml, const std::string &joint, double *low, double *high) {
  const std::string needle = "name=\"" + joint + "\"";
  const std::size_t at = xml.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t end = xml.find('>', at);
  const std::size_t range_at = xml.find("range=\"", at);
  if (range_at == std::string::npos || range_at > end) return false;
  const char *start = xml.c_str() + range_at + 7;
  char *mid = nullptr;
  *low = std::strtod(start, &mid);
  if (mid == start) return false;
  char *stop = nullptr;
  *high = std::strtod(mid, &stop);
  return stop != mid && *low < *high;
}

struct Buffers {
  float position[64];
  float velocity[64];
  float effort[64];

  skr_state state() {
    skr_state out;
    std::memset(&out, 0, sizeof out);
    // The caller's capacity, exactly as the runtime passes it (F45).
    out.joint_count = 64;
    out.position_rad = position;
    out.velocity_rad_s = velocity;
    out.effort_nm = effort;
    return out;
  }
};

skr_command command(std::uint8_t mode, const float *values, const std::uint8_t *mask,
                    std::uint16_t joints, std::uint8_t flags = 0, const std::uint8_t *modes = nullptr) {
  skr_command cmd;
  std::memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = mode;
  cmd.flags = flags;
  cmd.joint_count = joints;
  cmd.values = values;
  cmd.mask = mask;
  cmd.modes = modes;
  return cmd;
}

bool full_vector(const my_ros2_proto::msg::JointCommand &sent) {
  const std::size_t n = sent.name.size();
  return n == 4 && sent.position.size() == n && sent.velocity.size() == n &&
         sent.effort.size() == n && sent.stiffness.size() == n && sent.damping.size() == n;
}

bool names_are(const my_ros2_proto::msg::JointCommand &sent) {
  if (sent.name.size() != 4) return false;
  for (std::size_t i = 0; i < 4; ++i) {
    if (sent.name[i] != kJoints[i]) return false;
  }
  return true;
}

}  // namespace

int main(int argc, char **argv) {
  if (argc != 3) {
    std::fprintf(stderr, "usage: selftest <domain> <model.xml>\n");
    return 2;
  }
  const std::string domain = argv[1];
  const std::string model = argv[2];

  std::ifstream model_file(model);
  if (!model_file.good()) {
    std::fprintf(stderr, "selftest: cannot read %s\n", model.c_str());
    return 2;
  }
  std::ostringstream model_text;
  model_text << model_file.rdbuf();
  const std::string xml = model_text.str();
  double low[4], high[4];
  for (std::size_t i = 0; i < 4; ++i) {
    if (!mjcf_range(xml, kJoints[i], &low[i], &high[i])) {
      std::fprintf(stderr, "selftest: %s has no range for %s\n", model.c_str(), kJoints[i]);
      return 2;
    }
  }

  const skr_driver *driver = skynet_rhal_entry();

  section("refused before DDS is touched");
  {
    /* No rclcpp yet: these return before `rclcpp::init`, so a bad config never joins a domain
     * — and never a wrong one, which is what the domain isolation exists for. */
    skr_ctx *ctx = nullptr;
    check(driver->open("{\"domain\":999}", &ctx) == SKR_EINVAL,
          "a domain DDS does not have is refused before discovery");
    check(driver->open("{\"states\":\"/x\",\"cmd\":\"/x\"}", &ctx) == SKR_EINVAL,
          "one topic for both directions is refused before discovery");
    check(driver->open("{\"model\":\"/no/such/model.xml\"}", &ctx) == SKR_EINVAL,
          "a missing MJCF is refused before discovery — clamping to nothing is not clamping");
    check(driver->open("{\"kp\":-1.0,\"model\":\"/no/such/model.xml\"}", &ctx) == SKR_EINVAL,
          "and a negative gain is refused ahead of the file check");
    /* With a model that exists, so that it is the gain that is refused and not the missing file: the check above returns EINVAL for
     * either, and a driver that stopped checking one of the two gains would pass it (x1-damping-negative-accepted,
     * x1-stiffness-negative-accepted). */
    check(driver->open(("{\"model\":\"" + model + "\",\"kp\":-1.0}").c_str(), &ctx) == SKR_EINVAL,
          "a negative stiffness is refused with a model that exists, so it is the gain and not the file");
    check(driver->open(("{\"model\":\"" + model + "\",\"kd\":-1.0}").c_str(), &ctx) == SKR_EINVAL,
          "and so is a negative damping");
    check(driver->open("{\"domain\":3.5}", &ctx) == SKR_EINVAL,
          "a domain that is not a whole number is refused: 3.5 would be joined as 3");
    check(driver->open("{\"states\":\"\",\"cmd\":\"/x\"}", &ctx) == SKR_EINVAL &&
              driver->open("{\"states\":\"/x\",\"cmd\":\"\"}", &ctx) == SKR_EINVAL,
          "an empty topic name is refused in either direction: the node would come up and talk to nothing");
    check(driver->open(("{\"model\":\"" + model + "\",\"roles\":\"oops\"}").c_str(), &ctx) == SKR_EINVAL,
          "and `roles` that is not an object is refused rather than silently dropped");
    const char *saved_infer = std::getenv("X1_INFER");
    const std::string infer = saved_infer != nullptr ? saved_infer : "";
    ::unsetenv("X1_INFER");
    StderrCapture capture;
    const int without_model = driver->open("{}", &ctx);
    const std::string said = capture.finish();
    check(without_model == SKR_EINVAL, "no `model` and no $X1_INFER is refused rather than moving unclamped");
    /* x1-model-required: a missing file below is refused with the same code, so the code alone cannot tell the refusal that was meant from the one that happened to follow it. */
    check(said.find("`model` names the MJCF") != std::string::npos && said.find("$X1_INFER is unset") != std::string::npos,
          "and it says what it wants — a `model`, or $X1_INFER to default one from — not that a file was missing");
    if (!infer.empty()) ::setenv("X1_INFER", infer.c_str(), 1);
  }

  ::setenv("ROS_DOMAIN_ID", domain.c_str(), 1);
  rclcpp::init(argc, argv);
  Spy<my_ros2_proto::msg::JointCommand> spy("/joint_cmd");

  section("the body, and what it refuses to declare");
  skr_ctx *ctx = nullptr;
  const std::string with_gains = "{\"domain\":" + domain + ",\"model\":\"" + model +
                                 "\",\"kp\":50.0,\"kd\":2.0}";
  check(driver->abi_version == SKYNET_RHAL_ABI_VERSION, "the driver targets this ABI");
  /* The return code, not the handle: the C++ wrapper sets the out-handle unconditionally, so
   * a null check would walk into an unopened driver and read off empty vectors. */
  const int opened = driver->open(with_gains.c_str(), &ctx);
  check(opened == SKR_OK, "open() joins DDS and waits for the first state");
  if (opened != SKR_OK) {
    std::printf("\n  the fake is not publishing; nothing below can run.\n");
    return 1;
  }

  char json[16384];
  const int written = driver->describe(ctx, json, sizeof json);
  check(written > 0, "describe() writes the descriptor");
  const std::string descriptor(json, written > 0 ? written : 0);
  check(descriptor.find("\"robot_slug\":\"agibot-x1\"") != std::string::npos,
        "it names the body: agibot-x1");
  check(descriptor.find("\"dof\":4") != std::string::npos,
        "the joint count is the discovered four — not the twenty-nine the sim would report");
  check(descriptor.find("\"joint_torque_max_nm\":200.0") != std::string::npos,
        "the envelope carries the PowerFlow peak, with the hedge beside it");
  check(descriptor.find("\"watchdog_ms\":100.0") != std::string::npos,
        "the watchdog is driver design, stated as such");
  check(descriptor.find("\"basis\":\"declared\"") != std::string::npos,
        "and states the kind beside the citation (§5.3)");
  check(descriptor.find("\"rate_hz_max\":null") != std::string::npos,
        "no control rate: AgiBot publishes none, and the sim's step rate is not a rating (§63.4)");
  check(descriptor.find("joint_velocity_max_rad_s") == std::string::npos,
        "but no velocity ceiling: AgiBot publishes no joint speed, so joint_ratings stays unmet");
  check(descriptor.find("effort_max_nm") == std::string::npos,
        "and no per-joint effort: the 200 N·m peak is a body figure, not a joint rating");
  bool travel = true;
  for (std::size_t i = 0; i < 4; ++i) {
    char pin[128];
    std::snprintf(pin, sizeof pin, "\"position_min_rad\":%.5f,\"position_max_rad\":%.5f", low[i],
                  high[i]);
    if (descriptor.find(pin) == std::string::npos) travel = false;
  }
  check(travel, "travel comes from the sim's MJCF, pinned on all four joints");
  check(descriptor.find("\"capabilities\":[]") != std::string::npos,
        "no capability is declared for the stop: nothing here is rated");
  check(descriptor.find("\"simulator\":true") != std::string::npos,
        "declared simulated: the default is the safe direction (§35.3)");
  check(descriptor.find("\"base\":\"legged\"") != std::string::npos,
        "the base is legged, so nothing treats it as a wheeled one");
  check(descriptor.find("\"per_joint_modes\":true") != std::string::npos,
        "it declares it reads the per-joint array (§86.1)");
  /* x1-descriptor-modes: every joint takes all three, and the body-wide list says so. */
  check(descriptor.find("\"control_modes\":[\"position\",\"velocity\",\"torque\"]") != std::string::npos &&
            descriptor.find("\"modes\":[\"position\",\"velocity\",\"torque\"]") != std::string::npos,
        "every joint offers position, velocity and torque, and so does the body");

  section("read_state");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.joint_count == 4, "and reports the joints written, not the capacity it was handed");
  /* x1-homing-pending: an X1's actuators report absolute positions, so the positions are trustworthy from the first read. */
  check((state.flags & SKR_HOMING_MASK) == SKR_HOMING_NOT_REQUIRED,
        "homing is reported as not required: a body that needs none must not read as one that is yet to be homed");
  const std::uint32_t first_seq = state.seq;
  bool scripted = true;
  for (std::size_t i = 0; i < 4; ++i) {
    if (std::fabs(state.position_rad[i] - kStarts[i]) > 1e-6) scripted = false;
    if (std::fabs(state.velocity_rad_s[i] - kVelocities[i]) > 1e-6) scripted = false;
    if (std::fabs(state.effort_nm[i] - kEfforts[i]) > 1e-6) scripted = false;
  }
  check(scripted,
        "positions, velocities and efforts are the fake's scripted nonzero values — a driver "
        "writing zeros, or smuggling NaN, would not look correct");
  bool pose_nan = true;
  for (std::size_t i = 0; i < 7; ++i) {
    if (!std::isnan(state.base_pose[i])) pose_nan = false;
  }
  check(pose_nan,
        "the base pose is all NaN — nothing on this wire localises the body, and a driver that "
        "wrote the IMU quaternion into a pose would be claiming a position it does not have");
  bool twist_zero = true;
  for (std::size_t i = 0; i < 6; ++i) {
    if (state.base_twist[i] != 0.0f) twist_zero = false;
  }
  check(twist_zero, "the base twist is zeros beside it");
  std::this_thread::sleep_for(std::chrono::milliseconds(50));
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check(state.seq > first_seq, "the sequence advances between reads");

  section("read_state: the sequence counts samples, and a read waits for one the last read did not return");
  /* F328. The sequence was `++seq_` per read, so a link that had stopped carrying states read as a producer that was alive: §24 treats a
   * repeated `seq` as a stalled producer and the driver never repeated one (the same flaw as Unitree's and Booster's, F277 and F278). The
   * fake publishes every 10 ms, so a read that did not wait would return the sample before it nine times in ten. */
  {
    std::uint64_t last_stamp = state.stamp_ns;
    std::uint32_t last_seq = state.seq;
    std::size_t advanced = 0;
    long slowest_ms = 0;
    for (int i = 0; i < 100; ++i) {
      state = buffers.state();
      const auto begun = std::chrono::steady_clock::now();
      const int rc = driver->read_state(ctx, &state, 0);
      slowest_ms = std::max<long>(slowest_ms, std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - begun).count());
      if (rc == SKR_OK && state.stamp_ns > last_stamp && state.seq != last_seq) advanced += 1;
      last_stamp = state.stamp_ns;
      last_seq = state.seq;
    }
    check(advanced == 100, "a hundred reads in a row each returned a sample the one before did not: the stamp and the sequence both moved on, every time (F328)");
    check(slowest_ms <= 100, "and none waited past the watchdog window the descriptor declares (slowest " + std::to_string(slowest_ms) + " ms)");
  }
  {
    /* A command and the read after it: the read is not the sample it already had. */
    float middle[4];
    std::uint8_t all[4] = {1, 1, 1, 1};
    for (std::size_t i = 0; i < 4; ++i) middle[i] = static_cast<float>((low[i] + high[i]) / 2.0);
    skr_command nudge = command(SKR_MODE_POSITION, middle, all, 4);
    std::size_t followed = 0;
    for (int i = 0; i < 20; ++i) {
      std::this_thread::sleep_for(std::chrono::milliseconds(3));   /* off the fake's 10 ms phase, a different offset each time */
      state = buffers.state();
      driver->read_state(ctx, &state, 0);
      const std::uint64_t stamp_before = state.stamp_ns;
      driver->write_command(ctx, &nudge, 0);
      state = buffers.state();
      driver->read_state(ctx, &state, 0);
      if (state.stamp_ns > stamp_before) followed += 1;
    }
    check(followed == 20, "a read after a command returns a sample that came after the read before it, 20 times in 20");
  }
  {
    /* The producer stopped: the fake is stopped as a process (SIGSTOP), which is a link that carries nothing and is the one case a sequence that counted calls could not tell
     * from a producer that was alive. `run-check.sh` names the process. A read waits the watchdog window for a sample, and then returns the last one again, its stamp and its
     * sequence as they were; and when the producer runs again the sequence moves on. */
    const char *named = std::getenv("X1_FAKE_PID");
    const long fake = named != nullptr ? std::strtol(named, nullptr, 10) : 0;
    check(fake > 1, "X1_FAKE_PID names the fake's process (run-check.sh sets it): without it a stalled producer cannot be made, and the checks below would say nothing");
    if (fake > 1) {
      state = buffers.state();
      driver->read_state(ctx, &state, 0);
      ::kill(static_cast<pid_t>(fake), SIGSTOP);
      std::this_thread::sleep_for(std::chrono::milliseconds(150));   /* a sample published as it stopped has reached the driver by now */
      state = buffers.state();
      driver->read_state(ctx, &state, 0);   /* takes that one, or waits the window out */
      const std::uint64_t held_stamp = state.stamp_ns;
      const std::uint32_t held_seq = state.seq;
      bool held = true;
      long fastest_ms = 1000000;
      long slowest_ms = 0;
      for (int i = 0; i < 3; ++i) {
        state = buffers.state();
        const auto begun = std::chrono::steady_clock::now();
        const int rc = driver->read_state(ctx, &state, 0);
        const long took = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - begun).count();
        fastest_ms = std::min(fastest_ms, took);
        slowest_ms = std::max(slowest_ms, took);
        if (rc != SKR_OK || state.stamp_ns != held_stamp || state.seq != held_seq) held = false;
      }
      ::kill(static_cast<pid_t>(fake), SIGCONT);
      check(held, "with the producer stopped, a read returns the last sample again, with the stamp and the sequence it had: a repeated sequence is the stalled producer (§24, F328)");
      check(fastest_ms >= 90 && slowest_ms <= 250,
            "after waiting the watchdog window for a sample that did not come (" + std::to_string(fastest_ms) + " to " + std::to_string(slowest_ms) + " ms)");
      bool resumed = false;
      for (int i = 0; i < 30 && !resumed; ++i) {
        state = buffers.state();
        if (driver->read_state(ctx, &state, 0) == SKR_OK && state.seq != held_seq && state.stamp_ns > held_stamp) resumed = true;
      }
      check(resumed, "and when the producer runs again the sequence and the stamp move on");
    }
  }

  section("the mode mapping, read off the wire");
  /* Mid-travel plus a distinct nudge per joint, so the placement read below can tell joint 2's
   * setpoint from joint 3's — and mid-travel so the clamp has nothing to do yet. The elbow and
   * knee ranges start at 0, which is why the nudge stays small. */
  float values[4];
  std::uint8_t mask[4] = {1, 1, 1, 1};
  for (std::size_t i = 0; i < 4; ++i) {
    values[i] = static_cast<float>((low[i] + high[i]) / 2.0 + 0.05 * (i + 1));
  }
  std::uint64_t before = spy.seen();
  skr_command cmd = command(SKR_MODE_POSITION, values, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a well-formed position command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  auto sent = spy.last();
  check(full_vector(sent),
        "every array is full length — the sim indexes position[0] of a short command");
  check(names_are(sent),
        "every joint is named, in discovery order — a missing joint would drive from index 0");
  bool placed = true;
  for (std::size_t i = 0; i < 4; ++i) {
    if (std::fabs(sent.position[i] - values[i]) >= 1e-6) placed = false;
  }
  check(placed, "each setpoint landed on its named joint");
  check(sent.stiffness[0] == 50.0f && sent.damping[0] == 2.0f,
        "with the operator's `kp` and `kd`, not a default");
  bool frame_clean = true;
  for (std::size_t i = 0; i < 4; ++i) {
    if (sent.velocity[i] != 0.0 || sent.effort[i] != 0.0) frame_clean = false;
  }
  check(frame_clean, "and the velocity and effort lanes are zero beside them");

  before = spy.seen();
  cmd = command(SKR_MODE_VELOCITY, values, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a velocity command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  bool vel_placed = true;
  for (std::size_t i = 0; i < 4; ++i) {
    if (std::fabs(sent.velocity[i] - values[i]) >= 1e-6) vel_placed = false;
  }
  check(vel_placed, "the setpoint is in the velocity lane");
  check(sent.stiffness[0] == 0.0 && sent.damping[0] == 2.0f,
        "with damping and no stiffness — a velocity frame, not a position one wearing its gains");

  before = spy.seen();
  cmd = command(SKR_MODE_TORQUE, values, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a torque command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  bool tau_placed = true;
  for (std::size_t i = 0; i < 4; ++i) {
    if (std::fabs(sent.effort[i] - values[i]) >= 1e-6) tau_placed = false;
  }
  check(tau_placed, "the setpoint is in the effort lane");
  bool tau_clean = true;
  for (std::size_t i = 0; i < 4; ++i) {
    if (sent.position[i] != 0.0 || sent.velocity[i] != 0.0 || sent.stiffness[i] != 0.0 ||
        sent.damping[i] != 0.0) {
      tau_clean = false;
    }
  }
  check(tau_clean,
        "position, velocity and both gains are zero — the vendor's own RL shape, torque alone");

  section("§86.1, one joint in each mode");
  /* Whole-body position, but joint 1 takes velocity and joint 2 torque; joint 3 inherits. The
   * driver offers all three modes on every joint, so there is no per-joint refusal to probe —
   * only the mapping. */
  std::uint8_t modes[4] = {SKR_JMODE_POSITION, SKR_JMODE_VELOCITY, SKR_JMODE_TORQUE,
                           SKR_JMODE_INHERIT};
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, mask, 4, SKR_CMD_PER_JOINT_MODES, modes);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a mixed-mode command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  check(std::fabs(sent.position[0] - values[0]) < 1e-6 && sent.stiffness[0] == 50.0f,
        "joint 0 took position, with the gain");
  check(std::fabs(sent.velocity[1] - values[1]) < 1e-6 && sent.stiffness[1] == 0.0 &&
            sent.damping[1] == 2.0f,
        "joint 1 took velocity, damped and unstiffened");
  check(std::fabs(sent.effort[2] - values[2]) < 1e-6 && sent.position[2] == 0.0 &&
            sent.stiffness[2] == 0.0,
        "joint 2 took torque, gains zero");
  check(std::fabs(sent.position[3] - values[3]) < 1e-6 && sent.stiffness[3] == 50.0f,
        "joint 3 inherited the body's position");

  section("§24's mask, on an impedance drive");
  /* The hold target is the last known position, which is what the fake last echoed — so the
   * suite waits for the echo of the mixed command above before masking, or the hold target is
   * a race. */
  {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    for (;;) {
      skr_state echo = buffers.state();
      driver->read_state(ctx, &echo, 0);
      bool settled = true;
      for (std::size_t i = 0; i < 4; ++i) {
        /* Joints 1 and 2 carried zeros in the position lane of the mixed command — that is
         * what the fake echoed, and that is what a hold holds. */
        const float want = (i == 0 || i == 3) ? values[i] : 0.0f;
        if (std::fabs(echo.position_rad[i] - want) > 1e-6) settled = false;
      }
      if (settled) break;
      if (std::chrono::steady_clock::now() > deadline) break;
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
  }
  float held[4] = {values[0], 0.0f, 0.0f, values[3]};
  float stepped[4];
  for (std::size_t i = 0; i < 4; ++i) {
    stepped[i] = static_cast<float>((low[i] + high[i]) / 2.0 - 0.05 * (i + 1));
  }
  std::uint8_t partial[4] = {1, 0, 1, 0};
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, stepped, partial, 4);
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  sent = spy.last();
  check(full_vector(sent) && names_are(sent),
        "a masked command still names every joint — omission would drive from index 0");
  check(std::fabs(sent.position[0] - stepped[0]) < 1e-6,
        "an unmasked joint carries the new setpoint");
  check(std::fabs(sent.position[1] - held[1]) < 1e-6 && sent.stiffness[1] == 50.0f,
        "a masked joint with no position setpoint (joint 1 was last in velocity) is held where "
        "it was measured, with the operator's gains");
  check(std::fabs(sent.position[3] - values[3]) < 1e-6 && sent.stiffness[3] == 50.0f,
        "a masked joint with one is held at its last position setpoint — §24's 'current "
        "setpoint'");

  /* The kernel's hold frame after a position command: every joint masked, the values lane all
   * zeros. Each joint is held at the setpoint the command left, not at the zeros the frame
   * carries. (Latched rather than re-read is F121's other half; this fake echoes every position
   * it is sent, so the two readings agree here, and the vendor's sim is where they part.) */
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, mask, 4);
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  float zeros[4] = {0.0f, 0.0f, 0.0f, 0.0f};
  std::uint8_t none[4] = {0, 0, 0, 0};
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, zeros, none, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK,
        "the kernel's hold frame — every joint masked, values irrelevant — is accepted");
  spy.wait_for(before + 1);
  sent = spy.last();
  bool at_setpoint = full_vector(sent);
  for (std::size_t i = 0; i < 4 && at_setpoint; ++i) {
    if (std::fabs(sent.position[i] - values[i]) >= 1e-6 || sent.stiffness[i] != 50.0f) {
      at_setpoint = false;
    }
  }
  check(at_setpoint, "and holds every joint at the last position setpoint, stiffly");

  section("the clamp");
  /* Past both ends on alternating joints: below low on 0 and 2, above high on 1 and 3. */
  float wild[4] = {
      static_cast<float>(low[0] - 1.0), static_cast<float>(high[1] + 1.0),
      static_cast<float>(low[2] - 1.0), static_cast<float>(high[3] + 1.0),
  };
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, wild, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK,
        "an out-of-travel command is accepted — and clamped, not refused");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  check(std::fabs(sent.position[0] - low[0]) < 1e-6 &&
            std::fabs(sent.position[2] - low[2]) < 1e-6,
        "below-low targets arrive at the MJCF low");
  check(std::fabs(sent.position[1] - high[1]) < 1e-6 &&
            std::fabs(sent.position[3] - high[3]) < 1e-6,
        "above-high targets arrive at the MJCF high");

  section("refusals");
  cmd = command(SKR_MODE_POSITION, values, mask, 3);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a three-joint command to a four-joint body is refused");
  cmd = command(200, values, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a mode this ABI does not define is malformed, not merely refused");
  std::uint8_t raw[4] = {SKR_MODE_TORQUE, SKR_MODE_TORQUE, SKR_MODE_TORQUE, SKR_MODE_TORQUE};
  cmd = command(SKR_MODE_POSITION, values, mask, 4, SKR_CMD_PER_JOINT_MODES, raw);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a bare SKR_MODE_* in the per-joint array is the transcription error the offset exists "
        "to catch");
  std::uint8_t all_held[4] = {0, 0, 0, 0};
  cmd = command(SKR_MODE_POSITION, values, all_held, 4, SKR_CMD_PER_JOINT_MODES, raw);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "…even when every joint is held: the mask decides which joints move, not which entries "
        "are well-formed");
  std::uint8_t unknown[4] = {SKR_JMODE_INHERIT, 0x13, SKR_JMODE_INHERIT, SKR_JMODE_INHERIT};
  cmd = command(SKR_MODE_POSITION, values, mask, 4, SKR_CMD_PER_JOINT_MODES, unknown);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a per-joint value the vocabulary does not define is refused");

  /* A setpoint that is not a number never reaches the sim (F282). The clamp does not catch one — `std::max(NaN, low)` is NaN — so
   * a NaN position or velocity went to the wire as it was and a NaN torque went to MuJoCo's actuator as a force. Refused on a
   * joint other than the first and in each of the three modes, because the lane a value lands in is chosen per joint. */
  for (const std::uint8_t mode : {SKR_MODE_POSITION, SKR_MODE_VELOCITY, SKR_MODE_TORQUE}) {
    for (const float bad : {std::nanf(""), INFINITY, -INFINITY}) {
      float poisoned[4] = {values[0], values[1], bad, values[3]};
      before = spy.seen();
      cmd = command(mode, poisoned, mask, 4);
      check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
            std::string("a ") + (std::isnan(bad) ? "NaN" : bad > 0 ? "+inf" : "-inf") + " setpoint on the third joint is refused in " +
                (mode == SKR_MODE_POSITION ? "position" : mode == SKR_MODE_VELOCITY ? "velocity" : "torque") + " mode");
      check(!spy.wait_for(before + 1, 0.3), "and nothing was published for it");
    }
  }
  /* A joint the mask leaves out carries a value nobody reads, and the kernel's hold frame fills it with whatever it has. */
  {
    float unread[4] = {values[0], std::nanf(""), values[2], values[3]};
    std::uint8_t second_held[4] = {1, 0, 1, 1};
    before = spy.seen();
    cmd = command(SKR_MODE_POSITION, unread, second_held, 4);
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK,
          "a NaN in the entry of a joint the mask holds is not read, so the command is accepted");
    check(spy.wait_for(before + 1), "and it reached the wire");
  }

  section("emergency_stop (§24's second rule)");
  before = spy.seen();
  check(driver->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
  check(spy.wait_for(before + 1), "and publishes one message");
  sent = spy.last();
  bool everything_zero = names_are(sent);
  for (std::size_t i = 0; i < 4 && everything_zero; ++i) {
    if (sent.position[i] != 0.0 || sent.velocity[i] != 0.0 || sent.effort[i] != 0.0 ||
        sent.stiffness[i] != 0.0 || sent.damping[i] != 0.0) {
      everything_zero = false;
    }
  }
  check(everything_zero,
        "every lane is zero — the vendor's RL zeros — and every joint is still named");
  cmd = command(SKR_MODE_POSITION, values, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EPERM,
        "commands are refused while the latch is set");
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check((state.flags & SKR_FLAG_ESTOP) != 0, "and the stop is visible in the next state");
  /* x1-release-keeps-latch: a release that returns success must have released something. §48.2's self-test leaves the body at the zeros above, a person looks at it, and the
   * latch clears; nothing resumes by itself. */
  check(driver->release_stop(ctx) == SKR_OK, "release_stop returns");
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check((state.flags & SKR_FLAG_ESTOP) == 0, "and the next state carries no stop");
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, mask, 4);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "and a command is accepted again: the latch is cleared and not only reported");
  check(spy.wait_for(before + 1), "and it reaches the wire");
  /* The session is closed with the latch set again, as it was before the release was tried: x1-stop-survives-reopen is only visible to the sessions that open after a close that
   * left a stop latched, and a suite that cleared the latch first would have nothing for it to carry over. */
  check(driver->emergency_stop(ctx) == SKR_OK, "a second stop latches again, and the session closes with it latched");
  driver->close(ctx);
  ctx = nullptr;

  section("§63.4 through a gain: a driver with no gains configured");
  const std::string no_gains = "{\"domain\":" + domain + ",\"model\":\"" + model + "\"}";
  skr_ctx *bare = nullptr;
  check(driver->open(no_gains.c_str(), &bare) == SKR_OK, "opens without `kp` and `kd`");
  if (bare != nullptr) {
    cmd = command(SKR_MODE_POSITION, values, mask, 4);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and refuses a position command, because a stiffness is the operator's to choose and "
          "not this driver's to invent");
    cmd = command(SKR_MODE_VELOCITY, values, mask, 4);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and a velocity command for the same reason — the damping is a gain too");
    cmd = command(SKR_MODE_TORQUE, values, mask, 4);
    check(driver->write_command(bare, &cmd, 0) == SKR_OK,
          "…while a torque command needs no gain and is accepted");
    std::uint8_t one_held[4] = {1, 1, 0, 1};
    cmd = command(SKR_MODE_TORQUE, values, one_held, 4);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "but a torque command that masks a joint is refused: that joint is to be held, a hold "
          "is a stiffness, and a free joint is the reading §24 rules out (F121)");
    std::uint8_t hold_all[4] = {0, 0, 0, 0};
    cmd = command(SKR_MODE_POSITION, values, hold_all, 4);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and so is the kernel's hold frame, loudly, rather than answered with a free body");
    driver->close(bare);
  }

  /* x1-gains-need-both: a position command carries a stiffness and a damping, and a body configured with one of the two has not been
   * given the other. The missing one would be zero, which is a limp joint or an undamped one by accident. */
  section("one gain is not both");
  for (const char *only : {"\"kp\":50.0", "\"kd\":2.0"}) {
    skr_ctx *half = nullptr;
    const std::string half_config = "{\"domain\":" + domain + ",\"model\":\"" + model + "\"," + only + "}";
    check(driver->open(half_config.c_str(), &half) == SKR_OK, std::string("a session configured with only ") + only + " opens");
    if (half != nullptr) {
      cmd = command(SKR_MODE_POSITION, values, mask, 4);
      check(driver->write_command(half, &cmd, 0) == SKR_EPERM, "and refuses a position command: the other gain was never chosen");
      cmd = command(SKR_MODE_VELOCITY, values, mask, 4);
      check(driver->write_command(half, &cmd, 0) == SKR_EPERM, "and a velocity command");
      driver->close(half);
    }
  }

  section("the flag follows the substrate, declared by the operator");
  const std::string declared_real =
      "{\"domain\":" + domain + ",\"model\":\"" + model + "\",\"simulator\":false}";
  skr_ctx *real = nullptr;
  check(driver->open(declared_real.c_str(), &real) == SKR_OK, "a declared-real session opens");
  if (real != nullptr) {
    char real_json[16384];
    const int real_written = driver->describe(real, real_json, sizeof real_json);
    const std::string real_descriptor(real_json, real_written > 0 ? real_written : 0);
    check(real_descriptor.find("\"simulator\":false") != std::string::npos,
          "and declares real when the operator says so");
    driver->close(real);
  }

  section("roles wait for a human, and are validated at open");
  const std::string with_roles = "{\"domain\":" + domain + ",\"model\":\"" + model +
                                 "\",\"roles\":{\"left_elbow_pitch_joint\":\"Left_Arm.Elbow.Pitch\"}}";
  skr_ctx *roled = nullptr;
  check(driver->open(with_roles.c_str(), &roled) == SKR_OK, "a session with roles opens");
  if (roled != nullptr) {
    char roled_json[16384];
    const int roled_written = driver->describe(roled, roled_json, sizeof roled_json);
    const std::string roled_descriptor(roled_json, roled_written > 0 ? roled_written : 0);
    check(roled_descriptor.find("\"role\":\"left_arm.elbow.pitch\"") != std::string::npos,
          "and the descriptor carries the normalised §5.1 role");
    driver->close(roled);
  }
  skr_ctx *typo = nullptr;
  check(driver->open(("{\"domain\":" + domain + ",\"model\":\"" + model +
                      "\",\"roles\":{\"nope_joint\":\"torso.lumbar.yaw\"}}")
                         .c_str(),
                     &typo) == SKR_EINVAL,
        "a role naming a joint the body did not report is refused — a typo, loudly");
  driver->close(typo);
  skr_ctx *grammar = nullptr;
  check(driver->open(("{\"domain\":" + domain + ",\"model\":\"" + model +
                      "\",\"roles\":{\"left_elbow_pitch_joint\":\"nonsense\"}}")
                         .c_str(),
                     &grammar) == SKR_EINVAL,
        "a role outside §5.1's grammar is refused");
  driver->close(grammar);
  /* One refusal per way the grammar can be too loose (x1-role-grammar-length, -axis-unchecked, -chain-unchecked, -index-overflow), each
   * with every other part of the role well-formed. The index is a u32: 4294967295 is the last one and 4294967296 is not. */
  const char *const bad_roles[] = {
      "left_arm.elbow.pitch.extra",
      "left_arm.elbow.twist",
      "arm.elbow.pitch",
      "left_hand.finger#4294967296",
      "left_hand.finger#",
      "left_arm.elbow.pitch#x",
  };
  for (const char *role : bad_roles) {
    skr_ctx *loose = nullptr;
    const std::string config = "{\"domain\":" + domain + ",\"model\":\"" + model +
                               "\",\"roles\":{\"left_elbow_pitch_joint\":\"" + role + "\"}}";
    check(driver->open(config.c_str(), &loose) == SKR_EINVAL,
          std::string("a role of `") + role + "` is refused: it is not chain.link[.axis][#index]");
    driver->close(loose);
  }
  {
    skr_ctx *last_index = nullptr;
    const std::string config = "{\"domain\":" + domain + ",\"model\":\"" + model +
                               "\",\"roles\":{\"left_elbow_pitch_joint\":\"left_hand.finger#4294967295\"}}";
    check(driver->open(config.c_str(), &last_index) == SKR_OK, "while the last index a u32 holds is accepted");
    driver->close(last_index);
  }

  section("a model that does not bound the body is a mismatch, not an unclamped joint");
  {
    /* The suite's own broken MJCF: the knee renamed past recognition, so the range lookup
     * fails for a joint the wire did report. Written beside the fake's report, not into the
     * vendor tree. */
    const std::string broken_path =
        std::string(std::getenv("TMPDIR") != nullptr ? std::getenv("TMPDIR") : "/tmp") +
        "/skynet-x1-check-broken.xml";
    std::string broken = xml;
    const std::string needle = "name=\"left_knee_pitch_joint\"";
    const std::size_t at = broken.find(needle);
    if (at != std::string::npos) {
      broken.replace(at, needle.size(), "name=\"left_knee_pitch_joint_renamed\"");
    }
    std::ofstream out(broken_path);
    out << broken;
    out.close();
    skr_ctx *mismatch = nullptr;
    check(driver->open(("{\"domain\":" + domain + ",\"model\":\"" + broken_path + "\"}").c_str(),
                       &mismatch) == SKR_EINVAL,
          "open against a model missing a reported joint's range is refused");
    driver->close(mismatch);

    /* A range that is there and means nothing is as much a mismatch as one that is missing (x1-range-order-unchecked, x1-range-nonfinite):
     * a clamp to an interval whose lower end is not below its upper sends every setpoint to one end, and one to `nan` or `inf` clamps
     * nothing. The knee's own range is rewritten, in a copy beside the other, and every other joint keeps the vendor's. */
    const auto with_knee_range = [&](const std::string &range) {
      std::string text = xml;
      const std::size_t joint_at = text.find(needle);
      const std::size_t range_at = joint_at == std::string::npos ? std::string::npos : text.find("range=\"", joint_at);
      const std::size_t range_end = range_at == std::string::npos ? std::string::npos : text.find('"', range_at + 7);
      if (range_end != std::string::npos) text.replace(range_at + 7, range_end - range_at - 7, range);
      const std::string path = broken_path + ".range.xml";
      std::ofstream file(path);
      file << text;
      return path;
    };
    for (const char *range : {"1.0 -1.0", "0.5 0.5", "nan 1.0", "-1.0 nan", "-inf inf", "-1.0 inf"}) {
      const std::string path = with_knee_range(range);
      skr_ctx *odd = nullptr;
      check(driver->open(("{\"domain\":" + domain + ",\"model\":\"" + path + "\"}").c_str(), &odd) == SKR_EINVAL,
            std::string("a knee range of `") + range + "` is not an interval: open refuses it, naming nothing it could clamp to");
      driver->close(odd);
    }
    skr_ctx *sane = nullptr;
    check(driver->open(("{\"domain\":" + domain + ",\"model\":\"" + with_knee_range("-1.5 1.5") + "\"}").c_str(), &sane) == SKR_OK,
          "while an ordinary range in the same file is accepted");
    driver->close(sane);

    /* x1-range-search-unbounded: a joint whose own element has no `range` takes nobody else's. The lookup stops at the end of the element, not the end of the file; with the knee's
     * `range=` renamed the next `range="` in the text is another joint's, and a driver that took it would clamp the knee to a neighbour's travel. */
    {
      std::string text = xml;
      const std::size_t joint_at = text.find(needle);
      const std::size_t range_at = joint_at == std::string::npos ? std::string::npos : text.find("range=\"", joint_at);
      check(range_at != std::string::npos, "the vendor's knee has a range of its own to take away");
      if (range_at != std::string::npos) text.replace(range_at, 5, "rangx");
      const std::string path = broken_path + ".norange.xml";
      std::ofstream file(path);
      file << text;
      file.close();
      skr_ctx *unbounded = nullptr;
      check(driver->open(("{\"domain\":" + domain + ",\"model\":\"" + path + "\"}").c_str(), &unbounded) == SKR_EINVAL,
            "a joint whose own element has no range is refused, and does not take the next joint's");
      driver->close(unbounded);
    }
  }

  /* x1-setpoint-survives-reopen, x1-hold-survives-reopen (F47's reset: every field a previous session set). A new session has no setpoint
   * and no hold, so the kernel's first hold frame holds each joint where the body is, not at the zero the arrays were sized with and not
   * where the last session left them. The fake echoes the positions it is sent, so the first session leaves the body somewhere that is
   * neither zero nor where it started. */
  section("a new session holds where the body is");
  {
    skr_ctx *first = nullptr;
    check(driver->open(with_gains.c_str(), &first) == SKR_OK, "a first session opens");
    float left_at[4];
    for (std::size_t i = 0; i < 4; ++i) left_at[i] = static_cast<float>(low[i] + 0.7 * (high[i] - low[i]));
    if (first != nullptr) {
      cmd = command(SKR_MODE_POSITION, left_at, mask, 4);
      check(driver->write_command(first, &cmd, 0) == SKR_OK, "and commands every joint to a position well off zero");
      driver->close(first);
    }
    skr_ctx *second = nullptr;
    check(driver->open(with_gains.c_str(), &second) == SKR_OK, "a second session opens on the same body");
    if (second != nullptr) {
      state = buffers.state();
      const auto settled_by = std::chrono::steady_clock::now() + std::chrono::seconds(5);
      bool where_it_was = false;
      while (!where_it_was && std::chrono::steady_clock::now() < settled_by) {
        driver->read_state(second, &state, 0);
        where_it_was = true;
        for (std::size_t i = 0; i < 4; ++i) where_it_was = where_it_was && std::fabs(state.position_rad[i] - left_at[i]) < 1e-4;
        if (!where_it_was) std::this_thread::sleep_for(std::chrono::milliseconds(10));
      }
      check(where_it_was, "which reports the body where the first one left it");
      std::uint8_t hold_everything[4] = {0, 0, 0, 0};
      float ignored[4] = {0.0f, 0.0f, 0.0f, 0.0f};
      const std::uint64_t held_before = spy.seen();
      cmd = command(SKR_MODE_POSITION, ignored, hold_everything, 4);
      check(driver->write_command(second, &cmd, 0) == SKR_OK, "and the kernel's hold frame is accepted");
      check(spy.wait_for(held_before + 1), "and reaches the wire");
      sent = spy.last();
      bool held_there = full_vector(sent);
      for (std::size_t i = 0; i < 4 && held_there; ++i) {
        held_there = std::fabs(sent.position[i] - left_at[i]) < 1e-4 && sent.stiffness[i] == 50.0f;
      }
      check(held_there, "and holds each joint where it is, stiffly: not at zero, which is where a session with no setpoint would be sized");

      /* x1-applied-joint-keeps-hold: a joint that is held and then commanded takes the new setpoint as its hold, not the latch it had.
       * Joint 0 is held (latching at where it is), commanded elsewhere, and held again. */
      float elsewhere[4];
      for (std::size_t i = 0; i < 4; ++i) elsewhere[i] = static_cast<float>(low[i] + 0.3 * (high[i] - low[i]));
      cmd = command(SKR_MODE_POSITION, elsewhere, mask, 4);
      check(driver->write_command(second, &cmd, 0) == SKR_OK, "a held joint is commanded to a new position");
      const std::uint64_t again_before = spy.seen();
      cmd = command(SKR_MODE_POSITION, ignored, hold_everything, 4);
      check(driver->write_command(second, &cmd, 0) == SKR_OK && spy.wait_for(again_before + 1), "and held again");
      sent = spy.last();
      held_there = full_vector(sent);
      for (std::size_t i = 0; i < 4 && held_there; ++i) held_there = std::fabs(sent.position[i] - elsewhere[i]) < 1e-4;
      check(held_there, "at the position it was just given, and not at the place it was latched before");
      driver->close(second);
    }
  }

  /* x1-open-waits-for-ever, x1-close-keeps-state: a states topic nobody publishes on is a graph that is silent, and open() says so
   * after five seconds instead of waiting on it. The first session's last state must not satisfy the wait. */
  section("a silent graph is refused after five seconds");
  {
    skr_ctx *silent = nullptr;
    const auto began = std::chrono::steady_clock::now();
    const int refused = driver->open(("{\"domain\":" + domain + ",\"model\":\"" + model +
                                      "\",\"kp\":50.0,\"kd\":2.0,\"states\":\"/x1_selftest/nobody\",\"cmd\":\"/x1_selftest/nobody_cmd\"}")
                                         .c_str(),
                                     &silent);
    const double waited = std::chrono::duration<double>(std::chrono::steady_clock::now() - began).count();
    check(refused == SKR_EIO, "open() against a topic nobody publishes on is refused as EIO");
    check(waited > 4.5 && waited < 12.0, "after the five seconds it says it waits, not at once and not for ever");
    driver->close(silent);
  }

  /* x1-state-names-unchecked, x1-state-lengths-unchecked, x1-stamp-zero-kept: the suite is the substrate here, publishing a JointState of
   * its own on a topic of its own, so that a state which is not this body's can be sent on purpose — which the fake, publishing the right
   * one every millisecond, never does. */
  section("a JointState that is not this body's is not reported");
  {
    auto feeder_node = std::make_shared<rclcpp::Node>("x1_selftest_feeder");
    auto feeder_pub = feeder_node->create_publisher<sensor_msgs::msg::JointState>("/x1_selftest/states", 10);
    std::atomic<int> shape{0};  // 0 this body's, 1 two names swapped, 2 a velocity array one short, 3 an effort array one long
    std::atomic<bool> feeding{true};
    std::thread feeder([&] {
      while (feeding) {
        sensor_msgs::msg::JointState message;
        for (const char *name : kJoints) message.name.push_back(name);
        if (shape == 1) std::swap(message.name[0], message.name[1]);
        message.position.assign(4, 0.1);
        message.velocity.assign(shape == 2 ? 3 : 4, 0.0);
        message.effort.assign(shape == 3 ? 5 : 4, 0.0);
        /* header.stamp left at zero on purpose: the driver stamps what the sim did not. */
        feeder_pub->publish(message);
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
      }
    });
    skr_ctx *fed = nullptr;
    const std::string fed_config = "{\"domain\":" + domain + ",\"model\":\"" + model +
                                   "\",\"kp\":50.0,\"kd\":2.0,\"states\":\"/x1_selftest/states\",\"cmd\":\"/x1_selftest/cmd\"}";
    check(driver->open(fed_config.c_str(), &fed) == SKR_OK, "a session on the suite's own states topic opens");
    if (fed != nullptr) {
      state = buffers.state();
      check(driver->read_state(fed, &state, 0) == SKR_OK, "and reads this body's state");
      check(state.stamp_ns != 0, "a sample the sim did not stamp is stamped by the driver, never reported with no time (§24)");
      for (const int wrong : {1, 2, 3}) {
        shape = wrong;
        std::this_thread::sleep_for(std::chrono::milliseconds(150));
        check(driver->read_state(fed, &state, 0) == SKR_EIO,
              wrong == 1   ? "a state that names the joints in another order is refused: its positions would slide into the wrong joints"
              : wrong == 2 ? "a state whose velocity array is one short is refused, not read past its end"
                           : "a state whose effort array is longer than the body is refused");
      }
      shape = 0;
      std::this_thread::sleep_for(std::chrono::milliseconds(150));
      check(driver->read_state(fed, &state, 0) == SKR_OK, "and this body's state is read again once it is the one arriving");
      driver->close(fed);
    }
    feeding = false;
    feeder.join();
  }

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  rclcpp::shutdown();
  return FAILURES.empty() ? 0 : 1;
}


