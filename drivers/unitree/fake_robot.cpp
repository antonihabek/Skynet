/*
 * fake_robot — a Go2 that is not there, speaking unitree_sdk2's own channels.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 2 table asks for an *"SDK sim"* as this row's substrate and this is
 * it: a process that publishes `LowState_` on `rt/lowstate` and subscribes to `LowCmd_` on
 * `rt/lowcmd`, using **the vendor's SDK on this side too**. So the CDR encoding, the topic types and
 * the DDS discovery are Unitree's code at both ends, and nothing here can be wrong about the wire
 * without being wrong in the way a real robot would refuse.
 *
 * What it is **not** is a robot, and the differences are the row's *not dry-verifiable* column:
 *
 *   * The joints integrate a first-order model with no dynamics, no contact and no gravity. A leg
 *     here never touches the ground.
 *   * The rate is nominal. A real Go2 publishes at 500 Hz from its own onboard controller; this
 *     publishes from a thread with `sleep_for`, which is a rate and not a guarantee.
 *   * There is no safety hardware behind it. The estop this fake observes is a message.
 *
 * ── The three things it refuses, which is why it is worth having ───────────
 *
 * A fake that accepts everything tests the happy path, and the happy path is the part a driver gets
 * right by accident. This one counts and reports:
 *
 *   * **a bad CRC** — a real robot drops the command silently, so a driver that omitted the
 *     checksum would present as a robot that ignores commands;
 *   * **a zero written where a sentinel belongs** — `q = 0` is not "say nothing about position",
 *     it is "go to zero radians", and with any `kp` at all that is a motion;
 *   * **a command to a motor slot this body does not have** — slots 12..19 exist in the message and
 *     not on a Go2.
 *
 * Licence: Apache-2.0 (this file). Links unitree_sdk2 (BSD-3-Clause) and CycloneDDS; neither is
 * vendored here.
 */

#include <unitree/idl/go2/LowCmd_.hpp>
#include <unitree/idl/go2/LowState_.hpp>
#include <unitree/robot/channel/channel_publisher.hpp>
#include <unitree/robot/channel/channel_subscriber.hpp>

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

constexpr float POS_STOP_F = 2.146e9f;
constexpr float VEL_STOP_F = 16000.0f;
constexpr std::size_t GO2_JOINTS = 12;
constexpr std::size_t MOTOR_SLOTS = 20;

std::atomic<bool> RUNNING{true};

void on_signal(int) { RUNNING = false; }

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

struct Robot {
  std::mutex lock;
  float q[GO2_JOINTS];
  float dq[GO2_JOINTS];
  float tau[GO2_JOINTS];
  std::uint64_t commands = 0;
  std::uint64_t bad_crc = 0;
  std::uint64_t zero_where_sentinel = 0;
  std::uint64_t phantom_slot = 0;

  Robot() {
    /* A Go2's standing pose rather than zeros: a body that starts at zero would make a driver
     * writing zeros look correct. */
    const float stand[GO2_JOINTS] = {0.0f,  0.9f,  -1.8f, 0.0f, 0.9f, -1.8f,
                                     0.0f,  0.9f,  -1.8f, 0.0f, 0.9f, -1.8f};
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
      q[i] = stand[i];
      dq[i] = 0.0f;
      tau[i] = 0.0f;
    }
  }

  void apply(const unitree_go::msg::dds_::LowCmd_ &cmd) {
    std::lock_guard<std::mutex> guard(lock);
    const std::uint32_t expected =
        crc32_core(reinterpret_cast<const std::uint32_t *>(&cmd),
                   (sizeof(unitree_go::msg::dds_::LowCmd_) >> 2) - 1);
    if (cmd.crc() != expected) {
      /* Dropped, exactly as the robot drops it, and counted so the suite can say the driver's
       * checksum is right rather than assuming it. */
      bad_crc += 1;
      return;
    }
    commands += 1;
    for (std::size_t i = MOTOR_SLOTS; i-- > GO2_JOINTS;) {
      const auto &motor = cmd.motor_cmd()[i];
      const bool commands_position = motor.q() != POS_STOP_F && motor.kp() != 0.0f;
      const bool commands_torque = motor.tau() != 0.0f;
      if (commands_position || commands_torque) phantom_slot += 1;
    }
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
      const auto &motor = cmd.motor_cmd()[i];
      /* The defect this substrate exists to catch. `q = 0` with a stiffness is a command to move
       * to zero radians; a driver meaning "say nothing about position" writes `PosStopF`. */
      if (motor.q() == 0.0f && motor.kp() != 0.0f) zero_where_sentinel += 1;
      if (motor.dq() == 0.0f && motor.kd() != 0.0f && motor.q() == POS_STOP_F) {
        zero_where_sentinel += 1;
      }
      const float position_term =
          motor.q() == POS_STOP_F ? 0.0f : motor.kp() * (motor.q() - q[i]);
      const float velocity_term =
          motor.dq() == VEL_STOP_F ? 0.0f : motor.kd() * (motor.dq() - dq[i]);
      tau[i] = motor.tau() + position_term + velocity_term;
    }
  }

  void integrate(float dt) {
    std::lock_guard<std::mutex> guard(lock);
    for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
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

  unitree::robot::ChannelFactory::Instance()->Init(0, interface);
  auto publisher = std::make_shared<
      unitree::robot::ChannelPublisher<unitree_go::msg::dds_::LowState_>>("rt/lowstate");
  publisher->InitChannel();

  Robot robot;
  auto subscriber = std::make_shared<
      unitree::robot::ChannelSubscriber<unitree_go::msg::dds_::LowCmd_>>("rt/lowcmd");
  subscriber->InitChannel(
      [&robot](const void *message) {
        robot.apply(*static_cast<const unitree_go::msg::dds_::LowCmd_ *>(message));
      },
      1);

  std::fprintf(stderr,
               "fake-go2: publishing rt/lowstate on `%s` at %g Hz. This is not a robot: no "
               "dynamics, no contact, no safety hardware, and the rate is nominal.\n",
               interface.c_str(), hz);

  const auto period = std::chrono::duration<double>(1.0 / hz);
  const auto started = std::chrono::steady_clock::now();
  std::uint32_t tick = 0;
  while (RUNNING) {
    robot.integrate(static_cast<float>(1.0 / hz));
    unitree_go::msg::dds_::LowState_ state{};
    state.head()[0] = 0xFE;
    state.head()[1] = 0xEF;
    state.level_flag() = 0xFF;
    state.tick() = tick++;
    {
      std::lock_guard<std::mutex> guard(robot.lock);
      for (std::size_t i = 0; i < GO2_JOINTS; ++i) {
        state.motor_state()[i].q() = robot.q[i];
        state.motor_state()[i].dq() = robot.dq[i];
        state.motor_state()[i].tau_est() = robot.tau[i];
        state.motor_state()[i].temperature() = 30;
        state.motor_state()[i].lost() = 0;
      }
    }
    publisher->Write(state);
    std::this_thread::sleep_for(period);
    if (seconds > 0.0 &&
        std::chrono::steady_clock::now() - started > std::chrono::duration<double>(seconds)) {
      break;
    }
  }

  std::lock_guard<std::mutex> guard(robot.lock);
  std::printf("{\"commands\":%llu,\"bad_crc\":%llu,\"zero_where_sentinel\":%llu,"
              "\"phantom_slot\":%llu}\n",
              (unsigned long long)robot.commands, (unsigned long long)robot.bad_crc,
              (unsigned long long)robot.zero_where_sentinel,
              (unsigned long long)robot.phantom_slot);
  return 0;
}
