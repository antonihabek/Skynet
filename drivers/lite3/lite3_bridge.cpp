/*
 * skynet-lite3 — a Jueying Lite3 quadruped over Deep Robotics' MotionSDK wire.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 7, quadruped row: *"Lite3 MotionSDK public MIT, joint-level UDP;
 * no vendor sim or fake; no X20 SDK (F92)"*, and F92 specifies the shape — *"Fourier-shaped, SDK
 * client unmodified against a fake motion host"*. This is the driver; `fake_motion_host.cpp`
 * beside it is the substrate, and `cross_check.cpp` drives that substrate with the **vendor's own
 * `Sender` and `Receiver`**, unmodified, so the datagram this driver writes is held against the
 * one the vendor's client writes rather than against this tree's reading of a header.
 *
 * ── Why the driver does not link the vendor's library ────────────────────
 *
 * MotionSDK ships prebuilt shared objects for x86_64 and aarch64 and a `Sender`/`Receiver` pair
 * whose shape is a demo's rather than a driver's: `Receiver`'s port is fixed inside the library,
 * `RobotStateInit()` moves every joint to zero and then sleeps seven seconds, `ControlGet(ROBOT)`
 * sleeps two more, and none of it can be called from `emergency_stop()`, which §4 requires to work
 * from a signal handler and from a dying process. A driver built on those entry points would move
 * a robot at `open()` and block in its stop.
 *
 * So the wire is spoken directly — it is three words and a struct, all of it in the vendor's own
 * MIT headers — and the vendor's library is put where it is worth most instead: on the *other* end
 * of the substrate, as the independent implementation this row is measured against. That is the
 * Fourier arrangement exactly, and `lite3_wire.hpp` records where every constant came from,
 * including which ones were read out of the prebuilt library rather than out of a document.
 *
 * ── The three things the mapping has to get right ─────────────────────────
 *
 * §24 commands a body in one mode and `JointCmd` is not a mode at all. It is an impedance command:
 * every joint carries `position`, `velocity`, `torque`, `kp` and `kd` at once, and the drive
 * computes, at 20 kHz (MotionSDK README §2),
 *
 *     T = kp * (pos_goal - pos_real) + kd * (vel_goal - vel_real) + t_ff
 *
 * 1. **Zero gains are this wire's "say nothing", and they are also a limp leg.** Unitree needed
 *    sentinel values because a zero in `q` is a command to go to zero radians; here the same
 *    protection falls out of the arithmetic, since `kp = kd = t_ff = 0` makes `T` zero whatever
 *    the setpoints say. What does *not* fall out is §24's mask: a masked joint on this wire cannot
 *    be left out of the datagram, because the datagram is all twelve joints or nothing, so a zero
 *    in the mask has to become bytes. §24 says which: *"hold this joint at its current setpoint"*,
 *    and §25 *"a zero mask holds position"*. So a masked joint carries its hold target with the
 *    operator's `kp` and `kd` — its last position setpoint when it has one, otherwise the position
 *    measured when the hold began, latched once rather than re-read. With no gains configured a
 *    command that masks a joint is refused, because the only hold this wire could then write is a
 *    slack leg.
 *
 *    Until 2026-09-24 this driver read the mask the other way: limp by default, or damped at a
 *    configured `hold_kd`, and never the hold, which it called *"a command the OS never issued"*.
 *    §24's "current setpoint" is the answer to that — the OS issued it, as the setpoint it last
 *    sent or as the hold it is sending now — and Unitree's own simulator measured what the limp
 *    reading does to a standing quadruped: `skynet-kernel-host` writes an all-zero mask every
 *    cycle it has nothing to forward, and the first such cycle folded a Go2 from 0.385 m to
 *    0.106 m (DRY-COVERAGE-PLAN.md F121, carried here as item 8.13). `hold_kd` is refused by name
 *    rather than ignored, so a configuration written for the old reading does not silently get
 *    the new one.
 *
 * 2. **`kp` and `kd` are the operator's, and are not guessed.** They are not in a §24 command. A
 *    position command with no `kp` and `kd` in this driver's config is refused, and a velocity
 *    command with no `kd`; a torque command needs neither and is accepted. §63.4's rule arriving
 *    through a gain rather than through a rating — `drivers/unitree` makes the same refusal about
 *    the same kind of drive.
 *
 * 3. **Control right is a state on the robot, and taking it is a command.** The underlying
 *    controller holds the legs until the SDK takes control (`0x0114`), and takes them back one
 *    second after the last command (MotionSDK README §2). This driver takes control in the same
 *    call that sends the first joint command, never at `open()`: a session that took control and
 *    then commanded nothing would be a robot whose own controller had been told to stand aside
 *    for a driver that had not started driving.
 *
 * ── Where the numbers come from ──────────────────────────────────────────
 *
 * Joint names, travel, velocity and torque ratings are all Deep Robotics' own, from
 * `DeepRoboticsLab/Lite3_rl_deploy` (Apache-2.0): the names from
 * `run_policy/lite3_test_policy_runner_onnx.cpp`, and the figures from
 * `state_machine/parameters/lite3_control_parameters.cpp`:
 *
 *     fl_joint_lower_ << -0.530, -3.50, 0.349;      fl_joint_upper_ << 0.530, 0.320, 2.80;
 *     joint_vel_limit_ << 30, 30, 20;               torque_limit_ << 40, 40, 65;
 *
 * The mirror across the body is the vendor's too — `idle_state.hpp`'s `JointDataNormalCheck`
 * builds the right legs as `(-fl_upper(0), fl_lower(1), fl_lower(2))` and applies the front pair's
 * bounds to the hind pair — and the abduction range being symmetric is why all twelve joints end
 * up with the same numbers. Nothing here is a CAD value or a community figure.
 *
 * That makes this the **first legged body in this tree whose `joint_ratings` readiness check is
 * met**: every Unitree body, the Fourier GR-1, the Booster T1 and the AgiBot X1 declare a torque
 * ceiling and no joint velocity, because their vendors publish none (F64). Deep Robotics publishes
 * both, in code, per joint.
 *
 * Licence: Apache-2.0 (this file). It speaks a wire described by MotionSDK's MIT headers and links
 * nothing of the vendor's; `cross_check.cpp` is the only part that needs a checkout.
 */

#include "skynet_rhal.hpp"
#include "lite3_wire.hpp"

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>

namespace {

using namespace skynet_lite3;

/* ── the body ─────────────────────────────────────────────────────────── */

struct Travel {
  double low;
  double high;
};

/* Deep Robotics' own canonical order: front-left, front-right, hind-left, hind-right, each
 * HipX (abduction), HipY (hip flexion), Knee. `JOINT_NAMES` in the shared header. */
const char *const ROLES[JOINTS] = {
    "left_leg.hip.abduct#0",  "left_leg.thigh.pitch#0",  "left_leg.calf.flex#0",
    "right_leg.hip.abduct#0", "right_leg.thigh.pitch#0", "right_leg.calf.flex#0",
    "left_leg.hip.abduct#1",  "left_leg.thigh.pitch#1",  "left_leg.calf.flex#1",
    "right_leg.hip.abduct#1", "right_leg.thigh.pitch#1", "right_leg.calf.flex#1",
};

/* The vendor's `fl_joint_lower_` / `fl_joint_upper_`, mirrored by the vendor's own rule. The
 * right legs' abduction bound is `(-upper, -lower)`, which for a symmetric range is the same
 * pair of numbers — so the mirror is applied rather than assumed away, and it happens to be the
 * identity here. */
constexpr Travel TRAVEL[JOINTS] = {
    {-0.530, 0.530}, {-3.50, 0.320}, {0.349, 2.80}, {-0.530, 0.530},
    {-3.50, 0.320},  {0.349, 2.80},  {-0.530, 0.530}, {-3.50, 0.320},
    {0.349, 2.80},   {-0.530, 0.530}, {-3.50, 0.320}, {0.349, 2.80},
};

/* `joint_vel_limit_ << 30, 30, 20`, indexed `i % 3` exactly as the vendor indexes it. */
constexpr double VELOCITY_MAX[3] = {30.0, 30.0, 20.0};
/* `torque_limit_ << 40, 40, 65`. */
constexpr double EFFORT_MAX[3] = {40.0, 40.0, 65.0};

/* The whole-body ceilings the envelope carries: the largest joint's of each, the convention every
 * other envelope in this tree uses. The per-joint figures above are the finer truth and are
 * declared per joint, where §86's per-channel ceiling takes precedence (F36). */
constexpr double ENVELOPE_VELOCITY_MAX = 30.0;
constexpr double ENVELOPE_TORQUE_MAX = 65.0;

constexpr const char *ENVELOPE_SOURCE =
    "DeepRoboticsLab/Lite3_rl_deploy, state_machine/parameters/lite3_control_parameters.cpp: "
    "joint_vel_limit_ 30/30/20 rad/s and torque_limit_ 40/40/65 N.m per HipX/HipY/Knee, with "
    "travel fl_joint_lower_/-upper_ mirrored by the vendor's own idle_state.hpp rule; whole-body "
    "figures are the largest joint's of each and the per-joint ratings are declared per joint; "
    "watchdog is driver design at a fifth of MotionSDK README 2's one-second reclaim";

/* A fifth of the vendor's reclaim. The second is the *robot's* window and this driver does not
 * get to call it its own: what the descriptor declares is the window this driver expects the OS
 * to command inside, chosen so that a missed command still leaves four more before the underlying
 * controller takes the legs back. */
constexpr double WATCHDOG_MS = 200.0;

/* The one token this driver serves. §24's `State` has nowhere for a body IMU, a joint temperature
 * or a foot force, and `read_sensor` is a frame channel for cameras — so the rest of what the
 * robot reports about itself is served here, verbatim, rather than dropped or bent into a field
 * that means something else. */
constexpr const char *TELEMETRY_TOKEN = "vendor.lite3.telemetry";

/* ── config ───────────────────────────────────────────────────────────── */

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

constexpr double DEG_TO_RAD = 3.14159265358979323846 / 180.0;

class Lite3Bridge : public skynet::Driver {
 public:
  int open(std::string_view config_json) override {
    if (opened_) return SKR_EPERM;
    const std::string config(config_json);

    /* Every field a previous session set, reset here rather than in the constructor:
     * `SKYNET_RHAL_EXPORT` holds one instance for the process's life, so `close()` then `open()`
     * lands on the same object and the estop latch would otherwise carry into a session that
     * could not see why it was refusing (the F47 shape). The latch is announced rather than
     * silently dropped. */
    if (estopped_.load()) {
      std::fprintf(stderr,
                   "skynet-lite3: the previous session left an emergency stop latched. This is a "
                   "new session and the latch does not carry into it — what went out on the wire "
                   "was a damping command and a hand-back, and nothing here can tell you what the "
                   "robot's own controller has done since.\n");
    }
    estopped_.store(false);
    control_taken_.store(false);
    have_state_ = false;
    arrivals_ = 0;
    reported_arrivals_ = 0;
    interval_ns_ = 0;
    stale_cycles_ = 0;
    malformed_ = 0;
    short_datagrams_ = 0;
    have_kp_ = have_kd_ = false;
    kp_ = kd_ = 0.0;
    /* No setpoint and no hold carries into a new session. */
    for (int i = 0; i < JOINTS; ++i) {
      setpoint_[i] = 0.0f;
      have_setpoint_[i] = false;
      hold_[i] = 0.0f;
      holding_[i] = false;
    }

    std::string model = "lite3";
    config_string(config, "model", &model);
    if (model != "lite3") {
      /* The X20 shares the family and not the SDK: Deep Robotics publishes no MotionSDK for it
       * (F92), so a driver claiming it would be claiming a wire nobody has published. */
      std::fprintf(stderr,
                   "skynet-lite3: unknown `model` \"%s\". This driver speaks the Jueying Lite3's "
                   "MotionSDK wire and nothing else — the X20 is the same vendor and has no "
                   "published SDK (DRY-COVERAGE-PLAN.md F92).\n",
                   model.c_str());
      return SKR_EINVAL;
    }

    host_ = "127.0.0.1";
    config_string(config, "host", &host_);
    slug_ = "deeprobotics-lite3";
    config_string(config, "robot_slug", &slug_);
    simulator_ = true;
    config_bool(config, "simulator", &simulator_);

    double port = COMMAND_PORT;
    config_number(config, "command_port", &port);
    command_port_ = static_cast<std::uint16_t>(port);
    port = STATE_PORT;
    config_number(config, "state_port", &port);
    state_port_ = static_cast<std::uint16_t>(port);

    have_kp_ = config_number(config, "kp", &kp_);
    have_kd_ = config_number(config, "kd", &kd_);
    double retired_hold_kd = 0.0;
    if (config_number(config, "hold_kd", &retired_hold_kd)) {
      std::fprintf(stderr,
                   "skynet-lite3: `hold_kd` is no longer a setting. It chose between a slack and a "
                   "damped leg for a masked joint, and §24 says a masked joint is held at its "
                   "current setpoint — which this driver now does, with `kp` and `kd` "
                   "(DRY-COVERAGE-PLAN.md F121). Remove it; refused rather than ignored so this "
                   "configuration does not silently get a stiffer robot than it asked for.\n");
      return SKR_EINVAL;
    }

    double open_timeout = 3000.0;
    config_number(config, "open_timeout_ms", &open_timeout);
    double handback = 2000.0;
    config_number(config, "handback_ms", &handback);
    handback_ms_ = handback;
    double guard = RECLAIM_MS / 2.0;
    config_number(config, "reclaim_guard_ms", &guard);
    if (!(guard > 0.0)) {
      std::fprintf(stderr, "skynet-lite3: `reclaim_guard_ms` must be positive (got %g).\n", guard);
      return SKR_EINVAL;
    }
    reclaim_guard_ns_ = static_cast<std::uint64_t>(guard * 1e6);
    last_sent_ns_ = 0;
    retakes_.store(0);

    /* The state socket first, so nothing is commanded before the body is listened to. */
    state_fd_ = ::socket(AF_INET, SOCK_DGRAM, 0);
    if (state_fd_ < 0) return SKR_EIO;
    int reuse = 1;
    ::setsockopt(state_fd_, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof reuse);
    sockaddr_in local{};
    local.sin_family = AF_INET;
    local.sin_addr.s_addr = htonl(INADDR_ANY);
    local.sin_port = htons(state_port_);
    if (::bind(state_fd_, reinterpret_cast<const sockaddr *>(&local), sizeof local) < 0) {
      std::fprintf(stderr,
                   "skynet-lite3: cannot bind UDP %u for the robot's state push. The motion host "
                   "sends to the port in its own `~/jy_exe/conf/network.toml` (`target_port`), so "
                   "either something else holds this port or the robot is configured for another "
                   "one.\n",
                   static_cast<unsigned>(state_port_));
      teardown();
      return SKR_EIO;
    }
    /* A receive timeout so the thread wakes to notice `running_` rather than being cancelled
     * inside a blocking call. */
    timeval timeout{};
    timeout.tv_sec = 0;
    timeout.tv_usec = 100000;
    ::setsockopt(state_fd_, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);

    /* The command socket is **connected**, which is what makes `emergency_stop()` legal: `send`
     * on a connected socket is async-signal-safe, and `sendto` with an address the handler would
     * have to format is not. Everything the stop sends is built here too, for the same reason. */
    cmd_fd_ = ::socket(AF_INET, SOCK_DGRAM, 0);
    if (cmd_fd_ < 0) {
      teardown();
      return SKR_EIO;
    }
    sockaddr_in remote{};
    remote.sin_family = AF_INET;
    remote.sin_port = htons(command_port_);
    if (::inet_pton(AF_INET, host_.c_str(), &remote.sin_addr) != 1) {
      std::fprintf(stderr,
                   "skynet-lite3: `host` \"%s\" is not an IPv4 address. The motion host is "
                   "192.168.1.120 over Ethernet or on the 1 segment, 192.168.2.1 on the 2 "
                   "segment (MotionSDK README 4.1).\n",
                   host_.c_str());
      teardown();
      return SKR_EINVAL;
    }
    if (::connect(cmd_fd_, reinterpret_cast<const sockaddr *>(&remote), sizeof remote) < 0) {
      teardown();
      return SKR_EIO;
    }

    build_static_datagrams();

    running_.store(true);
    receiver_ = std::thread([this] { receive_loop(); });

    /* Wait for the body to say something about itself. A driver that opened against silence would
     * report `EIO` on every read and leave the operator to guess whether the robot is off, on
     * another segment, or pointed at a different `target_port`. */
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::milliseconds(static_cast<int>(open_timeout));
    while (std::chrono::steady_clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (have_state_) {
          opened_ = true;
          announce_hold_policy();
          return SKR_OK;
        }
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    std::fprintf(stderr,
                 "skynet-lite3: no state arrived on UDP %u within %g ms. The motion host pushes "
                 "joint and IMU data to the address in `~/jy_exe/conf/network.toml`; check that "
                 "its `ip` is this machine and its `target_port` is %u (MotionSDK README 5).\n",
                 static_cast<unsigned>(state_port_), open_timeout,
                 static_cast<unsigned>(state_port_));
    teardown();
    return SKR_ETIMEDOUT;
  }

  int close() override {
    if (opened_ && control_taken_.load() && !estopped_.load()) {
      /* The vendor's own hand-back, in the vendor's own order: damp the legs, wait, then return
       * control right. `ControlGet(ROBOT)` sleeps two seconds between the two, and the wait is
       * what keeps the underlying controller from resuming against a stiff pose. `close()` is
       * allowed to block; `emergency_stop()` is not, and does not. */
      send_datagram(damping_, JOINT_DATAGRAM);
      if (handback_ms_ > 0.0) {
        std::this_thread::sleep_for(std::chrono::milliseconds(static_cast<int>(handback_ms_)));
      }
      send_datagram(control_robot_, CONTROL_DATAGRAM);
    }
    opened_ = false;
    control_taken_.store(false);
    teardown();
    return SKR_OK;
  }

  int read_state(skynet::State &out, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    if (out.joint_count() < static_cast<std::size_t>(JOINTS)) return SKR_EINVAL;

    /* Wait for a **new** push, bounded by this body's own measured interval.
     *
     * The motion host has a clock of its own: it pushes whatever the caller does, so two
     * `read_state` calls microseconds apart genuinely have one sample between them. Returning the
     * previous one immediately is truthful and reads, to §15's `rhal.state.completeness`, as
     * *"two consecutive reads returned seq=3 and seq=3"* — a stalled producer, which is exactly
     * what §24 says a repeated sequence means. `drivers/ur_rtde` met this first and answered it by
     * waiting a bounded period; the difference here is that MotionSDK publishes no rate to bound
     * it with, so the bound is **measured**: twice the interval between the last two arrivals,
     * held inside 1..100 ms. A caller faster than the robot is slowed to the robot, which is the
     * only honest thing a driver in that position can do.
     *
     * §4's rule 1 — wait-free on the real-time path — is what this trades, and the trade is stated
     * rather than hidden: this driver is Guest-tier plumbing over UDP and the body behind it
     * publishes at a rate nobody has declared. */
    RobotData snapshot;
    std::uint64_t arrivals = 0;
    std::uint64_t stamp = 0;
    bool fresh = false;
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!have_state_) return SKR_EIO;
    }
    {
      std::uint64_t bound_ns = 20ull * 1000ull * 1000ull;
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (interval_ns_ > 0) bound_ns = interval_ns_ * 2;
      }
      if (bound_ns < 1000000ull) bound_ns = 1000000ull;
      if (bound_ns > 100000000ull) bound_ns = 100000000ull;
      const auto deadline =
          std::chrono::steady_clock::now() + std::chrono::nanoseconds(bound_ns);
      for (;;) {
        {
          std::lock_guard<std::mutex> guard(lock_);
          snapshot = state_;
          arrivals = arrivals_;
          stamp = stamp_ns_;
          if (arrivals_ > reported_arrivals_) {
            reported_arrivals_ = arrivals_;
            fresh = true;
            break;
          }
        }
        if (std::chrono::steady_clock::now() >= deadline) break;
        std::this_thread::sleep_for(std::chrono::microseconds(200));
      }
      if (!fresh) {
        /* A whole bound with nothing pushed. That is a stalled producer, and it is reported as
         * one: the previous sample, with its sequence held. */
        std::lock_guard<std::mutex> guard(lock_);
        stale_cycles_ += 1;
        arrivals = reported_arrivals_;
      }
    }

    std::uint16_t flags = 0;
    /* §27's homing. The vendor's own `idle_state.hpp` checks every joint position against the
     * absolute travel bounds *before* any motion and refuses to leave idle if one is outside —
     * which is only a meaningful check if the positions mean something at power-on. That is the
     * vendor stating, in code, that these encoders are absolute; `not_required` follows, and
     * `unreported` would be this driver declining to read its own evidence. */
    flags |= SKR_HOMING_NOT_REQUIRED;
    if (estopped_.load()) flags |= SKR_FLAG_ESTOP;

    /* The stamp is the arrival's, taken in the receive thread at the instant the datagram was
     * read (§24, §35.1) — not now, which would fold this call's own latency into it. */
    out.set_stamp(stamp);
    /* `seq` counts **datagrams**, not reads. The ABI says a repeat is how the watchdog guard
     * recognises a stalled producer, so a counter that advanced per call would make a robot whose
     * push has stopped indistinguishable from one that is answering. */
    out.set_seq(static_cast<std::uint32_t>(arrivals));
    out.set_flags(flags);
    /* Nothing on this wire reports a fault word. `RobotData` carries positions, velocities,
     * torques, temperatures, an IMU and foot forces, and no drive status at all — so the honest
     * value is zero-as-nothing-reported, said here and in the README rather than filled with a
     * temperature threshold this driver would have invented. */
    out.set_fault(0);

    auto position = out.position();
    auto velocity = out.velocity();
    auto effort = out.effort();
    for (int i = 0; i < JOINTS; ++i) {
      position[i] = snapshot.joint_data.joint_data[i].position;
      velocity[i] = snapshot.joint_data.joint_data[i].velocity;
      /* The vendor calls this field `torque` and does not say whether it is sensed or estimated
       * from drive current. It is a measurement either way and is surfaced as one; which of the
       * two it is belongs in the README, not in a field that cannot carry the distinction. */
      effort[i] = snapshot.joint_data.joint_data[i].torque;
    }
    out.set_joint_count(JOINTS);

    /* Nothing localises this base: the IMU gives orientation and rates, and nothing gives
     * position. §24's rule is all-NaN rather than a zero that reads as a measurement. */
    auto pose = out.base_pose();
    for (std::size_t i = 0; i < 7; ++i) pose[i] = std::nanf("");
    /* The twist's angular half *is* measured — the IMU's body rates, degrees per second on the
     * wire (`Lite3_rl_deploy`'s `GetImuOmega` divides by 180 and multiplies by pi, which is the
     * vendor stating the unit in code). The linear half is not measured by anything on this
     * body, so it is NaN and not zero. */
    auto twist = out.base_twist();
    twist[0] = twist[1] = twist[2] = std::nanf("");
    twist[3] = static_cast<float>(snapshot.imu.angular_velocity_roll * DEG_TO_RAD);
    twist[4] = static_cast<float>(snapshot.imu.angular_velocity_pitch * DEG_TO_RAD);
    twist[5] = static_cast<float>(snapshot.imu.angular_velocity_yaw * DEG_TO_RAD);
    return SKR_OK;
  }

  int write_command(const skynet::Command &cmd, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    if (estopped_.load()) return SKR_EPERM;
    if (cmd.joint_count() != static_cast<std::size_t>(JOINTS)) return SKR_EINVAL;

    const int body_mode = cmd.mode();
    if (body_mode != SKR_MODE_POSITION && body_mode != SKR_MODE_VELOCITY &&
        body_mode != SKR_MODE_TORQUE) {
      return SKR_EINVAL;
    }

    /* Every joint's mode resolved first, so a command that is going to be refused is refused
     * before any of it reaches the wire. §86.1: an entry this ABI does not define is most likely
     * a bare `SKR_MODE_*` written into the per-joint array, and moving a joint in the wrong units
     * is exactly what the offset exists to prevent. */
    const auto values = cmd.values();
    int modes[JOINTS];
    for (int i = 0; i < JOINTS; ++i) {
      modes[i] = cmd.mode_of(static_cast<std::size_t>(i));
      if (modes[i] < 0) return SKR_EINVAL;
    }
    const bool gains = have_kp_ && have_kd_;
    for (int i = 0; i < JOINTS; ++i) {
      if (!cmd.applies(static_cast<std::size_t>(i))) {
        if (!gains) {
          std::fprintf(stderr,
                       "skynet-lite3: this command masks %s, and a masked joint is §24's hold — "
                       "held at its current setpoint. On an impedance drive a hold is a "
                       "stiffness, and with no `kp` and `kd` in this driver's config there is "
                       "nothing to hold with. A slack leg is the one reading §24 rules out, so "
                       "the command is refused; configure the gains the robot should hold with "
                       "(63.4).\n",
                       JOINT_NAMES[i]);
          return SKR_EPERM;
        }
        continue;
      }
      if (modes[i] == SKR_MODE_POSITION && !gains) {
        std::fprintf(stderr,
                     "skynet-lite3: a position command needs `kp` and `kd` in this driver's "
                     "config. A Lite3 joint takes an impedance command, not a setpoint — the "
                     "drive computes kp*(pos_goal-pos) + kd*(vel_goal-vel) + t_ff — and this "
                     "driver will not choose a stiffness the operator did not (63.4).\n");
        return SKR_EPERM;
      }
      if (modes[i] == SKR_MODE_VELOCITY && !have_kd_) {
        std::fprintf(stderr,
                     "skynet-lite3: a velocity command needs `kd` in this driver's config. The "
                     "vendor's own velocity example is kp=0, kd=1, and a velocity setpoint with "
                     "no damping term behind it commands nothing at all (63.4).\n");
        return SKR_EPERM;
      }
      /* A position setpoint outside the travel this driver's own descriptor publishes is
       * refused rather than clamped. Clamping would send a command nobody wrote; sending it
       * unchanged would make the descriptor a document the driver itself does not honour. The
       * kernel's envelope clamp is a second line and not a reason to skip the first. */
      if (modes[i] == SKR_MODE_POSITION &&
          (values[i] < TRAVEL[i].low || values[i] > TRAVEL[i].high)) {
        std::fprintf(stderr,
                     "skynet-lite3: %s commanded to %.4f rad, outside the travel Deep Robotics "
                     "publishes for it (%.4f to %.4f). Refused rather than clamped: a clamp "
                     "would move the joint somewhere nobody asked for.\n",
                     JOINT_NAMES[i], static_cast<double>(values[i]), TRAVEL[i].low,
                     TRAVEL[i].high);
        return SKR_EINVAL;
      }
    }

    /* §24's hold, resolved per joint before anything is sent, on copies committed only once the
     * datagram is on the wire. Only a position command leaves a position setpoint; after any
     * other a hold latches where the joint was last measured to be. */
    float setpoint[JOINTS];
    bool have_setpoint[JOINTS];
    float hold[JOINTS];
    bool holding[JOINTS];
    for (int i = 0; i < JOINTS; ++i) {
      setpoint[i] = setpoint_[i];
      have_setpoint[i] = have_setpoint_[i];
      hold[i] = hold_[i];
      holding[i] = holding_[i];
      if (cmd.applies(static_cast<std::size_t>(i))) {
        holding[i] = false;
        have_setpoint[i] = modes[i] == SKR_MODE_POSITION;
        if (have_setpoint[i]) setpoint[i] = values[i];
        continue;
      }
      if (!holding[i]) {
        hold[i] = have_setpoint[i] ? setpoint[i] : measured_[i].load(std::memory_order_relaxed);
        holding[i] = true;
      }
    }

    RobotCmd out{};
    for (int i = 0; i < JOINTS; ++i) {
      JointCmd &joint = out.joint_cmd[i];
      if (!cmd.applies(static_cast<std::size_t>(i))) {
        /* §24's hold on a wire that has no way to leave a joint out: the latched target with the
         * operator's gains, a zero velocity setpoint and no feed-forward (the header, point 1). */
        joint.position = hold[i];
        joint.kp = static_cast<float>(kp_);
        joint.velocity = 0.0f;
        joint.kd = static_cast<float>(kd_);
        continue;
      }
      switch (modes[i]) {
        case SKR_MODE_POSITION:
          joint.position = values[i];
          joint.kp = static_cast<float>(kp_);
          joint.velocity = 0.0f;
          joint.kd = static_cast<float>(kd_);
          break;
        case SKR_MODE_VELOCITY:
          joint.velocity = values[i];
          joint.kd = static_cast<float>(kd_);
          break;
        case SKR_MODE_TORQUE:
          joint.torque = values[i];
          break;
        default:
          return SKR_EINVAL;
      }
    }

    /* Control right, taken in the same call as the first command it is for (see the header) —
     * and **re-asserted whenever this driver has been quiet long enough for the robot to have
     * taken the legs back**. One second of silence and the underlying controller reclaims
     * (MotionSDK README 2), so a latch that remembered "we took control at open" would be a
     * driver commanding a machine that stopped listening: every datagram accepted by the socket,
     * every command ignored by the robot, and nothing anywhere saying so. The guard is half the
     * reclaim window, which leaves the re-take a full window of margin.
     *
     * A failure here is reported rather than swallowed: the joint command that follows would be
     * ignored by a robot whose own controller still holds the legs. */
    const std::uint64_t now_ns = skynet::acquisition_ns();
    const bool held = control_taken_.load();
    const bool stale = held && now_ns - last_sent_ns_ > reclaim_guard_ns_;
    if (!held || stale) {
      if (!send_datagram(control_sdk_, CONTROL_DATAGRAM)) return SKR_EIO;
      control_taken_.store(true);
      if (stale) retakes_.fetch_add(1);
    }

    unsigned char datagram[JOINT_DATAGRAM];
    build_joint_datagram(datagram, out);
    if (!send_datagram(datagram, JOINT_DATAGRAM)) return SKR_EIO;
    last_sent_ns_ = skynet::acquisition_ns();
    for (int i = 0; i < JOINTS; ++i) {
      setpoint_[i] = setpoint[i];
      have_setpoint_[i] = have_setpoint[i];
      hold_[i] = hold[i];
      holding_[i] = holding[i];
    }
    return SKR_OK;
  }

  int emergency_stop() override {
    /* §4: from a signal handler, and from a dying process. Everything below is built at `open()`,
     * so this does no formatting and no allocation; `send` on a connected socket is on POSIX's
     * async-signal-safe list and `sendto` with an address to fill in would not be.
     *
     * What goes out is the vendor's own hand-back, minus the two-second wait that would make this
     * block: the damping command, then `0x0113` returning control right to the underlying
     * controller, which is what MotionSDK README 2 describes the robot falling back into. The
     * latch is what protects the rest: every later command is refused until a new session.
     *
     * It is not a power cut and this driver does not claim `safety.estop` for it — see
     * `describe()`. */
    estopped_.store(true);
    if (cmd_fd_ < 0) return SKR_EPERM;
    const bool damped = send_datagram(damping_, JOINT_DATAGRAM);
    const bool handed = send_datagram(control_robot_, CONTROL_DATAGRAM);
    return (damped && handed) ? SKR_OK : SKR_EIO;
  }

  int extension(std::string_view token, const void *, std::size_t, void *out,
                std::size_t out_len) override {
    if (token != TELEMETRY_TOKEN) return SKR_ENOSYS;
    if (!opened_) return SKR_EPERM;

    RobotData snapshot;
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!have_state_) return SKR_EIO;
      snapshot = state_;
    }

    /* Everything `RobotData` carries that §24's `State` has no field for, in the units the robot
     * reports them in, with the unit named beside each. Angles are degrees on this wire and are
     * left in degrees here rather than converted twice — `base_twist` is where the radians live. */
    std::string json = "{\"tick_ms\":" + std::to_string(snapshot.tick) +
                       ",\"imu\":{\"timestamp_ms\":" + std::to_string(snapshot.imu.timestamp) +
                       ",\"unit_angle\":\"degree\",\"unit_rate\":\"degree_per_second\","
                       "\"unit_acceleration\":\"m_per_s2\"";
    char buffer[512];
    std::snprintf(buffer, sizeof buffer,
                  ",\"roll\":%.6f,\"pitch\":%.6f,\"yaw\":%.6f,\"rate_roll\":%.6f,"
                  "\"rate_pitch\":%.6f,\"rate_yaw\":%.6f,\"acc_x\":%.6f,\"acc_y\":%.6f,"
                  "\"acc_z\":%.6f}",
                  snapshot.imu.angle_roll, snapshot.imu.angle_pitch, snapshot.imu.angle_yaw,
                  snapshot.imu.angular_velocity_roll, snapshot.imu.angular_velocity_pitch,
                  snapshot.imu.angular_velocity_yaw, snapshot.imu.acc_x, snapshot.imu.acc_y,
                  snapshot.imu.acc_z);
    json += buffer;
    json += ",\"joint_temperature_c\":[";
    for (int i = 0; i < JOINTS; ++i) {
      std::snprintf(buffer, sizeof buffer, "%s%.3f", i ? "," : "",
                    snapshot.joint_data.joint_data[i].temperature);
      json += buffer;
    }
    /* Three components per leg, the vendor's `leg_force`, and their own note that in most cases
     * only the z component is used. Reported as the twelve the wire carries. */
    json += "],\"contact_force_n\":[";
    for (int i = 0; i < JOINTS; ++i) {
      std::snprintf(buffer, sizeof buffer, "%s%.6f", i ? "," : "",
                    snapshot.contact_force.leg_force[i]);
      json += buffer;
    }
    json += "]";
    /* Five numbers about the session rather than about the body, and they are here because
     * nothing else surfaces them: how many state pushes have arrived, how many this driver
     * dropped as malformed or short, how many reads waited a whole bound and got nothing, the
     * push interval this driver measured, and how many times it had to take control right back
     * because it had been quiet longer than the robot's reclaim window. */
    {
      std::lock_guard<std::mutex> guard(lock_);
      std::snprintf(buffer, sizeof buffer,
                    ",\"arrivals\":%llu,\"dropped\":%llu,\"stale_reads\":%llu,"
                    "\"push_interval_ms\":%.3f,\"control_retakes\":%llu}",
                    static_cast<unsigned long long>(arrivals_),
                    static_cast<unsigned long long>(malformed_ + short_datagrams_),
                    static_cast<unsigned long long>(stale_cycles_),
                    static_cast<double>(interval_ns_) / 1e6,
                    static_cast<unsigned long long>(retakes_.load()));
    }
    json += buffer;

    if (out == nullptr || json.size() + 1 > out_len) return SKR_EINVAL;
    std::memcpy(out, json.data(), json.size());
    static_cast<char *>(out)[json.size()] = '\0';
    return static_cast<int>(json.size());
  }

  std::string describe() override {
    std::string json = "{\"schema_version\":1,\"robot_slug\":\"" + slug_ +
                       "\",\"digest\":null,\"verified\":false,\"simulator\":" +
                       (simulator_ ? "true" : "false") +
                       ",\"base\":\"legged\",\"dof\":12,\"joints\":[";
    for (int i = 0; i < JOINTS; ++i) {
      if (i) json += ",";
      char buffer[512];
      std::snprintf(buffer, sizeof buffer,
                    "{\"name\":\"%s\",\"role\":\"%s\",\"type\":\"revolute\","
                    "\"limits\":{\"position_min_rad\":%.5f,\"position_max_rad\":%.5f,"
                    "\"velocity_max_rad_s\":%.1f,\"effort_max_nm\":%.1f},"
                    "\"control_modes\":[\"position\",\"velocity\",\"torque\"]}",
                    JOINT_NAMES[i], ROLES[i], TRAVEL[i].low, TRAVEL[i].high,
                    VELOCITY_MAX[i % 3], EFFORT_MAX[i % 3]);
      json += buffer;
    }
    json +=
        "],\"end_effectors\":[],\"sensors\":[],"
        "\"control\":{\"modes\":[\"position\",\"velocity\",\"torque\"],\"per_joint_modes\":true,"
        /* The rate is the robot's own and is not published: MotionSDK's demo runs a one
         * millisecond timer and `RobotData.tick` counts milliseconds, which is the loop the
         * *example* drives, not a rate the vendor states the body holds. `null` is schema-valid
         * and readiness reads it as unknown rather than slow (F64's rule for a figure nobody
         * published). */
        "\"rate_hz_max\":null,\"rate_hz_min\":null,\"command_latency_ms\":null},";
    char envelope[1024];
    std::snprintf(envelope, sizeof envelope,
                  "\"envelope\":{\"joint_velocity_max_rad_s\":%.1f,\"joint_torque_max_nm\":%.1f,"
                  "\"watchdog_ms\":%.1f,\"source\":\"%s\",\"basis\":\"declared\"}",
                  ENVELOPE_VELOCITY_MAX, ENVELOPE_TORQUE_MAX, WATCHDOG_MS, ENVELOPE_SOURCE);
    json += envelope;
    /* `safety.estop` is the **software** stop, and this body has one: `emergency_stop()` sends
     * the vendor's damping command, hands control right back to the robot's own controller and
     * latches every later command. §15's readiness reads the token exactly that way — *"exposes
     * a software emergency stop"* — and the independence question is a different check on a
     * different field, `envelope.estop`, which this descriptor leaves absent because there is no
     * hardware stop to declare. So the token is claimed and the hedge is carried where it
     * belongs rather than by withholding a true statement: MotionSDK publishes no emergency verb
     * at all, what goes out is a datagram to a program on the robot's own computer, and the one
     * part of it that is certain — the refusal of every later command — is this driver's latch
     * rather than the machine's.
     *
     * (The Fourier row withholds the token on the *independence* argument. Every other driver in
     * this tree with a software stop declares it, and readiness caps a body that does not at no
     * tier at all, so the withholding costs a guest that the driver's own stop supports.)
     *
     * `platform.onboard_compute` is declared and earns its citation: the motion host is a Linux
     * computer the owner is told to SSH into (MotionSDK README 4.2 gives the usernames), whose
     * configuration the SDK edits (README 5), and which the SDK's own CMake cross-compiles for
     * (`-DBUILD_PLATFORM=arm`). That is the vendor documenting user code on the robot's computer.
     * It does not claim the OS runs there today — readiness is paperwork and a deployment is
     * evidence. */
    json += ",\"capabilities\":[\"safety.estop\",\"platform.onboard_compute\"],"
            "\"extensions\":[\"";
    json += TELEMETRY_TOKEN;
    json += "\"],\"solvers\":[]}";
    return json;
  }

 private:
  void announce_hold_policy() const {
    if (have_kp_ && have_kd_) {
      std::fprintf(stderr,
                   "skynet-lite3: a masked joint is held at its last position setpoint — or, "
                   "with none, where it was measured when the hold began — at kp=%g, kd=%g "
                   "(24's hold).\n",
                   kp_, kd_);
    } else {
      std::fprintf(stderr,
                   "skynet-lite3: no `kp` and `kd` configured, so there is nothing to hold a "
                   "masked joint with: a command that masks a joint will be refused rather than "
                   "sent as a slack leg (24's hold). Torque commands on every joint still go "
                   "out.\n");
    }
  }

  void build_static_datagrams() {
    build_control_datagram(control_sdk_, CODE_CONTROL_SDK);
    build_control_datagram(control_robot_, CODE_CONTROL_ROBOT);
    RobotCmd damping{};
    for (int i = 0; i < JOINTS; ++i) damping.joint_cmd[i].kd = VENDOR_DAMPING_KD;
    build_joint_datagram(damping_, damping);
  }

  static void build_control_datagram(unsigned char *out, std::uint32_t code) {
    EthCommand header{};
    header.code = code;
    /* The vendor's `Sender` builds these with the three-argument `Command(code, size, params)`
     * and a null parameter block, which its `CmdDone` then sends down the single-value path with
     * the union reading zero — type `0`, value `0`, twelve bytes on the wire. */
    header.value_or_size = 0;
    header.type_and_count = pack_type_count(TYPE_SINGLE_VALUE, 0);
    std::memcpy(out, &header, sizeof header);
  }

  static void build_joint_datagram(unsigned char *out, const RobotCmd &command) {
    EthCommand header{};
    header.code = CODE_JOINT_CMD;
    header.value_or_size = static_cast<std::uint32_t>(sizeof(RobotCmd));
    /* `count` is the vendor's sequence field and the vendor never sets it: `CmdDone` zeroes the
     * message and writes only the code, the size and the type. A driver that started numbering
     * here would be the only thing on the wire that did. */
    header.type_and_count = pack_type_count(TYPE_MESS_VALUES, 0);
    std::memcpy(out, &header, sizeof header);
    std::memcpy(out + sizeof header, &command, sizeof command);
  }

  bool send_datagram(const unsigned char *bytes, std::size_t length) const {
    if (cmd_fd_ < 0) return false;
    const ssize_t sent = ::send(cmd_fd_, bytes, length, MSG_NOSIGNAL);
    return sent == static_cast<ssize_t>(length);
  }

  void receive_loop() {
    unsigned char buffer[sizeof(CommandMessage)];
    while (running_.load()) {
      const ssize_t got = ::recv(state_fd_, buffer, sizeof buffer, 0);
      if (got < 0) continue;  /* the receive timeout, or a signal: look at `running_` again */
      /* The stamp is taken here, at acquisition, and never at `read_state`'s return (§24, §35.1).
       */
      const std::uint64_t stamp = skynet::acquisition_ns();
      if (static_cast<std::size_t>(got) < sizeof(EthCommand)) {
        std::lock_guard<std::mutex> guard(lock_);
        short_datagrams_ += 1;
        continue;
      }
      EthCommand header{};
      std::memcpy(&header, buffer, sizeof header);
      if (header.code != CODE_STATE_PUSH || wire_type(header.type_and_count) != TYPE_MESS_VALUES) {
        std::lock_guard<std::mutex> guard(lock_);
        malformed_ += 1;
        continue;
      }
      /* The length is checked against what actually arrived, which the vendor's own `Receiver`
       * does not do: its callback copies a fixed `sizeof(CommandMessage)` out of a reused 64 KB
       * buffer whatever the datagram's length, so a short push there reads the tail of whatever
       * arrived before it. Here a push that does not carry a whole `RobotData` is counted and
       * dropped. */
      if (static_cast<std::size_t>(got) < STATE_DATAGRAM ||
          header.value_or_size < sizeof(RobotData)) {
        std::lock_guard<std::mutex> guard(lock_);
        short_datagrams_ += 1;
        continue;
      }
      RobotData incoming{};
      std::memcpy(&incoming, buffer + sizeof(EthCommand), sizeof incoming);
      std::lock_guard<std::mutex> guard(lock_);
      /* The interval between arrivals, measured rather than declared: it is what bounds
       * `read_state`'s wait, and MotionSDK publishes no rate to take instead. */
      if (have_state_ && stamp > stamp_ns_) interval_ns_ = stamp - stamp_ns_;
      state_ = incoming;
      for (int i = 0; i < JOINTS; ++i) {
        measured_[i].store(incoming.joint_data.joint_data[i].position, std::memory_order_relaxed);
      }
      stamp_ns_ = stamp;
      arrivals_ += 1;
      have_state_ = true;
    }
  }

  void teardown() {
    running_.store(false);
    if (receiver_.joinable()) receiver_.join();
    if (state_fd_ >= 0) {
      ::close(state_fd_);
      state_fd_ = -1;
    }
    if (cmd_fd_ >= 0) {
      ::close(cmd_fd_);
      cmd_fd_ = -1;
    }
    have_state_ = false;
  }

  bool opened_ = false;
  std::atomic<bool> estopped_{false};
  std::atomic<bool> control_taken_{false};
  std::atomic<bool> running_{false};

  std::string host_ = "127.0.0.1";
  std::string slug_ = "deeprobotics-lite3";
  std::uint16_t command_port_ = COMMAND_PORT;
  std::uint16_t state_port_ = STATE_PORT;
  bool simulator_ = true;
  double kp_ = 0.0;
  double kd_ = 0.0;
  bool have_kp_ = false;
  bool have_kd_ = false;
  /* §24's hold, per joint: the last position setpoint and whether there is one, and the latched
   * hold target and whether the joint is being held. Touched only on the command path. */
  float setpoint_[JOINTS] = {};
  bool have_setpoint_[JOINTS] = {};
  float hold_[JOINTS] = {};
  bool holding_[JOINTS] = {};
  /* Each joint's last measured position, written by the receive thread and read by the command
   * path when a hold with no setpoint behind it begins. Atomic rather than under the state lock,
   * for `retakes_`' reason below. */
  std::atomic<float> measured_[JOINTS] = {};
  double handback_ms_ = 2000.0;
  /* Half the vendor's reclaim window, in nanoseconds. Configurable because the window is the
   * robot's and a future firmware could move it. */
  std::uint64_t reclaim_guard_ns_ = static_cast<std::uint64_t>(RECLAIM_MS / 2.0 * 1e6);
  std::uint64_t last_sent_ns_ = 0;
  /* Read by `extension` on whichever thread asks for telemetry, written by `write_command` on the
   * OS's command path. Atomic rather than under the state lock: the command path does not take
   * that lock and should not start. */
  std::atomic<std::uint64_t> retakes_{0};

  int cmd_fd_ = -1;
  int state_fd_ = -1;
  std::thread receiver_;
  mutable std::mutex lock_;
  RobotData state_{};
  std::uint64_t stamp_ns_ = 0;
  std::uint64_t arrivals_ = 0;
  std::uint64_t reported_arrivals_ = 0;
  std::uint64_t interval_ns_ = 0;
  std::uint64_t stale_cycles_ = 0;
  std::uint64_t malformed_ = 0;
  std::uint64_t short_datagrams_ = 0;
  bool have_state_ = false;

  unsigned char control_sdk_[CONTROL_DATAGRAM] = {0};
  unsigned char control_robot_[CONTROL_DATAGRAM] = {0};
  unsigned char damping_[JOINT_DATAGRAM] = {0};
};

}  // namespace

SKYNET_RHAL_EXPORT(Lite3Bridge)
