/*
 * skynet-unitree against Unitree's own simulator: the driver commanding a body with physics behind
 * it. DRY-COVERAGE-PLAN.md Phase 8, item 8.1, and F119.
 *
 * `selftest.cpp` holds the driver to `fake_robot`, which counts what a message *says* — the CRC,
 * the sentinels, the slots. This holds it to `unitree_mujoco`, the vendor's simulator, which does
 * what a message *means*: its bridge applies Unitree's impedance law, `tau + kp·(q − q̂) +
 * kd·(dq − dq̂)`, to MuJoCo actuators in the vendor's own models, under gravity and contact. So the
 * questions here are the ones a counter cannot answer:
 *
 *   * does the slot map agree with the vendor's model, joint by joint, by **name**? The model's
 *     actuator order is read out of the XML by the caller and handed in, so this is a second
 *     reading of the wire truth that `unitree_models.hpp` did not write;
 *   * does a Go2 **stand** when the driver commands the vendor's own stand pose with the vendor's
 *     own gains, and does a humanoid's arm go where it is sent;
 *   * does §24's **hold** — the all-zero mask the kernel writes whenever it has nothing to forward
 *     (`skynet-kernel-host`'s `cycle.rs`) — hold, measured by the body not moving;
 *   * does the **emergency stop** do what the driver says it does, measured as the body giving way.
 *
 * Measured by what the simulator publishes, not by what the driver reports: base height is read off
 * `rt/sportmodestate`, which the simulator fills from its own `frame_pos` sensor and the driver
 * never touches. Joint positions come back through `read_state`, because that is the path a policy
 * sees — and the name check above is what keeps that from being the driver grading itself.
 *
 * What this cannot say (Phase 8's list): timing at rate, firmware, the robot's own safety
 * controller, or whether the gains that stand a simulated Go2 stand a real one.
 *
 * Linked against `unitree_bridge.cpp` rather than dlopening it, the way `selftest.cpp` is: the
 * calls go through the same C vtable either way, and one copy of the SDK in the process is simpler
 * than two.
 */

#include "skynet_rhal.h"
#include "unitree_models.hpp"

#include <unitree/idl/go2/SportModeState_.hpp>
#include <unitree/robot/channel/channel_subscriber.hpp>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

using Clock = std::chrono::steady_clock;

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

void section(const char *title) { std::printf("\n%s\n", title); }

std::string metres(double value) {
  char buffer[32];
  std::snprintf(buffer, sizeof buffer, "%.3f m", value);
  return buffer;
}

std::string radians(double value) {
  char buffer[32];
  std::snprintf(buffer, sizeof buffer, "%.3f rad", value);
  return buffer;
}

/* One row per body. The gains are the vendor's, from its own low-level examples, because a
 * stiffness is the operator's to choose (§63.4) and the most defensible operator here is Unitree:
 *
 *   go2  — kp 50, kd 3.5: `unitree_mujoco/example/cpp/stand_go2.cpp`, the stand phase.
 *   b2   — kp 1000, kd 10: `unitree_sdk2/example/b2/b2_stand_example.cpp`, whose `Kp` and `Kd` it
 *          applies to every joint of every phase. Twenty and three times the Go2's, because a
 *          machine this size carries a load a stiffness of 50 would let sag to the floor; the
 *          simulator's `b2.xml` clips each drive at +-200 or +-300 N.m, so a stand whose ramp asks
 *          for more is a clipped command and not an error.
 *   g1   — kp 40, kd 1: `unitree_sdk2/example/g1/low_level/g1_ankle_swing_example.cpp`, the arm
 *          row, and the arm is what moves here.
 *   h1, h1_2 — kp 80, kd 2: `h1_27dof_example.cpp`'s `GearboxS`, the smallest gearbox's.
 *   r1   — kp 100, kd 2: the arm rows of `example/r1/low_level/r1_ankle_swing_example.cpp`'s `Kp` and
 *          `Kd` (the shoulder roll's), the joint that moves here.
 *   h2   — kp 60, kd 1: `example/h2/low_level/h2_ankle_swing_example.cpp`'s `Kp` and `Kd` at the
 *          left shoulder roll, the joint that moves here (the example gives every joint its own pair).
 *
 * The driver takes one pair for every joint, so each row applies the vendor's figure for the
 * joints the check moves to all of them. On a humanoid hanging from the simulator's elastic band
 * that is enough to hold the legs still; it is not a claim about standing one up. */
struct Body {
  const char *model;
  double kp;
  double kd;
  /* True for the humanoids: the simulator's elastic band (its `enable_elastic_band`, "used for
   * lifting h1") holds the torso, and the proof is an arm. False for the Go2, which stands. */
  bool on_band;
  /* The joint the humanoid proof moves, by the driver's own name, and where it is sent. The
   * shoulder *roll* at +1.2 rad lifts the left arm out sideways on all three bodies (each one's
   * left roll range is positive outward). Sideways rather than forward because a body hanging
   * from the band tilts forward by an amount that varies run to run, and a forward-raised arm on a
   * tilted body can hang almost straight down: the first runs used the shoulder pitch, and the
   * H1-2's arm, on a joint its model damps twenty times harder than the G1's, fell 0.32 rad after
   * a stop in one run and 0.095 in the next. A tilt about the lateral axis leaves a sideways arm
   * sideways, so gravity keeps its leverage — and the check asserts that load before relying on
   * it, rather than assuming it. */
  const char *arm_joint;
  double arm_target;
  /* The quadrupeds only: what the body is called in the messages below, and the pose the vendor's
   * own example stands it in, in the driver's joint order. */
  const char *label;
  const float *stand;
  /* The humanoids only: how long the arm is held at its target before the hold below is measured.
   * The drive is a spring and a weak damper, so an arm rings before it rests, and the hold check
   * measures sag from where the arm is when it begins: a second was enough for the first three
   * bodies, and the H2's heavier arm at its vendor gains (kp 60, kd 1) moved 0.056 rad in the
   * hold that followed it, against the 0.05 that hold allows. */
  int settle_ms;
  /* Joints the vendor's model does not carry at all, by the driver's name: the R1's head. A joint is
   * excused from the by-name slot check only if it is listed here *and* its slot is beyond the
   * model's actuators, so the exemption cannot hide a joint the model does have. */
  const char *unmodelled[2];
};

/* The vendor's Go2 stand pose, `stand_up_joint_pos` in `stand_go2.cpp`, in the driver's joint order
 * (which is the robot's: FR, FL, RR, RL, each hip/thigh/calf). */
constexpr float GO2_STAND[12] = {0.00571868f, 0.608813f, -1.21763f, -0.00571868f,
                                 0.608813f,   -1.21763f, 0.00571868f, 0.608813f,
                                 -1.21763f,   -0.00571868f, 0.608813f, -1.21763f};

/* The vendor's B2 stand pose: `_targetPos_2` in `b2_stand_example.cpp`, the example's standing
 * phase (the one before it, `_targetPos_1`, is the crouch it starts from), in the same leg order. */
constexpr float B2_STAND[12] = {0.0f, 0.67f, -1.3f, 0.0f, 0.67f, -1.3f,
                                0.0f, 0.67f, -1.3f, 0.0f, 0.67f, -1.3f};

const Body BODIES[] = {
    {"go2", 50.0, 3.5, false, nullptr, 0.0, "Go2", GO2_STAND, 0, {nullptr, nullptr}},
    {"b2", 1000.0, 10.0, false, nullptr, 0.0, "B2", B2_STAND, 0, {nullptr, nullptr}},
    {"h1", 80.0, 2.0, true, "L_shoulder_roll", 1.2, nullptr, nullptr, 1000, {nullptr, nullptr}},
    {"g1", 40.0, 1.0, true, "L_shoulder_roll", 1.2, nullptr, nullptr, 1000, {nullptr, nullptr}},
    {"h1_2", 80.0, 2.0, true, "L_shoulder_roll", 1.2, nullptr, nullptr, 1000, {nullptr, nullptr}},
    {"h2", 60.0, 1.0, true, "L_shoulder_roll", 1.2, nullptr, nullptr, 3000, {nullptr, nullptr}},
    {"r1", 100.0, 2.0, true, "L_shoulder_roll", 1.2, nullptr, nullptr, 1000, {"head_pitch", "head_yaw"}},
};

/* 500 Hz, the Go2's documented low-level rate, for every body. The humanoids' rate is unpublished
 * (the driver says so in its descriptor), and the simulator's bridge runs at 1 kHz either way. */
constexpr auto PERIOD = std::chrono::microseconds(2000);

class BaseHeight {
 public:
  explicit BaseHeight(const std::string &interface) {
    unitree::robot::ChannelFactory::Instance()->Init(0, interface);
    subscriber_.reset(
        new unitree::robot::ChannelSubscriber<unitree_go::msg::dds_::SportModeState_>(
            "rt/sportmodestate"));
    subscriber_->InitChannel(
        [this](const void *message) {
          const auto *state = static_cast<const unitree_go::msg::dds_::SportModeState_ *>(message);
          std::lock_guard<std::mutex> guard(lock_);
          z_ = state->position()[2];
          seen_ += 1;
        },
        1);
  }
  bool wait(std::chrono::milliseconds budget) {
    const auto deadline = Clock::now() + budget;
    while (Clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (seen_ > 0) return true;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    return false;
  }
  double z() {
    std::lock_guard<std::mutex> guard(lock_);
    return z_;
  }

 private:
  std::mutex lock_;
  double z_ = std::nan("");
  std::uint64_t seen_ = 0;
  unitree::robot::ChannelSubscriberPtr<unitree_go::msg::dds_::SportModeState_> subscriber_;
};

/* The driver behind its vtable, with the buffers a runtime would own. */
class Session {
 public:
  Session(const skr_driver *driver, skr_ctx *ctx, int joints)
      : driver_(driver), ctx_(ctx), joints_(joints) {}

  bool read(float *position) {
    float effort[skynet_unitree::MAX_JOINTS];
    return read(position, effort);
  }

  /* `effort` is the drive's `tau_est`, which unitree_mujoco fills from each actuator's own force
   * sensor (`jointactuatorfrc`) — the torque the simulated drive is applying, not the driver's
   * opinion of it. */
  bool read(float *position, float *effort) {
    float velocity[skynet_unitree::MAX_JOINTS];
    skr_state state;
    std::memset(&state, 0, sizeof state);
    state.joint_count = static_cast<std::uint16_t>(skynet_unitree::MAX_JOINTS);
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    return driver_->read_state(ctx_, &state, 0) == SKR_OK;
  }

  int send(std::uint8_t mode, const float *values, const std::uint8_t *mask) {
    skr_command cmd;
    std::memset(&cmd, 0, sizeof cmd);
    cmd.seq = ++seq_;
    cmd.mode = mode;
    cmd.joint_count = static_cast<std::uint16_t>(joints_);
    cmd.values = values;
    cmd.mask = mask;
    return driver_->write_command(ctx_, &cmd, 0);
  }

  /* One command a period for `duration`, the way the kernel's loop writes them. `frame` fills the
   * values and the mask for the elapsed fraction of the window. The first refusal ends the window
   * and is returned, because a driver that refused a command mid-motion must not be read as a
   * body that moved. */
  template <typename Frame>
  int drive(std::chrono::milliseconds duration, Frame frame) {
    float values[skynet_unitree::MAX_JOINTS];
    std::uint8_t mask[skynet_unitree::MAX_JOINTS];
    const auto start = Clock::now();
    auto next = start;
    while (Clock::now() - start < duration) {
      const double fraction =
          std::chrono::duration<double>(Clock::now() - start).count() /
          std::chrono::duration<double>(duration).count();
      frame(std::min(fraction, 1.0), values, mask);
      const int result = send(SKR_MODE_POSITION, values, mask);
      if (result != SKR_OK) return result;
      next += PERIOD;
      std::this_thread::sleep_until(next);
    }
    return SKR_OK;
  }

 private:
  const skr_driver *driver_;
  skr_ctx *ctx_;
  int joints_;
  std::uint32_t seq_ = 0;
};

/* The driver's joint names, in its order, read out of its own descriptor. */
std::vector<std::string> descriptor_names(const std::string &descriptor) {
  std::vector<std::string> names;
  const std::string key = "\"name\":\"";
  std::size_t at = descriptor.find("\"joints\":[");
  while (at != std::string::npos) {
    at = descriptor.find(key, at);
    if (at == std::string::npos) break;
    const std::size_t start = at + key.size();
    const std::size_t end = descriptor.find('"', start);
    if (end == std::string::npos) break;
    names.push_back(descriptor.substr(start, end - start));
    at = end;
  }
  return names;
}

/* The driver's short names against the vendor's XML joint names: `L_`/`R_` are `left_`/`right_`,
 * the XML appends `_joint`, and the H1 family calls the waist `torso`. Anything else is compared as
 * spelled, so a joint this does not know how to translate fails rather than matches. */
std::string vendor_spelling(std::string name) {
  if (name.rfind("L_", 0) == 0) name = "left_" + name.substr(2);
  if (name.rfind("R_", 0) == 0) name = "right_" + name.substr(2);
  return name + "_joint";
}

bool same_joint(const std::string &driver_name, const std::string &xml_joint) {
  if (vendor_spelling(driver_name) == xml_joint) return true;
  return driver_name == "waist_yaw" && xml_joint == "torso_joint";
}

/* One motor slot as the vendor's model wires it: the joint its actuator drives, and the joints its
 * position, velocity and torque sensors measure. `run-sim-check.sh` reads these out of the XML. */
struct Slot {
  std::string actuator;
  std::string position;
  std::string velocity;
  std::string torque;
};

std::vector<Slot> read_wiring(const std::string &path) {
  std::vector<Slot> slots;
  std::ifstream file(path);
  std::string line;
  while (std::getline(file, line)) {
    if (line.empty()) continue;
    Slot slot;
    std::size_t start = 0;
    std::string *fields[4] = {&slot.actuator, &slot.position, &slot.velocity, &slot.torque};
    for (std::string *field : fields) {
      const std::size_t tab = line.find('\t', start);
      *field = line.substr(start, tab == std::string::npos ? std::string::npos : tab - start);
      start = tab == std::string::npos ? line.size() : tab + 1;
    }
    slots.push_back(slot);
  }
  return slots;
}

int index_of(const std::vector<std::string> &names, const char *name) {
  for (std::size_t i = 0; i < names.size(); ++i) {
    if (names[i] == name) return static_cast<int>(i);
  }
  return -1;
}

double max_deviation(const float *a, const float *b, int count) {
  double worst = 0.0;
  for (int i = 0; i < count; ++i) worst = std::max(worst, std::fabs(double(a[i]) - double(b[i])));
  return worst;
}

double max_magnitude(const float *values, int count) {
  double worst = 0.0;
  for (int i = 0; i < count; ++i) worst = std::max(worst, std::fabs(double(values[i])));
  return worst;
}

std::string newton_metres(double value) {
  char buffer[32];
  std::snprintf(buffer, sizeof buffer, "%.3f N·m", value);
  return buffer;
}

/* After a stop, the simulator's bridge computes every actuator's command as `0 + 0·(PosStopF − q̂)
 * + 0·(VelStopF − dq̂)`, which is exactly zero — so anything the force sensors read above this is
 * a drive still being commanded. */
constexpr double ZERO_TORQUE = 1e-6;

int run(const Body &body, const skynet_unitree::WireModel *wire, const std::string &interface,
        const std::vector<Slot> &actuators) {
  const skr_driver *driver = skynet_rhal_entry();
  const int joints = wire->joints;
  /* Never destroyed, for the driver's own reason (`Link` in `unitree_bridge.cpp`, F122): a reader
   * torn down while the simulator still publishes can take CycloneDDS's self-deadlock abort. */
  BaseHeight &base = *new BaseHeight(interface);

  char config[256];
  std::snprintf(config, sizeof config,
                "{\"interface\":\"%s\",\"model\":\"%s\",\"kp\":%.1f,\"kd\":%.1f,"
                "\"simulator\":true}",
                interface.c_str(), body.model, body.kp, body.kd);
  skr_ctx *ctx = nullptr;
  section("the simulator is the robot's interface");
  check(driver->open(config, &ctx) == SKR_OK,
        "open() finds unitree_mujoco's LowState_ on DDS, the vendor's simulator speaking the "
        "robot's own topics");
  if (ctx == nullptr) return 1;
  check(base.wait(std::chrono::milliseconds(3000)),
        "and its rt/sportmodestate, which is where the base height below is read");

  char json[16384];
  const int written = driver->describe(ctx, json, sizeof json);
  const std::vector<std::string> names =
      descriptor_names(std::string(json, written > 0 ? written : 0));
  check(static_cast<int>(names.size()) == joints, "the descriptor names every joint");
  check(static_cast<int>(actuators.size()) >= joints,
        "the vendor's model has at least as many actuators as the body has joints");
  bool agrees = static_cast<int>(names.size()) == joints;
  for (int i = 0; agrees && i < joints; ++i) {
    const std::size_t slot = wire->slots[i];
    bool unmodelled = false;
    for (const char *name : body.unmodelled) {
      if (name != nullptr && names[i] == name && slot >= actuators.size()) unmodelled = true;
    }
    if (unmodelled) {
      std::printf("  note  %s (slot %zu) is not in the vendor's model, so the simulator cannot hold it to "
                  "a name and nothing below says anything about it\n",
                  names[i].c_str(), slot);
      continue;
    }
    if (slot >= actuators.size() || !same_joint(names[i], actuators[slot].actuator)) {
      std::printf("        joint %d \"%s\" is in slot %zu, which the vendor's model calls \"%s\"\n",
                  i, names[i].c_str(), slot,
                  slot < actuators.size() ? actuators[slot].actuator.c_str() : "(no actuator)");
      agrees = false;
    }
  }
  check(agrees,
        "every joint's slot is the actuator of the same name in the vendor's model — the slot "
        "map, held to a second reading of the wire that this repository did not write");
  if (!agrees) return 1;

  /* The substrate, checked before it is believed. The simulator publishes slot i's state from
   * sensors i, i+n and i+2n, so a sensor wired to another joint makes the simulator report that
   * joint's reading as this one's. Position and velocity are what every motion below is judged
   * by, so a misplaced one fails the run; a misplaced torque sensor is named, and every torque
   * read below is looked up by the joint the sensor measures rather than by slot (F123). */
  section("the vendor's model, read before it is trusted");
  bool kinematics_wired = true;
  std::vector<std::string> torque_misplaced;
  for (std::size_t slot = 0; slot < actuators.size(); ++slot) {
    const Slot &wiring = actuators[slot];
    if (wiring.position != wiring.actuator || wiring.velocity != wiring.actuator) {
      std::printf("        slot %zu (%s) reports position from %s and velocity from %s\n", slot,
                  wiring.actuator.c_str(), wiring.position.c_str(), wiring.velocity.c_str());
      kinematics_wired = false;
    }
    if (wiring.torque != wiring.actuator) {
      torque_misplaced.push_back("slot " + std::to_string(slot) + " " + wiring.actuator +
                                 " reports " + wiring.torque + "'s torque");
    }
  }
  check(kinematics_wired,
        "every slot's position and velocity sensors measure that slot's own joint, so the motion "
        "below is read off the joint it was commanded on");
  if (!kinematics_wired) return 1;
  if (torque_misplaced.empty()) {
    std::printf("  note  every torque sensor measures its own slot's joint\n");
  } else {
    std::printf("  note  the vendor's model wires %zu torque sensors to another slot's joint, so "
                "the simulator publishes those `tau_est` values under the wrong motor (F123):\n",
                torque_misplaced.size());
    for (const std::string &line : torque_misplaced) std::printf("          %s\n", line.c_str());
  }

  /* Where joint `i`'s true torque is in a `read_state` effort array: the driver joint whose slot's
   * torque sensor measures joint `i`'s actuator. -1 when no driver joint's slot carries it. */
  auto torque_index = [&](int joint) {
    const std::string &target = actuators[wire->slots[joint]].actuator;
    for (int k = 0; k < joints; ++k) {
      if (actuators[wire->slots[k]].torque == target) return k;
    }
    return -1;
  };

  Session session(driver, ctx, joints);
  float rest[skynet_unitree::MAX_JOINTS];
  float now[skynet_unitree::MAX_JOINTS];

  /* Nothing commanded yet: the simulator's LowCmd_ starts at every gain zero, so the body is
   * limp and lands where gravity puts it. */
  std::this_thread::sleep_for(std::chrono::milliseconds(1500));
  check(session.read(rest), "read_state returns the settled body");

  if (!body.on_band) {
    section("gravity, contact and a stand, commanded through the driver");
    const double lying = base.z();
    check(lying < 0.25,
          "limp from the spawn height, the body is on the ground (base " + metres(lying) + ")");

    /* The vendor's stand: from wherever the legs are to `stand_up_joint_pos`, then held. */
    int result = session.drive(std::chrono::milliseconds(1500),
                               [&](double t, float *values, std::uint8_t *mask) {
                                 for (int i = 0; i < joints; ++i) {
                                   values[i] = float(rest[i] + (body.stand[i] - rest[i]) * t);
                                   mask[i] = 1;
                                 }
                               });
    if (result == SKR_OK) {
      result = session.drive(std::chrono::milliseconds(1000),
                             [&](double, float *values, std::uint8_t *mask) {
                               for (int i = 0; i < joints; ++i) {
                                 values[i] = body.stand[i];
                                 mask[i] = 1;
                               }
                             });
    }
    check(result == SKR_OK, "every command of the stand was accepted");
    const double standing = base.z();
    session.read(now);
    const double stand_error = max_deviation(now, body.stand, joints);
    check(standing > lying + 0.1,
          std::string("the ") + body.label + " stood up: base " + metres(lying) + " → " + metres(standing));
    /* Not zero, and not meant to be: a PD drive holds a loaded joint at the offset where the
     * stiffness balances the load, and kp 50 against the ~10 N·m a calf carries under the Go2's
     * weight is ~0.2 rad. The first run measured 0.181. A joint further off than 0.25 did not
     * reach the pose. */
    check(stand_error < 0.25,
          "and every joint reached the vendor's stand pose, within the steady-state error the "
          "vendor's own gains leave under the body's weight (worst " + radians(stand_error) + ")");

    section("§24's hold: the all-zero mask the kernel writes when it has nothing to forward");
    session.read(rest);
    double lowest = standing;
    result = session.drive(std::chrono::milliseconds(2000),
                           [&](double, float *values, std::uint8_t *mask) {
                             lowest = std::min(lowest, base.z());
                             for (int i = 0; i < joints; ++i) {
                               /* The kernel's hold frame: values irrelevant, mask all zero. */
                               values[i] = 0.0f;
                               mask[i] = 0;
                             }
                           });
    check(result == SKR_OK, "two seconds of holds are accepted");
    session.read(now);
    const double drift = max_deviation(now, rest, joints);
    check(lowest > standing - 0.03,
          "the body is held, not dropped: lowest base " + metres(lowest) + " against " +
              metres(standing) + " standing");
    check(drift < 0.05, "and no joint moved (worst " + radians(drift) + ")");

    section("the emergency stop, measured as the body giving way");
    check(driver->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
    const double before = base.z();
    std::this_thread::sleep_for(std::chrono::milliseconds(1500));
    const double after = base.z();
    float effort[skynet_unitree::MAX_JOINTS];
    session.read(now, effort);
    check(max_magnitude(effort, joints) < ZERO_TORQUE,
          "every drive's output is zero, read off the simulator's actuator force sensors (largest " +
              newton_metres(max_magnitude(effort, joints)) + ")");
    check(after < before - 0.08,
          std::string("and the ") + body.label + " folded: base " + metres(before) + " → " + metres(after) +
              " — the stop the descriptor declares is the one physics sees");
  } else {
    section("an arm, commanded through the driver, on a body the elastic band holds up");
    const int arm = index_of(names, body.arm_joint);
    check(arm >= 0, std::string("the driver has a joint named ") + body.arm_joint);
    if (arm < 0) return 1;

    float target[skynet_unitree::MAX_JOINTS];
    std::memcpy(target, rest, sizeof(float) * joints);
    target[arm] = float(body.arm_target);
    int result = session.drive(std::chrono::milliseconds(1000),
                               [&](double t, float *values, std::uint8_t *mask) {
                                 for (int i = 0; i < joints; ++i) {
                                   values[i] = float(rest[i] + (target[i] - rest[i]) * t);
                                   mask[i] = 1;
                                 }
                               });
    if (result == SKR_OK) {
      result = session.drive(std::chrono::milliseconds(body.settle_ms),
                             [&](double, float *values, std::uint8_t *mask) {
                               for (int i = 0; i < joints; ++i) {
                                 values[i] = target[i];
                                 mask[i] = 1;
                               }
                             });
    }
    check(result == SKR_OK, "every command of the motion was accepted");
    float effort[skynet_unitree::MAX_JOINTS];
    session.read(now, effort);
    const double reached = now[arm];
    /* The precondition that makes the hold and the stop below mean anything: an arm that carries
     * no load stays put whether it is held or free. Read by the joint the sensor measures, since
     * the H1-2's model wires this very joint's sensor to its neighbour (F123). */
    const int arm_torque = torque_index(arm);
    check(arm_torque >= 0, std::string("the vendor's model measures ") + body.arm_joint +
                               "'s torque on some slot the body has");
    if (arm_torque < 0) return 1;
    /* A PD drive holding a load stands off its setpoint by the load over its stiffness: that is the
     * law the simulator applies, so it is the bound, with 0.05 rad of slack for what one torque
     * reading does not account for. The 0.15 rad the lighter arms (the H1, G1 and H1-2) were held to stays the floor,
     * so no body that passed is held to less. The H2's arm is heavier and its vendor gain at this
     * joint is 60: it stood 0.21 rad off a 1.2 rad setpoint, which is the load the force sensor
     * reads over 60 and not a drive that failed to move. The bound is printed so a reader sees it. */
    const double load = std::fabs(double(effort[arm_torque]));
    const double allowed = std::max(0.15, load / body.kp + 0.05);
    check(std::fabs(reached - body.arm_target) < allowed,
          std::string(body.arm_joint) + " went where it was sent: " + radians(reached) +
              " against " + radians(body.arm_target) + ", within " + radians(allowed) +
              " (the larger of 0.15 and the load " + newton_metres(load) + " over kp " +
              std::to_string(int(body.kp)) + ", plus 0.05)");
    check(std::fabs(reached - rest[arm]) > 0.5,
          "and that is a motion, not where it already was (" + radians(rest[arm]) + " at rest)");
    check(std::fabs(effort[arm_torque]) > 1.0,
          "the raised arm is carrying its own weight: the drive is applying " +
              newton_metres(std::fabs(effort[arm_torque])) + " to hold it there");

    section("§24's hold: the all-zero mask the kernel writes when it has nothing to forward");
    session.read(rest);
    double sag = 0.0;
    result = session.drive(std::chrono::milliseconds(2000),
                           [&](double, float *values, std::uint8_t *mask) {
                             float live[skynet_unitree::MAX_JOINTS];
                             if (session.read(live)) {
                               sag = std::max(sag, std::fabs(double(live[arm]) - rest[arm]));
                             }
                             for (int i = 0; i < joints; ++i) {
                               values[i] = 0.0f;
                               mask[i] = 0;
                             }
                           });
    check(result == SKR_OK, "two seconds of holds are accepted");
    check(sag < 0.05,
          "the raised arm is held against gravity, not dropped (worst " + radians(sag) + ")");

    section("the emergency stop, measured as the arm giving way");
    session.read(rest);
    check(driver->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
    std::this_thread::sleep_for(std::chrono::milliseconds(1500));
    session.read(now, effort);
    check(max_magnitude(effort, joints) < ZERO_TORQUE,
          "every drive's output is zero, read off the simulator's actuator force sensors (largest " +
              newton_metres(max_magnitude(effort, joints)) + ")");
    check(std::fabs(double(now[arm]) - rest[arm]) > 0.3,
          "and the arm fell: " + radians(rest[arm]) + " → " + radians(now[arm]));
  }

  float values[skynet_unitree::MAX_JOINTS] = {0};
  std::uint8_t mask[skynet_unitree::MAX_JOINTS];
  std::fill(mask, mask + joints, std::uint8_t{1});
  check(session.send(SKR_MODE_POSITION, values, mask) == SKR_EPERM,
        "and the latch refuses every command after it");
  driver->close(ctx);
  return 0;
}

}  // namespace

int main(int argc, char **argv) {
  if (argc < 4) {
    std::fprintf(stderr,
                 "usage: sim_check <interface> <model> <wiring-file>\n"
                 "  <wiring-file> has one line per motor slot of the vendor's model: the joint the\n"
                 "  actuator drives, then the joints its position, velocity and torque sensors\n"
                 "  measure, tab-separated — run-sim-check.sh writes it from the XML.\n");
    return 2;
  }
  const std::string interface = argv[1];
  const std::string model = argv[2];
  const std::vector<Slot> actuators = read_wiring(argv[3]);

  const skynet_unitree::WireModel *wire = skynet_unitree::find_wire_model(model);
  const Body *body = nullptr;
  for (const Body &row : BODIES) {
    if (model == row.model) body = &row;
  }
  if (wire == nullptr || body == nullptr) {
    std::fprintf(stderr, "sim_check: unknown model \"%s\" (go2, b2, h1, g1, h1_2, h2, r1)\n", model.c_str());
    return 2;
  }
  if (actuators.empty()) {
    std::fprintf(stderr,
                 "sim_check: %s holds no actuator names. Without the vendor's own list the slot "
                 "map would be checked against nothing, so this refuses rather than skipping it.\n",
                 argv[3]);
    return 2;
  }

  std::printf("\n== %s in unitree_mujoco: %d joints, kp %.1f kd %.1f ==\n", model.c_str(),
              wire->joints, body->kp, body->kd);
  const int result = run(*body, wire, interface, actuators);
  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  if (result != 0) return result;
  return FAILURES.empty() ? 0 : 1;
}
