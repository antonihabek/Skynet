/*
 * fake_robot — a Booster T1 (or K1: `model=k1`) that is not there, speaking the vendor SDK's own channels.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 7 item 3 asks for the T1 against the vendor's SDK, and this is the
 * substrate half of that: a process that publishes `LowState` on `rt/low_state` and subscribes to
 * `LowCmd` on `rt/joint_ctrl`, using **Booster's SDK on this side too**. So the CDR encoding, the
 * topic types and the DDS discovery are Booster's code at both ends, and nothing here can be wrong
 * about the wire without being wrong in the way a real robot would refuse.
 *
 * It behaves as a T1 in CUSTOM mode: every command is integrated, because in custom mode "the
 * internal controller will send no motor cmds" (manual p56) and the joints follow the user. The
 * joints integrate the standard impedance law the driver's mapping assumes (`tau = tau_ff +
 * kp*(q - qm) + kd*(dq - dqm)`), evaluated every step against the joint as it is — which is why a
 * driver bug in the gains moves the wrong joint here rather than passing silently. The serial and
 * parallel state vectors carry *different* numbers (the parallel one is offset), so that a driver
 * that read the view it does not command would report angles that are not its joints'; the serial
 * vector is the one the driver reads and commands.
 *
 * What it is **not** is a robot: no dynamics, no contact, no gravity (a humanoid here never falls
 * over), a nominal rate from a thread with `sleep_for`, and no safety hardware — the estop it
 * observes is a message.
 *
 * ── The three things it refuses, which is why it is worth having ───────────
 *
 *   * **a wrong-sized command** — the SDK header warns that a command with the wrong layout "can
 *     address the wrong actuator or cause the robot to reject the message". Dropped and counted.
 *   * **the wrong view** — a PARALLEL command addresses crank actuators this series mechanism does
 *     not have. Dropped and counted: the driver promised SERIAL everywhere.
 *   * **a blended command** — `weight < 1` hands part of the joint to the internal controller.
 *     Moot in custom mode, but the driver's contract is explicit full-user on every joint, so
 *     anything less is a defect in the driver's output. Counted, and the command still applies.
 *
 * ── The tap ────────────────────────────────────────────────────────────────
 *
 * `tap=<fd>` after the other arguments, in any position: where the body really is, and how many
 * commands it has acted on, for the link-chaos contract (`chaos.py`), which has to know both without
 * asking the driver and without crossing the link it is breaking. The descriptor is a pipe the
 * caller made, so no fault on the network interface can touch it. Every fifth state (100 Hz at the
 * nominal 500) one record is written: the twenty-three joints of the serial view as doubles, then
 * the count of commands acted on as a uint64, little-endian, nothing between. The write never
 * blocks, so a reader that has not drained the pipe loses a record and this loop is never held up.
 *
 * ── The K1 ─────────────────────────────────────────────────────────────────
 *
 * `model=k1`, in any position like `tap=`, makes it the 22-joint body of the SDK's `JointIndexK1`: the same law, the same refusals, one fewer joint. The "another body's layout"
 * (`layout=<seconds>`) is then the T1's twenty-three. Without it nothing here is different from before.
 *
 * Licence: Apache-2.0 (this file). Links booster_robotics_sdk (Apache-2.0); not vendored here.
 */

#include <booster/idl/b1/LowCmd.h>
#include <booster/idl/b1/LowState.h>
#include <booster/robot/b1/b1_api_const.hpp>
#include <booster/robot/channel/channel_publisher.hpp>
#include <booster/robot/channel/channel_subscriber.hpp>

#include <fcntl.h>
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
#include <vector>

namespace {

constexpr int T1_JOINTS = 23;
constexpr int K1_JOINTS = 22;

/* How many joints this fake has: the T1's, or the K1's with `model=k1`. Arrays are sized for the T1. */
int JOINTS = T1_JOINTS;

std::atomic<bool> RUNNING{true};

void on_signal(int) { RUNNING = false; }

struct Robot {
  float q[T1_JOINTS];
  float dq[T1_JOINTS];
  float tau[T1_JOINTS];
  /* The last command's impedance terms, per joint. The law is evaluated every integration step
   * against the joint's *current* position and velocity, as a drive does, and not once when the
   * command arrives: a torque frozen at its arrival value would run a joint away at a constant
   * speed until the next command, and nothing downstream could then say where a held joint is. */
  float cmd_q[T1_JOINTS];
  float cmd_dq[T1_JOINTS];
  float cmd_tau[T1_JOINTS];
  float cmd_kp[T1_JOINTS];
  float cmd_kd[T1_JOINTS];
  std::mutex lock;
  std::uint64_t commands = 0;
  std::uint64_t bad_size = 0;
  std::uint64_t wrong_view = 0;
  std::uint64_t blended = 0;
  std::uint64_t idle_mode = 0;

  Robot() {
    /* A pose with a different, non-zero angle on every joint: a body that starts at zero would
     * make a driver writing zeros look correct, and one whose joints share a value would make a
     * driver that read a neighbour's slot look correct too. */
    for (int i = 0; i < T1_JOINTS; ++i) {
      q[i] = 0.013f * static_cast<float>(i + 1);
      dq[i] = 0.0f;
      tau[i] = 0.0f;
      cmd_q[i] = q[i];
      cmd_dq[i] = 0.0f;
      cmd_tau[i] = 0.0f;
      cmd_kp[i] = 0.0f;
      cmd_kd[i] = 0.0f;
    }
  }

  void apply(const booster_interface::msg::LowCmd &cmd) {
    std::lock_guard<std::mutex> guard(lock);
    if (cmd.motor_cmd().size() != static_cast<std::size_t>(JOINTS)) {
      bad_size += 1;
      return;
    }
    if (cmd.cmd_type() != booster_interface::msg::SERIAL) {
      wrong_view += 1;
      return;
    }
    commands += 1;
    bool idle = false;
    for (int i = 0; i < JOINTS; ++i) {
      const auto &motor = cmd.motor_cmd()[i];
      if (motor.weight() < 1.0f) blended += 1;
      /* Mode 0 is idle: a drive told to be idle is unpowered, whatever else the command says. */
      if (motor.mode() != 1) idle = true;
      cmd_q[i] = motor.q();
      cmd_dq[i] = motor.dq();
      cmd_tau[i] = motor.tau();
      cmd_kp[i] = motor.kp();
      cmd_kd[i] = motor.kd();
    }
    if (idle) idle_mode += 1;
  }

  void integrate(float dt) {
    std::lock_guard<std::mutex> guard(lock);
    for (int i = 0; i < JOINTS; ++i) {
      /* The standard impedance law, shared with the driver's assumption: a proportional term on
       * the position error, a damping term on the velocity error, plus the feedforward torque. A
       * zero gain ignores its field — which is the whole of the driver's velocity mapping, so a
       * driver that forgot to zero `kp` drives the wrong torque here. */
      tau[i] = cmd_tau[i] + cmd_kp[i] * (cmd_q[i] - q[i]) + cmd_kd[i] * (cmd_dq[i] - dq[i]);
      /* First-order, unit inertia, a little damping. Not a model of a leg — a body that moves when
       * commanded and settles when the command is met, so that a driver's setpoint is visibly the
       * thing that moved it. */
      dq[i] += tau[i] * dt;
      dq[i] *= 0.98f;
      q[i] += dq[i] * dt;
    }
  }
};

/* The tap (the file header says what it is for). */
struct Tap {
  int fd = -1;

  void open(int descriptor) {
    fd = descriptor;
    /* A reader that stops reading must not stop the robot, and one that goes away must not kill it. */
    ::fcntl(fd, F_SETFL, ::fcntl(fd, F_GETFL) | O_NONBLOCK);
    std::signal(SIGPIPE, SIG_IGN);
  }

  void send(const Robot &robot) const {
    if (fd < 0) return;
    unsigned char record[8 * (T1_JOINTS + 1)];
    for (int i = 0; i < JOINTS; ++i) {
      const double position = robot.q[i];
      std::memcpy(record + 8 * i, &position, 8);
    }
    const std::uint64_t acted_on = robot.commands;
    std::memcpy(record + 8 * JOINTS, &acted_on, 8);
    if (::write(fd, record, 8 * (JOINTS + 1)) < 0) {
      /* A full pipe: the record is lost, and the next one says the same thing a moment later. */
    }
  }
};

}  // namespace

int main(int argc, char **argv) {
  /* `tap=<fd>` is taken out of the arguments first, so that the positional ones below keep their places. */
  Tap tap;
  {
    int kept = 1;
    for (int index = 1; index < argc; ++index) {
      if (std::strcmp(argv[index], "model=k1") == 0) {
        JOINTS = K1_JOINTS;
        continue;
      }
      if (std::strcmp(argv[index], "model=t1") == 0) continue;
      if (std::strncmp(argv[index], "tap=", 4) == 0) {
        const int descriptor = std::atoi(argv[index] + 4);
        if (descriptor < 3) {
          std::fprintf(stderr, "fake_robot: `tap=` wants a descriptor above 2 (got \"%s\")\n", argv[index]);
          return 2;
        }
        tap.open(descriptor);
        continue;
      }
      argv[kept++] = argv[index];
    }
    argc = kept;
  }
  const std::string interface = argc > 1 ? argv[1] : "lo";
  const double hz = argc > 2 ? std::atof(argv[2]) : 500.0;
  const double seconds = argc > 3 ? std::atof(argv[3]) : 0.0;
  /* `<joint>=<value>`: one joint's trouble, as the maker's own `lost` field carries it, so the suite
   * can see what the driver makes of a robot that reports one (F205). Absent: nothing is wrong, as
   * before. */
  int fault_joint = -1;
  std::uint32_t fault_value = 0;
  if (argc > 4 && std::strcmp(argv[4], "-") != 0) {
    char *rest = nullptr;
    fault_joint = static_cast<int>(std::strtol(argv[4], &rest, 10));
    if (rest == nullptr || *rest != '=' || fault_joint < 0 || fault_joint >= JOINTS) {
      std::fprintf(stderr, "fake_robot: the fourth argument is <joint>=<value> with a joint below %d (got \"%s\")\n",
                   JOINTS, argv[4]);
      return 2;
    }
    fault_value = static_cast<std::uint32_t>(std::strtoul(rest + 1, nullptr, 10));
  }
  /* `layout=<seconds>`: from that many seconds in, publish another body's layout — twenty-two
   * serial joints, as a K1 or T2 on the same topic would — so the suite can see what the driver
   * does with a state it cannot index. `layout=0` is from the first sample. Absent: always the
   * T1's twenty-three. */
  double layout_after = -1.0;
  if (argc > 5) {
    if (std::strncmp(argv[5], "layout=", 7) != 0) {
      std::fprintf(stderr, "fake_robot: the fifth argument is layout=<seconds> (got \"%s\")\n", argv[5]);
      return 2;
    }
    layout_after = std::atof(argv[5] + 7);
  }

  std::signal(SIGINT, on_signal);
  std::signal(SIGTERM, on_signal);

  booster::robot::ChannelFactory::Instance()->Init(0, interface);
  auto publisher =
      std::make_shared<booster::robot::ChannelPublisher<booster_interface::msg::LowState>>(
          booster::robot::b1::kTopicLowState);
  publisher->InitChannel();

  Robot robot;
  auto subscriber =
      std::make_shared<booster::robot::ChannelSubscriber<booster_interface::msg::LowCmd>>(
          booster::robot::b1::kTopicJointCtrl);
  subscriber->InitChannel([&robot](const void *message) {
    robot.apply(*static_cast<const booster_interface::msg::LowCmd *>(message));
  });

  std::fprintf(stderr,
               "fake-%s: publishing rt/low_state on `%s` at %g Hz. This is not a robot: no "
               "dynamics, no contact, no safety hardware, and the rate is nominal.\n",
               JOINTS == K1_JOINTS ? "k1" : "t1", interface.c_str(), hz);

  const auto period = std::chrono::duration<double>(1.0 / hz);
  const auto started = std::chrono::steady_clock::now();
  std::uint64_t tick = 0;
  while (RUNNING) {
    robot.integrate(static_cast<float>(1.0 / hz));
    booster_interface::msg::LowState state;
    const double elapsed =
        std::chrono::duration<double>(std::chrono::steady_clock::now() - started).count();
    const std::size_t published =
        (layout_after >= 0.0 && elapsed >= layout_after) ? (JOINTS == T1_JOINTS ? K1_JOINTS : T1_JOINTS) : JOINTS;
    std::vector<booster_interface::msg::MotorState> serial(published);
    std::vector<booster_interface::msg::MotorState> parallel(published);
    {
      std::lock_guard<std::mutex> guard(robot.lock);
      for (std::size_t i = 0; i < published; ++i) {
        const float step = static_cast<float>(i + 1);
        serial[i].q() = robot.q[i];
        /* A fixed offset on the velocity and torque this encoder reports, so that every joint
         * reads a different non-zero number even at rest: a sign, an index or a view slipped in
         * the driver changes a number that is not zero and not shared with a neighbour. */
        serial[i].dq() = robot.dq[i] + 0.03f * step;
        serial[i].tau_est() = robot.tau[i] + 0.05f * step;
        serial[i].lost() = static_cast<int>(i) == fault_joint ? fault_value : 0;
        /* The parallel view is another set of numbers — the crank motors' — so a driver that read
         * it while commanding the serial one would report angles that are not its joints'. */
        parallel[i].q() = robot.q[i] + 0.25f;
        parallel[i].dq() = robot.dq[i] + 0.5f;
        parallel[i].tau_est() = robot.tau[i] + 0.5f;
      }
      if (published == static_cast<std::size_t>(JOINTS) && tick++ % 5 == 0) tap.send(robot);
    }
    state.motor_state_serial() = serial;
    state.motor_state_parallel() = parallel;
    publisher->Write(&state);
    std::this_thread::sleep_for(period);
    if (seconds > 0.0 &&
        std::chrono::steady_clock::now() - started > std::chrono::duration<double>(seconds)) {
      break;
    }
  }

  std::lock_guard<std::mutex> guard(robot.lock);
  std::printf("{\"commands\":%llu,\"bad_size\":%llu,\"wrong_view\":%llu,\"blended\":%llu,"
              "\"idle_mode\":%llu}\n",
              (unsigned long long)robot.commands, (unsigned long long)robot.bad_size,
              (unsigned long long)robot.wrong_view, (unsigned long long)robot.blended,
              (unsigned long long)robot.idle_mode);
  return 0;
}
