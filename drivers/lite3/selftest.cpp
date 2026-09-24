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
#include <cstring>
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
      std::this_thread::sleep_for(std::chrono::duration<double>(1.0 / hz_));
    }
  }

  std::uint16_t command_port_;
  std::uint16_t state_port_;
  double hz_;
  int listen_fd_ = -1;
  int push_fd_ = -1;
  sockaddr_in remote_{};
  std::atomic<bool> running_{false};
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
  if (bytes.size() >= JOINT_DATAGRAM) {
    std::memcpy(&seen.command, bytes.data() + sizeof(EthCommand), sizeof seen.command);
  }
  return seen;
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
    check(driver->read_state(ctx, &again, 0) == SKR_OK, "reads again");
    check(again.seq > first_seq,
          "a second read waits for a new push rather than repeating the last one — the body has "
          "a clock of its own and a repeated seq means a stalled producer");
    check(again.stamp_ns > first_stamp, "the stamp is the arrival's, and it advances with them");

    /* And the other half of the same rule: when the body really has stopped, the sequence holds
     * rather than being advanced to look alive. */
    host.pause_pushes();
    skr_state stalled = state;
    check(driver->read_state(ctx, &stalled, 0) == SKR_OK,
          "a read with nothing arriving still returns the last sample");
    const std::uint32_t held = stalled.seq;
    skr_state stalled_again = state;
    check(driver->read_state(ctx, &stalled_again, 0) == SKR_OK, "and again");
    check(stalled_again.seq == held,
          "seq counts arrivals, not reads: a stopped body reads as stopped");
    host.resume_pushes();
    host.wait_for_pushes(2);
    skr_state recovered = state;
    check(driver->read_state(ctx, &recovered, 0) == SKR_OK, "reads once the body resumes");
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
    check(host.wait_for_datagrams(1), "it reached the wire");
    const Seen seen = decode(host.datagram(host.datagrams() - 1));
    check(near(seen.command.joint_cmd[3].velocity, 0.25), "the setpoint landed in `velocity`");
    check(seen.command.joint_cmd[3].kp == 0.0f && near(seen.command.joint_cmd[3].kd, 5.0),
          "velocity carries damping and no stiffness: the vendor's own example shape");
    check(seen.command.joint_cmd[3].position == 0.0f,
          "a velocity command writes no position, which with kp=0 commands nothing anyway");

    host.forget();
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 3.0f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 3), 0) == SKR_OK,
          "a torque command is accepted");
    check(host.wait_for_datagrams(1), "it reached the wire");
    const Seen torque = decode(host.datagram(host.datagrams() - 1));
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
    check(host.wait_for_datagrams(1), "it reached the wire");
    const Seen seen = decode(host.datagram(host.datagrams() - 1));
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
    check(host.wait_for_datagrams(1), "it reached the wire");
    check(near(decode(host.datagram(host.datagrams() - 1)).command.joint_cmd[1].position, 0.2),
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
    check(host.wait_for_datagrams(1),
          "and it still goes out: falling silent would let the robot reclaim the legs, which is "
          "a different thing from being told to hold them");
    const Seen held = decode(host.datagram(host.datagrams() - 1));
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
    check(host.wait_for_datagrams(1), "it reached the wire");
    const Seen seen = decode(host.datagram(host.datagrams() - 1));
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
    host.forget();
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) {
      cmd.mask[i] = 1;
      cmd.values[i] = 1.0f;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_TORQUE, 11), 0) == SKR_OK,
          "a command while control is held goes straight out");
    check(host.wait_for_datagrams(1) && host.datagrams() == 1,
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
    check(telemetry.find("\"stale_reads\":2") != std::string::npos,
          "and the two reads that waited a whole bound and got nothing, from the paused body "
          "above — a count, where the ABI has only a repeated seq");
    check(telemetry.find("\"push_interval_ms\":") != std::string::npos,
          "and the push interval this driver measured, since the vendor declares none");
    check(driver->extension(ctx, "vendor.lite3.telemetry", "", 0, reply, 8) == SKR_EINVAL,
          "a reply buffer too small to hold the answer is refused, never truncated");
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

  host.stop();
  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
