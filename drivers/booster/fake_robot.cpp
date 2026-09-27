/*
 * fake_robot — a Booster T1 that is not there, speaking the vendor SDK's own channels.
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
 * kp*(q - qm) + kd*(dq - dqm)`) — which is why a driver bug in the gains moves the wrong joint
 * here rather than passing silently. The serial and parallel state vectors carry the same values:
 * this fake models a series mechanism, so the conversion the robot does internally is the identity,
 * and the driver reads the serial vector it commands.
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
 * Licence: Apache-2.0 (this file). Links booster_robotics_sdk (Apache-2.0); not vendored here.
 */

#include <booster/idl/b1/LowCmd.h>
#include <booster/idl/b1/LowState.h>
#include <booster/robot/b1/b1_api_const.hpp>
#include <booster/robot/channel/channel_publisher.hpp>
#include <booster/robot/channel/channel_subscriber.hpp>

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

std::atomic<bool> RUNNING{true};

void on_signal(int) { RUNNING = false; }

struct Robot {
  float q[T1_JOINTS];
  float dq[T1_JOINTS];
  float tau[T1_JOINTS];
  std::mutex lock;
  std::uint64_t commands = 0;
  std::uint64_t bad_size = 0;
  std::uint64_t wrong_view = 0;
  std::uint64_t blended = 0;

  Robot() {
    /* A pose with almost no zeros rather than zeros: a body that starts at zero would make a
     * driver writing zeros look correct. Knees bent, ankles back, shoulders lifted, elbows bent;
     * the head and waist stay at zero, which still catches a driver writing zeros everywhere. */
    for (int i = 0; i < T1_JOINTS; ++i) {
      q[i] = 0.0f;
      dq[i] = 0.0f;
      tau[i] = 0.0f;
    }
    q[2] = 0.1f;
    q[5] = 0.3f;
    q[6] = 0.1f;
    q[9] = 0.3f;
    q[14] = 0.2f;
    q[15] = -0.05f;
    q[20] = 0.2f;
    q[21] = -0.05f;
  }

  void apply(const booster_interface::msg::LowCmd &cmd) {
    std::lock_guard<std::mutex> guard(lock);
    if (cmd.motor_cmd().size() != T1_JOINTS) {
      bad_size += 1;
      return;
    }
    if (cmd.cmd_type() != booster_interface::msg::SERIAL) {
      wrong_view += 1;
      return;
    }
    commands += 1;
    for (int i = 0; i < T1_JOINTS; ++i) {
      const auto &motor = cmd.motor_cmd()[i];
      if (motor.weight() < 1.0f) blended += 1;
      /* The standard impedance law, shared with the driver's assumption: a proportional term on
       * the position error, a damping term on the velocity error, plus the feedforward torque. A
       * zero gain ignores its field — which is the whole of the driver's velocity mapping, so a
       * driver that forgot to zero `kp` drives the wrong torque here. */
      tau[i] = motor.tau() + motor.kp() * (motor.q() - q[i]) +
               motor.kd() * (motor.dq() - dq[i]);
    }
  }

  void integrate(float dt) {
    std::lock_guard<std::mutex> guard(lock);
    for (int i = 0; i < T1_JOINTS; ++i) {
      /* First-order, unit inertia, a little damping. Not a model of a leg — a body that moves when
       * commanded and stops when not, so that a driver's setpoint is visibly the thing that moved
       * it. */
      dq[i] += tau[i] * dt;
      dq[i] *= 0.98f;
      q[i] += dq[i] * dt;
    }
  }
};

}  // namespace

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
  const double hz = argc > 2 ? std::atof(argv[2]) : 500.0;
  const double seconds = argc > 3 ? std::atof(argv[3]) : 0.0;

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
               "fake-t1: publishing rt/low_state on `%s` at %g Hz. This is not a robot: no "
               "dynamics, no contact, no safety hardware, and the rate is nominal.\n",
               interface.c_str(), hz);

  const auto period = std::chrono::duration<double>(1.0 / hz);
  const auto started = std::chrono::steady_clock::now();
  while (RUNNING) {
    robot.integrate(static_cast<float>(1.0 / hz));
    booster_interface::msg::LowState state;
    std::vector<booster_interface::msg::MotorState> serial(T1_JOINTS);
    {
      std::lock_guard<std::mutex> guard(robot.lock);
      for (int i = 0; i < T1_JOINTS; ++i) {
        serial[i].q() = robot.q[i];
        serial[i].dq() = robot.dq[i];
        serial[i].tau_est() = robot.tau[i];
        serial[i].lost() = 0;
      }
    }
    /* Both views, same values: a series mechanism needs no conversion. */
    state.motor_state_serial() = serial;
    state.motor_state_parallel() = serial;
    publisher->Write(&state);
    std::this_thread::sleep_for(period);
    if (seconds > 0.0 &&
        std::chrono::steady_clock::now() - started > std::chrono::duration<double>(seconds)) {
      break;
    }
  }

  std::lock_guard<std::mutex> guard(robot.lock);
  std::printf("{\"commands\":%llu,\"bad_size\":%llu,\"wrong_view\":%llu,\"blended\":%llu}\n",
              (unsigned long long)robot.commands, (unsigned long long)robot.bad_size,
              (unsigned long long)robot.wrong_view, (unsigned long long)robot.blended);
  return 0;
}
