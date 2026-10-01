/*
 * skynet-booster's own suite: the mode mapping, the layout, the view and every refusal.
 *
 * Linked against `booster_bridge.cpp` rather than dlopening it, so a refusal can be asserted
 * directly through the vtable. It runs against `fake_robot` on DDS — the vendor's SDK on both
 * sides — which the caller starts; `run-check.sh` is what starts it.
 *
 * What makes this worth more than a list of return codes is the **substrate's own report**. The
 * driver's job here is not only to return SKR_OK: it is to send twenty-three motors (the SDK header
 * warns a wrong layout "can address the wrong actuator"), in the SERIAL view it reads, with
 * `weight = 1` on every joint. Those are things only the thing on the other end can see, so the
 * fake counts them and this suite reads the count.
 */

#include "skynet_rhal.h"

#include <booster/idl/b1/LowCmd.h>
#include <booster/robot/b1/b1_api_const.hpp>
#include <booster/robot/channel/channel_subscriber.hpp>

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

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

constexpr int JOINTS = 23;

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

/* A listener on `rt/joint_ctrl`, so the suite can read the exact message the driver published
 * rather than infer it from a return code. */
class CommandSpy {
 public:
  explicit CommandSpy(const std::string &interface) {
    booster::robot::ChannelFactory::Instance()->Init(0, interface);
    subscriber_.reset(new booster::robot::ChannelSubscriber<booster_interface::msg::LowCmd>(
        booster::robot::b1::kTopicJointCtrl));
    subscriber_->InitChannel([this](const void *message) {
      std::lock_guard<std::mutex> guard(lock_);
      last_ = *static_cast<const booster_interface::msg::LowCmd *>(message);
      seen_ += 1;
    });
  }

  bool wait_for(std::uint64_t count, double seconds = 2.0) {
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

  booster_interface::msg::LowCmd last() {
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
  booster_interface::msg::LowCmd last_{};
  std::shared_ptr<booster::robot::ChannelSubscriber<booster_interface::msg::LowCmd>> subscriber_;
};

struct Buffers {
  float position[JOINTS * 4];
  float velocity[JOINTS * 4];
  float effort[JOINTS * 4];

  skr_state state() {
    skr_state out;
    std::memset(&out, 0, sizeof out);
    // The caller's capacity, exactly as the runtime passes it (F45).
    out.joint_count = static_cast<std::uint16_t>(JOINTS * 4);
    out.position_rad = position;
    out.velocity_rad_s = velocity;
    out.effort_nm = effort;
    return out;
  }
};

skr_command command(std::uint8_t mode, const float *values, const std::uint8_t *mask,
                    std::uint16_t joints = JOINTS) {
  skr_command cmd;
  std::memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = mode;
  cmd.joint_count = joints;
  cmd.values = values;
  cmd.mask = mask;
  return cmd;
}

/* DRY-COVERAGE-PLAN.md F205. A robot that reports trouble on one joint, in the maker's own `lost`
 * field, against a fake started with `<joint>=<value>` (`run-check.sh` does it). The driver's word
 * used to be the joint's ordinal alone, under a comment that called it the vendor's word verbatim;
 * the number the maker defined never reached the black box. The expectation is written out here by
 * hand and not taken from the driver: the joint, 1-based, in the high half, and the vendor's own
 * number in the low sixteen bits. */
int run_fault(const std::string &interface, int joint, std::uint32_t value) {
  const skr_driver *driver = skynet_rhal_entry();
  section("a joint the robot reports trouble on");
  skr_ctx *ctx = nullptr;
  const std::string with_gains =
      "{\"interface\":\"" + interface + "\",\"kp\":60.0,\"kd\":5.0}";
  check(driver->open(with_gains.c_str(), &ctx) == SKR_OK,
        "open() joins DDS and waits for the robot's first state");
  if (ctx == nullptr) {
    std::printf("\n  the fake robot is not publishing; nothing below can run.\n");
    return 1;
  }
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check((state.flags & SKR_FLAG_DEGRADED) != 0,
        "a joint the robot reports trouble on reads DEGRADED: §27 reads that bit");
  check((state.flags & SKR_FLAG_ESTOP) == 0,
        "and is not a stop: the maker says a joint is in trouble, not that the stop chain is open");
  const std::uint32_t wanted =
      (static_cast<std::uint32_t>(joint + 1) << 16) | (value & 0xFFFFu);
  check(state.fault_code == wanted,
        "the word carries the joint (1-based, high half) and the vendor's own number (low sixteen "
        "bits), not the ordinal alone: " + std::to_string(joint + 1) + " and " +
            std::to_string(value & 0xFFFFu));
  driver->close(ctx);
  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}

/* Another body's layout on the topic, against a fake started with `layout=<seconds>`
 * (`run-check.sh` does it): from the first sample (`0`), or from a moment after the driver has
 * opened. The SDK header warns that a layout this driver does not speak "can address the wrong
 * actuator", and the same holds for reading one — so open refuses the first and a read notices the
 * second, rather than indexing twenty-three joints out of twenty-two. */
int run_layout(const std::string &interface, double after) {
  const skr_driver *driver = skynet_rhal_entry();
  section("another body's layout on the topic");
  skr_ctx *ctx = nullptr;
  const std::string with_gains =
      "{\"interface\":\"" + interface + "\",\"kp\":60.0,\"kd\":5.0}";
  const int opened = driver->open(with_gains.c_str(), &ctx);
  if (after <= 0.0) {
    check(opened == SKR_EIO,
          "a state whose serial vector is not twenty-three joints is refused at open: this driver "
          "speaks the T1's layout only, and will not index one it does not speak");
    if (opened == SKR_OK) driver->close(ctx);
  } else {
    check(opened == SKR_OK, "a session opens while the T1's layout is the one published");
    if (opened == SKR_OK) {
      Buffers buffers;
      skr_state state = buffers.state();
      check(driver->read_state(ctx, &state, 0) == SKR_OK, "and reads it");
      std::this_thread::sleep_for(
          std::chrono::milliseconds(static_cast<int>(after * 1000.0) + 700));
      state = buffers.state();
      check(driver->read_state(ctx, &state, 0) == SKR_EIO,
            "a read after the topic starts carrying another body's layout is refused, not "
            "indexed: the layout is checked on every read, not only at open");
      driver->close(ctx);
    }
  }
  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}

/* One joint as the descriptor must write it, worked out by hand — the role from the manual's
 * naming, the travel in whole degrees from its limit table (pp3-4) — and written here, not read
 * back from the driver, so that a slot or a range slipped in the driver cannot also slip in the
 * expectation. */
struct JointSpec {
  const char *name;
  const char *role;
  double low_deg;
  double high_deg;
};

constexpr JointSpec JOINT_SPEC[JOINTS] = {
    {"head_yaw", "neck.neck.yaw", -58, 58},
    {"head_pitch", "neck.neck.pitch", -18, 47},
    {"L_shoulder_pitch", "left_arm.shoulder.pitch", -188, 68},
    {"L_shoulder_roll", "left_arm.shoulder.roll", -94, 88},
    {"L_shoulder_yaw", "left_arm.shoulder.yaw", -128, 128},
    {"L_elbow", "left_arm.elbow.flex", -120, 2},
    {"R_shoulder_pitch", "right_arm.shoulder.pitch", -188, 68},
    {"R_shoulder_roll", "right_arm.shoulder.roll", -88, 94},
    {"R_shoulder_yaw", "right_arm.shoulder.yaw", -128, 128},
    {"R_elbow", "right_arm.elbow.flex", -2, 120},
    {"waist_yaw", "torso.waist.yaw", -58, 58},
    {"L_hip_pitch", "left_leg.hip.pitch", -118, 118},
    {"L_hip_roll", "left_leg.hip.roll", -21, 88},
    {"L_hip_yaw", "left_leg.hip.yaw", -58, 58},
    {"L_knee", "left_leg.knee.flex", 0, 123},
    {"L_ankle_up", "left_leg.ankle.pitch", -23, 49},
    {"L_ankle_down", "left_leg.ankle.roll", -24, 45},
    {"R_hip_pitch", "right_leg.hip.pitch", -118, 118},
    {"R_hip_roll", "right_leg.hip.roll", -88, 21},
    {"R_hip_yaw", "right_leg.hip.yaw", -58, 58},
    {"R_knee", "right_leg.knee.flex", 0, 123},
    {"R_ankle_up", "right_leg.ankle.pitch", -23, 49},
    {"R_ankle_down", "right_leg.ankle.roll", -24, 45},
};

/* The number after `key` in the descriptor's entry for `name`, or NaN. */
double number_after(const std::string &descriptor, const std::string &name, const char *key) {
  const std::size_t entry = descriptor.find(std::string("\"name\":\"") + name + "\"");
  if (entry == std::string::npos) return std::nan("");
  const std::string needle = std::string("\"") + key + "\":";
  const std::size_t at = descriptor.find(needle, entry);
  if (at == std::string::npos) return std::nan("");
  return std::strtod(descriptor.c_str() + at + needle.size(), nullptr);
}

}  // namespace

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
  /* `layout=<seconds>`: run only the odd-layout section, against a fake started with the same
   * argument. */
  if (argc > 2 && std::strncmp(argv[2], "layout=", 7) == 0) {
    return run_layout(interface, std::atof(argv[2] + 7));
  }
  /* `<joint>=<value>`: run only the fault section, against a fake started with the same argument. */
  if (argc > 2) {
    char *rest = nullptr;
    const long joint = std::strtol(argv[2], &rest, 10);
    if (rest == nullptr || *rest != '=' || joint < 0 || joint >= JOINTS) {
      std::fprintf(stderr, "selftest: the second argument is <joint>=<value> with a joint below %d (got \"%s\")\n",
                   JOINTS, argv[2]);
      return 2;
    }
    const std::uint32_t value = static_cast<std::uint32_t>(std::strtoul(rest + 1, nullptr, 10));
    std::printf("\n== joint %ld reports %u ==\n", joint, value);
    return run_fault(interface, static_cast<int>(joint), value);
  }

  const skr_driver *driver = skynet_rhal_entry();
  CommandSpy spy(interface);

  section("the body, and what it refuses to declare");
  skr_ctx *ctx = nullptr;
  const std::string with_gains =
      "{\"interface\":\"" + interface + "\",\"kp\":60.0,\"kd\":5.0}";
  check(driver->abi_version == SKYNET_RHAL_ABI_VERSION, "the driver targets this ABI");
  check(driver->open(with_gains.c_str(), &ctx) == SKR_OK,
        "open() joins DDS and waits for the robot's first state");
  if (ctx == nullptr) {
    std::printf("\n  the fake robot is not publishing; nothing below can run.\n");
    return 1;
  }

  char json[16384];
  const int written = driver->describe(ctx, json, sizeof json);
  check(written > 0, "describe() writes the descriptor");
  const std::string descriptor(json, written > 0 ? written : 0);
  check(descriptor.find("\"robot_slug\":\"booster-t1\"") != std::string::npos,
        "it names the body the driver speaks");
  check(descriptor.find("\"dof\":23") != std::string::npos,
        "twenty-three joints: head 2, arms 4+4, waist 1, legs 6+6");
  check(descriptor.find("\"joint_torque_max_nm\":null") != std::string::npos,
        "the torque ceiling is null — Booster publishes no joint torque, and a rating invented "
        "here would be exactly the brochure §63.4 refuses");
  check(descriptor.find("effort_max_nm") == std::string::npos,
        "and no joint carries a rating either, for the same reason");
  check(descriptor.find("joint_velocity_max_rad_s") == std::string::npos,
        "no velocity ceiling: no joint speed is published either");
  check(descriptor.find("\"rate_hz_max\":null") != std::string::npos,
        "no control rate: the DDS rate is unpublished and there is no convention to cite");
  check(descriptor.find("\"watchdog_ms\":20.0") != std::string::npos,
        "the watchdog is driver design, stated as such beside the citation");
  check(descriptor.find("\"estop\":true") != std::string::npos,
        "the hard emergency stop is fitted — a published hardware fact (manual pp1-2)");
  check(descriptor.find("\"basis\":\"declared\"") != std::string::npos,
        "and states the kind beside the citation (§5.3)");
  check(descriptor.find("\"position_min_rad\":-3.28122") != std::string::npos,
        "travel comes from the manual's limit table: the shoulder's -188° in radians");
  check(descriptor.find("\"neck.neck.yaw\"") != std::string::npos,
        "the head takes the codebase's doubled neck role");
  check(descriptor.find("\"left_leg.ankle.pitch\"") != std::string::npos,
        "and the ankles read as series joints, because the driver speaks SERIAL");
  check(descriptor.find("\"capabilities\":[\"safety.estop\",\"platform.onboard_compute\"]") !=
            std::string::npos,
        "the zero-torque stop the driver has is a capability it declares, beside the cited AGX "
        "Orin");
  check(descriptor.find("\"simulator\":true") != std::string::npos,
        "declared simulated: the default is the safe direction (§35.3)");
  check(descriptor.find("\"base\":\"legged\"") != std::string::npos,
        "the base is legged, so nothing treats it as a wheeled one");
  {
    bool roles_right = true;
    bool travel_right = true;
    for (const JointSpec &joint : JOINT_SPEC) {
      const std::string entry = std::string("\"name\":\"") + joint.name + "\",\"role\":\"" +
                                joint.role + "\",\"type\":\"revolute\"";
      if (descriptor.find(entry) == std::string::npos) {
        roles_right = false;
        std::printf("        (no entry for %s as %s)\n", joint.name, joint.role);
      }
      const double low = number_after(descriptor, joint.name, "position_min_rad");
      const double high = number_after(descriptor, joint.name, "position_max_rad");
      if (!(std::fabs(low - joint.low_deg * M_PI / 180.0) < 2e-5) ||
          !(std::fabs(high - joint.high_deg * M_PI / 180.0) < 2e-5)) {
        travel_right = false;
        std::printf("        (%s travel is %g to %g rad)\n", joint.name, low, high);
      }
    }
    check(roles_right, "every one of the twenty-three joints carries its own name and role");
    check(travel_right,
          "and its own travel in both directions, the manual's whole degrees in radians: a table "
          "typed in degrees, or one joint widened past the manual, would be a body allowed past "
          "what its firmware faults at");
  }

  section("read_state");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.fault_code == 0 && (state.flags & SKR_FLAG_DEGRADED) == 0,
        "a robot that reports nothing wrong reads healthy: a zero fault word and no DEGRADED");
  check(state.joint_count == JOINTS,
        "and reports twenty-three joints written, not the capacity it was handed");
  const std::uint32_t first_seq = state.seq;
  bool finite = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (!std::isfinite(state.position_rad[i])) finite = false;
  }
  check(finite, "every joint reports a finite position");
  {
    /* The fake publishes a different, non-zero number for every joint's angle, and a fixed offset
     * on its velocity and torque estimates (see `fake_robot.cpp`), so that each of the three
     * fields is held to its own joint's value, with its own sign, from the view the driver
     * commands — not the parallel one, which carries other numbers. */
    bool positions = true;
    bool velocities = true;
    bool efforts = true;
    for (int i = 0; i < JOINTS; ++i) {
      if (std::fabs(state.position_rad[i] - 0.013f * static_cast<float>(i + 1)) > 1e-3f) {
        positions = false;
      }
      if (std::fabs(state.velocity_rad_s[i] - 0.03f * static_cast<float>(i + 1)) > 1e-3f) {
        velocities = false;
      }
      if (std::fabs(state.effort_nm[i] - 0.05f * static_cast<float>(i + 1)) > 1e-3f) {
        efforts = false;
      }
    }
    check(positions, "each joint's position is its own, from the serial view");
    check(velocities, "each joint's velocity is its own, with the sign the robot reports");
    check(efforts, "and each joint's torque estimate, likewise");
  }
  bool pose_nan = true;
  for (std::size_t i = 0; i < 7; ++i) {
    if (!std::isnan(state.base_pose[i])) pose_nan = false;
  }
  check(pose_nan,
        "the base pose is all NaN — the IMU gives orientation and nothing gives position");
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check(state.seq > first_seq, "the sequence advances between reads");

  section("the mode mapping, read off the wire");
  float values[JOINTS];
  std::uint8_t mask[JOINTS];
  for (int i = 0; i < JOINTS; ++i) {
    values[i] = 0.2f + static_cast<float>(i) * 0.01f;
    mask[i] = 1;
  }
  std::uint64_t before = spy.seen();
  skr_command cmd = command(SKR_MODE_POSITION, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a well-formed position command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  auto sent = spy.last();
  check(sent.cmd_type() == booster_interface::msg::SERIAL,
        "SERIAL view: joints as joints, the same view the driver reads");
  check(sent.motor_cmd().size() == JOINTS,
        "twenty-three motors — the SDK header warns a wrong layout can address the wrong "
        "actuator");
  bool placed = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (std::fabs(sent.motor_cmd()[i].q() - values[i]) >= 1e-6f) placed = false;
  }
  check(placed, "every joint's setpoint landed at its own index");
  check(sent.motor_cmd()[0].kp() > 0.0f, "with the operator's `kp`, not a default");
  check(sent.motor_cmd()[0].kd() > 0.0f, "and the operator's `kd`");
  bool full_user = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (sent.motor_cmd()[i].weight() != 1.0f) full_user = false;
  }
  check(full_user, "`weight = 1` on every joint: explicit full-user, on every cycle");
  bool servo_mode = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (sent.motor_cmd()[i].mode() != 1) servo_mode = false;
  }
  check(servo_mode,
        "and the PMSM servo mode on every motor: mode 0 is idle, and a drive told to be idle is "
        "unpowered whatever else the command says");

  before = spy.seen();
  cmd = command(SKR_MODE_VELOCITY, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a velocity command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[0].dq() - values[0]) < 1e-6f, "the setpoint is in `dq`");
  check(sent.motor_cmd()[0].kp() == 0.0f,
        "`kp = 0` is what says 'nothing about position' on a message with no sentinels");
  check(sent.motor_cmd()[0].kd() > 0.0f, "with the operator's `kd` behind the velocity");

  before = spy.seen();
  cmd = command(SKR_MODE_TORQUE, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a torque command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[0].tau() - values[0]) < 1e-6f, "the setpoint is in `tau`");
  check(sent.motor_cmd()[0].kp() == 0.0f && sent.motor_cmd()[0].kd() == 0.0f,
        "and both gains are zero, so the drive follows the torque and nothing else");

  /* §24: "0 = hold this joint at its current setpoint". The last command above was torque, which
   * leaves no position setpoint, so the first hold latches where the joint is measured to be. Until
   * 2026-09-24 this section asserted zero gains — a free joint — which is the reading Unitree's
   * simulator showed folding a standing body (DRY-COVERAGE-PLAN.md F121). */
  section("§24's mask, on a message with no sentinels: a masked joint is held");
  std::uint8_t partial[JOINTS];
  for (int i = 0; i < JOINTS; ++i) partial[i] = (i % 2 == 0) ? 1 : 0;
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  const float measured1 = state.position_rad[1];
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, partial);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a partly masked position command is sent");
  spy.wait_for(before + 1);
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[0].q() - values[0]) < 1e-6f,
        "an unmasked joint carries the setpoint");
  const float latched = sent.motor_cmd()[1].q();
  check(std::fabs(latched - measured1) < 0.1f,
        "a masked joint with no position setpoint is held where it was measured");
  check(sent.motor_cmd()[1].kp() == 60.0f && sent.motor_cmd()[1].kd() == 5.0f &&
            sent.motor_cmd()[1].dq() == 0.0f && sent.motor_cmd()[1].tau() == 0.0f,
        "with the operator's `kp` and `kd`, a zero velocity setpoint and no feed-forward — not "
        "zero gains, which on this message is a free joint and on a standing body a collapse");

  before = spy.seen();
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  check(spy.last().motor_cmd()[1].q() == latched,
        "the hold target is latched, not re-read: a target that followed the measurement every "
        "cycle would set the stiffness term to zero and hold nothing");

  for (int i = 0; i < JOINTS; ++i) values[i] = 0.3f + static_cast<float>(i) * 0.01f;
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, mask);
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  std::uint8_t none[JOINTS] = {0};
  float zeros[JOINTS] = {0.0f};
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, zeros, none);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK,
        "the kernel's hold frame — every joint masked, values irrelevant — is accepted");
  spy.wait_for(before + 1);
  sent = spy.last();
  bool at_setpoint = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (sent.motor_cmd()[i].q() != values[i]) at_setpoint = false;
  }
  check(at_setpoint,
        "and after a position command every joint is held at that setpoint — §24's 'current "
        "setpoint' — rather than at the zeros the hold frame carries");

  /* The hold above is latched, and the check that says so is only worth anything if the joint it
   * holds is moving: a target re-read from a measurement that never changes looks latched. So a
   * velocity command sets the second joint going, the first hold after it latches where that joint
   * is, and a second hold a moment later — with the joint still settling — must carry the same
   * target. */
  section("a hold is a target, not a reading: the joint moves and the target does not");
  {
    float rates[JOINTS];
    for (int i = 0; i < JOINTS; ++i) rates[i] = 0.0f;
    rates[1] = 0.8f;
    cmd = command(SKR_MODE_VELOCITY, rates, mask);
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a velocity command sets joint two going");
    std::this_thread::sleep_for(std::chrono::milliseconds(150));
    std::uint8_t second_only[JOINTS];
    for (int i = 0; i < JOINTS; ++i) second_only[i] = i == 1 ? 0 : 1;
    before = spy.seen();
    cmd = command(SKR_MODE_POSITION, values, second_only);
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "the first hold after it is sent");
    spy.wait_for(before + 1);
    const float first_target = spy.last().motor_cmd()[1].q();
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    state = buffers.state();
    driver->read_state(ctx, &state, 0);
    const float measured_now = state.position_rad[1];
    before = spy.seen();
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "and a second a moment later");
    spy.wait_for(before + 1);
    check(std::fabs(measured_now - first_target) > 1e-4f,
          "with the held joint no longer where the first hold found it");
    check(spy.last().motor_cmd()[1].q() == first_target,
          "the second hold carries the first one's target, not the position the joint has "
          "reached: a target that followed the measurement would set the stiffness term to zero "
          "and hold nothing, and the joint would creep");
  }

  section("refusals");
  cmd = command(SKR_MODE_POSITION, values, mask, 7);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a seven-joint command to a twenty-three-joint body is refused");
  cmd = command(200, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a mode this ABI does not define is malformed, not merely refused");

  section("emergency_stop (§24's second rule)");
  before = spy.seen();
  check(driver->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
  check(spy.wait_for(before + 1), "and publishes one message");
  sent = spy.last();
  bool everything_idle = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (sent.motor_cmd()[i].kp() != 0.0f || sent.motor_cmd()[i].kd() != 0.0f ||
        sent.motor_cmd()[i].tau() != 0.0f) {
      everything_idle = false;
    }
  }
  check(everything_idle,
        "every gain and every torque is zero — which under the impedance law is the definition "
        "of commanding nothing");
  cmd = command(SKR_MODE_POSITION, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EPERM,
        "commands are refused while the latch is set");
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check((state.flags & SKR_FLAG_ESTOP) != 0, "and the stop is visible in the next state");

  driver->close(ctx);

  /* One driver object lives for the process, so everything a session set is still there when the
   * next one opens unless `open()` clears it. A hold latched in a session, or a position setpoint
   * remembered, would send the next session's first masked joint to a pose from before. The gain
   * is zero here so that nothing moves: the joint is measured where it was, and the target is the
   * only number that can tell the sessions apart. */
  section("what a session leaves behind");
  {
    const std::string inert =
        "{\"interface\":\"" + interface + "\",\"kp\":0.0,\"kd\":5.0}";
    skr_ctx *first_session = nullptr;
    check(driver->open(inert.c_str(), &first_session) == SKR_OK, "a session opens");
    float far_away[JOINTS];
    for (int i = 0; i < JOINTS; ++i) far_away[i] = 3.0f;
    cmd = command(SKR_MODE_POSITION, far_away, mask);
    check(driver->write_command(first_session, &cmd, 0) == SKR_OK,
          "it takes a position command, which leaves a setpoint of 3 rad on every joint");
    std::uint8_t second_masked[JOINTS];
    for (int i = 0; i < JOINTS; ++i) second_masked[i] = i == 1 ? 0 : 1;
    cmd = command(SKR_MODE_POSITION, far_away, second_masked);
    check(driver->write_command(first_session, &cmd, 0) == SKR_OK,
          "and one that masks the second joint, which latches a hold at that setpoint");
    check(driver->close(first_session) == SKR_OK, "the session closes");

    skr_ctx *next_session = nullptr;
    check(driver->open(inert.c_str(), &next_session) == SKR_OK, "a new session opens");
    state = buffers.state();
    driver->read_state(next_session, &state, 0);
    const float where_it_is = state.position_rad[1];
    float nearby[JOINTS];
    for (int i = 0; i < JOINTS; ++i) nearby[i] = 0.1f;
    before = spy.seen();
    cmd = command(SKR_MODE_POSITION, nearby, second_masked);
    check(driver->write_command(next_session, &cmd, 0) == SKR_OK,
          "its first command masks the second joint");
    spy.wait_for(before + 1);
    check(std::fabs(spy.last().motor_cmd()[1].q() - where_it_is) < 0.05f,
          "and that joint is held where the robot is measured, not at the setpoint or the hold "
          "the last session left");
    driver->close(next_session);
  }

  section("releasing the latch without closing the session (§48.2)");
  {
    skr_ctx *session = nullptr;
    check(driver->open(with_gains.c_str(), &session) == SKR_OK, "a session opens");
    cmd = command(SKR_MODE_POSITION, values, mask);
    check(driver->write_command(session, &cmd, 0) == SKR_OK, "and takes a command");
    check(driver->emergency_stop(session) == SKR_OK, "the driver's own stop is sent");
    check(driver->write_command(session, &cmd, 0) == SKR_EPERM, "and the latch refuses what follows");
    check(driver->release_stop(session) == SKR_OK,
          "a power-on self-test releases the latch without closing the session");
    state = buffers.state();
    driver->read_state(session, &state, 0);
    check((state.flags & SKR_FLAG_ESTOP) == 0, "the state no longer carries the stop flag");
    check(driver->write_command(session, &cmd, 0) == SKR_OK, "and a command is accepted again");
    driver->close(session);
  }

  section("a second open on an open session");
  {
    skr_ctx *session = nullptr;
    check(driver->open(with_gains.c_str(), &session) == SKR_OK, "a session opens");
    skr_ctx *second = nullptr;
    check(driver->open(with_gains.c_str(), &second) == SKR_EPERM,
          "a second open on the same driver is refused while one session is open: it would make "
          "a second publisher and subscriber on the same topics and lose the first session's");
    driver->close(session);
  }

  section("§63.4 through a gain: a driver with no gains configured");
  const std::string no_gains = "{\"interface\":\"" + interface + "\"}";
  skr_ctx *bare = nullptr;
  check(driver->open(no_gains.c_str(), &bare) == SKR_OK, "opens without `kp` and `kd`");
  if (bare != nullptr) {
    cmd = command(SKR_MODE_POSITION, values, mask);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and refuses a position command, because a stiffness is the operator's to choose and "
          "not this driver's to invent");
    cmd = command(SKR_MODE_TORQUE, values, mask);
    check(driver->write_command(bare, &cmd, 0) == SKR_OK,
          "…while a torque command needs no gain and is accepted");
    cmd = command(SKR_MODE_TORQUE, values, partial);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "but a torque command that masks a joint is refused: that joint is to be held, a hold "
          "is a stiffness, and going limp is the reading §24 rules out");
    std::uint8_t hold_all[JOINTS] = {0};
    cmd = command(SKR_MODE_POSITION, values, hold_all);
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and so is the kernel's hold frame, loudly, rather than answered with a free body");
    driver->close(bare);
  }

  section("the flag follows the substrate, declared by the operator");
  const std::string declared_real =
      "{\"interface\":\"" + interface + "\",\"simulator\":false}";
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

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
