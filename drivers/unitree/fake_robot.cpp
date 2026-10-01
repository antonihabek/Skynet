/*
 * fake_robot — a Unitree body that is not there, speaking unitree_sdk2's own channels.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 2 table asks for an *"SDK sim"* as this row's substrate and this is
 * it: a process that publishes `LowState_` on `rt/lowstate` and subscribes to `LowCmd_` on
 * `rt/lowcmd`, using **the vendor's SDK on this side too**. So the CDR encoding, the topic types and
 * the DDS discovery are Unitree's code at both ends, and nothing here can be wrong about the wire
 * without being wrong in the way a real robot would refuse.
 *
 * One binary, seven bodies (`go2`, `b2`, `h1`, `g1`, `h1_2`, `h2`, `r1` as the fourth argument): the slot maps come
 * from `unitree_models.hpp`, the same header the driver and the suite read, so the three cannot
 * disagree about which slot is which joint. The `go2` and `hg` families share topic names with
 * different types, and DDS matches on both — a Go2 fake and an hg subscriber never meet, which is
 * why `run-check.sh` runs the seven bodies in sequence rather than in parallel.
 *
 * What it is **not** is a robot, and the differences are the row's *not dry-verifiable* column:
 *
 *   * The joints integrate a first-order model with no dynamics, no contact and no gravity. A leg
 *     here never touches the ground — and a humanoid here never falls over, which is the larger
 *     lie for the taller bodies.
 *   * The rate is nominal. A real Go2 publishes at 500 Hz from its own onboard controller; this
 *     publishes from a thread with `sleep_for`, which is a rate and not a guarantee.
 *   * There is no safety hardware behind it. The estop this fake observes is a message.
 *   * The hg machine code is best-effort: 4 for the G1-23, which is what Unitree's own simulator
 *     sends — and 0 for the H1-2, because Unitree publishes no code for it and a fake that
 *     invented one would be teaching the driver to expect it. The driver echoes whatever arrives,
 *     so the value exercises the path either way.
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
 *   * **a command to a motor slot this body does not have** — slots 12..19 on a Go2, slot 9 on an
 *     H1 (the vendor's own `kNotUsedJoint`), the sparse holes and the tail above 28 on a G1.
 *
 * It also publishes a correct state CRC in both families, because a real body does and the vendor
 * examples check it. The driver does not gate on it — that behaviour is unchanged — but the wire
 * is faithful, so a future check can rely on the field.
 *
 * Licence: Apache-2.0 (this file). Links unitree_sdk2 (BSD-3-Clause) and CycloneDDS; neither is
 * vendored here.
 */

#include "unitree_models.hpp"

#include <unitree/idl/go2/LowCmd_.hpp>
#include <unitree/idl/go2/LowState_.hpp>
#include <unitree/idl/hg/LowCmd_.hpp>
#include <unitree/idl/hg/LowState_.hpp>
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

/* The body, in joint order — the slots are the shared header's, because the fake models the robot
 * and the robot's layout is the wire truth. */
struct Robot {
  const skynet_unitree::WireModel *wire;
  float q[skynet_unitree::MAX_JOINTS];
  float dq[skynet_unitree::MAX_JOINTS];
  float tau[skynet_unitree::MAX_JOINTS];
  /* The last command's impedance terms, per joint. The law is evaluated every integration step
   * against the joint's *current* position and velocity, as a drive does, and not once when the
   * command arrives: a torque frozen at its arrival value would run a joint away at a constant
   * speed until the next command, and nothing downstream could then say where a held joint is. */
  float cmd_q[skynet_unitree::MAX_JOINTS];
  float cmd_dq[skynet_unitree::MAX_JOINTS];
  float cmd_tau[skynet_unitree::MAX_JOINTS];
  float cmd_kp[skynet_unitree::MAX_JOINTS];
  float cmd_kd[skynet_unitree::MAX_JOINTS];
  std::mutex lock;
  std::uint64_t commands = 0;
  std::uint64_t bad_crc = 0;
  std::uint64_t zero_where_sentinel = 0;
  std::uint64_t phantom_slot = 0;
  std::uint64_t idle_mode = 0;

  explicit Robot(const skynet_unitree::WireModel *model) : wire(model) {
    /* A different, non-zero angle on every joint: a body that starts at zero would make a driver
     * writing zeros look correct, and one whose joints share a value would make a driver that read
     * a neighbour's slot look correct too. Not a stance anyone would stand in. */
    for (int i = 0; i < wire->joints; ++i) {
      q[i] = 0.013f * static_cast<float>(i + 1);
      dq[i] = 0.0f;
      tau[i] = 0.0f;
      cmd_q[i] = POS_STOP_F;
      cmd_dq[i] = VEL_STOP_F;
      cmd_tau[i] = 0.0f;
      cmd_kp[i] = 0.0f;
      cmd_kd[i] = 0.0f;
    }
  }

  bool slot_used(std::size_t slot) const {
    for (int i = 0; i < wire->joints; ++i) {
      if (wire->slots[i] == slot) return true;
    }
    return false;
  }

  int joint_at(std::size_t slot) const {
    for (int i = 0; i < wire->joints; ++i) {
      if (wire->slots[i] == slot) return i;
    }
    return -1;
  }

  template <typename Cmd>
  void apply(const Cmd &cmd, std::size_t slots) {
    std::lock_guard<std::mutex> guard(lock);
    const std::uint32_t expected =
        crc32_core(reinterpret_cast<const std::uint32_t *>(&cmd), (sizeof(cmd) >> 2) - 1);
    if (cmd.crc() != expected) {
      /* Dropped, exactly as the robot drops it, and counted so the suite can say the driver's
       * checksum is right rather than assuming it. */
      bad_crc += 1;
      return;
    }
    commands += 1;
    bool idle = false;
    for (std::size_t slot = 0; slot < slots; ++slot) {
      const auto &motor = cmd.motor_cmd()[slot];
      /* Mode 0 is idle: a drive told to be idle is unpowered, whatever else the command says. */
      if (motor.mode() != 1) idle = true;
      if (!slot_used(slot)) {
        const bool commands_position = motor.q() != POS_STOP_F && motor.kp() != 0.0f;
        const bool commands_torque = motor.tau() != 0.0f;
        if (commands_position || commands_torque) phantom_slot += 1;
        continue;
      }
      const int joint = joint_at(slot);
      /* The defect this substrate exists to catch. `q = 0` with a stiffness is a command to move
       * to zero radians; a driver meaning "say nothing about position" writes `PosStopF`. Only a
       * zero that would pull the joint somewhere counts: §24's hold latches where a joint is
       * measured (F121), and a hold at zero on a joint that is at zero moves nothing. */
      if (motor.q() == 0.0f && motor.kp() != 0.0f && std::fabs(q[joint]) > 1e-3f) {
        zero_where_sentinel += 1;
      }
      if (motor.dq() == 0.0f && motor.kd() != 0.0f && motor.q() == POS_STOP_F) {
        zero_where_sentinel += 1;
      }
      cmd_q[joint] = motor.q();
      cmd_dq[joint] = motor.dq();
      cmd_tau[joint] = motor.tau();
      cmd_kp[joint] = motor.kp();
      cmd_kd[joint] = motor.kd();
    }
    if (idle) idle_mode += 1;
  }

  void integrate(float dt) {
    std::lock_guard<std::mutex> guard(lock);
    for (int i = 0; i < wire->joints; ++i) {
      /* The impedance law, with the sentinels meaning what Unitree says they mean: a `PosStopF`
       * position or a `VelStopF` velocity says nothing about that term. */
      const float position_term = cmd_q[i] == POS_STOP_F ? 0.0f : cmd_kp[i] * (cmd_q[i] - q[i]);
      const float velocity_term = cmd_dq[i] == VEL_STOP_F ? 0.0f : cmd_kd[i] * (cmd_dq[i] - dq[i]);
      tau[i] = cmd_tau[i] + position_term + velocity_term;
      /* First-order, unit inertia, a little damping. Not a model of a leg — a body that moves when
       * commanded and settles when the command is met, so that a driver's setpoint is visibly the
       * thing that moved it. */
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
  const std::string model = argc > 4 ? argv[4] : "go2";
  /* `<joint>=<value>`: one joint's trouble, as the maker's own field carries it — `lost` on the go2
   * message, `motorstate` on the hg one — so the suite can see what the driver makes of a robot that
   * reports one (F205). Absent: nothing is wrong, as before. */
  int fault_joint = -1;
  std::uint32_t fault_value = 0;
  if (argc > 5) {
    char *rest = nullptr;
    fault_joint = static_cast<int>(std::strtol(argv[5], &rest, 10));
    if (rest == nullptr || *rest != '=' || fault_joint < 0) {
      std::fprintf(stderr, "fake_robot: the fifth argument is <joint>=<value> (got \"%s\")\n", argv[5]);
      return 2;
    }
    fault_value = static_cast<std::uint32_t>(std::strtoul(rest + 1, nullptr, 10));
  }

  const skynet_unitree::WireModel *wire = skynet_unitree::find_wire_model(model);
  if (wire == nullptr) {
    std::fprintf(stderr, "fake_robot: unknown model \"%s\" (go2, b2, h1, g1, h1_2, h2, r1)\n", model.c_str());
    return 2;
  }

  std::signal(SIGINT, on_signal);
  std::signal(SIGTERM, on_signal);

  unitree::robot::ChannelFactory::Instance()->Init(0, interface);

  if (wire->family == skynet_unitree::Family::GO2) {
    auto publisher = std::make_shared<
        unitree::robot::ChannelPublisher<unitree_go::msg::dds_::LowState_>>("rt/lowstate");
    publisher->InitChannel();

    Robot robot(wire);
    auto subscriber = std::make_shared<
        unitree::robot::ChannelSubscriber<unitree_go::msg::dds_::LowCmd_>>("rt/lowcmd");
    subscriber->InitChannel(
        [&robot](const void *message) {
          robot.apply(*static_cast<const unitree_go::msg::dds_::LowCmd_ *>(message),
                      skynet_unitree::GO2_SLOTS);
        },
        1);

    std::fprintf(stderr,
                 "fake-%s: publishing rt/lowstate on `%s` at %g Hz. This is not a robot: no "
                 "dynamics, no contact, no safety hardware, and the rate is nominal.\n",
                 model.c_str(), interface.c_str(), hz);

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
        for (int i = 0; i < wire->joints; ++i) {
          const std::size_t slot = wire->slots[i];
          state.motor_state()[slot].q() = robot.q[i];
          state.motor_state()[slot].dq() = robot.dq[i] + 0.03f * static_cast<float>(i + 1);
          state.motor_state()[slot].tau_est() = robot.tau[i] + 0.05f * static_cast<float>(i + 1);
          state.motor_state()[slot].temperature() = 30;
          state.motor_state()[slot].lost() = i == fault_joint ? fault_value : 0;
        }
      }
      state.crc() =
          crc32_core(reinterpret_cast<const std::uint32_t *>(&state), (sizeof(state) >> 2) - 1);
      publisher->Write(state);
      std::this_thread::sleep_for(period);
      if (seconds > 0.0 &&
          std::chrono::steady_clock::now() - started > std::chrono::duration<double>(seconds)) {
        break;
      }
    }
    std::lock_guard<std::mutex> guard(robot.lock);
    std::printf("{\"commands\":%llu,\"bad_crc\":%llu,\"zero_where_sentinel\":%llu,"
                "\"phantom_slot\":%llu,\"idle_mode\":%llu}\n",
                (unsigned long long)robot.commands, (unsigned long long)robot.bad_crc,
                (unsigned long long)robot.zero_where_sentinel,
                (unsigned long long)robot.phantom_slot, (unsigned long long)robot.idle_mode);
    return 0;
  }

  auto publisher = std::make_shared<
      unitree::robot::ChannelPublisher<unitree_hg::msg::dds_::LowState_>>("rt/lowstate");
  publisher->InitChannel();

  Robot robot(wire);
  auto subscriber = std::make_shared<
      unitree::robot::ChannelSubscriber<unitree_hg::msg::dds_::LowCmd_>>("rt/lowcmd");
  subscriber->InitChannel(
      [&robot](const void *message) {
        robot.apply(*static_cast<const unitree_hg::msg::dds_::LowCmd_ *>(message),
                    skynet_unitree::HG_SLOTS);
      },
      1);

  std::fprintf(stderr,
               "fake-%s: publishing rt/lowstate on `%s` at %g Hz. This is not a robot: no "
               "dynamics, no contact, no safety hardware, and the rate is nominal.\n",
               model.c_str(), interface.c_str(), hz);

  /* The machine code, such as it is: 4 for the 23-DoF G1 (Unitree's own simulator's value), 0
   * for the H1-2 (unpublished — see the file header). */
  const std::uint8_t mode_machine = model == "g1" ? 4 : 0;
  const auto period = std::chrono::duration<double>(1.0 / hz);
  const auto started = std::chrono::steady_clock::now();
  std::uint32_t tick = 0;
  while (RUNNING) {
    robot.integrate(static_cast<float>(1.0 / hz));
    unitree_hg::msg::dds_::LowState_ state{};
    state.mode_machine() = mode_machine;
    state.tick() = tick++;
    {
      std::lock_guard<std::mutex> guard(robot.lock);
      for (int i = 0; i < wire->joints; ++i) {
        const std::size_t slot = wire->slots[i];
        state.motor_state()[slot].q() = robot.q[i];
        state.motor_state()[slot].dq() = robot.dq[i] + 0.03f * static_cast<float>(i + 1);
        state.motor_state()[slot].tau_est() = robot.tau[i] + 0.05f * static_cast<float>(i + 1);
        state.motor_state()[slot].motorstate() = i == fault_joint ? fault_value : 0;
      }
    }
    state.crc() =
        crc32_core(reinterpret_cast<const std::uint32_t *>(&state), (sizeof(state) >> 2) - 1);
    publisher->Write(state);
    std::this_thread::sleep_for(period);
    if (seconds > 0.0 &&
        std::chrono::steady_clock::now() - started > std::chrono::duration<double>(seconds)) {
      break;
    }
  }

  std::lock_guard<std::mutex> guard(robot.lock);
  std::printf("{\"commands\":%llu,\"bad_crc\":%llu,\"zero_where_sentinel\":%llu,"
              "\"phantom_slot\":%llu,\"idle_mode\":%llu}\n",
              (unsigned long long)robot.commands, (unsigned long long)robot.bad_crc,
              (unsigned long long)robot.zero_where_sentinel,
              (unsigned long long)robot.phantom_slot, (unsigned long long)robot.idle_mode);
  return 0;
}
