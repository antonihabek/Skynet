/*
 * skynet-booster's K1 suite (DRY-COVERAGE-PLAN.md item 12.D, the `booster` row): the driver told `"model":"k1"`, against a fake that publishes the 22-joint layout of the SDK's `JointIndexK1`.
 *
 * Linked against `booster_bridge.cpp`, and run against `fake_robot model=k1` on DDS (`run-check-k1.sh` starts it), the vendor's SDK on both sides. What it holds the driver to that the T1 suite does not:
 *
 *   * the K1's own table, written out here from the maker's K1 model (`BoosterRobotics/booster_assets`, `robots/K1/K1_22dof.xml`: the joints' `range`) and the SDK's `JointIndexK1`, **not read back from the
 *     driver**, so that a slot or a range slipped in the driver cannot also slip in the expectation;
 *   * the layout both ways: a K1 session refuses the T1's 23 joints (`against=t1` runs this program against a T1 fake) and a T1 session, which is what an unset `model` is, refuses the K1's 22;
 *   * the last K1 slot (index 21), where a driver still indexing the T1's tables would read past the end of the K1's.
 *
 *   selftest_k1 [interface]              the K1 suite, against `fake_robot model=k1`
 *   selftest_k1 [interface] against=t1   a K1 session against a T1 fake: refused
 *   selftest_k1 [interface] layout=<s>   a K1 session that meets the T1's layout `s` seconds in: refused on the read
 */

#include "skynet_rhal.h"

#include <booster/idl/b1/LowCmd.h>
#include <booster/idl/b1/LowState.h>
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

constexpr int JOINTS = 22;

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

int finish() {
  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}

/* A listener on `rt/joint_ctrl`, so the suite reads the exact message the driver published. */
class CommandSpy {
 public:
  explicit CommandSpy(const std::string &interface) {
    booster::robot::ChannelFactory::Instance()->Init(0, interface);
    subscriber_.reset(new booster::robot::ChannelSubscriber<booster_interface::msg::LowCmd>(booster::robot::b1::kTopicJointCtrl));
    subscriber_->InitChannel([this](const void *message) {
      std::lock_guard<std::mutex> guard(lock_);
      last_ = *static_cast<const booster_interface::msg::LowCmd *>(message);
      seen_ += 1;
    });
  }

  bool wait_for(std::uint64_t count, double seconds = 2.0) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::duration<double>(seconds);
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
    out.joint_count = static_cast<std::uint16_t>(JOINTS * 4);   /* the caller's capacity, as the runtime passes it (F45) */
    out.position_rad = position;
    out.velocity_rad_s = velocity;
    out.effort_nm = effort;
    return out;
  }
};

skr_command command(std::uint8_t mode, const float *values, const std::uint8_t *mask, std::uint16_t joints = JOINTS) {
  skr_command cmd;
  std::memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = mode;
  cmd.joint_count = joints;
  cmd.values = values;
  cmd.mask = mask;
  return cmd;
}

/* The K1, joint by joint, as the SDK's `JointIndexK1` orders them and the maker's K1 model gives their ranges (radians, the digits the model writes). Written here by hand. */
struct JointSpec {
  const char *name;
  const char *role;
  double low;
  double high;
};

constexpr JointSpec SPEC[JOINTS] = {
    {"head_yaw", "neck.neck.yaw", -1.0, 1.0},
    {"head_pitch", "neck.neck.pitch", -0.349, 0.855},
    {"L_shoulder_pitch", "left_arm.shoulder.pitch", -3.316, 1.22},
    {"L_shoulder_roll", "left_arm.shoulder.roll", -1.74, 1.57},
    {"L_elbow_pitch", "left_arm.elbow.pitch", -2.27, 2.27},
    {"L_elbow_yaw", "left_arm.elbow.yaw", -2.44, 0.0},
    {"R_shoulder_pitch", "right_arm.shoulder.pitch", -3.316, 1.22},
    {"R_shoulder_roll", "right_arm.shoulder.roll", -1.57, 1.74},
    {"R_elbow_pitch", "right_arm.elbow.pitch", -2.27, 2.27},
    {"R_elbow_yaw", "right_arm.elbow.yaw", 0.0, 2.44},
    {"L_hip_pitch", "left_leg.hip.pitch", -3.0, 2.21},
    {"L_hip_roll", "left_leg.hip.roll", -0.4, 1.57},
    {"L_hip_yaw", "left_leg.hip.yaw", -1.0, 1.0},
    {"L_knee", "left_leg.knee.flex", 0.0, 2.23},
    {"L_ankle_pitch", "left_leg.ankle.pitch", -0.87, 0.345},
    {"L_ankle_roll", "left_leg.ankle.roll", -0.345, 0.345},
    {"R_hip_pitch", "right_leg.hip.pitch", -3.0, 2.21},
    {"R_hip_roll", "right_leg.hip.roll", -1.57, 0.4},
    {"R_hip_yaw", "right_leg.hip.yaw", -1.0, 1.0},
    {"R_knee", "right_leg.knee.flex", 0.0, 2.23},
    {"R_ankle_pitch", "right_leg.ankle.pitch", -0.87, 0.345},
    {"R_ankle_roll", "right_leg.ankle.roll", -0.345, 0.345},
};

/* The number after `key` in the descriptor's entry for `name`, or NaN. */
/* The role that follows a joint's name in the descriptor, which is the joint's own: a role present somewhere in the descriptor is not a role present on the right joint. */
std::string role_after(const std::string &descriptor, const std::string &name) {
  const std::size_t entry = descriptor.find(std::string("\"name\":\"") + name + "\"");
  if (entry == std::string::npos) return "";
  const std::string needle = "\"role\":\"";
  const std::size_t at = descriptor.find(needle, entry);
  if (at == std::string::npos) return "";
  const std::size_t from = at + needle.size();
  return descriptor.substr(from, descriptor.find('"', from) - from);
}

double number_after(const std::string &descriptor, const std::string &name, const char *key) {
  const std::size_t entry = descriptor.find(std::string("\"name\":\"") + name + "\"");
  if (entry == std::string::npos) return std::nan("");
  const std::string needle = std::string("\"") + key + "\":";
  const std::size_t at = descriptor.find(needle, entry);
  if (at == std::string::npos) return std::nan("");
  return std::strtod(descriptor.c_str() + at + needle.size(), nullptr);
}

std::string with_model(const std::string &interface, const char *model_field) {
  return "{\"interface\":\"" + interface + "\",\"kp\":60.0,\"kd\":5.0" + (model_field[0] ? std::string(",") + model_field : std::string()) + "}";
}

/* A K1 session against a body that publishes another layout. */
int run_other_layout(const std::string &interface, bool from_the_start, double after) {
  const skr_driver *driver = skynet_rhal_entry();
  section("the K1 layout against another body's");
  skr_ctx *ctx = nullptr;
  const int opened = driver->open(with_model(interface, "\"model\":\"k1\"").c_str(), &ctx);
  if (from_the_start) {
    check(opened == SKR_EIO, "a state whose serial vector is the T1's twenty-three is refused at open by a session that speaks the K1: it will not index a layout it does not speak");
    if (opened == SKR_OK) driver->close(ctx);
  } else {
    check(opened == SKR_OK, "a K1 session opens while the K1's layout is the one published");
    if (opened == SKR_OK) {
      Buffers buffers;
      skr_state state = buffers.state();
      check(driver->read_state(ctx, &state, 0) == SKR_OK, "and reads it");
      std::this_thread::sleep_for(std::chrono::milliseconds(static_cast<int>(after * 1000.0) + 700));
      state = buffers.state();
      check(driver->read_state(ctx, &state, 0) == SKR_EIO, "a read after the topic starts carrying the T1's layout is refused, not indexed: the layout is checked on every read");
      driver->close(ctx);
    }
  }
  return finish();
}

}  // namespace

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
  if (argc > 2 && std::strcmp(argv[2], "against=t1") == 0) return run_other_layout(interface, true, 0.0);
  if (argc > 2 && std::strncmp(argv[2], "layout=", 7) == 0) return run_other_layout(interface, false, std::atof(argv[2] + 7));

  const skr_driver *driver = skynet_rhal_entry();
  CommandSpy spy(interface);

  section("a session that is told the K1");
  skr_ctx *ctx = nullptr;
  check(driver->open(with_model(interface, "\"model\":\"k1\"").c_str(), &ctx) == SKR_OK, "open() with `model: k1` joins DDS and waits for the robot's first state");
  if (ctx == nullptr) {
    std::printf("\n  the fake robot is not publishing; nothing below can run.\n");
    return 1;
  }

  char json[16384];
  const int written = driver->describe(ctx, json, sizeof json);
  check(written > 0, "describe() writes the descriptor");
  const std::string descriptor(json, written > 0 ? written : 0);
  check(descriptor.find("\"robot_slug\":\"booster-k1\"") != std::string::npos, "it names the K1");
  check(descriptor.find("\"dof\":22") != std::string::npos, "twenty-two joints: head 2, arms 4+4, legs 6+6, and no waist");
  check(descriptor.find("\"joint_torque_max_nm\":null") != std::string::npos && descriptor.find("effort_max_nm") == std::string::npos,
        "no joint torque rating: the maker's K1 model has an actuator force range, which is a simulation limit and not a rating, and none was taken");
  check(descriptor.find("\"estop\":false") != std::string::npos, "and no hard emergency stop is declared: no document read here says the K1 has one");
  check(descriptor.find("platform.onboard_compute") == std::string::npos && descriptor.find("\"safety.estop\"") != std::string::npos,
        "the software stop is declared and the T1's onboard compute is not claimed for a K1");
  check(descriptor.find("K1_22dof.xml") != std::string::npos && descriptor.find("Booster T1 Instruction Manual") == std::string::npos, "the citation is the K1 model's and not the T1 manual's");
  int named = 0, ranged = 0, roled = 0;
  for (int i = 0; i < JOINTS; ++i) {
    if (descriptor.find(std::string("\"name\":\"") + SPEC[i].name + "\"") != std::string::npos) named += 1;
    if (role_after(descriptor, SPEC[i].name) == SPEC[i].role) roled += 1;
    if (std::fabs(number_after(descriptor, SPEC[i].name, "position_min_rad") - SPEC[i].low) < 1e-4 &&
        std::fabs(number_after(descriptor, SPEC[i].name, "position_max_rad") - SPEC[i].high) < 1e-4) {
      ranged += 1;
    }
  }
  check(named == JOINTS, "every joint has the name the table gives it (" + std::to_string(named) + " of 22)");
  check(roled == JOINTS, "and, in the same joint's entry, the role (" + std::to_string(roled) + " of 22)");
  check(ranged == JOINTS, "and the travel the maker's K1 model gives it (" + std::to_string(ranged) + " of 22)");
  size_t order_ok = 0, at = 0;
  for (int i = 0; i < JOINTS; ++i) {
    const std::size_t found = descriptor.find(std::string("\"name\":\"") + SPEC[i].name + "\"", at);
    if (found != std::string::npos && found >= at) {
      order_ok += 1;
      at = found;
    }
  }
  check(order_ok == JOINTS, "in the SDK's `JointIndexK1` order: the descriptor's joint i is the state's and the command's slot i");

  section("the state, slot by slot");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.joint_count == JOINTS, "and says twenty-two joints");
  int positions = 0, velocities = 0, efforts = 0;
  for (int i = 0; i < JOINTS; ++i) {
    /* The fake gives joint i the angle 0.013(i+1), and reports a velocity 0.03(i+1) and a torque 0.05(i+1) above what the joint does. */
    const float step = static_cast<float>(i + 1);
    if (std::fabs(buffers.position[i] - 0.013f * step) < 2e-3f) positions += 1;
    if (std::fabs(buffers.velocity[i] - 0.03f * step) < 2e-3f) velocities += 1;
    if (std::fabs(buffers.effort[i] - 0.05f * step) < 2e-3f) efforts += 1;
  }
  check(positions == JOINTS && velocities == JOINTS && efforts == JOINTS,
        "every slot is the joint's own: positions " + std::to_string(positions) + ", velocities " + std::to_string(velocities) + ", efforts " + std::to_string(efforts) + " of 22");

  section("a position command, as the robot is sent it");
  float values[JOINTS];
  std::uint8_t all[JOINTS];
  for (int i = 0; i < JOINTS; ++i) {
    values[i] = 0.05f + 0.01f * static_cast<float>(i);
    all[i] = 1;
  }
  const std::uint64_t before = spy.seen();
  skr_command cmd = command(SKR_MODE_POSITION, values, all);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a position command for twenty-two joints is taken");
  check(spy.wait_for(before + 1), "and reaches `rt/joint_ctrl`");
  const auto sent = spy.last();
  check(sent.motor_cmd().size() == JOINTS, "as twenty-two motors: not the T1's twenty-three");
  check(sent.cmd_type() == booster_interface::msg::SERIAL, "in the SERIAL view");
  int q_ok = 0, gains_ok = 0, weight_ok = 0, mode_ok = 0;
  for (std::size_t i = 0; i < sent.motor_cmd().size() && i < JOINTS; ++i) {
    const auto &motor = sent.motor_cmd()[i];
    if (std::fabs(motor.q() - values[i]) < 1e-6f) q_ok += 1;
    if (motor.kp() == 60.0f && motor.kd() == 5.0f && motor.dq() == 0.0f) gains_ok += 1;
    if (motor.weight() == 1.0f) weight_ok += 1;
    if (motor.mode() == 1) mode_ok += 1;
  }
  check(q_ok == JOINTS && gains_ok == JOINTS && weight_ok == JOINTS && mode_ok == JOINTS,
        "each motor carries its own setpoint, the operator's gains, full user weight and the servo mode (" + std::to_string(q_ok) + ", " + std::to_string(gains_ok) + ", " + std::to_string(weight_ok) + ", " + std::to_string(mode_ok) + " of 22)");

  float too_many[JOINTS + 1] = {};
  std::uint8_t too_many_mask[JOINTS + 1];
  for (auto &m : too_many_mask) m = 1;
  cmd = command(SKR_MODE_POSITION, too_many, too_many_mask, JOINTS + 1);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL, "a command for the T1's twenty-three joints is refused: this body has twenty-two");
  cmd = command(SKR_MODE_POSITION, values, all, JOINTS - 1);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL, "and so is one for twenty-one");

  section("the last slot, held");
  std::uint8_t last_held[JOINTS];
  for (int i = 0; i < JOINTS; ++i) last_held[i] = (i == JOINTS - 1) ? 0 : 1;
  const std::uint64_t seen_before_hold = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, last_held);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a command that masks the last joint is taken");
  check(spy.wait_for(seen_before_hold + 1), "and reaches the robot");
  const auto held = spy.last();
  check(held.motor_cmd().size() == JOINTS && std::fabs(held.motor_cmd()[JOINTS - 1].q() - values[JOINTS - 1]) < 1e-6f && held.motor_cmd()[JOINTS - 1].kp() == 60.0f,
        "the masked joint carries its last position setpoint and the operator's gains: a hold is a stiffness, and the last K1 slot is the one a T1-sized table would run past");

  section("a velocity and a torque command");
  float velocity[JOINTS], torque[JOINTS];
  for (int i = 0; i < JOINTS; ++i) {
    velocity[i] = 0.2f + 0.01f * static_cast<float>(i);
    torque[i] = 1.0f + 0.1f * static_cast<float>(i);
  }
  std::uint64_t mark = spy.seen();
  cmd = command(SKR_MODE_VELOCITY, velocity, all);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK && spy.wait_for(mark + 1), "a velocity command is taken and published");
  const auto vel = spy.last();
  int dq_ok = 0, kp_zero = 0;
  for (std::size_t i = 0; i < vel.motor_cmd().size() && i < JOINTS; ++i) {
    if (std::fabs(vel.motor_cmd()[i].dq() - velocity[i]) < 1e-6f) dq_ok += 1;
    if (vel.motor_cmd()[i].kp() == 0.0f && vel.motor_cmd()[i].kd() == 5.0f) kp_zero += 1;
  }
  check(vel.motor_cmd().size() == JOINTS && dq_ok == JOINTS && kp_zero == JOINTS, "each joint's speed is its own and `kp` is zero: nothing about position (" + std::to_string(dq_ok) + ", " + std::to_string(kp_zero) + " of 22)");
  mark = spy.seen();
  cmd = command(SKR_MODE_TORQUE, torque, all);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK && spy.wait_for(mark + 1), "a torque command is taken and published");
  const auto tor = spy.last();
  int tau_ok = 0, gains_zero = 0;
  for (std::size_t i = 0; i < tor.motor_cmd().size() && i < JOINTS; ++i) {
    if (std::fabs(tor.motor_cmd()[i].tau() - torque[i]) < 1e-6f) tau_ok += 1;
    if (tor.motor_cmd()[i].kp() == 0.0f && tor.motor_cmd()[i].kd() == 0.0f) gains_zero += 1;
  }
  check(tor.motor_cmd().size() == JOINTS && tau_ok == JOINTS && gains_zero == JOINTS, "and each joint's torque is its own with both gains zero (" + std::to_string(tau_ok) + ", " + std::to_string(gains_zero) + " of 22)");

  section("the stop");
  mark = spy.seen();
  check(driver->emergency_stop(ctx) == SKR_OK, "a stop returns OK");
  check(spy.wait_for(mark + 1), "and one message goes out");
  const auto stop = spy.last();
  int stopped = 0;
  for (std::size_t i = 0; i < stop.motor_cmd().size() && i < JOINTS; ++i) {
    const auto &m = stop.motor_cmd()[i];
    if (m.kp() == 0.0f && m.kd() == 0.0f && m.tau() == 0.0f && m.dq() == 0.0f) stopped += 1;
  }
  check(stop.motor_cmd().size() == JOINTS && stopped == JOINTS, "of twenty-two motors with every gain and every torque at zero (" + std::to_string(stopped) + " of 22)");
  cmd = command(SKR_MODE_POSITION, values, all);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EPERM, "and a command after it is refused");
  check(driver->release_stop(ctx) == SKR_OK, "released");
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "and a command after the release is taken");
  driver->close(ctx);

  section("which body a session speaks is the operator's word");
  skr_ctx *other = nullptr;
  check(driver->open(with_model(interface, "").c_str(), &other) == SKR_EIO, "a session with no `model` is a T1's, and the K1's twenty-two joints are refused at open");
  if (other != nullptr) driver->close(other);
  other = nullptr;
  check(driver->open(with_model(interface, "\"model\":\"t1\"").c_str(), &other) == SKR_EIO, "and so is one that says `t1`");
  if (other != nullptr) driver->close(other);
  other = nullptr;
  check(driver->open(with_model(interface, "\"model\":\"t2\"").c_str(), &other) == SKR_EINVAL, "a model this driver does not speak is refused by name");
  if (other != nullptr) driver->close(other);
  other = nullptr;
  check(driver->open(with_model(interface, "\"model\":\"K1\"").c_str(), &other) == SKR_EINVAL, "and the name is exact: `K1` is not `k1`");
  if (other != nullptr) driver->close(other);
  other = nullptr;
  check(driver->open(with_model(interface, "\"model\":\"\"").c_str(), &other) == SKR_EINVAL, "nor is an empty one");
  if (other != nullptr) driver->close(other);
  other = nullptr;
  check(driver->open(with_model(interface, "\"model\":\"k1\"").c_str(), &other) == SKR_OK, "and a K1 session opens again after all of that");
  if (other != nullptr) {
    char again[16384];
    const int len = driver->describe(other, again, sizeof again);
    check(len > 0 && std::string(again, len).find("\"dof\":22") != std::string::npos, "with its own descriptor: nothing of another session's body carries over");
    driver->close(other);
  }
  return finish();
}
