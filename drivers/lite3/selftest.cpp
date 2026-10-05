/*
 * skynet-lite3's own suite: the mode mapping, the mask, control right, every refusal, and the
 * bytes each one puts on the wire.
 *
 * Linked against `lite3_bridge.cpp` rather than dlopening it, so a refusal can be asserted
 * directly through the vtable.
 *
 * **The suite is its own motion host.** It binds the command port, pushes `RobotData` from a
 * thread, and keeps every datagram the driver sends — so what is asserted is not a return code
 * but the twelve `JointCmd` structs that went out, the twelve-byte control-right commands around
 * them, and the order of the two. `fake_motion_host` is the same wire with a plant behind it and
 * is what `skynet driver check` and the cross-check run against; this one is a microscope.
 *
 * Held to controls. Each of these single-line edits fails a named check rather than passing
 * quietly:
 *
 *   * writing a masked joint as every field zero (the reading this driver had until F121) fails
 *     "a masked joint with no position setpoint is held where it was measured";
 *   * re-reading the hold target every cycle instead of latching it fails "the hold target is
 *     latched, not re-read";
 *   * taking control right once and never again fails "a command after the reclaim window takes
 *     control right back";
 *   * clamping an out-of-travel setpoint instead of refusing it fails "a setpoint outside the
 *     published travel is refused" *and* "nothing reached the wire";
 *   * advancing `seq` per read rather than per arrival fails "seq counts arrivals, not reads";
 *   * stamping at return rather than at acquisition fails "the stamp is the arrival's".
 *
 * Licence: Apache-2.0.
 */

#include "skynet_rhal.h"
#include "lite3_wire.hpp"

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

using namespace skynet_lite3;

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

bool near(double a, double b, double tolerance = 1e-5) { return std::fabs(a - b) <= tolerance; }

/* ── the suite's own motion host ──────────────────────────────────────── */

class MotionHost {
 public:
  MotionHost(std::uint16_t command_port, std::uint16_t state_port, double hz)
      : command_port_(command_port), state_port_(state_port), hz_(hz) {}

  bool start(bool push) {
    listen_fd_ = ::socket(AF_INET, SOCK_DGRAM, 0);
    if (listen_fd_ < 0) return false;
    int reuse = 1;
    ::setsockopt(listen_fd_, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof reuse);
    sockaddr_in local{};
    local.sin_family = AF_INET;
    local.sin_addr.s_addr = htonl(INADDR_ANY);
    local.sin_port = htons(command_port_);
    if (::bind(listen_fd_, reinterpret_cast<const sockaddr *>(&local), sizeof local) < 0) {
      return false;
    }
    timeval timeout{};
    timeout.tv_usec = 20000;
    ::setsockopt(listen_fd_, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);

    push_fd_ = ::socket(AF_INET, SOCK_DGRAM, 0);
    if (push_fd_ < 0) return false;
    remote_.sin_family = AF_INET;
    remote_.sin_port = htons(state_port_);
    ::inet_pton(AF_INET, "127.0.0.1", &remote_.sin_addr);

    running_.store(true);
    receiver_ = std::thread([this] { receive_loop(); });
    if (push) pusher_ = std::thread([this] { push_loop(); });
    return true;
  }

  void stop() {
    running_.store(false);
    if (receiver_.joinable()) receiver_.join();
    if (pusher_.joinable()) pusher_.join();
    if (listen_fd_ >= 0) ::close(listen_fd_);
    if (push_fd_ >= 0) ::close(push_fd_);
    listen_fd_ = push_fd_ = -1;
  }

  /* Send the driver a datagram of the suite's own making on its state port: a push that is too
   * short, or that carries another code, which a real network produces and a well-behaved host
   * never does. */
  void inject(const void *bytes, std::size_t length) {
    ::sendto(push_fd_, bytes, length, 0, reinterpret_cast<const sockaddr *>(&remote_),
             sizeof remote_);
  }

  /* Stop being a host at all: the command port is closed, as it is when the robot's motion host
   * has gone. A connected UDP socket learns this from the next datagram it sends. The receive
   * thread closes the socket itself, so no `recv` is in flight on a descriptor that goes away. */
  void stop_listening() {
    listening_.store(false);
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (!listen_closed_.load() && std::chrono::steady_clock::now() < deadline) {
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
  }

  /* What the body reports. Set it, then wait for an arrival so the driver has seen it. */
  void set_joint(int i, float position, float velocity, float torque, float temperature) {
    std::lock_guard<std::mutex> guard(lock_);
    position_[i] = position;
    velocity_[i] = velocity;
    torque_[i] = torque;
    temperature_[i] = temperature;
  }
  void set_imu(float roll, float pitch, float yaw, float rate_roll, float rate_pitch,
               float rate_yaw) {
    std::lock_guard<std::mutex> guard(lock_);
    roll_ = roll;
    pitch_ = pitch;
    yaw_ = yaw;
    rate_roll_ = rate_roll;
    rate_pitch_ = rate_pitch;
    rate_yaw_ = rate_yaw;
  }

  std::size_t datagrams() {
    std::lock_guard<std::mutex> guard(lock_);
    return seen_.size();
  }
  std::vector<unsigned char> datagram(std::size_t index) {
    std::lock_guard<std::mutex> guard(lock_);
    return seen_.at(index);
  }
  void forget() {
    std::lock_guard<std::mutex> guard(lock_);
    seen_.clear();
  }
  std::uint64_t pushes() const { return pushes_.load(); }
  /* Stop and restart the body's own clock, so a read can be asked what it does when nothing
   * arrives. */
  void pause_pushes() { pushing_.store(false); }
  /* How often the body pushes. A slow body is how the cap on the wait for a push is reached. */
  void set_rate(double hz) { hz_.store(hz); }
  void resume_pushes() { pushing_.store(true); }

  /* Wait until at least `count` datagrams have arrived, or the deadline passes. */
  bool wait_for_datagrams(std::size_t count, int milliseconds = 1000) {
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::milliseconds(milliseconds);
    while (std::chrono::steady_clock::now() < deadline) {
      if (datagrams() >= count) return true;
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
    return datagrams() >= count;
  }

  /* Wait for `count` more state pushes to have gone out, so a driver reading afterwards is
   * reading something that arrived after the caller's change. */
  void wait_for_pushes(std::uint64_t count) {
    const std::uint64_t target = pushes_.load() + count;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (pushes_.load() < target && std::chrono::steady_clock::now() < deadline) {
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
  }

 private:
  void receive_loop() {
    unsigned char buffer[sizeof(CommandMessage)];
    while (running_.load()) {
      if (!listening_.load()) {
        if (listen_fd_ >= 0) {
          ::close(listen_fd_);
          listen_fd_ = -1;
        }
        listen_closed_.store(true);
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
        continue;
      }
      const ssize_t got = ::recv(listen_fd_, buffer, sizeof buffer, 0);
      if (got <= 0) continue;
      std::lock_guard<std::mutex> guard(lock_);
      seen_.emplace_back(buffer, buffer + got);
    }
  }

  void push_loop() {
    unsigned char datagram[STATE_DATAGRAM];
    EthCommand header{};
    header.code = CODE_STATE_PUSH;
    header.value_or_size = static_cast<std::uint32_t>(sizeof(RobotData));
    header.type_and_count = pack_type_count(TYPE_MESS_VALUES, 0);
    std::uint32_t tick = 0;
    while (running_.load()) {
      RobotData state{};
      state.tick = tick++;
      {
        std::lock_guard<std::mutex> guard(lock_);
        state.imu.timestamp = static_cast<std::int32_t>(tick);
        state.imu.angle_roll = roll_;
        state.imu.angle_pitch = pitch_;
        state.imu.angle_yaw = yaw_;
        state.imu.angular_velocity_roll = rate_roll_;
        state.imu.angular_velocity_pitch = rate_pitch_;
        state.imu.angular_velocity_yaw = rate_yaw_;
        state.imu.acc_z = 9.81f;
        for (int i = 0; i < JOINTS; ++i) {
          state.joint_data.joint_data[i].position = position_[i];
          state.joint_data.joint_data[i].velocity = velocity_[i];
          state.joint_data.joint_data[i].torque = torque_[i];
          state.joint_data.joint_data[i].temperature = temperature_[i];
          state.contact_force.leg_force[i] = 0.5 * i;
        }
      }
      if (pushing_.load()) {
        std::memcpy(datagram, &header, sizeof header);
        std::memcpy(datagram + sizeof header, &state, sizeof state);
        ::sendto(push_fd_, datagram, sizeof datagram, 0,
                 reinterpret_cast<const sockaddr *>(&remote_), sizeof remote_);
        pushes_.fetch_add(1);
      }
      std::this_thread::sleep_for(std::chrono::duration<double>(1.0 / hz_.load()));
    }
  }

  std::uint16_t command_port_;
  std::uint16_t state_port_;
  std::atomic<double> hz_;
  int listen_fd_ = -1;
  int push_fd_ = -1;
  sockaddr_in remote_{};
  std::atomic<bool> running_{false};
  std::atomic<bool> listening_{true};
  std::atomic<bool> listen_closed_{false};
  std::atomic<bool> pushing_{true};
  std::atomic<std::uint64_t> pushes_{0};
  std::thread receiver_;
  std::thread pusher_;
  std::mutex lock_;
  std::vector<std::vector<unsigned char>> seen_;
  float position_[JOINTS] = {0};
  float velocity_[JOINTS] = {0};
  float torque_[JOINTS] = {0};
  float temperature_[JOINTS] = {0};
  float roll_ = 0, pitch_ = 0, yaw_ = 0, rate_roll_ = 0, rate_pitch_ = 0, rate_yaw_ = 0;
};

/* ── reading a datagram back ──────────────────────────────────────────── */

struct Seen {
  std::uint32_t code = 0;
  std::uint32_t size = 0;
  std::uint32_t type = 0;
  std::uint32_t count = 0;
  std::size_t length = 0;
  RobotCmd command{};
};

Seen decode(const std::vector<unsigned char> &bytes) {
  Seen seen;
  seen.length = bytes.size();
  if (bytes.size() < sizeof(EthCommand)) return seen;
  EthCommand header{};
  std::memcpy(&header, bytes.data(), sizeof header);
  seen.code = header.code;
  seen.size = header.value_or_size;
  seen.type = wire_type(header.type_and_count);
  seen.count = wire_count(header.type_and_count);
  if (bytes.size() >= JOINT_DATAGRAM) {
    std::memcpy(&seen.command, bytes.data() + sizeof(EthCommand), sizeof seen.command);
  }
  return seen;
}

/* CLOCK_MONOTONIC_RAW, the clock `skynet::acquisition_ns()` stamps on, so that a stamp can be aged
 * against the instant a read returned. */
std::uint64_t raw_ns() {
  timespec now{};
  clock_gettime(CLOCK_MONOTONIC_RAW, &now);
  return static_cast<std::uint64_t>(now.tv_sec) * 1000000000ull +
         static_cast<std::uint64_t>(now.tv_nsec);
}

/* What a read owes a caller who comes back before the next push: a sample it has not returned
 * before, or — when the body is late — the previous one **after waiting**. The driver's wait is
 * twice the interval it last measured, held inside 1 to 100 ms, so the only floor a test can state
 * without knowing the interval is the clamp's: a millisecond (less the clock's and the sleep's
 * granularity). A body that pushes every 5 ms is sometimes 10 ms late on a machine with every core
 * busy, and a suite that called the repeated `seq` it then gets a failure failed one run in five;
 * what this still refuses is a read that returns the previous sample *without waiting*, which a
 * driver that has lost its wait does in microseconds on every call. */
constexpr std::uint64_t MIN_WAIT_NS = 900000ull;
bool fresh_or_waited(std::uint32_t before, std::uint32_t after, std::uint64_t took_ns) {
  return after > before || took_ns >= MIN_WAIT_NS;
}

/* The newest joint-command datagram the host has seen that `matches`, waiting for one.
 *
 * Not "the last datagram, once any has arrived". A write that comes after the driver's reclaim
 * guard has run out (80 ms of silence) sends control right first and the command after it, and on a
 * machine slow enough to wake this thread between the two the last datagram is the twelve-byte
 * control one: a suite that decoded it saw twelve joints of zeros and failed three checks at once,
 * five runs in twenty-five with every core busy. `matches` names something only the command under
 * test carries, so that a datagram the previous command left in flight past `forget()` is not taken
 * for it. When none arrives in time the newest joint-command datagram is returned (a zeroed `Seen`
 * if there is none), so that the check which follows fails on what the driver did and not on a
 * timeout. */
template <class Matches>
Seen newest_joint_command(MotionHost &host, bool *arrived, Matches matches,
                          int milliseconds = 1000) {
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(milliseconds);
  Seen newest;
  for (;;) {
    bool any = false;
    for (std::size_t i = host.datagrams(); i-- > 0;) {
      const Seen seen = decode(host.datagram(i));
      if (seen.code != CODE_JOINT_CMD || seen.length != JOINT_DATAGRAM) continue;
      if (!any) {
        newest = seen;
        any = true;
      }
      if (matches(seen)) {
        *arrived = true;
        return seen;
      }
    }
    if (std::chrono::steady_clock::now() >= deadline) break;
    std::this_thread::sleep_for(std::chrono::milliseconds(2));
  }
  *arrived = false;
  return newest;
}

/* A joint of `seen` that carries `value` in any field the command has, which is how a test names
 * the command that sent a datagram without assuming the mapping of mode to field it is checking. */
bool carries(const Seen &seen, int joint, double value) {
  const JointCmd &j = seen.command.joint_cmd[joint];
  return near(j.position, value) || near(j.velocity, value) || near(j.torque, value);
}

/* Each joint as the descriptor writes it, worked out by hand from the vendor's own parameters
 * (`lite3_control_parameters.cpp`: travel `fl_joint_lower_`/`-upper_`, `joint_vel_limit_ 30/30/20`,
 * `torque_limit_ 40/40/65`) — written here, not read back from the driver, so that an index or a
 * range slipped in the driver cannot also slip in the expectation. */
struct JointText {
  const char *name;
  const char *role;
  const char *limits;
};

constexpr JointText JOINT_TEXT[JOINTS] = {
    {"FL_HipX_joint", "left_leg.hip.abduct#0",
     "\"position_min_rad\":-0.53000,\"position_max_rad\":0.53000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"FL_HipY_joint", "left_leg.thigh.pitch#0",
     "\"position_min_rad\":-3.50000,\"position_max_rad\":0.32000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"FL_Knee_joint", "left_leg.calf.flex#0",
     "\"position_min_rad\":0.34900,\"position_max_rad\":2.80000,\"velocity_max_rad_s\":20.0,\"effort_max_nm\":65.0"},
    {"FR_HipX_joint", "right_leg.hip.abduct#0",
     "\"position_min_rad\":-0.53000,\"position_max_rad\":0.53000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"FR_HipY_joint", "right_leg.thigh.pitch#0",
     "\"position_min_rad\":-3.50000,\"position_max_rad\":0.32000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"FR_Knee_joint", "right_leg.calf.flex#0",
     "\"position_min_rad\":0.34900,\"position_max_rad\":2.80000,\"velocity_max_rad_s\":20.0,\"effort_max_nm\":65.0"},
    {"HL_HipX_joint", "left_leg.hip.abduct#1",
     "\"position_min_rad\":-0.53000,\"position_max_rad\":0.53000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"HL_HipY_joint", "left_leg.thigh.pitch#1",
     "\"position_min_rad\":-3.50000,\"position_max_rad\":0.32000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"HL_Knee_joint", "left_leg.calf.flex#1",
     "\"position_min_rad\":0.34900,\"position_max_rad\":2.80000,\"velocity_max_rad_s\":20.0,\"effort_max_nm\":65.0"},
    {"HR_HipX_joint", "right_leg.hip.abduct#1",
     "\"position_min_rad\":-0.53000,\"position_max_rad\":0.53000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"HR_HipY_joint", "right_leg.thigh.pitch#1",
     "\"position_min_rad\":-3.50000,\"position_max_rad\":0.32000,\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0"},
    {"HR_Knee_joint", "right_leg.calf.flex#1",
     "\"position_min_rad\":0.34900,\"position_max_rad\":2.80000,\"velocity_max_rad_s\":20.0,\"effort_max_nm\":65.0"},
};

std::string joint_entry(const JointText &joint) {
  return std::string("\"name\":\"") + joint.name + "\",\"role\":\"" + joint.role +
         "\",\"type\":\"revolute\",\"limits\":{" + joint.limits + "}";
}

bool joint_is_zero(const JointCmd &joint) {
  return joint.position == 0.0f && joint.velocity == 0.0f && joint.torque == 0.0f &&
         joint.kp == 0.0f && joint.kd == 0.0f;
}

/* ── building commands ────────────────────────────────────────────────── */

struct Cmd {
  float values[JOINTS] = {0};
  std::uint8_t mask[JOINTS] = {0};
  std::uint8_t modes[JOINTS] = {0};
  skr_command raw{};

  skr_command *build(std::uint8_t mode, std::uint32_t seq, bool per_joint = false,
                     std::uint16_t joints = JOINTS) {
    raw.stamp_ns = 1;
    raw.seq = seq;
    raw.mode = mode;
    raw.flags = per_joint ? SKR_CMD_PER_JOINT_MODES : 0;
    raw.joint_count = joints;
    raw.values = values;
    raw.mask = mask;
    raw.modes = per_joint ? modes : nullptr;
    return &raw;
  }
};

std::string config_for(std::uint16_t command_port, std::uint16_t state_port,
                       const std::string &extra) {
  return "{\"host\":\"127.0.0.1\",\"command_port\":" + std::to_string(command_port) +
         ",\"state_port\":" + std::to_string(state_port) + ",\"handback_ms\":0" + extra + "}";
}

}  // namespace

int main(int argc, char **argv) {
  const std::uint16_t command_port =
      static_cast<std::uint16_t>(argc > 1 ? std::atoi(argv[1]) : COMMAND_PORT);
  const std::uint16_t state_port =
      static_cast<std::uint16_t>(argc > 2 ? std::atoi(argv[2]) : STATE_PORT);

  const skr_driver *driver = skynet_rhal_entry();
  skr_ctx *ctx = nullptr;

  std::printf("\n== the descriptor, before any body ==\n");
  {
    char buffer[16384];
    const int written = driver->describe(nullptr, buffer, sizeof buffer);
    const std::string descriptor(buffer, written > 0 ? written : 0);
    check(written > 0, "an unopened driver describes its body");
    check(descriptor.find("\"base\":\"legged\"") != std::string::npos, "the base is legged");
    check(descriptor.find("\"dof\":12") != std::string::npos, "twelve degrees of freedom");
    for (int i = 0; i < JOINTS; ++i) {
      const std::string name = std::string("\"name\":\"") + JOINT_NAMES[i] + "\"";
      if (descriptor.find(name) == std::string::npos) {
        check(false, std::string("the descriptor names ") + JOINT_NAMES[i]);
      }
    }
    check(descriptor.find("\"name\":\"FL_HipX_joint\"") != std::string::npos &&
              descriptor.find("\"name\":\"HR_Knee_joint\"") != std::string::npos,
          "the joints carry Deep Robotics' own names");
    check(descriptor.find("\"role\":\"left_leg.hip.abduct#0\"") != std::string::npos,
          "the first joint's canonical role is the front-left abductor");
    check(descriptor.find("\"position_min_rad\":-0.53000,\"position_max_rad\":0.53000") !=
              std::string::npos,
          "abduction travel is the vendor's -0.530 to 0.530");
    check(descriptor.find("\"position_min_rad\":0.34900,\"position_max_rad\":2.80000") !=
              std::string::npos,
          "knee travel is the vendor's 0.349 to 2.80");
    check(descriptor.find("\"velocity_max_rad_s\":30.0,\"effort_max_nm\":40.0") !=
              std::string::npos,
          "a hip joint carries both of the vendor's ratings");
    check(descriptor.find("\"velocity_max_rad_s\":20.0,\"effort_max_nm\":65.0") !=
              std::string::npos,
          "a knee carries its own pair, not a hip's");
    check(descriptor.find("\"joint_velocity_max_rad_s\":30.0") != std::string::npos &&
              descriptor.find("\"joint_torque_max_nm\":65.0") != std::string::npos,
          "the envelope declares both ceilings, which is what joint_ratings needs");
    check(descriptor.find("\"watchdog_ms\":200.0") != std::string::npos,
          "the watchdog window is a fifth of the vendor's reclaim");
    check(descriptor.find("Lite3_rl_deploy") != std::string::npos &&
              descriptor.find("\"basis\":\"declared\"") != std::string::npos,
          "the envelope cites the vendor file it was transcribed from, as a declaration");
    check(descriptor.find(
              "\"capabilities\":[\"safety.estop\",\"platform.onboard_compute\"]") !=
              std::string::npos,
          "a software stop and the robot's own computer are claimed, and nothing else");
    check(descriptor.find("\"estop\":true") == std::string::npos,
          "no hardware stop is claimed: there is no circuit here, only a datagram");
    check(descriptor.find("\"extensions\":[\"vendor.lite3.telemetry\"]") != std::string::npos,
          "the telemetry token is declared as a vendor extension");
    check(descriptor.find("\"per_joint_modes\":true") != std::string::npos,
          "per-joint modes are declared, and this wire is per joint");
    check(descriptor.find("\"rate_hz_max\":null") != std::string::npos,
          "no control rate is claimed: the vendor publishes none");
    bool entries_right = true;
    for (const JointText &joint : JOINT_TEXT) {
      if (descriptor.find(joint_entry(joint)) == std::string::npos) {
        entries_right = false;
        std::printf("        (no entry for %s)\n", joint.name);
      }
    }
    check(entries_right,
          "every joint's entry — name, role, travel in both directions, and the speed and torque "
          "ratings of its own position in the leg — is the vendor's");
  }

  std::printf("\n== refusals that need no body ==\n");
  check(driver->open("{\"model\":\"x20\"}", &ctx) == SKR_EINVAL,
        "the X20 is refused by name: same vendor, no published SDK");
  check(driver->open("{\"host\":\"not-an-address\"}", &ctx) == SKR_EINVAL,
        "a host that is not an IPv4 address is refused");
  check(driver->open(config_for(command_port, state_port, ",\"hold_kd\":2.0").c_str(), &ctx) ==
            SKR_EINVAL,
        "`hold_kd` is refused by name: it chose a slack or a damped leg for a masked joint, and "
        "§24's hold is neither (F121)");
  check(driver->open(config_for(command_port, state_port,
                                ",\"reclaim_guard_ms\":0,\"open_timeout_ms\":200").c_str(),
                     &ctx) == SKR_EINVAL,
        "a reclaim guard of zero is refused: control right would be re-taken on every command");
  check(driver->open(config_for(command_port, state_port,
                                ",\"reclaim_guard_ms\":-5,\"open_timeout_ms\":200").c_str(),
                     &ctx) == SKR_EINVAL,
        "and so is a negative one");
  {
    skr_state state{};
    float position[JOINTS] = {0}, velocity[JOINTS] = {0}, effort[JOINTS] = {0};
    state.joint_count = JOINTS;
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    check(driver->read_state(nullptr, &state, 0) == SKR_EPERM,
          "reading state before open is refused");
    Cmd cmd;
    check(driver->write_command(nullptr, cmd.build(SKR_MODE_TORQUE, 1), 0) == SKR_EPERM,
          "commanding before open is refused");
  }

  std::printf("\n== open against silence ==\n");
  {
    MotionHost quiet(command_port, state_port, 200.0);
    if (!quiet.start(/*push=*/false)) {
      std::fprintf(stderr, "selftest: cannot bind UDP %u\n", static_cast<unsigned>(command_port));
      return 2;
    }
    const std::string config = config_for(command_port, state_port, ",\"open_timeout_ms\":300");
    check(driver->open(config.c_str(), &ctx) == SKR_ETIMEDOUT,
          "a body that never pushes state is a failed open, not a silent one");
    quiet.stop();
  }

  MotionHost host(command_port, state_port, 200.0);
  if (!host.start(/*push=*/true)) {
    std::fprintf(stderr, "selftest: cannot bind UDP %u\n", static_cast<unsigned>(command_port));
    return 2;
  }
  for (int i = 0; i < JOINTS; ++i) {
    host.set_joint(i, 0.1f * static_cast<float>(i + 1), 0.01f * static_cast<float>(i + 1),
                   0.5f * static_cast<float>(i + 1), 31.0f + static_cast<float>(i));
  }
  host.set_imu(1.0f, 2.0f, 3.0f, 90.0f, -90.0f, 180.0f);

  std::printf("\n== open, and what the body says about itself ==\n");
  const std::string config =
      config_for(command_port, state_port, ",\"kp\":60.0,\"kd\":5.0,\"reclaim_guard_ms\":80");
  check(driver->open(config.c_str(), &ctx) == SKR_OK, "opens once state is arriving");
  check(host.datagrams() == 0, "open commands nothing: control right is not taken here");

  {
    skr_state state{};
    float position[JOINTS] = {0}, velocity[JOINTS] = {0}, effort[JOINTS] = {0};
    state.joint_count = JOINTS;
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    check(driver->read_state(ctx, &state, 0) == SKR_OK, "reads a state");
    check(state.joint_count == JOINTS, "reports twelve joints");
    check(near(position[0], 0.1) && near(position[11], 1.2), "positions are the body's");
    check(near(velocity[0], 0.01) && near(velocity[11], 0.12), "velocities are the body's");
    check(near(effort[0], 0.5) && near(effort[11], 6.0), "efforts are the body's");
    check((state.flags & SKR_HOMING_MASK) == SKR_HOMING_NOT_REQUIRED,
          "homing is not_required: the vendor's own idle check reads absolute positions");
    check(state.fault_code == 0, "no fault word is invented: this wire carries none");
    for (int i = 0; i < 7; ++i) {
      if (!std::isnan(state.base_pose[i])) check(false, "the base pose is all NaN");
    }
    check(std::isnan(state.base_pose[0]) && std::isnan(state.base_pose[6]),
          "the base pose is all NaN: nothing localises this body");
    check(std::isnan(state.base_twist[0]) && std::isnan(state.base_twist[1]) &&
              std::isnan(state.base_twist[2]),
          "the twist's linear half is NaN, not zero: nothing measures it");
    check(near(state.base_twist[3], 90.0 * 3.14159265358979323846 / 180.0, 1e-4) &&
              near(state.base_twist[5], 3.14159265358979323846, 1e-4),
          "the twist's angular half is the IMU's rates, converted out of the wire's degrees");

    const std::uint32_t first_seq = state.seq;
    const std::uint64_t first_stamp = state.stamp_ns;
    skr_state again = state;
    const std::uint64_t again_from = raw_ns();
    check(driver->read_state(ctx, &again, 0) == SKR_OK, "reads again");
    check(fresh_or_waited(first_seq, again.seq, raw_ns() - again_from),
          "a second read waits for a new push rather than repeating the last one — the body has "
          "a clock of its own and a repeated seq means a stalled producer");
    check((again.seq > first_seq) == (again.stamp_ns > first_stamp),
          "the stamp is the arrival's: it advances with the arrivals, and only with them");

    /* And the other half of the same rule: when the body really has stopped, the sequence holds
     * rather than being advanced to look alive. The driver counts the reads that waited a whole
     * bound for nothing; the two below are the ones this section makes certain of, so it is the
     * count's *change* that is held, not its value for the whole run, which a late push earlier
     * on a loaded machine would move. */
    auto stale_reads_now = [&]() -> long long {
      char reply[4096];
      const int written = driver->extension(ctx, "vendor.lite3.telemetry", "", 0, reply, sizeof reply);
      const std::string text(reply, written > 0 ? written : 0);
      const std::size_t at = text.find("\"stale_reads\":");
      return at == std::string::npos ? -1 : std::strtoll(text.c_str() + at + 14, nullptr, 10);
    };
    host.pause_pushes();
    std::this_thread::sleep_for(std::chrono::milliseconds(150));
    /* A push that was already on its way when the body was paused lands before this read, and the
     * read that takes it is fresh and not stale. One read here consumes it (or waits a bound for
     * nothing, if there was none) so that the two reads counted below are certain to be the stale
     * ones: the count is held across *them*. */
    {
      skr_state settle = state;
      driver->read_state(ctx, &settle, 0);
    }
    const long long stale_before = stale_reads_now();
    skr_state stalled = state;
    const std::uint64_t stalled_from = raw_ns();
    check(driver->read_state(ctx, &stalled, 0) == SKR_OK,
          "a read with nothing arriving still returns the last sample");
    check(raw_ns() - stalled_from < 400ull * 1000000ull,
          "and returns inside a few hundred milliseconds: the wait for a push is bounded by the "
          "interval measured, inside 1 to 100 ms, so a stalled body is reported and not waited on "
          "by the loop that is reading it");
    check(raw_ns() - stalled.stamp_ns >= 100ull * 1000000ull,
          "the sample's stamp is the instant its datagram arrived, well over a hundred "
          "milliseconds ago, and not the instant the read returned (§24, §35.1)");
    const std::uint32_t held = stalled.seq;
    skr_state stalled_again = state;
    check(driver->read_state(ctx, &stalled_again, 0) == SKR_OK, "and again");
    check(stalled_again.seq == held,
          "seq counts arrivals, not reads: a stopped body reads as stopped");
    check(stale_before >= 0 && stale_reads_now() - stale_before == 2,
          "and the two reads that waited a whole bound and got nothing, from the paused body, are "
          "counted as two — a count, where the ABI has only a repeated seq");
    host.resume_pushes();
    host.wait_for_pushes(2);
    skr_state recovered = state;
    check(driver->read_state(ctx, &recovered, 0) == SKR_OK, "reads once the body resumes");
    /* Recovery is "eventually", not "on the first read": the first push after a long stall is
     * followed by a bound the driver sized from the stall, and a read inside it can still be the
     * previous sample on a loaded machine. A driver that never advances `seq` again stays stuck for
     * the whole second and fails here. */
    {
      const auto give_up = std::chrono::steady_clock::now() + std::chrono::seconds(1);
      while (recovered.seq <= held && std::chrono::steady_clock::now() < give_up) {
        driver->read_state(ctx, &recovered, 0);
      }
    }
    check(recovered.seq > held, "and the sequence moves again");
  }

  std::printf("\n== a position command, and the bytes it becomes ==\n");
  {
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = (i % 3 == 2) ? 1.0f : 0.1f;  /* knees live in 0.349..2.80 */
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "a position command with gains configured is accepted");
    check(host.wait_for_datagrams(2), "two datagrams went out, not one");
    const Seen control = decode(host.datagram(0));
    check(control.code == CODE_CONTROL_SDK && control.length == CONTROL_DATAGRAM,
          "the first is a twelve-byte 0x0114: control right, taken with the command it is for");
    check(control.type == TYPE_SINGLE_VALUE && control.size == 0,
          "the control command carries the vendor's single-value header");
    const Seen joint = decode(host.datagram(1));
    check(joint.code == CODE_JOINT_CMD && joint.length == JOINT_DATAGRAM,
          "the second is a 252-byte 0x0111");
    check(joint.type == TYPE_MESS_VALUES && joint.size == sizeof(RobotCmd),
          "its header declares 240 bytes of parameters, as the vendor's Sender does");
    check(control.count == 0 && joint.count == 0,
          "and neither datagram is numbered: the count is the vendor's sequence field, and "
          "nothing the vendor sends ever sets it");
    check(near(joint.command.joint_cmd[0].position, 0.1) &&
              near(joint.command.joint_cmd[2].position, 1.0),
          "the setpoints landed in `position`");
    check(near(joint.command.joint_cmd[0].kp, 60.0) && near(joint.command.joint_cmd[0].kd, 5.0),
          "the operator's gains are on the joint, and nothing else chose them");
    check(joint.command.joint_cmd[0].velocity == 0.0f && joint.command.joint_cmd[0].torque == 0.0f,
          "a position command says nothing about velocity or feed-forward torque");
  }

  std::printf("\n== velocity and torque ==\n");
  {
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 0.25f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_VELOCITY, 2), 0) == SKR_OK,
          "a velocity command with kd configured is accepted");
    bool arrived = false;
    const Seen seen = newest_joint_command(
        host, &arrived, [](const Seen &s) { return carries(s, 3, 0.25); });
    check(arrived, "it reached the wire");
    check(near(seen.command.joint_cmd[3].velocity, 0.25), "the setpoint landed in `velocity`");
    check(seen.command.joint_cmd[3].kp == 0.0f && near(seen.command.joint_cmd[3].kd, 5.0),
          "velocity carries damping and no stiffness: the vendor's own example shape");
    check(seen.command.joint_cmd[3].position == 0.0f,
          "a velocity command writes no position, which with kp=0 commands nothing anyway");

    host.forget();
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 3.0f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 3), 0) == SKR_OK,
          "a torque command is accepted");
    const Seen torque = newest_joint_command(
        host, &arrived, [](const Seen &s) { return carries(s, 5, 3.0); });
    check(arrived, "it reached the wire");
    check(near(torque.command.joint_cmd[5].torque, 3.0), "the value landed in `torque`");
    check(torque.command.joint_cmd[5].kp == 0.0f && torque.command.joint_cmd[5].kd == 0.0f,
          "a torque command carries no gains: it needs none, and a gain would be a second law");
  }

  /* §24: "0 = hold this joint at its current setpoint". The last command above was torque, which
   * leaves no position setpoint, so the first hold latches where each joint is measured to be —
   * the motion host pushes 0.1·(i+1). Until 2026-09-24 this section asserted every field zero, a
   * slack leg, which is the reading Unitree's simulator showed folding a standing Go2
   * (DRY-COVERAGE-PLAN.md F121). */
  std::printf("\n== the mask: a masked joint is held (§24) ==\n");
  {
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = (i == 0) ? 1 : 0;
      cmd.values[i] = (i % 3 == 2) ? 1.0f : 0.1f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 4), 0) == SKR_OK,
          "a partly-masked command is accepted");
    bool arrived = false;
    const Seen seen = newest_joint_command(
        host, &arrived, [](const Seen &s) { return carries(s, 0, 0.1); });
    check(arrived, "it reached the wire");
    check(near(seen.command.joint_cmd[0].position, 0.1) && near(seen.command.joint_cmd[0].kp, 60.0),
          "the applied joint is commanded");
    check(near(seen.command.joint_cmd[1].position, 0.2) &&
              near(seen.command.joint_cmd[11].position, 1.2),
          "a masked joint with no position setpoint is held where it was measured");
    check(near(seen.command.joint_cmd[1].kp, 60.0) && near(seen.command.joint_cmd[1].kd, 5.0) &&
              seen.command.joint_cmd[1].velocity == 0.0f &&
              seen.command.joint_cmd[1].torque == 0.0f,
          "with the operator's `kp` and `kd`, a zero velocity setpoint and no feed-forward — not "
          "every field zero, which the drive law makes a slack leg");

    /* The body moves under the hold; the target must not follow it. */
    host.set_joint(1, 0.45f, 0.0f, 0.0f, 32.0f);
    host.wait_for_pushes(3);
    skr_state state{};
    float position[JOINTS] = {0}, velocity[JOINTS] = {0}, effort[JOINTS] = {0};
    state.joint_count = JOINTS;
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    driver->read_state(ctx, &state, 0);
    host.forget();
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 5), 0) == SKR_OK,
          "the same command again is accepted");
    const Seen again = newest_joint_command(
        host, &arrived, [](const Seen &s) { return carries(s, 0, 0.1); });
    check(arrived, "it reached the wire");
    check(near(again.command.joint_cmd[1].position, 0.2),
          "the hold target is latched, not re-read: a target that followed the measurement every "
          "cycle would zero the stiffness term and hold nothing");
    host.set_joint(1, 0.2f, 0.02f, 1.0f, 32.0f);

    host.forget();
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = (i % 3 == 2) ? 1.5f : 0.3f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 6), 0) == SKR_OK,
          "a position command on every joint is accepted");
    host.forget();
    float hold_frame[JOINTS] = {0};
    Cmd all_held;
    for (int i = 0; i < JOINTS; ++i) {
      all_held.mask[i] = 0;
      all_held.values[i] = hold_frame[i];
    }
    check(driver->write_command(ctx, all_held.build(SKR_MODE_POSITION, 7), 0) == SKR_OK,
          "the kernel's hold frame — every joint masked, values irrelevant — is accepted");
    /* Any joint-command datagram: the hold frame's setpoints are the previous command's, so
     * nothing in it names the command that sent it, and what is being checked is that it goes out. */
    const Seen held = newest_joint_command(host, &arrived, [](const Seen &) { return true; });
    check(arrived,
          "and it still goes out: falling silent would let the robot reclaim the legs, which is "
          "a different thing from being told to hold them");
    bool at_setpoint = true;
    for (int i = 0; i < JOINTS; ++i) {
      const float want = (i % 3 == 2) ? 1.5f : 0.3f;
      if (!near(held.command.joint_cmd[i].position, want) ||
          !near(held.command.joint_cmd[i].kp, 60.0)) {
        at_setpoint = false;
      }
    }
    check(at_setpoint,
          "and after a position command every joint is held at that setpoint — §24's 'current "
          "setpoint' — rather than at the zeros the hold frame carries");
  }

  std::printf("\n== per-joint modes (§86.1) ==\n");
  {
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 0.2f;
      cmd.modes[i] = SKR_JMODE_INHERIT;
    }
    cmd.modes[4] = SKR_JMODE_TORQUE;
    cmd.values[4] = 2.0f;
    cmd.modes[7] = SKR_JMODE_VELOCITY;
    cmd.values[7] = 0.75f;
    cmd.values[2] = cmd.values[5] = cmd.values[8] = cmd.values[11] = 1.0f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 6, /*per_joint=*/true), 0) ==
              SKR_OK,
          "a mixed-mode command is accepted");
    bool arrived = false;
    const Seen seen = newest_joint_command(
        host, &arrived, [](const Seen &s) { return carries(s, 4, 2.0) || carries(s, 7, 0.75); });
    check(arrived, "it reached the wire");
    check(near(seen.command.joint_cmd[0].position, 0.2) && near(seen.command.joint_cmd[0].kp, 60.0),
          "an inheriting joint took the command's own mode");
    check(near(seen.command.joint_cmd[4].torque, 2.0) && seen.command.joint_cmd[4].kp == 0.0f,
          "the torque joint is torque, with no stiffness beside it");
    check(near(seen.command.joint_cmd[7].velocity, 0.75) && seen.command.joint_cmd[7].kp == 0.0f,
          "the velocity joint is velocity");

    host.forget();
    cmd.modes[4] = SKR_MODE_TORQUE;  /* a bare mode written into the per-joint array */
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 7, /*per_joint=*/true), 0) ==
              SKR_EINVAL,
          "a bare SKR_MODE_* in the per-joint array is refused, not guessed at");
    check(host.datagrams() == 0, "and nothing reached the wire");
  }

  std::printf("\n== refusals that protect a body ==\n");
  {
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = (i % 3 == 2) ? 1.0f : 0.1f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 8, false, JOINTS - 1), 0) ==
              SKR_EINVAL,
          "a command for eleven joints is refused: this body has twelve");
    check(driver->write_command(ctx, cmd.build(9, 9), 0) == SKR_EINVAL,
          "a mode the ABI does not define is refused");
    cmd.values[2] = 3.5f;  /* past the knee's 2.80 */
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 10), 0) == SKR_EINVAL,
          "a setpoint outside the published travel is refused");
    check(host.datagrams() == 0, "nothing reached the wire");
  }

  std::printf("\n== control right expires, and a latch cannot track it ==\n");
  {
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 1.0f;
    }
    /* "A command inside the window" is a claim about the gap since the driver's last send, and a
     * suite thread that is not scheduled for 80 ms on a loaded machine makes the gap longer than
     * the window — then control right *is* re-taken, correctly, and the claim is not tested. So the
     * gap is measured (it is bounded by the clock around the two writes), a gap too long to be
     * "inside" is not counted against the driver, and the attempt is repeated; a driver that
     * re-takes control on every command fails every attempt. */
    constexpr std::uint64_t INSIDE_NS = 40ull * 1000000ull;  /* half the 80 ms guard */
    bool inside_held = false;
    bool conclusive = false;
    for (int attempt = 0; attempt < 8 && !conclusive; ++attempt) {
      host.forget();
      check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 10 + attempt), 0) == SKR_OK,
            "a command that primes the window is accepted");
      const std::uint64_t primed = raw_ns();
      /* The joint command is the last datagram of any write, so once it is here everything the
       * priming write sent is, and `forget()` below cannot be overtaken by one of its datagrams. */
      bool primed_seen = false;
      newest_joint_command(host, &primed_seen, [](const Seen &) { return true; });
      check(primed_seen, "and it reached the wire");
      host.forget();
      check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 11 + attempt), 0) == SKR_OK,
            "a command while control is held goes straight out");
      const std::uint64_t gap = raw_ns() - primed;
      /* Wait for the datagram, then for long enough that a second one (the joint command behind a
       * control-right re-take) would have arrived: counting at the first arrival can see one. */
      host.wait_for_datagrams(1);
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
      if (gap < INSIDE_NS) {
        conclusive = true;
        inside_held = host.datagrams() == 1;
      }
    }
    check(conclusive, "the gap between two commands was measured inside the window at least once");
    check(inside_held,
          "one datagram: control right was not re-taken for a command inside the window");
    std::this_thread::sleep_for(std::chrono::milliseconds(140));  /* past reclaim_guard_ms=80 */
    host.forget();
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 12), 0) == SKR_OK,
          "a command after a quiet spell is accepted");
    check(host.wait_for_datagrams(2), "two datagrams went out");
    check(decode(host.datagram(0)).code == CODE_CONTROL_SDK,
          "a command after the reclaim window takes control right back first — the robot has "
          "had the legs since one second of silence, and a latch would not know");
  }

  std::printf("\n== the telemetry the RHAL's State has no field for ==\n");
  {
    char reply[4096];
    check(driver->extension(ctx, "vendor.nope", "", 0, reply, sizeof reply) == SKR_ENOSYS,
          "a token this body never declared is refused");
    const int written =
        driver->extension(ctx, "vendor.lite3.telemetry", "", 0, reply, sizeof reply);
    check(written > 0, "the declared token answers");
    const std::string telemetry(reply, written > 0 ? written : 0);
    check(telemetry.find("\"unit_angle\":\"degree\"") != std::string::npos,
          "the IMU is reported in the unit the wire carries, and says so");
    check(telemetry.find("\"rate_yaw\":180.000000") != std::string::npos,
          "the IMU rates are the body's");
    check(telemetry.find("\"joint_temperature_c\":[31.000") != std::string::npos,
          "joint temperatures are surfaced, which §24's State has nowhere for");
    check(telemetry.find("\"contact_force_n\":[0.000000") != std::string::npos,
          "so are the foot forces");
    check(telemetry.find("\"control_retakes\":1") != std::string::npos,
          "and the session's own control-right retakes, counted rather than implied");
    check(telemetry.find("\"stale_reads\":") != std::string::npos,
          "and the reads that waited a whole bound and got nothing are reported (the two from "
          "the paused body are held where they happen)");
    check(telemetry.find("\"push_interval_ms\":") != std::string::npos,
          "and the push interval this driver measured, since the vendor declares none");
    check(driver->extension(ctx, "vendor.lite3.telemetry", "", 0, reply, 8) == SKR_EINVAL,
          "a reply buffer too small to hold the answer is refused, never truncated");
  }

  std::printf("\n== control right after the state went quiet while the commands went on (F276) ==\n");
  {
    /* The robot takes the legs back after a second without a command *received*. A link that drops for longer than that loses the commands as well as the state, and the driver, whose own
     * sends never stop, is never quiet: its re-take was a function of its own silence, so it was never made, the robot ignored every datagram after the link came back, and every write was
     * OK (the contract's sixth question read 0 commands acted on in the 1.5 s after a 1.5 s stall). The state stream going quiet for longer than the guard is the evidence the driver
     * has that the link was down. */
    Cmd cmd;
    host.forget();
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 30), 0) == SKR_OK,
          "a command while control is held, or taken");
    host.wait_for_datagrams(1);
    host.pause_pushes();
    for (std::uint32_t seq = 31; seq < 41; ++seq) {
      check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, seq), 0) == SKR_OK,
            "a command while the state is quiet is accepted: the driver is not quiet itself");
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    host.resume_pushes();
    host.wait_for_pushes(5);
    host.forget();
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 41), 0) == SKR_OK,
          "the first command after the state came back is accepted");
    check(host.wait_for_datagrams(2), "and two datagrams went out");
    check(decode(host.datagram(0)).code == CODE_CONTROL_SDK,
          "control right is taken again first: a state that was silent for longer than the guard "
          "means the link was down, whatever the driver sent meanwhile");
    host.forget();
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 42), 0) == SKR_OK,
          "and the next command is accepted");
    host.wait_for_datagrams(1);
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    check(host.datagrams() == 1,
          "and it goes out alone: the re-take is made once for the silence and not on every "
          "command after it");
  }

  std::printf("\n== the stop, and what it is not ==\n");
  {
    host.forget();
    check(driver->emergency_stop(ctx) == SKR_OK, "the stop reports what it did");
    check(host.wait_for_datagrams(2), "two datagrams went out");
    const Seen damping = decode(host.datagram(0));
    check(damping.code == CODE_JOINT_CMD, "the first is a joint command");
    bool damped = true;
    for (int i = 0; i < JOINTS; ++i) {
      const JointCmd &joint = damping.command.joint_cmd[i];
      if (!(joint.position == 0.0f && joint.velocity == 0.0f && joint.torque == 0.0f &&
            joint.kp == 0.0f && near(joint.kd, VENDOR_DAMPING_KD))) {
        damped = false;
      }
    }
    check(damped, "and it is the vendor's own damping shape: kd alone, on all twelve");
    check(decode(host.datagram(1)).code == CODE_CONTROL_ROBOT,
          "the second hands control right back to the robot's own controller");

    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.mask[i] = 1;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 13), 0) == SKR_EPERM,
          "the latch refuses every later command");
    skr_state state{};
    float position[JOINTS] = {0}, velocity[JOINTS] = {0}, effort[JOINTS] = {0};
    state.joint_count = JOINTS;
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    check(driver->read_state(ctx, &state, 0) == SKR_OK, "reading state still works after a stop");
    check((state.flags & SKR_FLAG_ESTOP) != 0, "and the state says the stop is asserted");
  }

  std::printf("\n== close, and the session after it ==\n");
  {
    host.forget();
    check(driver->close(ctx) == SKR_OK, "closes");
    check(host.datagrams() == 0,
          "a stopped session hands nothing back on close: the damping and the hand-back already "
          "went out, and repeating them would be commanding a robot the operator stopped");
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a new session opens");
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 0.5f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 1), 0) == SKR_OK,
          "the estop latch does not carry into it");
    /* Both of that command's datagrams — the control-right take and the command itself — before
     * forgetting, or one of them arrives after the clear and is read as the close's. */
    check(host.wait_for_datagrams(2), "a fresh session takes control right again");
    host.forget();
    check(driver->close(ctx) == SKR_OK, "closes the second session");
    check(host.wait_for_datagrams(2), "and this one hands back");
    check(decode(host.datagram(0)).code == CODE_JOINT_CMD &&
              near(decode(host.datagram(0)).command.joint_cmd[0].kd, VENDOR_DAMPING_KD),
          "damping first, the vendor's order");
    check(decode(host.datagram(1)).code == CODE_CONTROL_ROBOT, "then control right");
  }

  /* One driver object lives for the process, so everything a session set is still there when the
   * next one opens unless `open()` clears it. A hold latched in a session, or a position setpoint
   * remembered, would send the next session's first masked joint to a pose from before. */
  /* Datagrams a driver sent a moment ago are still arriving on the host's socket: forgetting the
   * list while some are in flight would hand the next check the last section's. */
  auto settle = [&host] {
    std::this_thread::sleep_for(std::chrono::milliseconds(60));
    host.forget();
  };

  std::printf("\n== what a session leaves behind ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    Cmd set;
    for (int i = 0; i < JOINTS; ++i) {
      set.mask[i] = 1;
      set.values[i] = (i % 3 == 0) ? 0.4f : (i % 3 == 1) ? -1.0f : 2.0f;
    }
    check(driver->write_command(ctx, set.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "it takes a position command, which leaves a setpoint for every joint");
    set.mask[1] = 0;
    check(driver->write_command(ctx, set.build(SKR_MODE_POSITION, 2), 0) == SKR_OK,
          "and one that masks the second joint, which latches a hold at that setpoint");
    check(driver->close(ctx) == SKR_OK, "the session closes");

    host.set_joint(1, 0.25f, 0.0f, 0.0f, 30.0f);
    host.wait_for_pushes(3);
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a new session opens");
    settle();
    Cmd first;
    for (int i = 0; i < JOINTS; ++i) {
      first.mask[i] = i == 1 ? 0 : 1;
      first.values[i] = (i % 3 == 0) ? 0.1f : (i % 3 == 1) ? -0.5f : 1.0f;
    }
    check(driver->write_command(ctx, first.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "its first command masks the second joint");
    check(host.wait_for_datagrams(2), "it reached the wire");
    check(near(decode(host.datagram(host.datagrams() - 1)).command.joint_cmd[1].position, 0.25),
          "and that joint is held where the robot is measured to be, not at the setpoint, or the "
          "hold, the last session left");
    host.set_joint(1, 0.2f, 0.02f, 1.0f, 32.0f);
    driver->close(ctx);
  }

  std::printf("\n== releasing the latch without closing the session (§48.2) ==\n");
  {
    /* A reclaim guard far longer than this section, so that the control right taken again after
     * the release is the release's doing and not a quiet spell's. */
    const std::string long_guard = config_for(
        command_port, state_port, ",\"kp\":60.0,\"kd\":5.0,\"reclaim_guard_ms\":5000");
    check(driver->open(long_guard.c_str(), &ctx) == SKR_OK, "a session opens");
    settle();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 0.5f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 1), 0) == SKR_OK,
          "it takes a command, and with it control right");
    check(host.wait_for_datagrams(2), "which reached the wire as two datagrams");
    settle();
    check(driver->emergency_stop(ctx) == SKR_OK, "the driver's own stop is sent");
    check(host.wait_for_datagrams(2), "damping, then the hand-back");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 2), 0) == SKR_EPERM,
          "and the latch refuses what follows");
    check(driver->release_stop(ctx) == SKR_OK,
          "a power-on self-test releases the latch without closing the session");
    skr_state state{};
    float position[JOINTS] = {0}, velocity[JOINTS] = {0}, effort[JOINTS] = {0};
    state.joint_count = JOINTS;
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    check(driver->read_state(ctx, &state, 0) == SKR_OK, "state reads");
    check((state.flags & SKR_FLAG_ESTOP) == 0, "and no longer carries the stop flag");
    settle();
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 3), 0) == SKR_OK,
          "a command is accepted again");
    check(host.wait_for_datagrams(2), "and two datagrams went out");
    check(decode(host.datagram(0)).code == CODE_CONTROL_SDK,
          "the first takes control right back: the stop handed the legs to the robot's own "
          "controller, and a command sent without taking them again is accepted by the socket and "
          "ignored by the robot");
    driver->close(ctx);
  }

  std::printf("\n== datagrams on the state port that are not a state push ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    auto counter = [&](const char *key) -> std::uint64_t {
      char reply[4096];
      const int written =
          driver->extension(ctx, "vendor.lite3.telemetry", "", 0, reply, sizeof reply);
      const std::string telemetry(reply, written > 0 ? written : 0);
      const std::string needle = std::string("\"") + key + "\":";
      const std::size_t at = telemetry.find(needle);
      return at == std::string::npos
                 ? ~0ull
                 : std::strtoull(telemetry.c_str() + at + needle.size(), nullptr, 10);
    };
    host.pause_pushes();
    std::this_thread::sleep_for(std::chrono::milliseconds(60));
    const std::uint64_t arrivals = counter("arrivals");
    const std::uint64_t dropped = counter("dropped");
    unsigned char datagram[STATE_DATAGRAM] = {0};
    EthCommand header{};
    header.code = CODE_STATE_PUSH + 1;
    header.value_or_size = static_cast<std::uint32_t>(sizeof(RobotData));
    header.type_and_count = pack_type_count(TYPE_MESS_VALUES, 0);
    std::memcpy(datagram, &header, sizeof header);
    host.inject(datagram, sizeof datagram);
    header.code = CODE_STATE_PUSH;
    std::memcpy(datagram, &header, sizeof header);
    host.inject(datagram, STATE_DATAGRAM - 8);
    std::this_thread::sleep_for(std::chrono::milliseconds(60));
    check(counter("dropped") == dropped + 2,
          "a push with another code, and a push too short to hold a whole RobotData, are both "
          "counted as dropped");
    check(counter("arrivals") == arrivals,
          "and neither is counted as an arrival: the vendor's own receiver reads a short push "
          "as the tail of whatever came before it, which is the defect this driver exists not to "
          "repeat");
    host.resume_pushes();
    driver->close(ctx);
  }

  std::printf("\n== a session with no gains has nothing to hold with ==\n");
  {
    check(driver->open(config_for(command_port, state_port, "").c_str(), &ctx) == SKR_OK,
          "a session with no gains opens");
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 0.5f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 1), 0) == SKR_OK,
          "a torque command on every joint needs no gain and is accepted");
    check(host.wait_for_datagrams(2), "it reached the wire");
    host.forget();
    Cmd in_range;
    for (int i = 0; i < JOINTS; ++i) {
      in_range.mask[i] = 1;
      in_range.values[i] = (i % 3 == 2) ? 1.0f : 0.1f;
    }
    check(driver->write_command(ctx, in_range.build(SKR_MODE_POSITION, 4), 0) == SKR_EPERM,
          "a position command on every joint is refused with no gains: a Lite3 drive takes an "
          "impedance command, and a stiffness is the operator's to choose (§63.4)");
    check(driver->write_command(ctx, in_range.build(SKR_MODE_VELOCITY, 5), 0) == SKR_EPERM,
          "and so is a velocity command, which needs `kd`: a velocity setpoint with no damping "
          "behind it commands nothing at all");
    cmd.mask[3] = 0;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 2), 0) == SKR_EPERM,
          "but a torque command that masks a joint is refused: that joint is to be held, a hold "
          "is a stiffness, and a slack leg is the reading §24 rules out");
    for (int i = 0; i < JOINTS; ++i) cmd.mask[i] = 0;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 3), 0) == SKR_EPERM,
          "and so is the kernel's hold frame, loudly, rather than answered with a free body");
    check(host.datagrams() == 0, "nothing reached the wire");
    driver->close(ctx);
  }

  std::printf("\n== a body that pushes slowly, and then stops ==\n");
  {
    host.set_rate(4.0);
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens against a slow body");
    host.wait_for_pushes(3);
    skr_state state{};
    float position[JOINTS] = {0}, velocity[JOINTS] = {0}, effort[JOINTS] = {0};
    state.joint_count = JOINTS;
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    check(driver->read_state(ctx, &state, 0) == SKR_OK, "it reads once the push interval is known");
    host.pause_pushes();
    std::this_thread::sleep_for(std::chrono::milliseconds(300));
    const std::uint64_t from = raw_ns();
    check(driver->read_state(ctx, &state, 0) == SKR_OK, "and a read after it stops returns");
    check(raw_ns() - from < 250ull * 1000000ull,
          "inside a quarter of a second: the wait for a push is twice the interval measured, but "
          "never more than a tenth of a second, so a stalled slow body is reported rather than "
          "waited on by the loop that is reading it");
    host.resume_pushes();
    host.set_rate(200.0);
    driver->close(ctx);
  }

  std::printf("\n== a motion host that has gone ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    host.stop_listening();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 0.5f;
    }
    /* A connected UDP socket learns the port is closed from the datagram after the one that found
     * out, so the failure shows within a few commands and not on the first. */
    bool reported = false;
    for (int n = 0; n < 6 && !reported; ++n) {
      if (driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 1 + n), 0) == SKR_EIO) {
        reported = true;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    check(reported,
          "a datagram the closed command port refuses is reported as a failure and not as sent: "
          "a driver that said OK would be commanding a robot that is no longer there");
    driver->close(ctx);
  }

  host.stop();
  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
