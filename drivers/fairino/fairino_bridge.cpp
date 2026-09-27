/*
 * skynet-fairino — a FAIRINO FR-series cobot over the vendor's own XML-RPC and CNDE wires.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 7 closes its cobot table with *"Fairino and JAKA close the class
 * unbuilt (unlicensed, and JAKA ships no hardware interface)"*, and F92 recorded Fairino as
 * R-blocked because `frcobot_ros2`'s `package.xml` files carry `TODO: License declaration`. That
 * is still true of the ROS 2 wrapper, and it was never true of the vendor's SDK:
 * `FAIR-INNOVATION/fairino-cpp-sdk` is **Apache-2.0 with its source**, and a licence on the
 * wrapper was never what route D needed. The class was blocked on the wrong artefact.
 *
 * So this is route D, and it is the best-evidenced row in this tree: every constant it speaks was
 * read out of the vendor's own source rather than out of a disassembly (the Lite3 row) or a
 * README (the Unitree row). `fairino_wire.hpp` names the file and the symbol for each.
 *
 * ── The two wires, and why these two ─────────────────────────────────────
 *
 * An FR controller answers on four sockets. This driver speaks the two the vendor's own client
 * speaks:
 *
 *   **20003, XML-RPC.** Every command verb. `ServoJ`, `ServoJV` and `ServoJT` are the three §24
 *   modes; `ServoMoveStart`/`End` and `ServoJTStart`/`End` are the windows they live in;
 *   `GetJointSoftLimitDeg`, `GetSoftwareVersion` and `GetRobotSN` are what this driver asks the
 *   robot about itself at `open()`.
 *
 *   **20005, CNDE.** The state stream. It is *configurable*: the client sends a period and a list
 *   of field names and the controller streams exactly those, in that order, every period. This
 *   driver asks for the eighteen fields §24's `State` and its flags actually need — 315 bytes —
 *   where the vendor's own client asks for seventy-three and moves four times the bytes.
 *
 * The other two are named in the wire header and not used: 20004 is the older fixed-layout push
 * the SDK no longer reads, and 20007 is a UDP command path with no reply, which cannot carry the
 * refusal §24 requires a driver to return.
 *
 * ── The three things the mapping has to get right ────────────────────────
 *
 * 1. **The stop is the vendor's own bytes.** `FRRobot::StopMotion()` does not use XML-RPC at all:
 *    it writes the literal frame `/f/bIII44III102III4IIISTOPIII/b/f` to port 8080. That is a
 *    constant, so `emergency_stop()` can hold the connected socket and those bytes from `open()`
 *    and `send()` them from a signal handler, which §4 requires and which an XML-RPC round trip
 *    could not do. What goes out is character-for-character what the vendor's own stop sends.
 *
 * 2. **This wire has no per-joint mode.** `ServoJ` takes six positions, `ServoJV` six velocities
 *    and `ServoJT` six torques, each in its own window, and there is no verb that mixes them. The
 *    descriptor declares `per_joint_modes: false` and a command whose joints disagree is refused
 *    rather than split across three verbs, which would be three different instants of motion
 *    presented as one command.
 *
 * 3. **A masked joint on a position wire is a held setpoint, and that is genuinely §24's hold.**
 *    The Lite3 row refused to fabricate a hold because its wire takes an impedance command and
 *    holding would have meant inventing a stiffness. Here the wire takes a position, so holding is
 *    *repeating the setpoint this driver last issued* — not the measured position, which sags
 *    under load and would creep a little further every cycle. The first command of a window
 *    initialises the held setpoints from the measured position, and says so. Under velocity a
 *    masked joint is commanded zero, which is the same statement in that mode's units. Under
 *    torque there is no such statement: zero torque on an arm with no brakes applied is a joint
 *    that gives way, and holding it would take a gravity model this driver is not. So a torque
 *    command that masks a joint is refused. It used to be sent with zero on the masked joint and
 *    a notice at the window, which is the limp reading of §24's mask that Unitree's simulator
 *    measured folding a standing body (DRY-COVERAGE-PLAN.md F121, carried as 8.13).
 *
 * ── Where the numbers come from ──────────────────────────────────────────
 *
 * Travel is **queried from the controller**, not transcribed: `GetJointSoftLimitDeg` returns the
 * six soft limits the robot is actually configured with, and the descriptor publishes those. No
 * other driver in this tree does that — everywhere else the limits are a datasheet's, and a
 * datasheet cannot know that this arm has a reduced wrist range because of what is bolted to it.
 * When the query fails the driver falls back to FAIR INNOVATION's published FR3 figures and the
 * envelope's `source` says which of the two it is using, because a provenance that claimed the
 * robot when it had used a catalogue would be worth less than no provenance at all.
 *
 * The velocity ceiling is the vendor's catalogue (`frtech.fr/FR/5.html`, FR3: every axis
 * ±180 °/s). **There is no joint torque ceiling here**, because FAIR INNOVATION publish none —
 * the product page gives payload, reach, repeatability and per-axis speed and no torque at all.
 * So `joint_torque_max_nm` is absent and `joint_ratings` fails, exactly as it does for the ten
 * velocity-only bodies F91 corrected. The wire *reports* joint torque, which is a measurement and
 * not a rating, and it is reported as one.
 *
 * Licence: Apache-2.0 (this file), matching the SDK whose interface it speaks. It links nothing of
 * the vendor's; `cross_check.cpp` is the only part of this row that needs a checkout.
 */

#include "skynet_rhal.hpp"

#include "fairino_wire.hpp"
#include "fairino_xmlrpc.hpp"

#include <arpa/inet.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
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
#include <vector>

namespace {

using namespace skynet_fairino;

/* ── the body ─────────────────────────────────────────────────────────── */

struct Travel {
  double low_deg;
  double high_deg;
};

/* FAIR INNOVATION's own FR3 page (`frtech.fr/FR/5.html`): base ±175°, shoulder +85/−265°,
 * elbow ±150°, wrist 1 +85/−265°, wrist 2 ±175°, wrist 3 ±175°. The fallback only — the driver
 * asks the controller first. */
constexpr Travel CATALOGUE_TRAVEL[JOINTS] = {
    {-175.0, 175.0}, {-265.0, 85.0}, {-150.0, 150.0},
    {-265.0, 85.0},  {-175.0, 175.0}, {-175.0, 175.0},
};

/* Every axis, same page: ±180 °/s. */
constexpr double CATALOGUE_VELOCITY_MAX_DEG_S = 180.0;

/* §5.1's vocabulary. An FR3 is kinematically the same six-axis arm with a spherical wrist that
 * every UR is, so these are the roles `drivers/ur_rtde` declares, for the reason the vocabulary
 * exists: two vendors' joints that mean the same thing should say so in the same words. */
constexpr const char *ROLES[JOINTS] = {
    "right_arm.shoulder.yaw", "right_arm.shoulder.pitch", "right_arm.elbow.flex",
    "right_arm.wrist.pitch",  "right_arm.wrist.yaw",      "right_arm.wrist.roll",
};

constexpr const char *TELEMETRY_TOKEN = "vendor.fairino.telemetry";

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

/* ── sockets ──────────────────────────────────────────────────────────── */

int connect_tcp(const std::string &host, std::uint16_t port, int timeout_ms) {
  const int fd = ::socket(AF_INET, SOCK_STREAM, 0);
  if (fd < 0) return -1;
  timeval timeout{};
  timeout.tv_sec = timeout_ms / 1000;
  timeout.tv_usec = (timeout_ms % 1000) * 1000;
  ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
  ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof timeout);
  int one = 1;
  ::setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
  sockaddr_in remote{};
  remote.sin_family = AF_INET;
  remote.sin_port = htons(port);
  if (::inet_pton(AF_INET, host.c_str(), &remote.sin_addr) != 1) {
    ::close(fd);
    return -2;
  }
  if (::connect(fd, reinterpret_cast<const sockaddr *>(&remote), sizeof remote) < 0) {
    ::close(fd);
    return -3;
  }
  return fd;
}

bool send_all(int fd, const void *bytes, std::size_t length) {
  const unsigned char *at = static_cast<const unsigned char *>(bytes);
  std::size_t left = length;
  while (left > 0) {
    const ssize_t sent = ::send(fd, at, left, MSG_NOSIGNAL);
    if (sent <= 0) return false;
    at += sent;
    left -= static_cast<std::size_t>(sent);
  }
  return true;
}

bool recv_exact(int fd, void *bytes, std::size_t length) {
  unsigned char *at = static_cast<unsigned char *>(bytes);
  std::size_t left = length;
  while (left > 0) {
    const ssize_t got = ::recv(fd, at, left, 0);
    if (got <= 0) return false;
    at += got;
    left -= static_cast<std::size_t>(got);
  }
  return true;
}

/* ── the driver ───────────────────────────────────────────────────────── */

/* What the CNDE stream carries, parsed once per frame. */
struct Reported {
  double position_deg[JOINTS] = {0, 0, 0, 0, 0, 0};
  double velocity_deg_s[JOINTS] = {0, 0, 0, 0, 0, 0};
  double torque_nm[JOINTS] = {0, 0, 0, 0, 0, 0};
  double driver_torque_nm[JOINTS] = {0, 0, 0, 0, 0, 0};
  double driver_temperature_c[JOINTS] = {0, 0, 0, 0, 0, 0};
  double last_servo_target_deg[JOINTS] = {0, 0, 0, 0, 0, 0};
  std::int32_t main_code = 0;
  std::int32_t sub_code = 0;
  std::int32_t motion_done = 0;
  std::int32_t queue_len = 0;
  std::int32_t enable_state = 0;
  std::uint8_t program_state = 0;
  std::uint8_t robot_state = 0;
  std::uint8_t robot_mode = 0;
  std::uint8_t emergency_stop = 0;
  std::uint8_t safety_stop0 = 0;
  std::uint8_t safety_stop1 = 0;
  std::uint8_t collision = 0;
};

enum class Window { None, Move, Torque };

class FairinoBridge : public skynet::Driver {
 public:
  /* The published travel is what `describe()` carries until a controller has been asked, and a
   * descriptor may be read before any session is opened. */
  FairinoBridge() {
    for (int i = 0; i < JOINTS; ++i) travel_[i] = CATALOGUE_TRAVEL[i];
  }

  int open(std::string_view config_json) override {
    if (opened_) return SKR_EPERM;
    const std::string config(config_json);

    /* One instance lives for the process's life behind `SKYNET_RHAL_EXPORT`, so every field a
     * previous session set is reset here rather than in the constructor (F47). */
    if (estopped_.load()) {
      std::fprintf(stderr,
                   "skynet-fairino: the previous session left an emergency stop latched. This is "
                   "a new session and the latch does not carry into it — what went out was the "
                   "vendor's own STOP frame, and nothing here can tell you what the controller "
                   "has done since.\n");
    }
    estopped_.store(false);
    window_ = Window::None;
    have_state_ = false;
    arrivals_ = 0;
    reported_arrivals_ = 0;
    interval_ns_ = 0;
    stale_cycles_ = 0;
    malformed_ = 0;
    commands_ = 0;
    refusals_ = 0;
    limits_from_robot_ = false;
    model_text_.clear();
    controller_version_.clear();
    serial_.clear();

    model_ = "fr3";
    config_string(config, "model", &model_);
    if (model_ != "fr3") {
      /* The FR series shares this wire end to end — FR5, FR10, FR16 and FR20 answer the same
       * verbs on the same ports. What they do not share is the published figures, and this
       * driver's descriptor is made of those. Refused by name rather than declared with another
       * model's ceiling, which is the Lite3 row's rule about the X20 applied to a vendor that
       * does publish, per model. */
      std::fprintf(stderr,
                   "skynet-fairino: unknown `model` \"%s\". This driver's descriptor carries FAIR "
                   "INNOVATION's published FR3 figures. FR5, FR10, FR16 and FR20 speak this exact "
                   "wire and have different ceilings, so each needs its own transcribed row "
                   "rather than this one's numbers.\n",
                   model_.c_str());
      return SKR_EINVAL;
    }

    host_ = "127.0.0.1";
    config_string(config, "host", &host_);
    slug_ = "fairino-fr3";
    config_string(config, "robot_slug", &slug_);
    simulator_ = true;
    config_bool(config, "simulator", &simulator_);

    double port = XMLRPC_PORT;
    config_number(config, "xmlrpc_port", &port);
    xmlrpc_port_ = static_cast<std::uint16_t>(port);
    port = STATE_PORT;
    config_number(config, "state_port", &port);
    state_port_ = static_cast<std::uint16_t>(port);
    port = COMMAND_PORT;
    config_number(config, "command_port", &port);
    command_port_ = static_cast<std::uint16_t>(port);

    double period = CNDE_DEFAULT_PERIOD_MS;
    config_number(config, "state_period_ms", &period);
    if (period < CNDE_MIN_PERIOD_MS || period > CNDE_MAX_PERIOD_MS) {
      std::fprintf(stderr,
                   "skynet-fairino: `state_period_ms` is %g, outside the 4..1000 ms the vendor's "
                   "`SetRobotRealtimeStatePeriod` documents.\n",
                   period);
      return SKR_EINVAL;
    }
    period_ms_ = static_cast<int>(period);

    /* `ServoJ`'s `cmdT`, the period the controller is told each setpoint covers. The vendor's own
     * header recommends 0.001..0.0016 s for it and its default state stream runs at 0.008 s; this
     * driver's default follows the stream, because a command period shorter than the interval
     * between the states it is computed from is a promise nothing here can keep. Config, because
     * it is the operator's to match to their loop. */
    double cmd_t = period_ms_ / 1000.0;
    config_number(config, "servo_cmd_t_s", &cmd_t);
    if (!(cmd_t > 0.0)) {
      std::fprintf(stderr, "skynet-fairino: `servo_cmd_t_s` must be positive (got %g).\n", cmd_t);
      return SKR_EINVAL;
    }
    servo_cmd_t_ = cmd_t;

    double open_timeout = 3000.0;
    config_number(config, "open_timeout_ms", &open_timeout);
    double rpc_timeout = 2000.0;
    config_number(config, "rpc_timeout_ms", &rpc_timeout);
    rpc_timeout_ms_ = static_cast<int>(rpc_timeout);

    enable_ = true;
    config_bool(config, "enable", &enable_);
    set_auto_mode_ = true;
    config_bool(config, "auto_mode", &set_auto_mode_);

    /* The instant-command socket first: it is the one `emergency_stop()` needs, and a session
     * that could command before it could stop would be the wrong order to fail in. */
    command_fd_ = connect_tcp(host_, command_port_, rpc_timeout_ms_);
    if (command_fd_ < 0) {
      std::fprintf(stderr,
                   "skynet-fairino: cannot reach the instant-command port %u on %s. That socket "
                   "carries the vendor's own STOP frame, so this driver will not open a session "
                   "it could not stop.\n",
                   static_cast<unsigned>(command_port_), host_.c_str());
      teardown();
      return SKR_EIO;
    }

    const int cnde = open_state_stream();
    if (cnde != SKR_OK) {
      teardown();
      return cnde;
    }

    /* Wait for the body to say something about itself before reporting the session open. */
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::milliseconds(static_cast<int>(open_timeout));
    bool arrived = false;
    while (std::chrono::steady_clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        arrived = have_state_;
      }
      if (arrived) break;
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
    if (!arrived) {
      std::fprintf(stderr,
                   "skynet-fairino: the CNDE stream on %u accepted the configuration and then "
                   "sent nothing within %g ms.\n",
                   static_cast<unsigned>(state_port_), open_timeout);
      teardown();
      return SKR_ETIMEDOUT;
    }

    /* What the robot says about itself. None of these is fatal: a controller that refuses one
     * still drives, and the descriptor says which of them it answered. */
    ask_identity();
    ask_joint_limits();

    opened_ = true;
    return SKR_OK;
  }

  int close() override {
    if (opened_ && !estopped_.load()) close_window();
    opened_ = false;
    teardown();
    return SKR_OK;
  }

  int read_state(skynet::State &out, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    if (out.joint_count() < static_cast<std::size_t>(JOINTS)) return SKR_EINVAL;

    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!have_state_) return SKR_EIO;
    }

    /* Wait for a **new** frame, bounded by the period this driver asked the controller for and
     * by the interval it has actually measured, whichever is longer.
     *
     * §15's `rhal.state.completeness` reads two reads returning the same `seq` as a stalled
     * producer, and §24 says that is what a repeat means — so a driver that returned the previous
     * sample to a caller faster than the robot would be reporting a stall that was not happening.
     * `drivers/ur_rtde` met this first and `drivers/lite3` measured its bound because its vendor
     * publishes no rate; here the rate is the one *this driver asked for*, which is a better
     * bound than a measurement, and the measurement is still used when the controller is slower
     * than it promised. §4's wait-free rule is what this trades, and it is stated rather than
     * hidden: this is guest-tier plumbing over TCP. */
    Reported snapshot;
    std::uint64_t arrivals = 0;
    std::uint64_t stamp = 0;
    bool fresh = false;
    {
      std::uint64_t bound_ns = static_cast<std::uint64_t>(period_ms_) * 2ull * 1000000ull;
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (interval_ns_ * 2 > bound_ns) bound_ns = interval_ns_ * 2;
      }
      if (bound_ns > 200000000ull) bound_ns = 200000000ull;
      const auto deadline = std::chrono::steady_clock::now() + std::chrono::nanoseconds(bound_ns);
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
        std::lock_guard<std::mutex> guard(lock_);
        stale_cycles_ += 1;
        arrivals = reported_arrivals_;
      }
    }

    std::uint16_t flags = 0;
    /* §27. Five hundred and ninety-nine entry points in the vendor's API and not one of them
     * homes or zeroes a joint; the controller reports absolute joint angles from the first frame
     * after power-on and `GetJointSoftLimitDeg` is meaningful against them. That is the vendor
     * stating, by what it does not provide, that these encoders need no homing. */
    flags |= SKR_HOMING_NOT_REQUIRED;
    /* The robot's own stop circuit, not this driver's latch: the button on the control box, and
     * the two safety-stop inputs the vendor's own `GetSafetyCode()` refuses motion on. */
    if (snapshot.emergency_stop || snapshot.safety_stop0 || snapshot.safety_stop1) {
      flags |= SKR_FLAG_ESTOP;
    }
    if (estopped_.load()) flags |= SKR_FLAG_ESTOP;

    out.set_stamp(stamp);
    out.set_seq(static_cast<std::uint32_t>(arrivals));
    out.set_flags(flags);
    /* This wire does report a fault word, which most in this tree do not: `main_code` is the
     * controller's error and `sub_code` narrows it. The main code goes in §24's field and the sub
     * code goes to the telemetry extension, because the field holds one number and dropping the
     * other would lose the half that says which. */
    out.set_fault(static_cast<std::uint32_t>(snapshot.main_code));

    auto position = out.position();
    auto velocity = out.velocity();
    auto effort = out.effort();
    for (int i = 0; i < JOINTS; ++i) {
      position[i] = static_cast<float>(snapshot.position_deg[i] * DEG_TO_RAD);
      velocity[i] = static_cast<float>(snapshot.velocity_deg_s[i] * DEG_TO_RAD);
      /* `actual_joint_torque`, the vendor's own name, in N.m. Whether it is sensed or estimated
       * from drive current the vendor does not say; it is a measurement either way, and the
       * drives' own torque is reported separately through the extension. */
      effort[i] = static_cast<float>(snapshot.torque_nm[i]);
    }
    out.set_joint_count(JOINTS);

    /* A bolted-down arm. §24's rule is all-NaN rather than a zero that reads as a measurement:
     * nothing on this body measures where its base is or how fast the base is moving, and the
     * tool pose the controller does report is the *tool's*, not the base's. */
    auto pose = out.base_pose();
    for (std::size_t i = 0; i < 7; ++i) pose[i] = std::nanf("");
    auto twist = out.base_twist();
    for (std::size_t i = 0; i < 6; ++i) twist[i] = std::nanf("");
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

    /* This wire has one mode per verb, so every joint this command applies to has to be in the
     * same one. §86.1's per-joint array is resolved first and a disagreement is refused before
     * anything reaches the controller — the descriptor declares `per_joint_modes: false` and this
     * is that declaration being kept. */
    const auto values = cmd.values();
    for (int i = 0; i < JOINTS; ++i) {
      const int mode = cmd.mode_of(static_cast<std::size_t>(i));
      if (mode < 0) return SKR_EINVAL;
      if (!cmd.applies(static_cast<std::size_t>(i))) continue;
      if (mode != body_mode) {
        std::fprintf(stderr,
                     "skynet-fairino: joint %s is commanded in a different mode from the body, "
                     "and this wire has no verb that mixes modes — ServoJ takes six positions, "
                     "ServoJV six velocities, ServoJT six torques, each in its own window. "
                     "Refused rather than split into three commands at three instants.\n",
                     JOINT_NAMES[i]);
        refusals_ += 1;
        return SKR_EINVAL;
      }
    }

    /* §24's hold has no value in torque (the header, point 3): a masked joint here would be a
     * joint commanded to give way. Refused before any window changes. */
    if (body_mode == SKR_MODE_TORQUE) {
      for (int i = 0; i < JOINTS; ++i) {
        if (cmd.applies(static_cast<std::size_t>(i))) continue;
        std::fprintf(stderr,
                     "skynet-fairino: this torque command masks %s, and a masked joint is §24's "
                     "hold. In torque a hold has no correct value — zero is a joint that gives "
                     "way on an arm holding itself up — so the command is refused rather than "
                     "sent limp. Command every joint's torque, or hold in position.\n",
                     JOINT_NAMES[i]);
        refusals_ += 1;
        return SKR_EPERM;
      }
    }

    /* The robot's own stop circuit is checked before anything is sent, which is what the vendor's
     * own `GetSafetyCode()` does before every servo verb. A command sent into an asserted safety
     * stop is refused by the controller anyway; refusing it here says *why* rather than returning
     * the vendor's bare 99. */
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (state_.emergency_stop || state_.safety_stop0 || state_.safety_stop1) {
        std::fprintf(stderr,
                     "skynet-fairino: refused — the robot reports its own stop asserted "
                     "(emergency_stop %u, SI0 %u, SI1 %u). Nothing this driver sends will move "
                     "it until that is cleared at the machine.\n",
                     static_cast<unsigned>(state_.emergency_stop),
                     static_cast<unsigned>(state_.safety_stop0),
                     static_cast<unsigned>(state_.safety_stop1));
        refusals_ += 1;
        return SKR_EPERM;
      }
    }

    if (body_mode == SKR_MODE_POSITION) {
      for (int i = 0; i < JOINTS; ++i) {
        if (!cmd.applies(static_cast<std::size_t>(i))) continue;
        const double degrees = static_cast<double>(values[i]) * RAD_TO_DEG;
        if (degrees < travel_[i].low_deg || degrees > travel_[i].high_deg) {
          std::fprintf(stderr,
                       "skynet-fairino: %s commanded to %.3f deg, outside the soft limit %s "
                       "(%.3f to %.3f). Refused rather than clamped: a clamp would move the joint "
                       "somewhere nobody asked for.\n",
                       JOINT_NAMES[i], degrees,
                       limits_from_robot_ ? "this controller reports" : "FAIR INNOVATION publish",
                       travel_[i].low_deg, travel_[i].high_deg);
          refusals_ += 1;
          return SKR_EINVAL;
        }
      }
    }

    const Window wanted = body_mode == SKR_MODE_TORQUE ? Window::Torque : Window::Move;
    const int windowed = ensure_window(wanted);
    if (windowed != SKR_OK) return windowed;

    double payload[JOINTS] = {0, 0, 0, 0, 0, 0};
    for (int i = 0; i < JOINTS; ++i) {
      const bool applies = cmd.applies(static_cast<std::size_t>(i));
      switch (body_mode) {
        case SKR_MODE_POSITION:
          /* §24's hold, in this wire's own terms: the setpoint this driver last issued for that
           * joint, which at the start of a window is the position the robot was measured at. Not
           * the *current* measurement, which sags under load — repeating it every cycle would
           * walk the joint downhill one sag at a time. */
          if (applies) held_deg_[i] = static_cast<double>(values[i]) * RAD_TO_DEG;
          payload[i] = held_deg_[i];
          break;
        case SKR_MODE_VELOCITY:
          payload[i] = applies ? static_cast<double>(values[i]) * RAD_TO_DEG : 0.0;
          break;
        case SKR_MODE_TORQUE:
          /* Every joint applies: a masked torque command was refused above. */
          payload[i] = static_cast<double>(values[i]);
          break;
        default:
          return SKR_EINVAL;
      }
    }

    long code = 0;
    bool sent = false;
    if (body_mode == SKR_MODE_POSITION) {
      const double exaxis[4] = {0.0, 0.0, 0.0, 0.0};
      sent = call_int(RPC_SERVO_J,
                      {RpcValue::of_doubles(payload, JOINTS), RpcValue::of_doubles(exaxis, 4),
                       RpcValue::of_double(0.0), RpcValue::of_double(0.0),
                       RpcValue::of_double(servo_cmd_t_), RpcValue::of_double(0.0),
                       RpcValue::of_double(0.0), RpcValue::of_int(0)},
                      &code);
    } else if (body_mode == SKR_MODE_VELOCITY) {
      const double exaxis[4] = {0.0, 0.0, 0.0, 0.0};
      sent = call_int(RPC_SERVO_JV,
                      {RpcValue::of_doubles(payload, JOINTS), RpcValue::of_doubles(exaxis, 4),
                       RpcValue::of_double(0.0), RpcValue::of_double(0.0),
                       RpcValue::of_double(servo_cmd_t_), RpcValue::of_double(0.0),
                       RpcValue::of_double(0.0), RpcValue::of_int(0)},
                      &code);
    } else {
      /* `checkFlag` 0 and both limit arrays zero: the vendor's own two-argument `ServoJT` fills
       * them exactly this way before calling the six-argument one. Power and velocity limiting
       * are the operator's policy and §63.4's rule applies — this driver will not invent a
       * ceiling the operator did not set. */
      const double zeros[JOINTS] = {0, 0, 0, 0, 0, 0};
      sent = call_int(RPC_SERVO_JT,
                      {RpcValue::of_doubles(payload, JOINTS), RpcValue::of_double(servo_cmd_t_),
                       RpcValue::of_int(0), RpcValue::of_doubles(zeros, JOINTS),
                       RpcValue::of_doubles(zeros, JOINTS)},
                      &code);
    }
    if (!sent) return SKR_EIO;
    if (code != 0) {
      std::fprintf(stderr, "skynet-fairino: the controller refused the command, error %ld.\n",
                   code);
      refusals_ += 1;
      return SKR_EIO;
    }
    commands_ += 1;
    return SKR_OK;
  }

  int emergency_stop() override {
    /* §4: from a signal handler and from a dying process. The bytes and the socket were both
     * made at `open()`, so this formats nothing and allocates nothing, and `send` on a connected
     * socket is on POSIX's async-signal-safe list.
     *
     * What goes out is `FRRobot::StopMotion()`'s own frame — the same thirty-two characters the
     * vendor's SDK sends, on the same port. That is as close as a software stop gets to being the
     * vendor's, and it is still a software stop: the controller's own emergency circuit is the
     * button on the box, this driver reports its state and does not pretend to be it. The latch
     * is what protects the rest of the session. */
    estopped_.store(true);
    if (command_fd_ < 0) return SKR_EPERM;
    const ssize_t sent = ::send(command_fd_, STOP_FRAME, STOP_FRAME_BYTES, MSG_NOSIGNAL);
    return sent == static_cast<ssize_t>(STOP_FRAME_BYTES) ? SKR_OK : SKR_EIO;
  }

  int extension(std::string_view token, const void *, std::size_t, void *out,
                std::size_t out_len) override {
    if (token != TELEMETRY_TOKEN) return SKR_ENOSYS;
    if (!opened_) return SKR_EPERM;

    Reported snapshot;
    std::uint64_t arrivals = 0;
    std::uint64_t malformed = 0;
    std::uint64_t stale = 0;
    double interval_ms = 0.0;
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!have_state_) return SKR_EIO;
      snapshot = state_;
      arrivals = arrivals_;
      malformed = malformed_;
      stale = stale_cycles_;
      interval_ms = static_cast<double>(interval_ns_) / 1e6;
    }

    /* Everything the stream carries that §24's `State` has no field for, in the units the
     * controller reports them in. */
    char buffer[768];
    std::string json = "{\"robot_model\":\"" + model_text_ + "\",\"controller_version\":\"" +
                       controller_version_ + "\",\"serial\":\"" + serial_ + "\"";
    std::snprintf(buffer, sizeof buffer,
                  ",\"program_state\":%u,\"robot_state\":%u,\"robot_mode\":%u,"
                  "\"main_code\":%d,\"sub_code\":%d,\"enable_state\":%d,\"motion_done\":%d,"
                  "\"motion_queue_len\":%d,\"collision_state\":%u,\"emergency_stop\":%u,"
                  "\"safety_stop0\":%u,\"safety_stop1\":%u",
                  static_cast<unsigned>(snapshot.program_state),
                  static_cast<unsigned>(snapshot.robot_state),
                  static_cast<unsigned>(snapshot.robot_mode), snapshot.main_code,
                  snapshot.sub_code, snapshot.enable_state, snapshot.motion_done,
                  snapshot.queue_len, static_cast<unsigned>(snapshot.collision),
                  static_cast<unsigned>(snapshot.emergency_stop),
                  static_cast<unsigned>(snapshot.safety_stop0),
                  static_cast<unsigned>(snapshot.safety_stop1));
    json += buffer;
    json += ",\"joint_driver_torque_nm\":[";
    for (int i = 0; i < JOINTS; ++i) {
      std::snprintf(buffer, sizeof buffer, "%s%.6f", i ? "," : "", snapshot.driver_torque_nm[i]);
      json += buffer;
    }
    json += "],\"joint_driver_temperature_c\":[";
    for (int i = 0; i < JOINTS; ++i) {
      std::snprintf(buffer, sizeof buffer, "%s%.3f", i ? "," : "",
                    snapshot.driver_temperature_c[i]);
      json += buffer;
    }
    /* The controller's own record of the last setpoint it accepted, which is not this driver's
     * record of the last one it sent — the two differing is how a dropped command shows up. */
    json += "],\"controller_last_servo_target_deg\":[";
    for (int i = 0; i < JOINTS; ++i) {
      std::snprintf(buffer, sizeof buffer, "%s%.6f", i ? "," : "",
                    snapshot.last_servo_target_deg[i]);
      json += buffer;
    }
    json += "],\"driver_held_setpoint_deg\":[";
    for (int i = 0; i < JOINTS; ++i) {
      std::snprintf(buffer, sizeof buffer, "%s%.6f", i ? "," : "", held_deg_[i]);
      json += buffer;
    }
    json += "]";
    std::snprintf(buffer, sizeof buffer,
                  ",\"arrivals\":%llu,\"dropped\":%llu,\"stale_reads\":%llu,"
                  "\"stream_interval_ms\":%.3f,\"requested_period_ms\":%d,\"commands\":%llu,"
                  "\"refusals\":%llu,\"servo_window\":\"%s\",\"limits_from_robot\":%s}",
                  static_cast<unsigned long long>(arrivals),
                  static_cast<unsigned long long>(malformed),
                  static_cast<unsigned long long>(stale), interval_ms, period_ms_,
                  static_cast<unsigned long long>(commands_),
                  static_cast<unsigned long long>(refusals_),
                  window_ == Window::Move ? "move" : (window_ == Window::Torque ? "torque" : "none"),
                  limits_from_robot_ ? "true" : "false");
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
                       ",\"base\":\"fixed\",\"dof\":6,\"joints\":[";
    for (int i = 0; i < JOINTS; ++i) {
      if (i) json += ",";
      char buffer[512];
      /* No `effort_max_nm`: FAIR INNOVATION publish no joint torque rating, and a key written
       * with a number this tree had guessed would be worse than the key being absent. The
       * schema's optional fields exist for exactly this (F91). */
      std::snprintf(buffer, sizeof buffer,
                    "{\"name\":\"%s\",\"role\":\"%s\",\"type\":\"revolute\","
                    "\"limits\":{\"position_min_rad\":%.5f,\"position_max_rad\":%.5f,"
                    "\"velocity_max_rad_s\":%.5f},"
                    "\"control_modes\":[\"position\",\"velocity\",\"torque\"]}",
                    JOINT_NAMES[i], ROLES[i], travel_[i].low_deg * DEG_TO_RAD,
                    travel_[i].high_deg * DEG_TO_RAD,
                    CATALOGUE_VELOCITY_MAX_DEG_S * DEG_TO_RAD);
      json += buffer;
    }
    json +=
        "],\"end_effectors\":[],\"sensors\":[],"
        "\"control\":{\"modes\":[\"position\",\"velocity\",\"torque\"],\"per_joint_modes\":false,";
    /* The rate: the state stream's period is this driver's, set by config and accepted by the
     * controller, and `ServoJ`'s own recommended command period is the vendor's. Neither is *the
     * loop rate of the body*, which is what §24's field means and which FAIR INNOVATION do not
     * publish — so both are reported where they are facts (the extension, the README) and the
     * field stays null rather than carrying a number that reads as the robot's (F64). */
    json += "\"rate_hz_max\":null,\"rate_hz_min\":null,\"command_latency_ms\":null},";

    char envelope[1280];
    std::snprintf(
        envelope, sizeof envelope,
        "\"envelope\":{\"joint_velocity_max_rad_s\":%.5f,\"watchdog_ms\":null,\"estop\":true,"
        "\"source\":\"FAIR INNOVATION FR3 product page (frtech.fr/FR/5.html): every axis "
        "+/-180 deg/s, travel base +/-175, shoulder +85/-265, elbow +/-150, wrist1 +85/-265, "
        "wrist2 +/-175, wrist3 +/-175; %s; no joint torque rating is published by this vendor, so "
        "none is declared; the hardware stop is the control box button, whose state the "
        "controller streams as emergency_stop with two independent safety-stop inputs beside it "
        "(fairino-cpp-sdk, FRCNDEClient InitAllStates and FRRobot::GetSafetyCode); no "
        "command-loss window is published, so watchdog_ms is null\",\"basis\":\"declared\"}",
        CATALOGUE_VELOCITY_MAX_DEG_S * DEG_TO_RAD,
        limits_from_robot_
            ? "the per-joint travel above is superseded in this descriptor by the soft limits this "
              "controller reported to GetJointSoftLimitDeg"
            : "the controller did not answer GetJointSoftLimitDeg, so the published travel is what "
              "the per-joint limits carry");
    json += envelope;

    /* `safety.estop` is §15's *software* stop and this driver has one — the vendor's own STOP
     * frame, on the vendor's own port, latched afterwards. `envelope.estop` is the different
     * question, and here it is answerable in the affirmative for the first time in this tree: an
     * FR controller streams the state of a physical emergency-stop button and of two independent
     * safety-stop inputs, and the vendor's own SDK refuses every servo verb while either input is
     * asserted. That is a stop path that does not run through this application processor, which
     * is what the `estop_hardware` check asks. It gates the certified tier and nothing else, and
     * the certified tier still needs the bench measurement nobody here has taken.
     *
     * `platform.onboard_compute` is **not** declared. The controller runs the vendor's own
     * software and accepts uploaded Lua programs, which is user code on the robot's computer in a
     * narrow sense; it is not a general-purpose computer the OS could be deployed onto, and the
     * vendor documents no way to run one's own process there. The Lite3 row declares the token on
     * evidence this row does not have. */
    json += ",\"capabilities\":[\"safety.estop\"],\"extensions\":[\"";
    json += TELEMETRY_TOKEN;
    json += "\"],\"solvers\":[]}";
    return json;
  }

 private:
  /* ── XML-RPC ────────────────────────────────────────────────────────── */

  /* One connection per call, closed after it — which is exactly what the vendor's SDK does
   * (`XmlRpcClient c(serverUrl, 20003); … c.close();` inside every verb). A persistent connection
   * would be cheaper and this driver does not take it: whether an FR controller's XML-RPC server
   * honours HTTP keep-alive is not documented anywhere, and a driver that assumed it would fail
   * on the robots where it does not, in the field, at the servo rate. */
  bool call(const char *method, const std::vector<RpcValue> &params, RpcValue *result) {
    const int fd = connect_tcp(host_, xmlrpc_port_, rpc_timeout_ms_);
    if (fd < 0) return false;
    const std::string request = rpc_request(host_, xmlrpc_port_, method, params);
    bool ok = send_all(fd, request.data(), request.size());
    std::string response;
    if (ok) {
      char chunk[2048];
      std::size_t header_bytes = 0;
      std::size_t content_length = 0;
      bool header_done = false;
      for (;;) {
        const ssize_t got = ::recv(fd, chunk, sizeof chunk, 0);
        if (got <= 0) break;
        response.append(chunk, static_cast<std::size_t>(got));
        if (!header_done) {
          header_done = rpc_content_length(response, &header_bytes, &content_length);
        }
        if (header_done && response.size() >= header_bytes + content_length) break;
        if (response.size() > 1u << 20) break;
      }
      ok = header_done && response.size() >= header_bytes + content_length && content_length > 0;
      if (ok) response = response.substr(header_bytes, content_length);
    }
    ::close(fd);
    if (!ok) {
      std::fprintf(stderr, "skynet-fairino: XML-RPC %s got no usable reply from %s:%u.\n", method,
                   host_.c_str(), static_cast<unsigned>(xmlrpc_port_));
      return false;
    }
    bool faulted = false;
    if (!rpc_parse_response(response, result, &faulted)) {
      std::fprintf(stderr, "skynet-fairino: XML-RPC %s returned a document this driver could not "
                           "parse.\n",
                   method);
      return false;
    }
    if (faulted) {
      std::fprintf(stderr,
                   "skynet-fairino: XML-RPC %s came back a fault — on a real controller that "
                   "means the verb does not exist on this firmware.\n",
                   method);
      return false;
    }
    return true;
  }

  /* The vendor reads every reply as either a bare integer or an array whose first element is one
   * (`int(result)` and `int(result[0])` throughout `robot.cpp`), so both are accepted here. */
  bool call_int(const char *method, const std::vector<RpcValue> &params, long *code) {
    RpcValue result;
    if (!call(method, params, &result)) return false;
    if (result.kind == RpcValue::Kind::Array) {
      if (result.items.empty()) return false;
      return result.items[0].as_int(code);
    }
    return result.as_int(code);
  }

  void ask_identity() {
    RpcValue result;
    if (call(RPC_GET_SOFTWARE_VERSION, {}, &result) &&
        result.kind == RpcValue::Kind::Array && result.items.size() >= 4) {
      model_text_ = result.items[1].text;
      controller_version_ = result.items[3].text;
    }
    if (call(RPC_GET_ROBOT_SN, {}, &result) && result.kind == RpcValue::Kind::Array &&
        result.items.size() >= 2) {
      serial_ = result.items[1].text;
    }
  }

  /* `GetJointSoftLimitDeg` answers `[errcode, negative0, positive0, negative1, positive1, …]` —
   * the interleaving is the vendor's, read straight out of their own result indexing. */
  void ask_joint_limits() {
    for (int i = 0; i < JOINTS; ++i) travel_[i] = CATALOGUE_TRAVEL[i];
    RpcValue result;
    if (!call(RPC_GET_JOINT_SOFT_LIMIT, {}, &result)) return;
    if (result.kind != RpcValue::Kind::Array || result.items.size() < 1 + 2 * JOINTS) return;
    long code = 0;
    if (!result.items[0].as_int(&code) || code != 0) return;
    Travel reported[JOINTS];
    for (int i = 0; i < JOINTS; ++i) {
      double low = 0.0;
      double high = 0.0;
      if (!result.items[1 + 2 * i].as_double(&low)) return;
      if (!result.items[2 + 2 * i].as_double(&high)) return;
      if (!(low < high)) {
        std::fprintf(stderr,
                     "skynet-fairino: the controller reported a soft limit for %s that is not a "
                     "range (%.3f to %.3f). Keeping the published travel.\n",
                     JOINT_NAMES[i], low, high);
        return;
      }
      reported[i].low_deg = low;
      reported[i].high_deg = high;
    }
    for (int i = 0; i < JOINTS; ++i) travel_[i] = reported[i];
    limits_from_robot_ = true;
    std::fprintf(stderr,
                 "skynet-fairino: travel taken from this controller's own soft limits, not from "
                 "the catalogue.\n");
  }

  /* ── the servo window ───────────────────────────────────────────────── */

  int ensure_window(Window wanted) {
    if (window_ == wanted) return SKR_OK;
    close_window();

    long code = 0;
    if (set_auto_mode_) {
      std::uint8_t mode = 0;
      {
        std::lock_guard<std::mutex> guard(lock_);
        mode = state_.robot_mode;
      }
      if (mode == 1) {
        /* Manual mode is the pendant's. A servo verb sent into it is refused by the controller,
         * so the switch is made here and announced — it is a change to the machine's state, not
         * a detail. */
        std::fprintf(stderr, "skynet-fairino: the controller is in manual mode; switching it to "
                             "auto so servo commands are accepted.\n");
        if (!call_int(RPC_MODE, {RpcValue::of_int(0)}, &code) || code != 0) {
          std::fprintf(stderr, "skynet-fairino: the controller refused Mode(0), error %ld.\n",
                       code);
          return SKR_EIO;
        }
      }
    }

    std::int32_t enabled = 0;
    {
      std::lock_guard<std::mutex> guard(lock_);
      enabled = state_.enable_state;
    }
    if (enabled == 0) {
      if (!enable_) {
        std::fprintf(stderr,
                     "skynet-fairino: the robot's drives are not enabled and `enable` is false in "
                     "this driver's config, so nothing will move. Enabling a cobot energises six "
                     "joints; this driver will not do it unless it is asked to.\n");
        return SKR_EPERM;
      }
      std::fprintf(stderr, "skynet-fairino: enabling the drives (RobotEnable(1)).\n");
      if (!call_int(RPC_ROBOT_ENABLE, {RpcValue::of_int(1)}, &code) || code != 0) {
        std::fprintf(stderr, "skynet-fairino: the controller refused RobotEnable(1), error %ld.\n",
                     code);
        return SKR_EIO;
      }
    }

    const char *verb = wanted == Window::Torque ? RPC_SERVO_JT_START : RPC_SERVO_MOVE_START;
    if (!call_int(verb, {}, &code) || code != 0) {
      std::fprintf(stderr, "skynet-fairino: the controller refused %s, error %ld.\n", verb, code);
      return SKR_EIO;
    }

    /* The held setpoints start where the robot is. Everything after this is this driver's own
     * record, never the measurement again — see `write_command`. */
    {
      std::lock_guard<std::mutex> guard(lock_);
      for (int i = 0; i < JOINTS; ++i) held_deg_[i] = state_.position_deg[i];
    }
    window_ = wanted;
    return SKR_OK;
  }

  void close_window() {
    if (window_ == Window::None) return;
    long code = 0;
    const char *verb = window_ == Window::Torque ? RPC_SERVO_JT_END : RPC_SERVO_MOVE_END;
    if (!call_int(verb, {}, &code) || code != 0) {
      std::fprintf(stderr, "skynet-fairino: %s did not come back clean (error %ld). The window "
                           "may still be open on the controller.\n",
                   verb, code);
    }
    window_ = Window::None;
  }

  /* ── the CNDE stream ────────────────────────────────────────────────── */

  int open_state_stream() {
    state_fd_ = connect_tcp(host_, state_port_, rpc_timeout_ms_);
    if (state_fd_ < 0) {
      std::fprintf(stderr,
                   "skynet-fairino: cannot reach the CNDE state port %u on %s.\n",
                   static_cast<unsigned>(state_port_), host_.c_str());
      return SKR_EIO;
    }

    char names[1024];
    const std::string_view list = state_field_list(names, sizeof names);
    if (list.empty()) return SKR_EINVAL;

    std::vector<unsigned char> payload;
    payload.push_back(static_cast<unsigned char>(period_ms_ & 0xFF));
    payload.push_back(static_cast<unsigned char>((period_ms_ >> 8) & 0xFF));
    payload.insert(payload.end(), list.begin(), list.end());

    std::vector<unsigned char> frame(payload.size() + CNDE_OVERHEAD);
    std::size_t length =
        build_cnde_frame(frame.data(), cnde_count_++, CNDE_TYPE_OUTPUT_CONFIG, payload.data(),
                         payload.size());
    if (!send_all(state_fd_, frame.data(), length)) return SKR_EIO;
    int replied = await_message("the field configuration");
    if (replied != SKR_OK) return replied;

    unsigned char start[CNDE_OVERHEAD];
    length = build_cnde_frame(start, cnde_count_++, CNDE_TYPE_START, nullptr, 0);
    if (!send_all(state_fd_, start, length)) return SKR_EIO;
    replied = await_message("the stream start");
    if (replied != SKR_OK) return replied;

    running_.store(true);
    reader_ = std::thread([this] { read_loop(); });
    return SKR_OK;
  }

  /* The controller answers a configuration and a start with a MESSAGE frame whose first byte is
   * zero for done; anything else carries the reason as text. Frames of other types that arrive in
   * between are skipped, which is what the vendor's own client does. */
  int await_message(const char *what) {
    for (int attempt = 0; attempt < 64; ++attempt) {
      std::vector<unsigned char> frame;
      if (!read_frame(&frame)) {
        std::fprintf(stderr, "skynet-fairino: the CNDE stream closed while waiting to hear about "
                             "%s.\n",
                     what);
        return SKR_EIO;
      }
      if (frame[3] != CNDE_TYPE_MESSAGE) continue;
      const std::size_t payload = frame.size() - CNDE_OVERHEAD;
      if (payload == 0) return SKR_EIO;
      if (frame[6] == CNDE_MESSAGE_OK) return SKR_OK;
      std::string reason(reinterpret_cast<const char *>(frame.data()) + 7, payload - 1);
      std::fprintf(stderr, "skynet-fairino: the controller refused %s: %s\n", what, reason.c_str());
      return SKR_EINVAL;
    }
    return SKR_EIO;
  }

  /* `5A 5A | count | type | len | payload | A5 A5`, length-prefixed exactly as the vendor's
   * `RecvCNDEPkg` reads it: six bytes, then `len + 2` more. */
  bool read_frame(std::vector<unsigned char> *out) {
    unsigned char head[6];
    if (!recv_exact(state_fd_, head, sizeof head)) return false;
    if (head[0] != CNDE_HEAD_0 || head[1] != CNDE_HEAD_1) {
      std::lock_guard<std::mutex> guard(lock_);
      malformed_ += 1;
      return false;
    }
    const std::size_t payload =
        static_cast<std::size_t>(head[4]) | (static_cast<std::size_t>(head[5]) << 8);
    if (payload + CNDE_OVERHEAD > CNDE_MAX_FRAME) {
      std::lock_guard<std::mutex> guard(lock_);
      malformed_ += 1;
      return false;
    }
    out->resize(payload + CNDE_OVERHEAD);
    std::memcpy(out->data(), head, sizeof head);
    if (!recv_exact(state_fd_, out->data() + 6, payload + 2)) return false;
    if ((*out)[payload + 6] != CNDE_TAIL_0 || (*out)[payload + 7] != CNDE_TAIL_1) {
      std::lock_guard<std::mutex> guard(lock_);
      malformed_ += 1;
      return false;
    }
    return true;
  }

  void read_loop() {
    while (running_.load()) {
      std::vector<unsigned char> frame;
      if (!read_frame(&frame)) {
        if (!running_.load()) return;
        std::this_thread::sleep_for(std::chrono::milliseconds(2));
        continue;
      }
      /* The stamp is taken here, at acquisition, never at `read_state`'s return (§24, §35.1). */
      const std::uint64_t stamp = skynet::acquisition_ns();
      if (frame[3] != CNDE_TYPE_OUTPUT_STATE) continue;
      const std::size_t payload = frame.size() - CNDE_OVERHEAD;
      if (payload != state_payload_bytes()) {
        /* The controller streamed a different number of bytes from the fields this driver asked
         * for. That is not a short read to recover from: it means the field list and the stream
         * disagree, and every offset below it would be wrong. */
        std::lock_guard<std::mutex> guard(lock_);
        malformed_ += 1;
        continue;
      }
      const unsigned char *at = frame.data() + 6;
      Reported incoming;
      incoming.program_state = read_uint8(at, state_offset(IDX_PROGRAM_STATE));
      incoming.robot_state = read_uint8(at, state_offset(IDX_ROBOT_STATE));
      incoming.main_code = read_int32(at, state_offset(IDX_MAIN_CODE));
      incoming.sub_code = read_int32(at, state_offset(IDX_SUB_CODE));
      incoming.robot_mode = read_uint8(at, state_offset(IDX_ROBOT_MODE));
      for (int i = 0; i < JOINTS; ++i) {
        const std::size_t step = static_cast<std::size_t>(i) * sizeof(double);
        incoming.position_deg[i] = read_double(at, state_offset(IDX_JOINT_POS) + step);
        incoming.velocity_deg_s[i] = read_double(at, state_offset(IDX_JOINT_VEL) + step);
        incoming.torque_nm[i] = read_double(at, state_offset(IDX_JOINT_TORQUE) + step);
        incoming.driver_torque_nm[i] = read_double(at, state_offset(IDX_DRIVER_TORQUE) + step);
        incoming.driver_temperature_c[i] = read_double(at, state_offset(IDX_JOINT_TEMP) + step);
        incoming.last_servo_target_deg[i] =
            read_double(at, state_offset(IDX_LAST_SERVO_TARGET) + step);
      }
      incoming.emergency_stop = read_uint8(at, state_offset(IDX_EMERGENCY_STOP));
      incoming.safety_stop0 = read_uint8(at, state_offset(IDX_SAFETY_STOP0));
      incoming.safety_stop1 = read_uint8(at, state_offset(IDX_SAFETY_STOP1));
      incoming.collision = read_uint8(at, state_offset(IDX_COLLISION));
      incoming.motion_done = read_int32(at, state_offset(IDX_MOTION_DONE));
      incoming.queue_len = read_int32(at, state_offset(IDX_QUEUE_LEN));
      incoming.enable_state = read_int32(at, state_offset(IDX_ENABLE_STATE));

      std::lock_guard<std::mutex> guard(lock_);
      if (have_state_ && stamp > stamp_ns_) interval_ns_ = stamp - stamp_ns_;
      state_ = incoming;
      stamp_ns_ = stamp;
      arrivals_ += 1;
      have_state_ = true;
    }
  }

  void teardown() {
    running_.store(false);
    if (state_fd_ >= 0) ::shutdown(state_fd_, SHUT_RDWR);
    if (reader_.joinable()) reader_.join();
    if (state_fd_ >= 0) {
      ::close(state_fd_);
      state_fd_ = -1;
    }
    if (command_fd_ >= 0) {
      ::close(command_fd_);
      command_fd_ = -1;
    }
    have_state_ = false;
    window_ = Window::None;
  }

  bool opened_ = false;
  std::atomic<bool> estopped_{false};
  std::atomic<bool> running_{false};

  std::string host_ = "127.0.0.1";
  std::string slug_ = "fairino-fr3";
  std::string model_ = "fr3";
  std::string model_text_;
  std::string controller_version_;
  std::string serial_;
  std::uint16_t xmlrpc_port_ = XMLRPC_PORT;
  std::uint16_t state_port_ = STATE_PORT;
  std::uint16_t command_port_ = COMMAND_PORT;
  bool simulator_ = true;
  bool enable_ = true;
  bool set_auto_mode_ = true;
  int period_ms_ = CNDE_DEFAULT_PERIOD_MS;
  int rpc_timeout_ms_ = 2000;
  double servo_cmd_t_ = CNDE_DEFAULT_PERIOD_MS / 1000.0;
  std::uint8_t cnde_count_ = 0;

  Travel travel_[JOINTS] = {};
  bool limits_from_robot_ = false;
  double held_deg_[JOINTS] = {0, 0, 0, 0, 0, 0};
  Window window_ = Window::None;

  int command_fd_ = -1;
  int state_fd_ = -1;
  std::thread reader_;
  mutable std::mutex lock_;
  Reported state_{};
  std::uint64_t stamp_ns_ = 0;
  std::uint64_t arrivals_ = 0;
  std::uint64_t reported_arrivals_ = 0;
  std::uint64_t interval_ns_ = 0;
  std::uint64_t stale_cycles_ = 0;
  std::uint64_t malformed_ = 0;
  std::uint64_t commands_ = 0;
  std::uint64_t refusals_ = 0;
  bool have_state_ = false;
};

}  // namespace

SKYNET_RHAL_EXPORT(FairinoBridge)
