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

}  // namespace

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
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

  section("read_state");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.joint_count == JOINTS,
        "and reports twenty-three joints written, not the capacity it was handed");
  const std::uint32_t first_seq = state.seq;
  bool finite = true;
  for (int i = 0; i < JOINTS; ++i) {
    if (!std::isfinite(state.position_rad[i])) finite = false;
  }
  check(finite, "every joint reports a finite position");
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

  section("§24's mask, on a message with no sentinels");
  std::uint8_t partial[JOINTS];
  for (int i = 0; i < JOINTS; ++i) partial[i] = (i % 2 == 0) ? 1 : 0;
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, partial);
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[0].q() - values[0]) < 1e-6f,
        "an unmasked joint carries the setpoint");
  check(sent.motor_cmd()[1].kp() == 0.0f && sent.motor_cmd()[1].kd() == 0.0f &&
            sent.motor_cmd()[1].tau() == 0.0f,
        "a masked joint carries zero gains and zero torque — §24's 'say nothing', which without "
        "sentinels is a free joint");

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
