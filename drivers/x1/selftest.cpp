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

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include "rclcpp/rclcpp.hpp"
#include "sensor_msgs/msg/joint_state.hpp"
#include "my_ros2_proto/msg/joint_command.hpp"

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

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
    const char *saved_infer = std::getenv("X1_INFER");
    const std::string infer = saved_infer != nullptr ? saved_infer : "";
    ::unsetenv("X1_INFER");
    check(driver->open("{}", &ctx) == SKR_EINVAL,
          "no `model` and no $X1_INFER is refused rather than moving unclamped");
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

  section("read_state");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.joint_count == 4, "and reports the joints written, not the capacity it was handed");
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
  }

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  rclcpp::shutdown();
  return FAILURES.empty() ? 0 : 1;
}


