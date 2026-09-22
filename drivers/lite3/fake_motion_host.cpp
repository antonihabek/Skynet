/*
 * fake_motion_host — a Jueying Lite3's motion host that is not there.
 *
 * F92 specifies this row's substrate as *"a loopback fake motion host + SDK client unmodified
 * (Fourier shape)"*, and this is the first half: a process that listens for MotionSDK's command
 * datagrams on 43893 and pushes `RobotData` to 43897, exactly as the robot's own motion host does
 * once `~/jy_exe/conf/network.toml` points at this machine. `cross_check.cpp` is the second half
 * and drives this process with the vendor's own `Sender` and `Receiver`.
 *
 * The wire comes from `lite3_wire.hpp`, the same header the driver and the suite read, so the
 * three cannot disagree about which bytes mean what.
 *
 * ── What it is not ───────────────────────────────────────────────────────
 *
 *   * The joints integrate the vendor's drive law with a constant inertia and nothing else: no
 *     gravity, no contact, no ground. A leg here never touches anything, so this fake cannot fall
 *     over and cannot stand up either, which is the largest thing a quadruped substrate leaves out.
 *   * The rate is nominal — a thread with `sleep_for`, not a 20 kHz current loop behind a real-time
 *     controller.
 *   * The foot forces it publishes are zero, because zero is what a body with no contact model
 *     measures. That is the one place where this fake's honesty and §24's "unmeasured is NaN" pull
 *     apart, and it is resolved the robot's way: the wire is `double leg_force[12]` and a real
 *     motion host writes numbers into it, so the fake writes the number its physics has.
 *   * There is no safety hardware behind it. The stop it observes is a datagram.
 *
 * ── What it refuses, which is why it is worth having ─────────────────────
 *
 * A fake that accepts everything tests the happy path, and the happy path is the part a driver
 * gets right by accident. This one counts, reports, and (where the robot would) ignores:
 *
 *   * **a joint command arriving while the robot's own controller holds the legs.** Control right
 *     is a state on the machine: MotionSDK's README §2 says the underlying controller takes the
 *     robot back one second after the last SDK command, and a driver that took control once and
 *     assumed it for ever would present as a robot that ignores commands. This is the Lite3's
 *     analogue of the Unitree row's CRC, and it caught the defect it was built for.
 *   * **a setpoint outside the travel Deep Robotics publishes.** The driver declares that travel
 *     in its own descriptor; commanding past it would make the descriptor a document nothing
 *     honours.
 *   * **a malformed datagram** — short, the wrong `type` octet, a payload length that is not
 *     `sizeof(RobotCmd)`, or a code no vendor method sends.
 *
 * It also counts **masked joints commanded limp**, which is not a defect and is worth seeing: it
 * is the number of joints this run left slack because §24's mask has no per-joint expression on
 * this wire (see the driver's `hold_kd`).
 *
 * Licence: Apache-2.0. Links nothing of the vendor's.
 */

#include "lite3_wire.hpp"

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>

namespace {

using namespace skynet_lite3;

std::atomic<bool> RUNNING{true};

void stop(int) { RUNNING.store(false); }

/* The vendor's own per-joint ratings, indexed `i % 3` the way `Lite3_rl_deploy` indexes them:
 * `joint_vel_limit_ << 30, 30, 20` and `torque_limit_ << 40, 40, 65`. */
constexpr double VELOCITY_MAX[3] = {30.0, 30.0, 20.0};
constexpr double EFFORT_MAX[3] = {40.0, 40.0, 65.0};
constexpr double TRAVEL_LOW[3] = {-0.530, -3.50, 0.349};
constexpr double TRAVEL_HIGH[3] = {0.530, 0.320, 2.80};

/* One number with no vendor behind it, and the only one in this file: the rotor inertia the
 * integration divides by. It decides how fast this fake's joints move under a torque and nothing
 * else, and no measurement of it is published — so it is named here as the fake's own rather than
 * dressed as the robot's. */
constexpr double JOINT_INERTIA = 0.05;

struct Fake {
  std::mutex lock;

  /* Control right, the machine's state. `false` is the underlying controller holding the legs,
   * which is where a Lite3 powers on. */
  bool sdk_control = false;
  std::chrono::steady_clock::time_point last_command{};
  bool ever_commanded = false;

  double position[JOINTS] = {0};
  double velocity[JOINTS] = {0};
  double torque[JOINTS] = {0};
  JointCmd command[JOINTS] = {};

  unsigned long long datagrams = 0;
  unsigned long long joint_commands = 0;
  unsigned long long control_taken = 0;
  unsigned long long control_returned = 0;
  unsigned long long joints_to_zero = 0;
  unsigned long long malformed = 0;
  unsigned long long unknown_code = 0;
  unsigned long long commanded_without_control = 0;
  unsigned long long beyond_travel = 0;
  unsigned long long clamped_torque = 0;
  unsigned long long clamped_velocity = 0;
  unsigned long long clamped_position = 0;
  unsigned long long masked_free = 0;
  unsigned long long reclaims = 0;

  void apply(const RobotCmd &incoming) {
    joint_commands += 1;
    if (!sdk_control) {
      /* What the robot does: nothing. The underlying controller has the legs. */
      commanded_without_control += 1;
      return;
    }
    last_command = std::chrono::steady_clock::now();
    ever_commanded = true;
    for (int i = 0; i < JOINTS; ++i) {
      const JointCmd &joint = incoming.joint_cmd[i];
      if (joint.kp == 0.0f && joint.kd == 0.0f && joint.torque == 0.0f) {
        /* Zero gains and no feed-forward: the drive law makes this zero torque whatever the
         * setpoints say. Not a defect — it is what this wire has instead of "say nothing" — and
         * counted because a run that left legs slack should be legible afterwards. */
        masked_free += 1;
      } else if (joint.kp != 0.0f) {
        /* A stiffness means the position field is a setpoint, so it is one the published travel
         * has to admit. */
        const int kind = i % 3;
        if (joint.position < TRAVEL_LOW[kind] - 1e-6 || joint.position > TRAVEL_HIGH[kind] + 1e-6) {
          beyond_travel += 1;
        }
      }
      command[i] = joint;
    }
  }

  /* The vendor's drive law, `T = kp*(pos_goal - pos) + kd*(vel_goal - vel) + t_ff`, integrated
   * with nothing else in it. */
  void integrate(double dt) {
    const auto now = std::chrono::steady_clock::now();
    if (sdk_control && ever_commanded &&
        std::chrono::duration<double, std::milli>(now - last_command).count() > RECLAIM_MS) {
      /* MotionSDK README §2: one second without a command and the underlying controller takes
       * the robot back into damping protection, then clears the joint commands. */
      sdk_control = false;
      reclaims += 1;
      for (int i = 0; i < JOINTS; ++i) {
        command[i] = JointCmd{};
        command[i].kd = VENDOR_DAMPING_KD;
      }
    }
    for (int i = 0; i < JOINTS; ++i) {
      const int kind = i % 3;
      const JointCmd &joint = command[i];
      double tau = static_cast<double>(joint.kp) * (joint.position - position[i]) +
                   static_cast<double>(joint.kd) * (joint.velocity - velocity[i]) +
                   static_cast<double>(joint.torque);
      if (tau > EFFORT_MAX[kind]) {
        tau = EFFORT_MAX[kind];
        clamped_torque += 1;
      } else if (tau < -EFFORT_MAX[kind]) {
        tau = -EFFORT_MAX[kind];
        clamped_torque += 1;
      }
      torque[i] = tau;
      velocity[i] += tau / JOINT_INERTIA * dt;
      if (velocity[i] > VELOCITY_MAX[kind]) {
        velocity[i] = VELOCITY_MAX[kind];
        clamped_velocity += 1;
      } else if (velocity[i] < -VELOCITY_MAX[kind]) {
        velocity[i] = -VELOCITY_MAX[kind];
        clamped_velocity += 1;
      }
      position[i] += velocity[i] * dt;
      if (position[i] > TRAVEL_HIGH[kind]) {
        position[i] = TRAVEL_HIGH[kind];
        velocity[i] = 0.0;
        clamped_position += 1;
      } else if (position[i] < TRAVEL_LOW[kind]) {
        position[i] = TRAVEL_LOW[kind];
        velocity[i] = 0.0;
        clamped_position += 1;
      }
    }
  }

  void report() const {
    std::printf(
        "{\"datagrams\":%llu,\"joint_commands\":%llu,\"control_taken\":%llu,"
        "\"control_returned\":%llu,\"joints_to_zero\":%llu,\"malformed\":%llu,"
        "\"unknown_code\":%llu,\"commanded_without_control\":%llu,\"beyond_travel\":%llu,"
        "\"clamped_torque\":%llu,\"clamped_velocity\":%llu,\"clamped_position\":%llu,"
        "\"masked_free\":%llu,\"reclaims\":%llu}\n",
        datagrams, joint_commands, control_taken, control_returned, joints_to_zero, malformed,
        unknown_code, commanded_without_control, beyond_travel, clamped_torque, clamped_velocity,
        clamped_position, masked_free, reclaims);
  }
};

void handle(Fake &fake, const unsigned char *bytes, std::size_t length) {
  std::lock_guard<std::mutex> guard(fake.lock);
  fake.datagrams += 1;
  if (length < sizeof(EthCommand)) {
    fake.malformed += 1;
    return;
  }
  EthCommand header{};
  std::memcpy(&header, bytes, sizeof header);
  switch (header.code) {
    case CODE_JOINT_CMD: {
      if (wire_type(header.type_and_count) != TYPE_MESS_VALUES ||
          header.value_or_size != sizeof(RobotCmd) || length < JOINT_DATAGRAM) {
        fake.malformed += 1;
        return;
      }
      RobotCmd incoming{};
      std::memcpy(&incoming, bytes + sizeof(EthCommand), sizeof incoming);
      fake.apply(incoming);
      return;
    }
    case CODE_CONTROL_SDK:
      fake.sdk_control = true;
      fake.last_command = std::chrono::steady_clock::now();
      fake.control_taken += 1;
      return;
    case CODE_CONTROL_ROBOT:
      fake.sdk_control = false;
      fake.ever_commanded = false;
      fake.control_returned += 1;
      for (int i = 0; i < JOINTS; ++i) {
        fake.command[i] = JointCmd{};
        fake.command[i].kd = VENDOR_DAMPING_KD;
      }
      return;
    case CODE_JOINTS_TO_ZERO:
      /* "Send a command to make all joints return to the zero position and acquisition of
       * control" (`sender.h`). The acquisition is modelled; the motion is not — a fake that
       * drove the legs to zero on its own would be inventing the robot's internal trajectory. */
      fake.joints_to_zero += 1;
      fake.sdk_control = true;
      fake.last_command = std::chrono::steady_clock::now();
      return;
    default:
      fake.unknown_code += 1;
      return;
  }
}

}  // namespace

int main(int argc, char **argv) {
  const std::uint16_t command_port =
      static_cast<std::uint16_t>(argc > 1 ? std::atoi(argv[1]) : COMMAND_PORT);
  const std::string state_host = argc > 2 ? argv[2] : "127.0.0.1";
  const std::uint16_t state_port =
      static_cast<std::uint16_t>(argc > 3 ? std::atoi(argv[3]) : STATE_PORT);
  const double hz = argc > 4 ? std::atof(argv[4]) : 500.0;
  const double seconds = argc > 5 ? std::atof(argv[5]) : 0.0;

  std::signal(SIGINT, stop);
  std::signal(SIGTERM, stop);

  const int listen_fd = ::socket(AF_INET, SOCK_DGRAM, 0);
  if (listen_fd < 0) {
    std::fprintf(stderr, "fake-lite3: cannot make a socket\n");
    return 2;
  }
  int reuse = 1;
  ::setsockopt(listen_fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof reuse);
  sockaddr_in local{};
  local.sin_family = AF_INET;
  local.sin_addr.s_addr = htonl(INADDR_ANY);
  local.sin_port = htons(command_port);
  if (::bind(listen_fd, reinterpret_cast<const sockaddr *>(&local), sizeof local) < 0) {
    std::fprintf(stderr, "fake-lite3: cannot bind UDP %u for commands\n",
                 static_cast<unsigned>(command_port));
    return 2;
  }
  timeval timeout{};
  timeout.tv_sec = 0;
  timeout.tv_usec = 50000;
  ::setsockopt(listen_fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);

  const int push_fd = ::socket(AF_INET, SOCK_DGRAM, 0);
  if (push_fd < 0) {
    std::fprintf(stderr, "fake-lite3: cannot make a push socket\n");
    return 2;
  }
  sockaddr_in remote{};
  remote.sin_family = AF_INET;
  remote.sin_port = htons(state_port);
  if (::inet_pton(AF_INET, state_host.c_str(), &remote.sin_addr) != 1) {
    std::fprintf(stderr, "fake-lite3: \"%s\" is not an IPv4 address\n", state_host.c_str());
    return 2;
  }

  Fake fake;
  fake.last_command = std::chrono::steady_clock::now();
  /* A rest pose inside the published travel, and the vendor's own choice of it: `standup_state.hpp`
   * takes `default_pos = (lower + upper) / 2` for the hip and the knee. Starting every joint at
   * zero would put all four knees below their 0.349 lower bound, and the fake would spend its
   * first cycles clamping a body nobody had commanded. */
  for (int i = 0; i < JOINTS; ++i) {
    const int kind = i % 3;
    fake.position[i] = (TRAVEL_LOW[kind] + TRAVEL_HIGH[kind]) / 2.0;
  }

  std::fprintf(stderr,
               "fake-lite3: listening for commands on UDP %u, pushing state to %s:%u at %g Hz. "
               "This is not a robot: no gravity, no contact, no ground, and the rate is nominal.\n",
               static_cast<unsigned>(command_port), state_host.c_str(),
               static_cast<unsigned>(state_port), hz);

  std::thread pusher([&] {
    const auto period = std::chrono::duration<double>(1.0 / hz);
    const auto started = std::chrono::steady_clock::now();
    unsigned char datagram[STATE_DATAGRAM];
    EthCommand header{};
    header.code = CODE_STATE_PUSH;
    header.value_or_size = static_cast<std::uint32_t>(sizeof(RobotData));
    header.type_and_count = pack_type_count(TYPE_MESS_VALUES, 0);
    while (RUNNING.load()) {
      RobotData state{};
      {
        std::lock_guard<std::mutex> guard(fake.lock);
        fake.integrate(1.0 / hz);
        const double elapsed =
            std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - started)
                .count();
        state.tick = static_cast<std::uint32_t>(elapsed);
        state.imu.timestamp = static_cast<std::int32_t>(elapsed);
        /* A body that is level and still, in the degrees this wire carries. The rates are the
         * only part the driver reads into `base_twist`, and they are zero because nothing here
         * rotates the base. */
        state.imu.acc_z = 9.81f;
        for (int i = 0; i < JOINTS; ++i) {
          state.joint_data.joint_data[i].position = static_cast<float>(fake.position[i]);
          state.joint_data.joint_data[i].velocity = static_cast<float>(fake.velocity[i]);
          state.joint_data.joint_data[i].torque = static_cast<float>(fake.torque[i]);
          /* A plausible constant, and a fake's: nothing here heats up. */
          state.joint_data.joint_data[i].temperature = 30.0f;
        }
      }
      std::memcpy(datagram, &header, sizeof header);
      std::memcpy(datagram + sizeof header, &state, sizeof state);
      ::sendto(push_fd, datagram, sizeof datagram, 0,
               reinterpret_cast<const sockaddr *>(&remote), sizeof remote);
      std::this_thread::sleep_for(period);
      if (seconds > 0.0 && std::chrono::steady_clock::now() - started >
                               std::chrono::duration<double>(seconds)) {
        RUNNING.store(false);
      }
    }
  });

  unsigned char buffer[sizeof(CommandMessage)];
  while (RUNNING.load()) {
    const ssize_t got = ::recv(listen_fd, buffer, sizeof buffer, 0);
    if (got < 0) continue;
    handle(fake, buffer, static_cast<std::size_t>(got));
  }

  pusher.join();
  ::close(listen_fd);
  ::close(push_fd);
  std::lock_guard<std::mutex> guard(fake.lock);
  fake.report();
  return 0;
}
