/*
 * skynet-booster — a Booster T1 over the vendor SDK's low-level channels.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 7 item 3: the buyable humanoids past Unitree. This is the driver;
 * `fake_robot.cpp` beside it is the substrate, and both use **the vendor's own SDK** (Apache-2.0),
 * which is what makes this row worth more than a transcription: the CDR encoding, the DDS discovery
 * and the topic types are Booster's code on both sides, so nothing here can be wrong about the wire
 * without being wrong in the same way a real robot would refuse.
 *
 * ── The interface, as the T1 manual documents it ─────────────────────────
 *
 * Commands publish to `rt/joint_ctrl`, states subscribe from `rt/low_state` (T1 Instruction
 * Manual v1.1, pp55-56). `LowCmd` carries a `cmd_type` — SERIAL or PARALLEL — and one `MotorCmd`
 * per joint: `q`, `dq`, `tau`, `kp`, `kd` and `weight`. `weight` in [0,1] blends the user's command
 * with the internal controller's (0 = fully internal, 1 = fully user), and is moot in custom mode,
 * where the internal controller sends nothing. This driver sends SERIAL with `weight = 1` on every
 * joint, every cycle, and reads the serial state vector — the manual's consistency rule ("ensure
 * that the serial-parallel consistency is also maintained") in both directions.
 *
 * Three facts shape everything below:
 *
 * 1. **The robot gates on custom mode, so the driver does not have to.** "The low level publish
 *    interface will only take effect when the robot is in custom mode" (p55). Entering custom mode
 *    is the operator's, through the high-level ChangeMode API or the joystick, the way UR's
 *    dashboard setup is the operator's. A driver commanding in WALK presents as a robot that ignores
 *    commands — annoying, legible, and the robot's documented behaviour, not a fight with the
 *    balance controller.
 *
 * 2. **There are no sentinels.** A Booster `MotorCmd` carries plain values, and a zero gain is what
 *    says "ignore this field": a velocity command sets `kp = 0` with the `dq` setpoint, a torque
 *    command zeroes both gains with `tau`. That reading assumes the standard impedance law behind
 *    the field set (`tau_out = tau + kp*(q - qm) + kd*(dq - dqm)`), which no obtainable Booster
 *    document states outright. It is marked as what it is — the bench confirms it — and the fake
 *    implements exactly that law, so a driver bug in the gains is still caught dry: the suite reads
 *    every field off the wire.
 *
 *    And because a zero gain is the ignore, §24's mask cannot be written as zero gains. A zero in
 *    the mask is *"hold this joint at its current setpoint"*, and the kernel writes an all-zero
 *    mask every cycle it has nothing to forward — so a masked joint carries its hold target with
 *    the operator's gains: its last position setpoint when it has one, otherwise the position
 *    measured when the hold began, latched once rather than re-read. Until 2026-09-24 this driver
 *    wrote zero gains there, the Unitree driver's reading, and Unitree's own simulator measured
 *    what that reading does to a standing body (DRY-COVERAGE-PLAN.md F121). With no gains
 *    configured a hold is refused rather than answered with a free joint.
 *
 * 3. **The manual and the SDK disagree about the arm, and the manual wins twice.** The SDK's
 *    `JointIndex` names IDs 4/5 (8/9) elbow pitch/yaw; the manual names them shoulder yaw + elbow
 *    in the product overview (p1) and again in the limit table (p3). The limits settle it: ±128°
 *    symmetric reads as a shoulder yaw, and +2°/-120° reads as an elbow that cannot hyperextend —
 *    an elbow yaw with 120° of one-sided travel would be a stranger joint. IDs and limits are
 *    unaffected either way; only the names and roles follow the manual. (MuJoCo Menagerie's T1
 *    follows the SDK's names — noted, not followed.)
 *
 * ── What the vendor did not publish ───────────────────────────────────────
 *
 * No joint torque anywhere in the manual or specs, and no joint speed, so per-joint `effort_max_nm`
 * is absent on all twenty-three joints and the envelope's torque ceiling is null — §63.4 and §5.3
 * refuse every alternative. No DDS rate either, so `rate_hz_max` is null. What the manual does give
 * is the limit table (p3-4, converted to radians below), the head/arm/waist/leg structure (p1), a
 * hard emergency stop fitted (pp1-2, the envelope's `estop`), and the onboard compute: an AGX Orin
 * on Basic, i7 + AGX Orin on Standard (p2) — a development platform with user compute, which earns
 * `platform.onboard_compute` on its own citation.
 *
 * ── The robot has a clock of its own ──────────────────────────────────────
 *
 * The body publishes `LowState` when it likes, whatever the caller does, so what a read returns is a
 * *sample*, and the two facts §24 gives a state are the sample's: the stamp is when it arrived — "at
 * acquisition, never at return" — and the sequence counts samples, so that a repeat is the stalled
 * producer the watchdog looks for. Until F278 (DRY-COVERAGE-PLAN.md, the Unitree driver's F277 found
 * again) a read stamped the state with the time of the read and advanced the sequence with the call,
 * so a robot whose link had been cut read as fresh for ever: the link-chaos contract's *outage
 * reported* question found it, with the subject's blackhole. Now the delivery thread stamps each
 * sample as it arrives, the sequence is the count of arrivals, and a read waits for a sample the last
 * read did not return — for the watchdog window, which is this driver's own definition of a robot
 * gone quiet — and then returns the last one with its sequence and stamp as they were. It is the
 * rule `drivers/lite3` (F104) and `drivers/ur_rtde` (F27) follow for a body with a clock of its own,
 * and the price they pay too: a read is held for the watchdog window on a robot that has stopped
 * publishing.
 *
 * Licence: Apache-2.0 (this file). It links booster_robotics_sdk, which is Apache-2.0 — neither is
 * vendored here; see the Makefile.
 */

#include "skynet_rhal.hpp"

#include <booster/idl/b1/LowCmd.h>
#include <booster/idl/b1/LowState.h>
#include <booster/robot/b1/b1_api_const.hpp>
#include <booster/robot/channel/channel_publisher.hpp>
#include <booster/robot/channel/channel_subscriber.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <iterator>
#include <mutex>
#include <thread>
#include <string>
#include <vector>

namespace {

constexpr int T1_JOINTS = 23;

/* Weight 1: fully user-controlled (manual p56). In custom mode the internal controller sends
 * nothing and the field is moot — it is sent as 1.0 anyway, because the driver's contract is
 * explicit full-user on every joint, and the fake counts anything less as a defect in the driver's
 * output. */
constexpr float WEIGHT_FULL_USER = 1.0f;

/* PMSM servo mode. `0x00` is idle and leaves the drive unpowered, which is not what a command
 * means. */
constexpr std::uint8_t MOTOR_MODE_SERVO = 0x01;

/* §24's fault word is one number and the robot's trouble has two parts: which joint, and what the
 * vendor says about it (the `lost` count). The joint goes in the high half, 1-based so that a word
 * of zero stays "nothing reported", and the vendor's own number in the low half — the layout the
 * CANopen driver uses for a node and its emergency code. A vendor number wider than sixteen bits
 * keeps its low sixteen, and the word is still non-zero for a non-zero joint (F205). */
constexpr std::uint32_t fault_word(int joint, std::uint32_t vendor) {
  return (static_cast<std::uint32_t>(joint + 1) << 16) | (vendor & 0xFFFFu);
}

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

bool config_bool(const std::string &config, const char *key, bool *out) {
  const std::string needle = std::string("\"") + key + "\"";
  const std::size_t at = config.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle.size());
  if (colon == std::string::npos) return false;
  const std::string rest = config.substr(colon + 1);
  const std::size_t start = rest.find_first_not_of(" \t\n\r");
  if (start == std::string::npos) return false;
  if (rest.compare(start, 4, "true") == 0) {
    *out = true;
    return true;
  }
  if (rest.compare(start, 5, "false") == 0) {
    *out = false;
    return true;
  }
  return false;
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

/* §5.1 roles in the SDK's ID order: head, arms, waist, legs. The head joints take the codebase's
 * own doubled form (`neck.neck.yaw` — what `skynet-urdf suggest` emits for a neck). The ankles are
 * pitch/roll because the driver speaks SERIAL, in which the parallel crank linkages read as series
 * ankle joints (MuJoCo Menagerie's series T1 names them `Ankle_Pitch`/`Ankle_Roll` the same way).
 * The arms follow the manual, not the SDK enum — see the file header. */
const char *const T1_ROLES[T1_JOINTS] = {
    "neck.neck.yaw", "neck.neck.pitch",
    "left_arm.shoulder.pitch", "left_arm.shoulder.roll", "left_arm.shoulder.yaw",
    "left_arm.elbow.flex",
    "right_arm.shoulder.pitch", "right_arm.shoulder.roll", "right_arm.shoulder.yaw",
    "right_arm.elbow.flex",
    "torso.waist.yaw",
    "left_leg.hip.pitch", "left_leg.hip.roll", "left_leg.hip.yaw", "left_leg.knee.flex",
    "left_leg.ankle.pitch", "left_leg.ankle.roll",
    "right_leg.hip.pitch", "right_leg.hip.roll", "right_leg.hip.yaw", "right_leg.knee.flex",
    "right_leg.ankle.pitch", "right_leg.ankle.roll",
};

const char *const T1_NAMES[T1_JOINTS] = {
    "head_yaw", "head_pitch",
    "L_shoulder_pitch", "L_shoulder_roll", "L_shoulder_yaw", "L_elbow",
    "R_shoulder_pitch", "R_shoulder_roll", "R_shoulder_yaw", "R_elbow",
    "waist_yaw",
    "L_hip_pitch", "L_hip_roll", "L_hip_yaw", "L_knee", "L_ankle_up", "L_ankle_down",
    "R_hip_pitch", "R_hip_roll", "R_hip_yaw", "R_knee", "R_ankle_up", "R_ankle_down",
};

/* Travel, radians: the manual's limit table (pp3-4, degrees, Max then Min) times pi/180. Travel is
 * a kinematic fact about the machine. */
struct Travel {
  double low;
  double high;
};
constexpr Travel T1_TRAVEL[T1_JOINTS] = {
    {-1.0122910, 1.0122910}, {-0.3141593, 0.8203047},
    {-3.2812190, 1.1868239}, {-1.6406095, 1.5358897}, {-2.2340214, 2.2340214},
    {-2.0943951, 0.0349066},
    {-3.2812190, 1.1868239}, {-1.5358897, 1.6406095}, {-2.2340214, 2.2340214},
    {-0.0349066, 2.0943951},
    {-1.0122910, 1.0122910},
    {-2.0594885, 2.0594885}, {-0.3665191, 1.5358897}, {-1.0122910, 1.0122910},
    {0.0000000, 2.1467550}, {-0.4014257, 0.8552113}, {-0.4188790, 0.7853982},
    {-2.0594885, 2.0594885}, {-1.5358897, 0.3665191}, {-1.0122910, 1.0122910},
    {0.0000000, 2.1467550}, {-0.4014257, 0.8552113}, {-0.4188790, 0.7853982},
};

/* The watchdog window, in milliseconds. The T1's DDS rate is unpublished, so unlike the Go2's this
 * window is not ten measured periods — it is driver design, stated as such in the envelope, and
 * §46.3's nesting wants it comfortably above the worst-case cycle either way. */
constexpr double T1_WATCHDOG_MS = 20.0;

class BoosterBridge : public skynet::Driver {
 public:
  int open(std::string_view config_json) override {
    if (opened_) return SKR_EPERM;
    const std::string config(config_json);

    /* Every field a previous session set, reset here rather than in the constructor.
     * `SKYNET_RHAL_EXPORT` builds the driver as a function-local static, so one process holds one
     * instance for its whole life (DRY-COVERAGE-PLAN.md F47). The estop latch is announced rather
     * than silently dropped. */
    if (estopped_) {
      std::fprintf(stderr,
                   "skynet-booster: the previous session left an emergency stop latched. This is "
                   "a new session and the latch does not carry into it — the robot's own state is "
                   "read from LowState_, and nothing here can tell you whether it is still "
                   "stopped.\n");
    }
    estopped_ = false;
    have_state_ = false;
    arrivals_ = 0;
    stamp_ns_ = 0;
    interval_ns_ = 0;
    reported_arrivals_ = 0;
    have_kp_ = have_kd_ = false;
    kp_ = kd_ = 0.0;
    /* No setpoint and no hold carries into a new session. */
    std::fill(std::begin(setpoint_), std::end(setpoint_), 0.0f);
    std::fill(std::begin(have_setpoint_), std::end(have_setpoint_), false);
    std::fill(std::begin(hold_), std::end(hold_), 0.0f);
    std::fill(std::begin(holding_), std::end(holding_), false);

    std::string interface;
    config_string(config, "interface", &interface);
    config_string(config, "robot_slug", &slug_);
    /* Gains, and there is deliberately no default — §63.4 through a gain, the way the Unitree
     * driver refuses it: a stiffness nobody chose, in front of a Safety Kernel that cannot see
     * it. */
    have_kp_ = config_number(config, "kp", &kp_);
    have_kd_ = config_number(config, "kd", &kd_);
    /* Declared by the operator; the default is the safe direction (§35.3). */
    simulator_ = true;
    config_bool(config, "simulator", &simulator_);

    try {
      /* An empty interface is the SDK default (all interfaces); `lo` keeps the substrate inside
       * this machine. A test that discovered a robot on the office network would be a test that
       * commanded one. */
      booster::robot::ChannelFactory::Instance()->Init(0, interface);
      publisher_.reset(
          new booster::robot::ChannelPublisher<booster_interface::msg::LowCmd>(
              booster::robot::b1::kTopicJointCtrl));
      publisher_->InitChannel();
      subscriber_.reset(
          new booster::robot::ChannelSubscriber<booster_interface::msg::LowState>(
              booster::robot::b1::kTopicLowState));
      subscriber_->InitChannel(
          [this](const void *message) {
            std::lock_guard<std::mutex> guard(lock_);
            state_ = *static_cast<const booster_interface::msg::LowState *>(message);
            note_arrival();
          });
    } catch (const std::exception &error) {
      std::fprintf(stderr, "skynet-booster: could not open DDS: %s\n", error.what());
      return SKR_EIO;
    }

    /* The first state, waited for rather than assumed — and checked for the T1's layout. The SDK
     * header warns that a command with the wrong layout "can address the wrong actuator or cause
     * the robot to reject the message"; the same holds for reading one, so a state whose serial
     * vector is not 23 joints is refused rather than indexed. */
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
                     "skynet-booster: no LowState arrived on `%s` within five seconds. Either "
                     "nothing is publishing `%s`, or the T1 is not on this network.\n",
                     booster::robot::b1::kTopicLowState.c_str(),
                     booster::robot::b1::kTopicLowState.c_str());
        return SKR_EIO;
      }
      if (state_.motor_state_serial().size() != T1_JOINTS) {
        std::fprintf(stderr,
                     "skynet-booster: the state on `%s` carries %zu serial joints, not the T1's "
                     "23. This driver speaks the T1 layout only (a K1 or T2 would need its own "
                     "row), and it will not index a layout it does not speak.\n",
                     booster::robot::b1::kTopicLowState.c_str(),
                     state_.motor_state_serial().size());
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
    {
      /* A read that is waiting for the robot's next state has nothing left to wait for. */
      std::lock_guard<std::mutex> guard(lock_);
      have_state_ = false;
      arrived_.notify_all();
    }
    return SKR_OK;
  }

  int read_state(skynet::State &out, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    booster_interface::msg::LowState snapshot;
    Latest latest;
    if (!take_latest(&snapshot, &latest)) return SKR_EIO;
    /* The layout is re-checked on every read, not just at open: a K1 publishing onto the same
     * topic mid-session would otherwise be indexed as a T1. */
    if (snapshot.motor_state_serial().size() != T1_JOINTS) return SKR_EIO;
    if (out.joint_count() < T1_JOINTS) return SKR_EINVAL;

    std::uint16_t flags = 0;
    /* §27's homing, by a chain rather than a datasheet: the documented path to custom mode runs
     * through PREP, in which the T1 "assumes a standing posture and holds it" (manual p8) — a body
     * that stands from boot knows where its joints are. Saying so is not the same as saying
     * nothing — `unreported` holds the robot. */
    flags |= SKR_HOMING_NOT_REQUIRED;
    if (estopped_) flags |= SKR_FLAG_ESTOP;

    /* The first joint reporting a non-zero `lost` count, with *both* halves of what that is in
     * §24's one word: the joint's 1-based ordinal in the high half and the vendor's own number in
     * the low half (`fault_word`). The word used to be the ordinal alone, under a comment that
     * called it the vendor's word verbatim, and the number the maker defined was thrown away
     * (F205). */
    std::uint32_t fault = 0;
    for (int i = 0; i < T1_JOINTS; ++i) {
      const std::uint32_t lost = snapshot.motor_state_serial()[i].lost();
      if (lost != 0) {
        fault = fault_word(i, lost);
        flags |= SKR_FLAG_DEGRADED;
        break;
      }
    }

    /* The sample's own stamp and count: the file header's "The robot has a clock of its own". */
    out.set_stamp(latest.stamp_ns);
    out.set_seq(static_cast<std::uint32_t>(latest.arrivals));
    out.set_flags(flags);
    out.set_fault(fault);
    auto position = out.position();
    auto velocity = out.velocity();
    auto effort = out.effort();
    for (int i = 0; i < T1_JOINTS; ++i) {
      position[i] = snapshot.motor_state_serial()[i].q();
      velocity[i] = snapshot.motor_state_serial()[i].dq();
      /* `tau_est` is the drive's estimate, not a torque sensor — still a measurement rather than
       * §24's NaN. */
      effort[i] = snapshot.motor_state_serial()[i].tau_est();
    }
    out.set_joint_count(T1_JOINTS);
    /* The IMU gives orientation and nothing gives position — the T1's odometry lives on a topic
     * this driver does not read, and a driver that wrote the IMU quaternion into a pose would be
     * claiming a position it does not have. */
    auto pose = out.base_pose();
    for (std::size_t i = 0; i < 7; ++i) pose[i] = std::nanf("");
    return SKR_OK;
  }

  int write_command(const skynet::Command &cmd, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    if (estopped_) return SKR_EPERM;
    if (cmd.joint_count() != T1_JOINTS) return SKR_EINVAL;
    const int mode = cmd.mode();
    if (mode != SKR_MODE_POSITION && mode != SKR_MODE_VELOCITY && mode != SKR_MODE_TORQUE) {
      return SKR_EINVAL;
    }
    const bool gains = have_kp_ && have_kd_;
    if ((mode == SKR_MODE_POSITION || mode == SKR_MODE_VELOCITY) && !gains) {
      std::fprintf(stderr,
                   "skynet-booster: a %s command needs `kp` and `kd` in this driver's config. "
                   "A Booster drive takes an impedance command, not a setpoint, and this driver "
                   "will not choose a stiffness the operator did not (§63.4).\n",
                   mode == SKR_MODE_POSITION ? "position" : "velocity");
      return SKR_EPERM;
    }
    bool any_masked = false;
    for (int i = 0; i < T1_JOINTS; ++i) {
      if (!cmd.applies(i)) any_masked = true;
    }
    if (any_masked && !gains) {
      std::fprintf(stderr,
                   "skynet-booster: this command masks a joint, and a masked joint is §24's hold "
                   "— held at its current setpoint. On an impedance drive a hold is a stiffness, "
                   "and with no `kp` and `kd` in this driver's config there is nothing to hold "
                   "with. Going limp instead is the one reading §24 rules out, so the command is "
                   "refused; configure the gains the robot should hold with (§63.4).\n");
      return SKR_EPERM;
    }

    const auto values = cmd.values();
    /* A setpoint that is not a number is refused with the other malformed commands, before anything is built or committed (F282). Nothing
     * below bounds one — a NaN position is written to the message as it is, a NaN torque is a force — and a NaN position setpoint
     * would be latched as the hold every later frame repeats. A joint the mask holds is not read, so it is not checked. */
    for (int i = 0; i < T1_JOINTS; ++i) {
      if (!cmd.applies(i) || std::isfinite(values[i])) continue;
      std::fprintf(stderr,
                   "skynet-booster: the setpoint for joint %d is not a finite number. A NaN or an "
                   "infinity would be published to the robot as it is, so the whole command is "
                   "refused; send a number (§24).\n",
                   i);
      return SKR_EINVAL;
    }
    /* §24's hold, resolved per joint before anything is published, on copies committed only once
     * the message is on the wire. Only a position command leaves a position setpoint; after any
     * other a hold latches where the joint is measured to be. */
    float setpoint[T1_JOINTS];
    bool have_setpoint[T1_JOINTS];
    float hold[T1_JOINTS];
    bool holding[T1_JOINTS];
    std::copy(std::begin(setpoint_), std::end(setpoint_), setpoint);
    std::copy(std::begin(have_setpoint_), std::end(have_setpoint_), have_setpoint);
    std::copy(std::begin(hold_), std::end(hold_), hold);
    std::copy(std::begin(holding_), std::end(holding_), holding);
    for (int i = 0; i < T1_JOINTS; ++i) {
      if (cmd.applies(i)) {
        holding[i] = false;
        have_setpoint[i] = mode == SKR_MODE_POSITION;
        if (have_setpoint[i]) setpoint[i] = values[i];
        continue;
      }
      if (!holding[i]) {
        hold[i] = have_setpoint[i] ? setpoint[i] : measured_position(i);
        holding[i] = true;
      }
    }

    booster_interface::msg::LowCmd out;
    /* SERIAL: joints as joints, the ankle linkages addressed as pitch/roll with the conversion
     * inside the robot — and the same view the driver reads, per the manual's consistency rule. */
    out.cmd_type() = booster_interface::msg::SERIAL;
    std::vector<booster_interface::msg::MotorCmd> motors(T1_JOINTS);
    for (int i = 0; i < T1_JOINTS; ++i) {
      motors[i].mode() = MOTOR_MODE_SERVO;
      motors[i].weight() = WEIGHT_FULL_USER;
    }
    for (int i = 0; i < T1_JOINTS; ++i) {
      if (!cmd.applies(i)) {
        /* §24's hold: the latched target with the operator's gains and a zero velocity setpoint
         * (the file header, fact 2). */
        motors[i].q() = hold[i];
        motors[i].kp() = static_cast<float>(kp_);
        motors[i].dq() = 0.0f;
        motors[i].kd() = static_cast<float>(kd_);
        motors[i].tau() = 0.0f;
        continue;
      }
      switch (mode) {
        case SKR_MODE_POSITION:
          motors[i].q() = values[i];
          motors[i].kp() = static_cast<float>(kp_);
          motors[i].dq() = 0.0f;
          motors[i].kd() = static_cast<float>(kd_);
          break;
        case SKR_MODE_VELOCITY:
          /* `kp = 0` is what says "nothing about position" on a message with no sentinels. */
          motors[i].dq() = values[i];
          motors[i].kd() = static_cast<float>(kd_);
          break;
        case SKR_MODE_TORQUE:
          motors[i].tau() = values[i];
          break;
        default:
          return SKR_EINVAL;
      }
    }
    out.motor_cmd() = std::move(motors);
    if (!publisher_->Write(&out)) return SKR_EIO;
    std::copy(setpoint, setpoint + T1_JOINTS, setpoint_);
    std::copy(have_setpoint, have_setpoint + T1_JOINTS, have_setpoint_);
    std::copy(hold, hold + T1_JOINTS, hold_);
    std::copy(holding, holding + T1_JOINTS, holding_);
    return SKR_OK;
  }

  int emergency_stop() override {
    /* §24's second rule. The latch is set first and unconditionally; then one message with every
     * gain and every torque at zero, which under the impedance law is the definition of commanding
     * nothing. What this is **not** is the T1's hard emergency stop (manual p1): a button on the
     * robot, which this driver cannot press and does not claim to. */
    estopped_ = true;
    if (!publisher_) return SKR_OK;
    booster_interface::msg::LowCmd out;
    out.cmd_type() = booster_interface::msg::SERIAL;
    std::vector<booster_interface::msg::MotorCmd> motors(T1_JOINTS);
    for (int i = 0; i < T1_JOINTS; ++i) {
      motors[i].mode() = MOTOR_MODE_SERVO;
      motors[i].weight() = WEIGHT_FULL_USER;
      motors[i].q() = 0.0f;
      motors[i].dq() = 0.0f;
      motors[i].tau() = 0.0f;
      motors[i].kp() = 0.0f;
      motors[i].kd() = 0.0f;
    }
    out.motor_cmd() = std::move(motors);
    if (!publisher_->Write(&out)) return SKR_EIO;
    return SKR_OK;
  }

  /* Clear the session latch for §48.2's power-on self-test. The latch clears; nothing resumes:
   * the stop published zero gains and zero torques, and the next command decides what moves.
   * Idempotent. */
  int release_stop() override {
    estopped_ = false;
    return SKR_OK;
  }

  std::string describe() override {
    std::string json = "{\"schema_version\":1,\"robot_slug\":\"" + slug_ +
                       "\",\"digest\":null,\"verified\":false,\"simulator\":" +
                       (simulator_ ? "true" : "false") +
                       ",\"base\":\"legged\",\"dof\":23,\"joints\":[";
    for (int i = 0; i < T1_JOINTS; ++i) {
      if (i) json += ",";
      /* No `effort_max_nm` on any joint: Booster publishes no joint torque, and a rating invented
       * here would be exactly the brochure §63.4 refuses. No velocity either, for the same
       * reason. */
      char buffer[512];
      std::snprintf(buffer, sizeof buffer,
                    "{\"name\":\"%s\",\"role\":\"%s\",\"type\":\"revolute\","
                    "\"limits\":{\"position_min_rad\":%.5f,\"position_max_rad\":%.5f},"
                    "\"control_modes\":[\"position\",\"velocity\",\"torque\"]}",
                    T1_NAMES[i], T1_ROLES[i], T1_TRAVEL[i].low, T1_TRAVEL[i].high);
      json += buffer;
    }
    /* The envelope with a null torque ceiling: honest about the watchdog and the fitted estop,
     * silent about the rating nobody published. */
    json +=
        "],\"end_effectors\":[],\"sensors\":[],"
        "\"control\":{\"modes\":[\"position\",\"velocity\",\"torque\"],"
        "\"rate_hz_max\":null,\"rate_hz_min\":null,\"command_latency_ms\":null},"
        "\"envelope\":{\"joint_torque_max_nm\":null,\"watchdog_ms\":20.0,\"estop\":true,"
        "\"source\":\"Booster T1 Instruction Manual v1.1: joint limit table pp3-4 (travel); no "
        "joint torque and no joint speed published; hard emergency stop fitted pp1-2; watchdog "
        "per driver design\","
        "\"basis\":\"declared\"}";
    /* `emergency_stop` latches, publishes zero gains and zero torques, and refuses every later
     * command — the suite covers all three — so the software stop the driver actually has is the
     * capability it declares. The hardware stop stays a fitted fact in the envelope, not a driver
     * action.
     *
     * `platform.onboard_compute` is the second declaration, on the manual's own citation (p2):
     * an AGX Orin on the Basic T1, i7 + AGX Orin on the Standard — and the T1 is sold as "a
     * humanoid robot development platform" (p1), which is what user-programmable means. What the
     * token does not claim is that the OS runs there today: readiness is paperwork, and the
     * deployment and the bench are the evidence. */
    json += ",\"capabilities\":[\"safety.estop\",\"platform.onboard_compute\"],\"solvers\":[]}";
    return json;
  }

 private:
  /* Where the robot last said this joint was. `open()` waited for a state with the T1's layout,
   * and `read_state` re-checks it, so the index is in range. */
  float measured_position(int joint) {
    std::lock_guard<std::mutex> guard(lock_);
    return state_.motor_state_serial()[joint].q();
  }

  /* A sample has just been stored: when it arrived, how often they come, and the count. Called by the
   * delivery thread with `lock_` held. */
  void note_arrival() {
    constexpr std::uint64_t GAP_CAP_NS = 100ull * 1000ull * 1000ull;
    const std::uint64_t now = skynet::acquisition_ns();
    if (arrivals_ > 0 && now > stamp_ns_) {
      const std::int64_t gap = static_cast<std::int64_t>(std::min(now - stamp_ns_, GAP_CAP_NS));
      if (interval_ns_ == 0) {
        interval_ns_ = static_cast<std::uint64_t>(gap);
      } else {
        /* An exponential average of the gaps, each held to a tenth of a second so that a long silence is a
         * moment's rise and not a number the average cannot come back from. */
        const std::int64_t average = static_cast<std::int64_t>(interval_ns_);
        interval_ns_ = static_cast<std::uint64_t>(average + (gap - average) / 8);
      }
    }
    stamp_ns_ = now;
    have_state_ = true;
    arrivals_ += 1;
    arrived_.notify_all();
  }

  /* What a read says about the state it returns: when the sample arrived, and its place in the count of
   * everything the robot has sent this session. */
  struct Latest {
    std::uint64_t arrivals = 0;
    std::uint64_t stamp_ns = 0;
  };

  /* The robot's latest sample, once it is one the last read did not return — or, when none arrives within
   * `wait_bound()`, the same one again with the count and the stamp it had: the file header's "The robot
   * has a clock of its own". False when no state has arrived since this session opened, or the session
   * closed while this read waited. */
  bool take_latest(booster_interface::msg::LowState *snapshot, Latest *latest) {
    std::unique_lock<std::mutex> guard(lock_);
    if (!have_state_) return false;
    if (arrivals_ <= reported_arrivals_) {
      arrived_.wait_for(guard, wait_bound(), [this] { return arrivals_ > reported_arrivals_ || !have_state_; });
      if (!have_state_) return false;
    }
    *snapshot = state_;
    latest->arrivals = arrivals_;
    latest->stamp_ns = stamp_ns_;
    reported_arrivals_ = arrivals_;
    return true;
  }

  /* How long a read waits for a state the last one did not return before it calls the robot quiet: the
   * watchdog window — this driver's own definition of a robot gone quiet — or twice the interval the robot
   * has been measured publishing at when that is longer (a body slower than fifty hertz), and never more
   * than a tenth of a second. Called with `lock_` held. */
  std::chrono::nanoseconds wait_bound() const {
    constexpr std::uint64_t WATCHDOG_NS = static_cast<std::uint64_t>(T1_WATCHDOG_MS * 1.0e6);
    constexpr std::uint64_t CAP_NS = 100ull * 1000ull * 1000ull;
    return std::chrono::nanoseconds(std::min(std::max(WATCHDOG_NS, 2 * interval_ns_), CAP_NS));
  }

  bool opened_ = false;
  std::atomic<bool> estopped_{false};
  double kp_ = 0.0;
  double kd_ = 0.0;
  bool have_kp_ = false;
  bool have_kd_ = false;
  std::string slug_ = "booster-t1";
  bool simulator_ = true;

  /* §24's hold, per joint: the last position setpoint and whether there is one, and the latched
   * hold target and whether the joint is being held. */
  float setpoint_[T1_JOINTS] = {};
  bool have_setpoint_[T1_JOINTS] = {};
  float hold_[T1_JOINTS] = {};
  bool holding_[T1_JOINTS] = {};

  std::mutex lock_;
  /* Signalled by every sample that arrives, and by `close()`: a read that is waiting for the robot's
   * next state wakes on it. */
  std::condition_variable arrived_;
  bool have_state_ = false;
  std::uint64_t arrivals_ = 0;
  /* When the latest sample arrived, CLOCK_MONOTONIC_RAW, taken in the delivery thread: §24's
   * acquisition, which a read hands on as the state's stamp and never replaces with its own time. */
  std::uint64_t stamp_ns_ = 0;
  /* The interval the robot publishes at, as measured (`note_arrival`); zero until two samples have
   * arrived. */
  std::uint64_t interval_ns_ = 0;
  /* How many of the robot's samples this session has already returned: a read that finds the count has
   * not moved has nothing new to say. */
  std::uint64_t reported_arrivals_ = 0;
  booster_interface::msg::LowState state_{};

  std::shared_ptr<booster::robot::ChannelPublisher<booster_interface::msg::LowCmd>> publisher_;
  std::shared_ptr<booster::robot::ChannelSubscriber<booster_interface::msg::LowState>> subscriber_;
};

}  // namespace

SKYNET_RHAL_EXPORT(BoosterBridge)