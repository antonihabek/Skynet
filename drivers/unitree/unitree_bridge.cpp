/*
 * skynet-unitree — a Unitree quadruped over unitree_sdk2's low-level channels.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 2 table: substrate *"SDK sim"*, dry-verifiable *"protocol, low-level
 * mode mapping"*, not dry-verifiable *"joint access at rate"*. This is the driver; `fake_robot.cpp`
 * beside it is the substrate, and both use **the vendor's own SDK**, which is what makes this row
 * worth more than a transcription: the CDR encoding, the DDS discovery and the topic types are
 * Unitree's code on both sides, so nothing here can be wrong about the wire without being wrong in
 * the same way a real robot would refuse.
 *
 * ── The three things the mapping has to get right ─────────────────────────
 *
 * §24 commands a body in one mode — position, velocity or torque — and `MotorCmd_` is not a mode at
 * all. It is an impedance command: every motor carries `q`, `dq`, `tau`, `kp` and `kd` at once, and
 * what the drive does is `tau_out = tau + kp*(q - q_measured) + kd*(dq - dq_measured)`. So:
 *
 * 1. **The sentinels are not zeros.** `PosStopF` (2.146e9) in `q` and `VelStopF` (16000) in `dq` are
 *    Unitree's "ignore this field" values. A driver writing 0.0 into `q` for a torque command is not
 *    saying nothing — it is commanding the joint to zero radians, and with any `kp` at all that is a
 *    motion. This is the defect this row exists to catch, and `fake_robot` refuses a command that
 *    does it.
 *
 * 2. **`kp` and `kd` are not in a §24 command, and are not guessed.** They are the operator's, they
 *    come from the config JSON, and a position or velocity command with no gains configured is
 *    **refused**. A default would be a stiffness nobody chose, in front of a Safety Kernel that
 *    cannot see it — §63.4's rule, arriving through a gain rather than through a rating.
 *
 * 3. **The CRC is load-bearing.** A `LowCmd_` whose CRC32 does not match is dropped by the robot,
 *    silently, and a driver that omitted it would present as a robot that ignores commands. The
 *    fake checks it for exactly that reason.
 *
 * Licence: Apache-2.0 (this file). It links unitree_sdk2, which is BSD-3-Clause, and CycloneDDS,
 * which is EPL-2.0/EDL-1.0 — neither is vendored here; see the Makefile.
 */

#include "skynet_rhal.hpp"

#include <unitree/idl/go2/LowCmd_.hpp>
#include <unitree/idl/go2/LowState_.hpp>
#include <unitree/robot/channel/channel_publisher.hpp>
#include <unitree/robot/channel/channel_subscriber.hpp>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <thread>
#include <string>
#include <vector>

namespace {

constexpr const char *TOPIC_LOWCMD = "rt/lowcmd";
constexpr const char *TOPIC_LOWSTATE = "rt/lowstate";

/* Unitree's own sentinels, from their published low-level example. `q = PosStopF` means "this
 * command says nothing about position"; `dq = VelStopF` means the same about velocity. */
constexpr float POS_STOP_F = 2.146e9f;
constexpr float VEL_STOP_F = 16000.0f;

/* A Go2 has twelve joints and `LowCmd_` has twenty motor slots. The extra eight are for the
 * larger bodies that share the message, and a driver that reported twenty would be inventing
 * eight joints that do not exist. */
constexpr std::size_t GO2_JOINTS = 12;
constexpr std::size_t MOTOR_SLOTS = 20;

/* PMSM servo mode. `0x00` is idle and leaves the drive unpowered, which is not what a command
 * means. */
constexpr std::uint8_t MOTOR_MODE_SERVO = 0x01;

/* Unitree's CRC32, transcribed from their published example because the SDK does not export it.
 * Bit-reflected input, polynomial 0x04C11DB7, initial value all ones, no final inversion — which
 * is not any of the common CRC32 parameterisations, and is why it is written out rather than
 * reached for from a library. */
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

/* The tiny amount of JSON this driver reads, by hand.
 *
 * A driver is a shared object a robot loads and this repository has no third-party dependencies, so
 * a parser for `{"kp": 60.0}` is four lines rather than a library. It reads a number after a key and
 * answers `false` when there is not one — which is the whole of what the caller needs, because a
 * missing gain is a refusal rather than a default. */
bool config_number(const std::string &config, const char *key, double *out) {
  const std::string needle = std::string("\"") + key + "\"";
  const std::size_t at = config.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle.size());
  if (colon == std::string::npos) return false;
  try {
    std::size_t used = 0;
    const double value = std::stod(config.substr(colon + 1), &used);
    if (used == 0) return false;
    *out = value;
    return true;
  } catch (const std::exception &) {
    return false;
  }
}

bool config_string(const std::string &config, const char *key, std::string *out) {
  const std::string needle = std::string("\"") + key + "\"";
  const std::size_t at = config.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle.size());
  if (colon == std::string::npos) return false;
  const std::size_t open = config.find('"', colon);
  if (open == std::string::npos) return false;
  const std::size_t close = config.find('"', open + 1);
  if (close == std::string::npos) return false;
  *out = config.substr(open + 1, close - open - 1);
  return true;
}

/* §5.1 roles for a Go2's twelve joints, in the order Unitree publishes them: front-right,
 * front-left, rear-right, rear-left, each hip/thigh/calf. The order is the robot's and is not a
 * choice this driver makes — a driver that sorted them would report the left legs as the right
 * ones. */
const char *const GO2_ROLES[GO2_JOINTS] = {
    "right_leg.hip.abduct#0",  "right_leg.thigh.pitch#0", "right_leg.calf.flex#0",
    "left_leg.hip.abduct#0",   "left_leg.thigh.pitch#0",  "left_leg.calf.flex#0",
    "right_leg.hip.abduct#1",  "right_leg.thigh.pitch#1", "right_leg.calf.flex#1",
    "left_leg.hip.abduct#1",   "left_leg.thigh.pitch#1",  "left_leg.calf.flex#1",
};

const char *const GO2_NAMES[GO2_JOINTS] = {
    "FR_hip", "FR_thigh", "FR_calf", "FL_hip", "FL_thigh", "FL_calf",
    "RR_hip", "RR_thigh", "RR_calf", "RL_hip", "RL_thigh", "RL_calf",
};

/* Travel, radians, from Unitree's published Go2 joint-limit table. Travel is a kinematic fact about
 * the machine. No torque or velocity ceiling is declared and `envelope` is null: §63.4 — a number
 * out of a datasheet is a design value and not a safety rating, and the Safety Kernel must not be
 * handed a brochure. */
struct Travel {
  double low;
  double high;
};
constexpr Travel GO2_TRAVEL[GO2_JOINTS] = {
    {-1.0472, 1.0472}, {-1.5708, 3.4907}, {-2.7227, -0.83776},
    {-1.0472, 1.0472}, {-1.5708, 3.4907}, {-2.7227, -0.83776},
    {-1.0472, 1.0472}, {-0.5236, 4.5379}, {-2.7227, -0.83776},
    {-1.0472, 1.0472}, {-0.5236, 4.5379}, {-2.7227, -0.83776},
};

class UnitreeBridge : public skynet::Driver {
 public:
  int open(std::string_view config_json) override {
    if (opened_) return SKR_EPERM;
    const std::string config(config_json);

    /* Every field a previous session set, reset here rather than in the constructor.
     *
     * `SKYNET_RHAL_EXPORT` builds the driver as a function-local static, so one process holds one
     * instance for its whole life and `close()` followed by `open()` lands on the same object. A
     * driver that initialised only in its constructor would carry the last session's state into
     * the next one — and the field that matters is the estop latch, which would make a fresh
     * `open()` refuse every command with no way to see why (DRY-COVERAGE-PLAN.md F47).
     *
     * The latch is announced rather than silently dropped: a new session is a new session, and an
     * operator who stopped this robot a moment ago should read that this open cleared it. */
    if (estopped_) {
      std::fprintf(stderr,
                   "skynet-unitree: the previous session left an emergency stop latched. This is "
                   "a new session and the latch does not carry into it — the robot's own state is "
                   "read from LowState_, and nothing here can tell you whether it is still "
                   "stopped.\n");
    }
    estopped_ = false;
    seq_ = 0;
    have_state_ = false;
    arrivals_ = 0;
    have_kp_ = have_kd_ = false;
    kp_ = kd_ = 0.0;

    std::string interface = "lo";
    config_string(config, "interface", &interface);
    config_string(config, "robot_slug", &slug_);
    /* Gains, and there is deliberately no default. A position command with `kp = 0` moves nothing
     * and a position command with a `kp` this driver invented is a stiffness nobody chose. */
    have_kp_ = config_number(config, "kp", &kp_);
    have_kd_ = config_number(config, "kd", &kd_);

    try {
      unitree::robot::ChannelFactory::Instance()->Init(0, interface);
      publisher_.reset(
          new unitree::robot::ChannelPublisher<unitree_go::msg::dds_::LowCmd_>(TOPIC_LOWCMD));
      publisher_->InitChannel();
      subscriber_.reset(
          new unitree::robot::ChannelSubscriber<unitree_go::msg::dds_::LowState_>(TOPIC_LOWSTATE));
      subscriber_->InitChannel(
          [this](const void *message) {
            std::lock_guard<std::mutex> guard(lock_);
            state_ = *static_cast<const unitree_go::msg::dds_::LowState_ *>(message);
            have_state_ = true;
            arrivals_ += 1;
          },
          1);
    } catch (const std::exception &error) {
      std::fprintf(stderr, "skynet-unitree: could not open DDS on `%s`: %s\n", interface.c_str(),
                   error.what());
      return SKR_EIO;
    }

    /* The first state, waited for rather than assumed. DDS discovery is not instant and a driver
     * that returned success here would have reported a body it had never heard from — the same
     * rule `drivers/franka` follows about its first UDP datagram. */
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (std::chrono::steady_clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (have_state_) break;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!have_state_) {
        std::fprintf(stderr,
                     "skynet-unitree: no LowState_ arrived on `%s` within five seconds. Either "
                     "nothing is publishing `%s`, or the interface is not the one the robot is "
                     "on.\n",
                     interface.c_str(), TOPIC_LOWSTATE);
        return SKR_EIO;
      }
    }
    opened_ = true;
    return SKR_OK;
  }

  int close() override {
    opened_ = false;
    subscriber_.reset();
    publisher_.reset();
    return SKR_OK;
  }

  int read_state(skynet::State &out, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    unitree_go::msg::dds_::LowState_ snapshot;
    std::uint64_t arrivals = 0;
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!have_state_) return SKR_EIO;
      snapshot = state_;
      arrivals = arrivals_;
    }
    if (out.joint_count() < GO2_JOINTS) return SKR_EINVAL;

    seq_ += 1;
    std::uint16_t flags = 0;
    /* §27's homing: these are absolute encoders on a leg, and there is nothing to home. Saying so
     * is not the same as saying nothing — `unreported` holds the robot. */
    flags |= SKR_HOMING_NOT_REQUIRED;
    if (estopped_) flags |= SKR_FLAG_ESTOP;

    /* The vendor's own fault word, surfaced verbatim (§24). `bms_state.version_high` and friends
     * are Unitree's business; what reaches the OS is the first motor that reports a non-zero
     * `lost` count, because that is the one the robot itself says it cannot hear. */
    std::uint32_t fault = 0;
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
      if (snapshot.motor_state()[i].lost() != 0) {
        fault = static_cast<std::uint32_t>(i + 1);
        flags |= SKR_FLAG_DEGRADED;
        break;
      }
    }

    out.set_stamp(now_ns());
    out.set_seq(seq_);
    out.set_flags(flags);
    out.set_fault(fault);
    auto position = out.position();
    auto velocity = out.velocity();
    auto effort = out.effort();
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
      position[i] = snapshot.motor_state()[i].q();
      velocity[i] = snapshot.motor_state()[i].dq();
      /* `tau_est` is the drive's *estimate* from current, not a torque sensor. It is still a
       * measurement rather than §24's NaN, and the descriptor is where the difference is stated. */
      effort[i] = snapshot.motor_state()[i].tau_est();
    }
    out.set_joint_count(GO2_JOINTS);
    /* A Go2 is a legged body and its base pose is not localised by anything in this message: the
     * IMU gives orientation and nothing gives position. §24 says all-NaN for a base nobody
     * localised, and a driver that wrote the IMU quaternion into a pose would be claiming a
     * position it does not have. */
    auto pose = out.base_pose();
    for (std::size_t i = 0; i < 7; ++i) pose[i] = std::nanf("");
    (void)arrivals;
    return SKR_OK;
  }

  int write_command(const skynet::Command &cmd, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    if (estopped_) return SKR_EPERM;
    if (cmd.joint_count() != GO2_JOINTS) return SKR_EINVAL;
    const int mode = cmd.mode();
    if (mode != SKR_MODE_POSITION && mode != SKR_MODE_VELOCITY && mode != SKR_MODE_TORQUE) {
      return SKR_EINVAL;
    }
    /* §63.4 through a gain. A position or velocity command is a *setpoint*, and what turns a
     * setpoint into a torque on this body is `kp` and `kd`, which §24's command does not carry. A
     * default would be a stiffness the operator never chose and the Safety Kernel cannot see. */
    if ((mode == SKR_MODE_POSITION || mode == SKR_MODE_VELOCITY) && !(have_kp_ && have_kd_)) {
      std::fprintf(stderr,
                   "skynet-unitree: a %s command needs `kp` and `kd` in this driver's config. "
                   "A Unitree drive takes an impedance command, not a setpoint, and this driver "
                   "will not choose a stiffness the operator did not (§63.4).\n",
                   mode == SKR_MODE_POSITION ? "position" : "velocity");
      return SKR_EPERM;
    }

    unitree_go::msg::dds_::LowCmd_ out{};
    out.head()[0] = 0xFE;
    out.head()[1] = 0xEF;
    out.level_flag() = 0xFF;
    out.gpio() = 0;
    for (std::size_t i = 0; i < MOTOR_SLOTS; ++i) {
      auto &motor = out.motor_cmd()[i];
      /* Every slot, including the eight this body does not have. A slot left at the struct's zero
       * would carry mode 0 and `q = 0`, which is a well-formed command to a joint that is not
       * there — and on a body that does have twenty, it is a command to move it to zero. */
      motor.mode() = MOTOR_MODE_SERVO;
      motor.q() = POS_STOP_F;
      motor.dq() = VEL_STOP_F;
      motor.kp() = 0.0f;
      motor.kd() = 0.0f;
      motor.tau() = 0.0f;
    }

    const auto values = cmd.values();
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
      auto &motor = out.motor_cmd()[i];
      if (!cmd.applies(i)) {
        /* §24's mask: say nothing about this joint. On an impedance drive that means both
         * sentinels and both gains at zero, which leaves the leg free — and is why the mask is
         * not "write the last value", which would be a command. */
        continue;
      }
      switch (mode) {
        case SKR_MODE_POSITION:
          motor.q() = values[i];
          motor.kp() = static_cast<float>(kp_);
          motor.dq() = 0.0f;
          motor.kd() = static_cast<float>(kd_);
          break;
        case SKR_MODE_VELOCITY:
          motor.dq() = values[i];
          motor.kd() = static_cast<float>(kd_);
          break;
        case SKR_MODE_TORQUE:
          motor.tau() = values[i];
          break;
        default:
          return SKR_EINVAL;
      }
    }
    out.crc() = command_crc(out);
    try {
      publisher_->Write(out);
    } catch (const std::exception &) {
      return SKR_EIO;
    }
    return SKR_OK;
  }

  int emergency_stop() override {
    /* §24's second rule. The latch is set first and unconditionally, so a command racing this one
     * is refused even if nothing can be published; then one message with every gain and every
     * torque at zero, which on an impedance drive is the definition of commanding nothing.
     *
     * What this is **not** is a power cut. A Go2's hardware stop is a button on the robot, and a
     * driver that reported SKR_OK for a message it merely wrote would be claiming the drives were
     * cut. The README says so under what a bench session has to measure. */
    estopped_ = true;
    if (!publisher_) return SKR_OK;
    unitree_go::msg::dds_::LowCmd_ out{};
    out.head()[0] = 0xFE;
    out.head()[1] = 0xEF;
    out.level_flag() = 0xFF;
    for (std::size_t i = 0; i < MOTOR_SLOTS; ++i) {
      auto &motor = out.motor_cmd()[i];
      motor.mode() = MOTOR_MODE_SERVO;
      motor.q() = POS_STOP_F;
      motor.dq() = VEL_STOP_F;
      motor.kp() = 0.0f;
      motor.kd() = 0.0f;
      motor.tau() = 0.0f;
    }
    out.crc() = command_crc(out);
    try {
      publisher_->Write(out);
    } catch (const std::exception &) {
      return SKR_EIO;
    }
    return SKR_OK;
  }

  std::string describe() override {
    std::string json = "{\"schema_version\":1,\"robot_slug\":\"" + slug_ +
                       "\",\"digest\":null,\"verified\":false,\"simulator\":false,"
                       "\"base\":\"legged\",\"dof\":12,\"joints\":[";
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
      if (i) json += ",";
      char buffer[320];
      std::snprintf(buffer, sizeof buffer,
                    "{\"name\":\"%s\",\"role\":\"%s\",\"type\":\"revolute\","
                    "\"limits\":{\"position_min_rad\":%.5f,\"position_max_rad\":%.5f},"
                    "\"control_modes\":[\"position\",\"velocity\",\"torque\"]}",
                    GO2_NAMES[i], GO2_ROLES[i], GO2_TRAVEL[i].low, GO2_TRAVEL[i].high);
      json += buffer;
    }
    json +=
        "],\"end_effectors\":[],\"sensors\":[],"
        "\"control\":{\"modes\":[\"position\",\"velocity\",\"torque\"],"
        "\"rate_hz_max\":500,\"rate_hz_min\":50,\"command_latency_ms\":null},"
        "\"envelope\":null,\"solvers\":[]}";
    return json;
  }

 private:
  static std::uint64_t now_ns() {
    return static_cast<std::uint64_t>(
        std::chrono::duration_cast<std::chrono::nanoseconds>(
            std::chrono::steady_clock::now().time_since_epoch())
            .count());
  }

  /* The CRC, over the message with its own `crc` field excluded — which is the only way a checksum
   * over a struct containing itself can work, and is what Unitree's example does by taking
   * `sizeof(LowCmd_) / 4 - 1` words. */
  static std::uint32_t command_crc(const unitree_go::msg::dds_::LowCmd_ &message) {
    return crc32_core(reinterpret_cast<const std::uint32_t *>(&message),
                      (sizeof(unitree_go::msg::dds_::LowCmd_) >> 2) - 1);
  }

  bool opened_ = false;
  std::atomic<bool> estopped_{false};
  std::uint32_t seq_ = 0;
  double kp_ = 0.0;
  double kd_ = 0.0;
  bool have_kp_ = false;
  bool have_kd_ = false;
  std::string slug_ = "unitree-go2";

  std::mutex lock_;
  bool have_state_ = false;
  std::uint64_t arrivals_ = 0;
  unitree_go::msg::dds_::LowState_ state_{};

  unitree::robot::ChannelPublisherPtr<unitree_go::msg::dds_::LowCmd_> publisher_;
  unitree::robot::ChannelSubscriberPtr<unitree_go::msg::dds_::LowState_> subscriber_;
};

}  // namespace

SKYNET_RHAL_EXPORT(UnitreeBridge)
