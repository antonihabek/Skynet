/*
 * skynet-unitree's own suite: the mode mapping, the sentinels, the CRC and every refusal.
 *
 * Linked against `unitree_bridge.cpp` rather than dlopening it, so a refusal can be asserted
 * directly through the vtable. It runs against `fake_robot` on DDS — the vendor's SDK on both
 * sides — which the caller starts; `run-check.sh` is what starts it.
 *
 * What makes this worth more than a list of return codes is the **substrate's own report**. The
 * driver's job here is not only to return SKR_OK: it is to write Unitree's sentinels where they
 * belong, a CRC the robot will accept, and nothing at all into the eight motor slots a Go2 does not
 * have. Those are things only the thing on the other end can see, so the fake counts them and this
 * suite reads the count.
 */

#include "skynet_rhal.h"

#include <unitree/idl/go2/LowCmd_.hpp>
#include <unitree/robot/channel/channel_subscriber.hpp>

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

constexpr float POS_STOP_F = 2.146e9f;
constexpr float VEL_STOP_F = 16000.0f;
constexpr std::size_t JOINTS = 12;
constexpr std::size_t MOTOR_SLOTS = 20;

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

/* A listener on `rt/lowcmd`, so the suite can read the exact message the driver published rather
 * than infer it from a return code. This is the other half of what `fake_robot` counts: the fake
 * says *whether* something was wrong, and this says *what was in the message*. */
class CommandSpy {
 public:
  explicit CommandSpy(const std::string &interface) {
    unitree::robot::ChannelFactory::Instance()->Init(0, interface);
    subscriber_.reset(
        new unitree::robot::ChannelSubscriber<unitree_go::msg::dds_::LowCmd_>("rt/lowcmd"));
    subscriber_->InitChannel(
        [this](const void *message) {
          std::lock_guard<std::mutex> guard(lock_);
          last_ = *static_cast<const unitree_go::msg::dds_::LowCmd_ *>(message);
          seen_ += 1;
        },
        1);
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

  unitree_go::msg::dds_::LowCmd_ last() {
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
  unitree_go::msg::dds_::LowCmd_ last_{};
  unitree::robot::ChannelSubscriberPtr<unitree_go::msg::dds_::LowCmd_> subscriber_;
};

std::uint32_t crc32_core(const std::uint32_t *ptr, std::uint32_t len) {
  std::uint32_t crc = 0xFFFFFFFFu;
  const std::uint32_t polynomial = 0x04C11DB7u;
  for (std::uint32_t i = 0; i < len; i++) {
    std::uint32_t xbit = 1u << 31;
    const std::uint32_t data = ptr[i];
    for (std::uint32_t bits = 0; bits < 32; bits++) {
      if (crc & 0x80000000u) {
        crc <<= 1;
        crc ^= polynomial;
      } else {
        crc <<= 1;
      }
      if (data & xbit) crc ^= polynomial;
      xbit >>= 1;
    }
  }
  return crc;
}

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

  char json[8192];
  const int written = driver->describe(ctx, json, sizeof json);
  check(written > 0, "describe() writes the descriptor");
  const std::string descriptor(json, written > 0 ? written : 0);
  check(descriptor.find("\"dof\":12") != std::string::npos,
        "twelve joints — not the twenty slots the message carries");
  check(descriptor.find("\"envelope\":null") != std::string::npos,
        "no envelope: a datasheet torque is a design value, not a safety rating (§63.4)");
  check(descriptor.find("effort_max_nm") == std::string::npos,
        "and no joint declares a torque ceiling either");
  check(descriptor.find("\"base\":\"legged\"") != std::string::npos,
        "the base is legged, so nothing treats it as a wheeled one");

  section("read_state");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.joint_count == JOINTS,
        "and reports twelve joints written, not the capacity it was handed");
  const std::uint32_t first_seq = state.seq;
  bool finite = true;
  for (std::size_t i = 0; i < JOINTS; ++i) {
    if (!std::isfinite(state.position_rad[i])) finite = false;
  }
  check(finite, "every joint reports a finite position");
  bool pose_nan = true;
  for (std::size_t i = 0; i < 7; ++i) {
    if (!std::isnan(state.base_pose[i])) pose_nan = false;
  }
  check(pose_nan,
        "the base pose is all NaN — nothing in LowState_ localises a Go2, and a driver that wrote "
        "the IMU quaternion into a pose would be claiming a position it does not have");
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check(state.seq > first_seq, "the sequence advances between reads");

  section("the mode mapping, read off the wire");
  float values[JOINTS];
  std::uint8_t mask[JOINTS];
  for (std::size_t i = 0; i < JOINTS; ++i) {
    values[i] = 0.25f;
    mask[i] = 1;
  }
  std::uint64_t before = spy.seen();
  skr_command cmd = command(SKR_MODE_POSITION, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a well-formed position command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  auto sent = spy.last();
  check(sent.head()[0] == 0xFE && sent.head()[1] == 0xEF, "the message carries Unitree's header");
  check(sent.level_flag() == 0xFF, "and the low-level flag");
  check(std::fabs(sent.motor_cmd()[0].q() - 0.25f) < 1e-6f, "the setpoint is in `q`");
  check(sent.motor_cmd()[0].kp() > 0.0f, "with the operator's `kp`, not a default");
  check(sent.motor_cmd()[0].kd() > 0.0f, "and the operator's `kd`");
  check(sent.crc() == crc32_core(reinterpret_cast<const std::uint32_t *>(&sent),
                                 (sizeof sent >> 2) - 1),
        "the CRC is the one Unitree computes — a wrong one is dropped silently by the robot");
  bool slots_idle = true;
  for (std::size_t i = JOINTS; i < MOTOR_SLOTS; ++i) {
    if (sent.motor_cmd()[i].q() != POS_STOP_F || sent.motor_cmd()[i].kp() != 0.0f ||
        sent.motor_cmd()[i].tau() != 0.0f) {
      slots_idle = false;
    }
  }
  check(slots_idle,
        "the eight motor slots a Go2 does not have carry the sentinels and no gain — a zero there "
        "is a command to a joint that is not present, and on a bigger body it is a motion");

  before = spy.seen();
  cmd = command(SKR_MODE_TORQUE, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a torque command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[0].tau() - 0.25f) < 1e-6f, "the setpoint is in `tau`");
  check(sent.motor_cmd()[0].q() == POS_STOP_F,
        "`q` carries PosStopF and not 0.0 — this is the defect the row exists to catch: a zero "
        "there is a command to go to zero radians");
  check(sent.motor_cmd()[0].dq() == VEL_STOP_F, "`dq` carries VelStopF for the same reason");
  check(sent.motor_cmd()[0].kp() == 0.0f && sent.motor_cmd()[0].kd() == 0.0f,
        "and both gains are zero, so the drive follows the torque and nothing else");

  section("§24's mask, on an impedance drive");
  std::uint8_t partial[JOINTS];
  for (std::size_t i = 0; i < JOINTS; ++i) partial[i] = (i % 2 == 0) ? 1 : 0;
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, partial);
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[0].q() - 0.25f) < 1e-6f, "an unmasked joint carries the setpoint");
  check(sent.motor_cmd()[1].q() == POS_STOP_F && sent.motor_cmd()[1].kp() == 0.0f,
        "a masked joint carries the sentinel and no gain — §24's 'say nothing', which on an "
        "impedance drive is the only way to say it");

  section("refusals");
  cmd = command(SKR_MODE_POSITION, values, mask, 7);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a seven-joint command to a twelve-joint body is refused");
  cmd = command(200, values, mask);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a mode this ABI does not define is malformed, not merely refused");

  section("emergency_stop (§24's second rule)");
  before = spy.seen();
  check(driver->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
  check(spy.wait_for(before + 1), "and publishes one message");
  sent = spy.last();
  bool everything_idle = true;
  for (std::size_t i = 0; i < MOTOR_SLOTS; ++i) {
    if (sent.motor_cmd()[i].kp() != 0.0f || sent.motor_cmd()[i].kd() != 0.0f ||
        sent.motor_cmd()[i].tau() != 0.0f || sent.motor_cmd()[i].q() != POS_STOP_F) {
      everything_idle = false;
    }
  }
  check(everything_idle,
        "every gain, every torque and every setpoint is idle — which on an impedance drive is the "
        "definition of commanding nothing");
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

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
