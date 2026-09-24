/*
 * skynet-unitree — Unitree bodies over unitree_sdk2's low-level channels.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 2 table: substrate *"SDK sim"*, dry-verifiable *"protocol, low-level
 * mode mapping"*, not dry-verifiable *"joint access at rate"*. Phase 7 item 3 extends the row from
 * the Go2 it opened with to the buyable humanoids: the G1, the H1, and the H1-2. This is the driver;
 * `fake_robot.cpp` beside it is the substrate, and both use **the vendor's own SDK**, which is what
 * makes this row worth more than a transcription: the CDR encoding, the DDS discovery and the topic
 * types are Unitree's code on both sides, so nothing here can be wrong about the wire without being
 * wrong in the same way a real robot would refuse.
 *
 * ── One driver, two message families, four bodies ────────────────────────
 *
 * The four bodies share the SDK, the DDS transport and the channel classes, and nothing else. The
 * Go2 and the original H1 speak the `go2` message family (twenty motor slots); the G1 and the H1-2
 * speak the `hg` family (thirty-five slots). Same topic names — `rt/lowcmd`, `rt/lowstate` — with
 * different types, so DDS keeps them apart and one driver can hold both behind a `model` key in its
 * config. What differs per body is one table row: the joint count, the slot map, the names, the
 * roles, the travel, and the per-joint ratings. The mapping below it is shared, because an impedance
 * command is an impedance command in either family.
 *
 * The H1 appears twice because the product spans both families: the original H1's low-level example
 * drives the `go2`-shaped message (`motors.hpp`, nineteen joints, slot 9 unused), while the H1-2's
 * drives `hg` (`h1_27dof_example.cpp`, twenty-seven joints). A buyable H1 in 2026 could be either
 * generation and no firmware note says which interface the older one speaks today, so the driver
 * carries both maps rather than guessing. `model=h1` is the nineteen-joint map, `model=h1_2` the
 * twenty-seven.
 *
 * ── The four things the mapping has to get right ──────────────────────────
 *
 * §24 commands a body in one mode — position, velocity or torque — and `MotorCmd_` is not a mode at
 * all. It is an impedance command: every motor carries `q`, `dq`, `tau`, `kp` and `kd` at once, and
 * what the drive does is `tau_out = tau + kp*(q - q_measured) + kd*(dq - dq_measured)`. So:
 *
 * 1. **The sentinels are not zeros.** `PosStopF` (2.146e9) in `q` and `VelStopF` (16000) in `dq` are
 *    Unitree's "ignore this field" values. A driver writing 0.0 into `q` for a torque command is not
 *    saying nothing — it is commanding the joint to zero radians, and with any `kp` at all that is a
 *    motion. This is the defect this row exists to catch, and `fake_robot` refuses a command that
 *    does it.
 *
 * 2. **`kp` and `kd` are not in a §24 command, and are not guessed.** They are the operator's, they
 *    come from the config JSON, and a position or velocity command with no gains configured is
 *    **refused**. A default would be a stiffness nobody chose, in front of a Safety Kernel that
 *    cannot see it — §63.4's rule, arriving through a gain rather than through a rating.
 *
 * 3. **The CRC is load-bearing.** A `LowCmd_` whose CRC32 does not match is dropped by the robot,
 *    silently, and a driver that omitted it would present as a robot that ignores commands. The
 *    fake checks it for exactly that reason.
 *
 * 4. **A masked joint is held, and on this drive a hold is a stiffness.** §24: *"0 = hold this
 *    joint at its current setpoint"*, and §25 is blunter — *"a zero mask holds position"*. The
 *    kernel writes an all-zero mask every cycle it has nothing to forward (`skynet-kernel-host`'s
 *    `cycle.rs`), so this is not a corner of the ABI: it is what the body receives whenever a
 *    policy is silent or a guard holds. Until 2026-09-24 this driver read the mask as "say nothing"
 *    — sentinels and zero gains, a free joint — and `unitree_mujoco` measured what that means: a
 *    standing Go2 folded to the ground on the first hold, and every humanoid's raised arm fell
 *    (DRY-COVERAGE-PLAN.md F121). So a masked joint now carries its hold target with the operator's
 *    `kp` and `kd`. The target is the joint's last position setpoint when it has one, and otherwise
 *    the position measured when the hold began — latched once and kept, because re-reading the
 *    measured position every cycle sets the error term to zero and holds nothing. With no gains
 *    configured a hold is refused rather than approximated: going limp is the one reading §24
 *    rules out, and a stiffness is the operator's to choose (§63.4).
 *
 * ── Where the humanoid numbers come from ──────────────────────────────────
 *
 * Slot maps are the vendor's published low-level examples, not this driver's invention: `G1JointIndex`
 * in `example/g1/low_level/g1_ankle_swing_example.cpp`, `H1JointIndex` in
 * `example/h1/low_level/h1_27dof_example.cpp`, and the `JointIndex` enum in
 * `example/h1/low_level/motors.hpp`. Travel is the vendor's MuJoCo models (unitree_mujoco,
 * BSD-3-Clause): `g1_29dof.xml`, `h1.xml`, `h1_2_handless.xml` — kinematic facts about the machine,
 * the same standing the `<limit>` travel has in the `ros2_control` substrates.
 *
 * Two warnings a reader needs. First, the G1's slot layout is *sparse*: a 23-DoF G1 reads the waist
 * at slot 12 and the left shoulder at slot 15, leaving 13, 14, 20, 21, 27 and 28 empty — the SDK
 * example marks each "NOTE INVALID for g1 23dof", and the dual-arm table in Unitree's
 * `g1_joint_index_dds.md` puts arms at 15..28 too. Unitree's own simulator once packed its 23-DoF
 * XML contiguously into slots 0..22; as of `unitree_mujoco` 1eb6642 it carries twenty-nine
 * actuators in the robot's layout, with the six absent joints on placeholder bodies, and
 * `sim_check.cpp` holds every slot here to the actuator of the same name in that file
 * (DRY-COVERAGE-PLAN.md F119). Second, a reseller copy of
 * the G1 datasheet carries "90 N.m" where Unitree's brochure carries "MAX JOINT TORQUE 120 N.m" —
 * revision drift between documents, unresolved, and the envelope takes the vendor's current figure
 * with the footnote beside it.
 *
 * Licence: Apache-2.0 (this file). It links unitree_sdk2, which is BSD-3-Clause, and CycloneDDS,
 * which is EPL-2.0/EDL-1.0 — neither is vendored here; see the Makefile.
 */

#include "skynet_rhal.hpp"
#include "unitree_models.hpp"

#include <unitree/idl/go2/LowCmd_.hpp>
#include <unitree/idl/go2/LowState_.hpp>
#include <unitree/idl/hg/LowCmd_.hpp>
#include <unitree/idl/hg/LowState_.hpp>
#include <unitree/robot/channel/channel_publisher.hpp>
#include <unitree/robot/channel/channel_subscriber.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <iterator>
#include <mutex>
#include <thread>
#include <string>
#include <vector>

namespace {

constexpr const char *TOPIC_LOWCMD = "rt/lowcmd";
constexpr const char *TOPIC_LOWSTATE = "rt/lowstate";

/* Unitree's own sentinels, from their published low-level example. `q = PosStopF` means "this
 * command says nothing about position"; `dq = VelStopF` means the same about velocity. Shared by
 * both message families. */
constexpr float POS_STOP_F = 2.146e9f;
constexpr float VEL_STOP_F = 16000.0f;

/* PMSM servo mode. `0x00` is idle and leaves the drive unpowered, which is not what a command
 * means. Shared by both families. */
constexpr std::uint8_t MOTOR_MODE_SERVO = 0x01;

/* hg series control for pitch/roll joints, the vendor example's default. The alternative (`1`) is
 * parallel A/B control of the differential ankle and waist joints, which addresses the same slots
 * as motor pairs rather than joints — this driver addresses joints. */
constexpr std::uint8_t HG_MODE_PR_SERIES = 0x00;

/* The message families and the slot maps live in `unitree_models.hpp`, shared with the fake
 * and the suite so the three cannot disagree about the wire. In both families the body has fewer
 * joints than the message has slots, which is why every model carries a slot map rather than an
 * index. */
using skynet_unitree::Family;
using skynet_unitree::G1_SLOTS;
using skynet_unitree::GO2_SLOTS;
using skynet_unitree::GO2_SLOTS_MAP;
using skynet_unitree::H1_2_SLOTS;
using skynet_unitree::H1_SLOTS;
using skynet_unitree::HG_SLOTS;

/* A rating the vendor did not publish. Torques and rates are positive quantities, so -1.0 is
 * unmistakable, and the descriptor omits the field rather than printing it (§5.3: stated, never
 * defaulted). */
constexpr double UNDECLARED = -1.0;

/* Unitree's CRC32, transcribed from their published example because the SDK does not export it.
 * Bit-reflected input, polynomial 0x04C11DB7, initial value all ones, no final inversion — which
 * is not any of the common CRC32 parameterisations, and is why it is written out rather than
 * reached for from a library. One function for both families: the G1 and H1 examples compute it
 * identically. */
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

/* The tiny amount of JSON this driver reads, by hand.
 *
 * A driver is a shared object a robot loads and this repository has no third-party dependencies, so
 * a parser for `{"kp": 60.0}` is four lines rather than a library. It reads a number after a key and
 * answers `false` when there is not one — which is the whole of what the caller needs, because a
 * missing gain is a refusal rather than a default. */
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

/* A boolean after a key: the literal `true` or `false`, nothing else. Answers `false`
 * when there is not one — which for `simulator` is what the caller wants, because a missing
 * flag is the safe default rather than a refusal. */
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
  const std::string needle_ws = needle;
  const std::size_t at = config.find(needle_ws);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle_ws.size());
  if (colon == std::string::npos) return false;
  const std::size_t open = config.find('"', colon);
  if (open == std::string::npos) return false;
  const std::size_t close = config.find('"', open + 1);
  if (close == std::string::npos) return false;
  *out = config.substr(open + 1, close - open - 1);
  return true;
}

/* Travel, radians. Per-joint tables below, per body. */
struct Travel {
  double low;
  double high;
};

/* ── the Go2: twelve joints, `go2` family, slots 0..11 ──────────────────────
 *
 * §5.1 roles in the order Unitree publishes them: front-right, front-left, rear-right, rear-left,
 * each hip/thigh/calf. The order is the robot's and is not a choice this driver makes — a driver
 * that sorted them would report the left legs as the right ones. */
const char *const GO2_ROLES[12] = {
    "right_leg.hip.abduct#0",  "right_leg.thigh.pitch#0", "right_leg.calf.flex#0",
    "left_leg.hip.abduct#0",   "left_leg.thigh.pitch#0",  "left_leg.calf.flex#0",
    "right_leg.hip.abduct#1",  "right_leg.thigh.pitch#1", "right_leg.calf.flex#1",
    "left_leg.hip.abduct#1",   "left_leg.thigh.pitch#1",  "left_leg.calf.flex#1",
};

const char *const GO2_NAMES[12] = {
    "FR_hip", "FR_thigh", "FR_calf", "FL_hip", "FL_thigh", "FL_calf",
    "RR_hip", "RR_thigh", "RR_calf", "RL_hip", "RL_thigh", "RL_calf",
};

/* Travel, radians, from Unitree's published Go2 joint-limit table. Travel is a kinematic fact about
 * the machine. */
constexpr Travel GO2_TRAVEL[12] = {
    {-1.0472, 1.0472}, {-1.5708, 3.4907}, {-2.7227, -0.83776},
    {-1.0472, 1.0472}, {-1.5708, 3.4907}, {-2.7227, -0.83776},
    {-1.0472, 1.0472}, {-0.5236, 4.5379}, {-2.7227, -0.83776},
    {-1.0472, 1.0472}, {-0.5236, 4.5379}, {-2.7227, -0.83776},
};

/* Peak joint torque, newton-metres, from Unitree's Go2 specs (unitree.com/go2): "approx. 45N.m",
 * with the footnote that it is the largest motor's and varies across the twelve. That footnote
 * is quoted in the envelope's `source` beside the figure, because a ceiling that reads as
 * per-joint truth would be exactly the brochure §63.4 refuses: no joint is commanded beyond the
 * largest motor's peak, and no smaller joint is claimed to reach it. No velocity ceiling is
 * declared — Unitree publishes no joint speed, and the sim configs that carry one are CAD
 * values, which is what §63.4 forbids. */
constexpr double GO2_EFFORT[12] = {
    45.0, 45.0, 45.0, 45.0, 45.0, 45.0, 45.0, 45.0, 45.0, 45.0, 45.0, 45.0,
};

/* ── the H1 (original): nineteen joints, `go2` family, slot 9 empty ─────────
 *
 * The slot map is the vendor's `example/h1/low_level/motors.hpp`: legs at 0..5, 7, 8, 10, 11,
 * waist yaw at 6, arms at 12..19 — and slot 9 is `kNotUsedJoint`, which the fake counts against.
 * Driver order below is left leg, right leg, waist, left arm, right arm, each yaw/pitch/roll where
 * the leg has all three. The map itself is `H1_SLOTS` in the shared header. */
const char *const H1_NAMES[19] = {
    "L_hip_yaw", "L_hip_pitch", "L_hip_roll", "L_knee", "L_ankle",
    "R_hip_yaw", "R_hip_pitch", "R_hip_roll", "R_knee", "R_ankle",
    "waist_yaw",
    "L_shoulder_pitch", "L_shoulder_roll", "L_shoulder_yaw", "L_elbow",
    "R_shoulder_pitch", "R_shoulder_roll", "R_shoulder_yaw", "R_elbow",
};

const char *const H1_ROLES[19] = {
    "left_leg.hip.yaw", "left_leg.hip.pitch", "left_leg.hip.roll", "left_leg.knee.flex",
    "left_leg.ankle.pitch",
    "right_leg.hip.yaw", "right_leg.hip.pitch", "right_leg.hip.roll", "right_leg.knee.flex",
    "right_leg.ankle.pitch",
    "torso.waist.yaw",
    "left_arm.shoulder.pitch", "left_arm.shoulder.roll", "left_arm.shoulder.yaw",
    "left_arm.elbow.flex",
    "right_arm.shoulder.pitch", "right_arm.shoulder.roll", "right_arm.shoulder.yaw",
    "right_arm.elbow.flex",
};

/* Travel from the vendor's `h1.xml` (unitree_mujoco). The knee's -0.26/2.05 and the ankle's
 * -0.87/0.52 are the single-DoF joints of the original leg. */
constexpr Travel H1_TRAVEL[19] = {
    {-0.43, 0.43}, {-3.14, 2.53}, {-0.43, 0.43}, {-0.26, 2.05}, {-0.87, 0.52},
    {-0.43, 0.43}, {-3.14, 2.53}, {-0.43, 0.43}, {-0.26, 2.05}, {-0.87, 0.52},
    {-2.35, 2.35},
    {-2.87, 2.87}, {-0.34, 3.11}, {-1.3, 4.45}, {-1.25, 2.61},
    {-2.87, 2.87}, {-3.11, 0.34}, {-4.45, 1.3}, {-1.25, 2.61},
};

/* Per-group peaks from Unitree's H1 parameters (the GTC release: knee approx. 360 N.m, hip approx.
 * 220, ankle approx. 45, arm approx. 75). More precise than the Go2's single figure, because the
 * vendor published the groups. The waist is unpublished and stays undeclared rather than inheriting
 * a neighbour's rating. */
constexpr double H1_EFFORT[19] = {
    220.0, 220.0, 220.0, 360.0, 45.0,
    220.0, 220.0, 220.0, 360.0, 45.0,
    UNDECLARED,
    75.0, 75.0, 75.0, 75.0,
    75.0, 75.0, 75.0, 75.0,
};

/* ── the G1: twenty-three joints, `hg` family, sparse slots ─────────────────
 *
 * The slot map is the vendor's `G1JointIndex`: legs 0..11, waist yaw 12, arms 15..19 and 22..26.
 * Slots 13, 14 (waist roll/pitch), 20, 21, 27, 28 (wrist pitch/yaw) are marked "NOTE INVALID for g1
 * 23dof" in the example and this driver never addresses them — they are the 29-DoF body's joints,
 * and a command there would be a command to a joint that is not present. See the file header for
 * why the simulator's contiguous packing is not followed. */
const char *const G1_NAMES[23] = {
    "L_hip_pitch", "L_hip_roll", "L_hip_yaw", "L_knee", "L_ankle_pitch", "L_ankle_roll",
    "R_hip_pitch", "R_hip_roll", "R_hip_yaw", "R_knee", "R_ankle_pitch", "R_ankle_roll",
    "waist_yaw",
    "L_shoulder_pitch", "L_shoulder_roll", "L_shoulder_yaw", "L_elbow", "L_wrist_roll",
    "R_shoulder_pitch", "R_shoulder_roll", "R_shoulder_yaw", "R_elbow", "R_wrist_roll",
};

const char *const G1_ROLES[23] = {
    "left_leg.hip.pitch", "left_leg.hip.roll", "left_leg.hip.yaw", "left_leg.knee.flex",
    "left_leg.ankle.pitch", "left_leg.ankle.roll",
    "right_leg.hip.pitch", "right_leg.hip.roll", "right_leg.hip.yaw", "right_leg.knee.flex",
    "right_leg.ankle.pitch", "right_leg.ankle.roll",
    "torso.waist.yaw",
    "left_arm.shoulder.pitch", "left_arm.shoulder.roll", "left_arm.shoulder.yaw",
    "left_arm.elbow.flex", "left_arm.wrist.roll",
    "right_arm.shoulder.pitch", "right_arm.shoulder.roll", "right_arm.shoulder.yaw",
    "right_arm.elbow.flex", "right_arm.wrist.roll",
};

/* Travel from the vendor's `g1_29dof.xml` (unitree_mujoco) — the 29-DoF file, because the 23-DoF
 * XML packs its joints in the simulator's contiguous order and names are what carry across. Each
 * row below was matched by joint name, not by position. */
constexpr Travel G1_TRAVEL[23] = {
    {-2.5307, 2.8798}, {-0.5236, 2.9671}, {-2.7576, 2.7576}, {-0.087267, 2.8798},
    {-0.87267, 0.5236}, {-0.2618, 0.2618},
    {-2.5307, 2.8798}, {-2.9671, 0.5236}, {-2.7576, 2.7576}, {-0.087267, 2.8798},
    {-0.87267, 0.5236}, {-0.2618, 0.2618},
    {-2.618, 2.618},
    {-3.0892, 2.6704}, {-1.5882, 2.2515}, {-2.618, 2.618}, {-1.0472, 2.0944},
    {-1.97222, 1.97222},
    {-3.0892, 2.6704}, {-2.2515, 1.5882}, {-2.618, 2.618}, {-1.0472, 2.0944},
    {-1.97222, 1.97222},
};

/* 120 N.m on every joint, the Go2's treatment: Unitree's G1 brochure carries "MAX JOINT TORQUE 120
 * N.m" with footnote [1] — "the maximum torque of the largest joint motor among them" — which is
 * quoted in the envelope's `source` beside the figure. */
constexpr double G1_EFFORT[23] = {
    120.0, 120.0, 120.0, 120.0, 120.0, 120.0,
    120.0, 120.0, 120.0, 120.0, 120.0, 120.0,
    120.0,
    120.0, 120.0, 120.0, 120.0, 120.0,
    120.0, 120.0, 120.0, 120.0, 120.0,
};

/* ── the H1-2: twenty-seven joints, `hg` family, slots 0..26 ────────────────
 *
 * The slot map is the vendor's `H1JointIndex` in `h1_27dof_example.cpp`: six-DoF legs (the ankle
 * gains roll), waist yaw, seven-DoF arms (the wrist gains pitch and yaw). Contiguous, for once. */
const char *const H1_2_NAMES[27] = {
    "L_hip_yaw", "L_hip_pitch", "L_hip_roll", "L_knee", "L_ankle_pitch", "L_ankle_roll",
    "R_hip_yaw", "R_hip_pitch", "R_hip_roll", "R_knee", "R_ankle_pitch", "R_ankle_roll",
    "waist_yaw",
    "L_shoulder_pitch", "L_shoulder_roll", "L_shoulder_yaw", "L_elbow",
    "L_wrist_roll", "L_wrist_pitch", "L_wrist_yaw",
    "R_shoulder_pitch", "R_shoulder_roll", "R_shoulder_yaw", "R_elbow",
    "R_wrist_roll", "R_wrist_pitch", "R_wrist_yaw",
};

const char *const H1_2_ROLES[27] = {
    "left_leg.hip.yaw", "left_leg.hip.pitch", "left_leg.hip.roll", "left_leg.knee.flex",
    "left_leg.ankle.pitch", "left_leg.ankle.roll",
    "right_leg.hip.yaw", "right_leg.hip.pitch", "right_leg.hip.roll", "right_leg.knee.flex",
    "right_leg.ankle.pitch", "right_leg.ankle.roll",
    "torso.waist.yaw",
    "left_arm.shoulder.pitch", "left_arm.shoulder.roll", "left_arm.shoulder.yaw",
    "left_arm.elbow.flex", "left_arm.wrist.roll", "left_arm.wrist.pitch", "left_arm.wrist.yaw",
    "right_arm.shoulder.pitch", "right_arm.shoulder.roll", "right_arm.shoulder.yaw",
    "right_arm.elbow.flex", "right_arm.wrist.roll", "right_arm.wrist.pitch", "right_arm.wrist.yaw",
};

/* Travel from the vendor's `h1_2_handless.xml` (unitree_mujoco). */
constexpr Travel H1_2_TRAVEL[27] = {
    {-0.43, 0.43}, {-3.14, 2.5}, {-0.43, 3.14}, {-0.12, 2.19}, {-0.897334, 0.523598},
    {-0.261799, 0.261799},
    {-0.43, 0.43}, {-3.14, 2.5}, {-3.14, 0.43}, {-0.12, 2.19}, {-0.897334, 0.523598},
    {-0.261799, 0.261799},
    {-2.35, 2.35},
    {-3.14, 1.57}, {-0.38, 3.4}, {-2.66, 3.01}, {-0.95, 3.18}, {-3.01, 2.75},
    {-0.4625, 0.4625}, {-1.27, 1.27},
    {-3.14, 1.57}, {-3.4, 0.38}, {-3.01, 2.66}, {-0.95, 3.18}, {-2.75, 3.01},
    {-0.4625, 0.4625}, {-1.27, 1.27},
};

/* The H1 group peaks, which the H1-2 comparison table repeats unchanged (knee 360, hip 220, ankle
 * 45, arm 75). Waist and wrists are unpublished on both bodies and stay undeclared — the arm's 75
 * is the upper arm's, and a wrist is not an upper arm. */
constexpr double H1_2_EFFORT[27] = {
    220.0, 220.0, 220.0, 360.0, 45.0, 45.0,
    220.0, 220.0, 220.0, 360.0, 45.0, 45.0,
    UNDECLARED,
    75.0, 75.0, 75.0, 75.0, UNDECLARED, UNDECLARED, UNDECLARED,
    75.0, 75.0, 75.0, 75.0, UNDECLARED, UNDECLARED, UNDECLARED,
};

struct ModelSpec {
  const char *model;
  const char *slug;
  Family family;
  int joints;
  const std::uint8_t *slots;
  const char *const *names;
  const char *const *roles;
  const Travel *travel;
  const double *effort;
  double torque_max_nm;
  const char *torque_source;
  double rate_hz_max;
  double rate_hz_min;
};

/* The watchdog window, in milliseconds: ten DDS periods at the 500 Hz the Go2 runs. Ten missed
 * states is a dead robot rather than jitter, and §46.3's nesting wants the window comfortably
 * above the worst-case cycle. One window for all four bodies: the humanoids' DDS rate is
 * unpublished, so the window is driver design everywhere, stated as such in each envelope. */
constexpr double WATCHDOG_MS = 20.0;

const ModelSpec MODELS[] = {
    {
        "go2",
        "unitree-go2",
        Family::GO2,
        12,
        GO2_SLOTS_MAP,
        GO2_NAMES,
        GO2_ROLES,
        GO2_TRAVEL,
        GO2_EFFORT,
        45.0,
        "Unitree Go2 specs (unitree.com/go2): peak joint torque approx. 45N.m of the largest "
        "motor, varying per motor; no joint velocity published; watchdog ten DDS periods per "
        "driver design",
        500.0,
        50.0,
    },
    {
        "h1",
        "unitree-h1",
        Family::GO2,
        19,
        H1_SLOTS,
        H1_NAMES,
        H1_ROLES,
        H1_TRAVEL,
        H1_EFFORT,
        360.0,
        "Unitree H1 parameters (GTC release): maximum joint torque knee approx. 360N.m, hip "
        "approx. 220N.m, ankle approx. 45N.m, arm approx. 75N.m; waist unpublished; no joint "
        "velocity published; watchdog ten DDS periods per driver design",
        UNDECLARED,
        UNDECLARED,
    },
    {
        "g1",
        "unitree-g1",
        Family::HG,
        23,
        G1_SLOTS,
        G1_NAMES,
        G1_ROLES,
        G1_TRAVEL,
        G1_EFFORT,
        120.0,
        "Unitree G1 brochure: max joint torque 120 N.m of the largest joint motor (knee), "
        "footnote [1]: per-motor maxima differ; no joint velocity published; watchdog ten DDS "
        "periods per driver design",
        UNDECLARED,
        UNDECLARED,
    },
    {
        "h1_2",
        "unitree-h1-2",
        Family::HG,
        27,
        H1_2_SLOTS,
        H1_2_NAMES,
        H1_2_ROLES,
        H1_2_TRAVEL,
        H1_2_EFFORT,
        360.0,
        "Unitree H1 parameters (GTC release), repeated for H1-2: knee approx. 360N.m, hip "
        "approx. 220N.m, ankle approx. 45N.m, arm approx. 75N.m; waist and wrists unpublished; "
        "no joint velocity published; watchdog ten DDS periods per driver design",
        UNDECLARED,
        UNDECLARED,
    },
};

const ModelSpec *find_model(const std::string &model) {
  for (const ModelSpec &spec : MODELS) {
    if (model == spec.model) return &spec;
  }
  return nullptr;
}

}  // namespace (tables)

namespace {

/* What arrives from the robot, and the channels it arrives on — created once per process and never
 * freed.
 *
 * unitree_sdk2's `DdsReader` drops its reader in its destructor without closing it first. If that
 * destructor runs while CycloneDDS is dispatching a sample, the dispatch thread holds the last
 * reference, the destructor runs *on* that thread, and CycloneDDS aborts the process rather than
 * wait for itself (`EntityDelegate::prevent_callbacks`, cyclonedds-cxx 0.10.2, the `assert(false)`
 * its own comment explains). `close()` used to reset the subscriber while the robot was still
 * publishing at 500 Hz, which is exactly that window, and the conformance run against the fake
 * aborted in it one run in two (DRY-COVERAGE-PLAN.md F122). A robot's control loop that closes its
 * driver on the way down would have died the same way.
 *
 * So a family's channels are made on the first `open()` that needs them and are never destroyed:
 * `close()` detaches and `open()` re-attaches. The callbacks write here rather than into the
 * driver, so a sample that arrives while the driver object is being destroyed at exit lands in
 * memory that still exists. The cost is a subscription that keeps receiving, and ignoring, state
 * after `close()`; the process's DDS participant is torn down with the process. */
struct Link {
  std::mutex lock;
  /* Whether a session is open. A sample arriving while detached is dropped, so the first state a
   * new session sees is one that arrived after it opened. */
  bool attached = false;
  bool have_state = false;
  std::uint64_t arrivals = 0;
  std::uint8_t mode_machine = 0;
  unitree_go::msg::dds_::LowState_ go2_state{};
  unitree_hg::msg::dds_::LowState_ hg_state{};
  /* The interface the channels were made on. unitree_sdk2 joins DDS once per process, so a later
   * session naming a different one is refused rather than silently given the first. */
  std::string interface;

  unitree::robot::ChannelPublisherPtr<unitree_go::msg::dds_::LowCmd_> go2_publisher;
  unitree::robot::ChannelSubscriberPtr<unitree_go::msg::dds_::LowState_> go2_subscriber;
  unitree::robot::ChannelPublisherPtr<unitree_hg::msg::dds_::LowCmd_> hg_publisher;
  unitree::robot::ChannelSubscriberPtr<unitree_hg::msg::dds_::LowState_> hg_subscriber;
};

Link &shared_link() {
  /* Deliberately never deleted: the argument is `Link`'s comment. */
  static Link *const link = new Link();
  return *link;
}

class UnitreeBridge : public skynet::Driver {
 public:
  int open(std::string_view config_json) override {
    if (opened_) return SKR_EPERM;
    const std::string config(config_json);

    /* Every field a previous session set, reset here rather than in the constructor.
     *
     * `SKYNET_RHAL_EXPORT` builds the driver as a function-local static, so one process holds one
     * instance for its whole life and `close()` followed by `open()` lands on the same object. A
     * driver that initialised only in its constructor would carry the last session's state into
     * the next one — and the field that matters is the estop latch, which would make a fresh
     * `open()` refuse every command with no way to see why (DRY-COVERAGE-PLAN.md F47).
     *
     * The latch is announced rather than silently dropped: a new session is a new session, and an
     * operator who stopped this robot a moment ago should read that this open cleared it. */
    if (estopped_) {
      std::fprintf(stderr,
                   "skynet-unitree: the previous session left an emergency stop latched. This is "
                   "a new session and the latch does not carry into it — the robot's own state is "
                   "read from LowState_, and nothing here can tell you whether it is still "
                   "stopped.\n");
    }
    estopped_ = false;
    seq_ = 0;
    have_kp_ = have_kd_ = false;
    kp_ = kd_ = 0.0;
    model_ = nullptr;
    /* No setpoint and no hold carries into a new session: the first hold of this one latches
     * what the robot is measured at, not what the last session left it commanded to. */
    std::fill(std::begin(setpoint_), std::end(setpoint_), 0.0f);
    std::fill(std::begin(have_setpoint_), std::end(have_setpoint_), false);
    std::fill(std::begin(hold_), std::end(hold_), 0.0f);
    std::fill(std::begin(holding_), std::end(holding_), false);

    /* The body, defaulting to the Go2 the driver opened with. An unknown model is refused before
     * DDS is touched: joining discovery for a body this driver cannot speak would only waste the
     * five seconds below. */
    std::string model_key = "go2";
    config_string(config, "model", &model_key);
    model_ = find_model(model_key);
    if (model_ == nullptr) {
      std::fprintf(stderr,
                   "skynet-unitree: unknown `model` \"%s\". The bodies this driver speaks are "
                   "go2, h1, g1 and h1_2.\n",
                   model_key.c_str());
      return SKR_EINVAL;
    }

    std::string interface = "lo";
    config_string(config, "interface", &interface);
    slug_ = model_->slug;
    config_string(config, "robot_slug", &slug_);
    /* Gains, and there is deliberately no default. A position command with `kp = 0` moves nothing
     * and a position command with a `kp` this driver invented is a stiffness nobody chose. */
    have_kp_ = config_number(config, "kp", &kp_);
    have_kd_ = config_number(config, "kd", &kd_);
    /* Whether the body is simulated, declared by the operator — the fake publishes the same DDS
     * as the robot, so the driver cannot tell them apart. §35.3's direction of safety (declaring
     * simulated about real caps the tier; the reverse presents sim as evidence) makes the
     * default `true`, and a real robot says so deliberately. Reset here with everything else:
     * a session that declared real must not leak into the next one. */
    simulator_ = true;
    config_bool(config, "simulator", &simulator_);

    if (!link_.interface.empty() && link_.interface != interface) {
      std::fprintf(stderr,
                   "skynet-unitree: this process joined DDS on `%s`, and this session names "
                   "`%s`. unitree_sdk2 joins one interface per process, so the session is refused "
                   "rather than silently given the first one; open it in a new process.\n",
                   link_.interface.c_str(), interface.c_str());
      return SKR_EINVAL;
    }

    Link *const link = &link_;
    try {
      unitree::robot::ChannelFactory::Instance()->Init(0, interface);
      link_.interface = interface;
      /* Same topic names in both families, different types — DDS matches on both, so a Go2
       * publisher and an hg subscriber never meet. Only the model's pair is created, once per
       * process, and reused by every later session of the same family (`Link`). */
      if (model_->family == Family::GO2 && !link_.go2_subscriber) {
        link_.go2_publisher.reset(
            new unitree::robot::ChannelPublisher<unitree_go::msg::dds_::LowCmd_>(TOPIC_LOWCMD));
        link_.go2_publisher->InitChannel();
        link_.go2_subscriber.reset(new unitree::robot::ChannelSubscriber<unitree_go::msg::dds_::LowState_>(
            TOPIC_LOWSTATE));
        link_.go2_subscriber->InitChannel(
            [link](const void *message) {
              std::lock_guard<std::mutex> guard(link->lock);
              if (!link->attached) return;
              link->go2_state = *static_cast<const unitree_go::msg::dds_::LowState_ *>(message);
              link->have_state = true;
              link->arrivals += 1;
            },
            1);
      } else if (model_->family == Family::HG && !link_.hg_subscriber) {
        link_.hg_publisher.reset(
            new unitree::robot::ChannelPublisher<unitree_hg::msg::dds_::LowCmd_>(TOPIC_LOWCMD));
        link_.hg_publisher->InitChannel();
        link_.hg_subscriber.reset(new unitree::robot::ChannelSubscriber<unitree_hg::msg::dds_::LowState_>(
            TOPIC_LOWSTATE));
        link_.hg_subscriber->InitChannel(
            [link](const void *message) {
              std::lock_guard<std::mutex> guard(link->lock);
              if (!link->attached) return;
              link->hg_state = *static_cast<const unitree_hg::msg::dds_::LowState_ *>(message);
              /* The machine code, kept for the echo: the vendor examples send back whatever the
               * state carries (4 for a 23-DoF G1 in Unitree's own sim), and a driver that sent a
               * constant would misidentify every other body. */
              link->mode_machine = link->hg_state.mode_machine();
              link->have_state = true;
              link->arrivals += 1;
            },
            1);
      }
    } catch (const std::exception &error) {
      std::fprintf(stderr, "skynet-unitree: could not open DDS on `%s`: %s\n", interface.c_str(),
                   error.what());
      return SKR_EIO;
    }
    {
      /* Attached with nothing received: the state this session waits for below is one that
       * arrived after it opened, never one a previous session left behind. */
      std::lock_guard<std::mutex> guard(link_.lock);
      link_.attached = true;
      link_.have_state = false;
      link_.arrivals = 0;
      link_.mode_machine = 0;
    }

    /* The first state, waited for rather than assumed. DDS discovery is not instant and a driver
     * that returned success here would have reported a body it had never heard from — the same
     * rule `drivers/franka` follows about its first UDP datagram. */
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (std::chrono::steady_clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(link_.lock);
        if (link_.have_state) break;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    {
      std::lock_guard<std::mutex> guard(link_.lock);
      if (!link_.have_state) {
        link_.attached = false;
        std::fprintf(stderr,
                     "skynet-unitree: no LowState_ arrived on `%s` within five seconds. Either "
                     "nothing is publishing `%s`, or the interface is not the one the robot is "
                     "on.\n",
                     interface.c_str(), TOPIC_LOWSTATE);
        return SKR_EIO;
      }
    }
    opened_ = true;
    return SKR_OK;
  }

  int close() override {
    /* Detach, and destroy nothing: `Link` says why the channels outlive the session. */
    opened_ = false;
    std::lock_guard<std::mutex> guard(link_.lock);
    link_.attached = false;
    link_.have_state = false;
    return SKR_OK;
  }

  int read_state(skynet::State &out, std::uint64_t) override {
    if (!opened_ || model_ == nullptr) return SKR_EPERM;
    const int joints = model_->joints;
    if (out.joint_count() < static_cast<std::size_t>(joints)) return SKR_EINVAL;

    if (model_->family == Family::GO2) {
      unitree_go::msg::dds_::LowState_ snapshot;
      std::uint64_t arrivals = 0;
      {
        std::lock_guard<std::mutex> guard(link_.lock);
        if (!link_.have_state) return SKR_EIO;
        snapshot = link_.go2_state;
        arrivals = link_.arrivals;
      }

      seq_ += 1;
      std::uint16_t flags = 0;
      /* §27's homing: these are absolute encoders on legs, and there is nothing to home. Saying so
       * is not the same as saying nothing — `unreported` holds the robot. */
      flags |= SKR_HOMING_NOT_REQUIRED;
      if (estopped_) flags |= SKR_FLAG_ESTOP;

      /* The vendor's own fault word, surfaced verbatim (§24). What reaches the OS is the first
       * joint that reports a non-zero `lost` count, because that is the one the robot itself says
       * it cannot hear. */
      std::uint32_t fault = 0;
      for (int i = 0; i < joints; ++i) {
        if (snapshot.motor_state()[model_->slots[i]].lost() != 0) {
          fault = static_cast<std::uint32_t>(i + 1);
          flags |= SKR_FLAG_DEGRADED;
          break;
        }
      }

      out.set_stamp(skynet::acquisition_ns());
      out.set_seq(seq_);
      out.set_flags(flags);
      out.set_fault(fault);
      auto position = out.position();
      auto velocity = out.velocity();
      auto effort = out.effort();
      for (int i = 0; i < joints; ++i) {
        const std::uint8_t slot = model_->slots[i];
        position[i] = snapshot.motor_state()[slot].q();
        velocity[i] = snapshot.motor_state()[slot].dq();
        /* `tau_est` is the drive's *estimate* from current, not a torque sensor. It is still a
         * measurement rather than §24's NaN, and the descriptor is where the difference is
         * stated. */
        effort[i] = snapshot.motor_state()[slot].tau_est();
      }
      out.set_joint_count(joints);
      set_base_pose_unknown(out);
      (void)arrivals;
      return SKR_OK;
    }

    unitree_hg::msg::dds_::LowState_ snapshot;
    std::uint64_t arrivals = 0;
    {
      std::lock_guard<std::mutex> guard(link_.lock);
      if (!link_.have_state) return SKR_EIO;
      snapshot = link_.hg_state;
      arrivals = link_.arrivals;
    }

    seq_ += 1;
    std::uint16_t flags = 0;
    flags |= SKR_HOMING_NOT_REQUIRED;
    if (estopped_) flags |= SKR_FLAG_ESTOP;

    /* hg's fault word is `motorstate()`, a per-motor code the vendor example prints and this
     * driver surfaces the same way: the first joint whose drive reports trouble. */
    std::uint32_t fault = 0;
    for (int i = 0; i < joints; ++i) {
      if (snapshot.motor_state()[model_->slots[i]].motorstate() != 0) {
        fault = static_cast<std::uint32_t>(i + 1);
        flags |= SKR_FLAG_DEGRADED;
        break;
      }
    }

    out.set_stamp(skynet::acquisition_ns());
    out.set_seq(seq_);
    out.set_flags(flags);
    out.set_fault(fault);
    auto position = out.position();
    auto velocity = out.velocity();
    auto effort = out.effort();
    for (int i = 0; i < joints; ++i) {
      const std::uint8_t slot = model_->slots[i];
      position[i] = snapshot.motor_state()[slot].q();
      velocity[i] = snapshot.motor_state()[slot].dq();
      effort[i] = snapshot.motor_state()[slot].tau_est();
    }
    out.set_joint_count(joints);
    set_base_pose_unknown(out);
    (void)arrivals;
    return SKR_OK;
  }

  int write_command(const skynet::Command &cmd, std::uint64_t) override {
    if (!opened_ || model_ == nullptr) return SKR_EPERM;
    if (estopped_) return SKR_EPERM;
    const int joints = model_->joints;
    if (cmd.joint_count() != static_cast<std::size_t>(joints)) return SKR_EINVAL;
    const int mode = cmd.mode();
    if (mode != SKR_MODE_POSITION && mode != SKR_MODE_VELOCITY && mode != SKR_MODE_TORQUE) {
      return SKR_EINVAL;
    }
    /* §63.4 through a gain. A position or velocity command is a *setpoint*, and what turns a
     * setpoint into a torque on this body is `kp` and `kd`, which §24's command does not carry. A
     * default would be a stiffness the operator never chose and the Safety Kernel cannot see. */
    const bool gains = have_kp_ && have_kd_;
    if ((mode == SKR_MODE_POSITION || mode == SKR_MODE_VELOCITY) && !gains) {
      std::fprintf(stderr,
                   "skynet-unitree: a %s command needs `kp` and `kd` in this driver's config. "
                   "A Unitree drive takes an impedance command, not a setpoint, and this driver "
                   "will not choose a stiffness the operator did not (§63.4).\n",
                   mode == SKR_MODE_POSITION ? "position" : "velocity");
      return SKR_EPERM;
    }
    bool any_masked = false;
    for (int i = 0; i < joints; ++i) {
      if (!cmd.applies(i)) any_masked = true;
    }
    if (any_masked && !gains) {
      std::fprintf(stderr,
                   "skynet-unitree: this command masks a joint, and a masked joint is §24's hold "
                   "— held at its current setpoint. On an impedance drive a hold is a stiffness, "
                   "and with no `kp` and `kd` in this driver's config there is nothing to hold "
                   "with. Going limp instead is the one reading §24 rules out, so the command is "
                   "refused; configure the gains the robot should hold with (§63.4).\n");
      return SKR_EPERM;
    }

    const auto values = cmd.values();

    /* §24's hold, resolved per joint before anything is published (the file header, point 4).
     * Worked on copies and committed only once the message is on the wire, so a publish that
     * fails leaves the bookkeeping describing what the robot last received. */
    float setpoint[skynet_unitree::MAX_JOINTS];
    bool have_setpoint[skynet_unitree::MAX_JOINTS];
    float hold[skynet_unitree::MAX_JOINTS];
    bool holding[skynet_unitree::MAX_JOINTS];
    std::copy(std::begin(setpoint_), std::end(setpoint_), setpoint);
    std::copy(std::begin(have_setpoint_), std::end(have_setpoint_), have_setpoint);
    std::copy(std::begin(hold_), std::end(hold_), hold);
    std::copy(std::begin(holding_), std::end(holding_), holding);
    for (int i = 0; i < joints; ++i) {
      if (cmd.applies(i)) {
        holding[i] = false;
        /* Only a position command leaves a position setpoint. After a velocity or torque command
         * the joint's current setpoint is not a position at all, so a hold that follows latches
         * where the joint is measured to be. */
        have_setpoint[i] = mode == SKR_MODE_POSITION;
        if (have_setpoint[i]) setpoint[i] = values[i];
        continue;
      }
      if (!holding[i]) {
        hold[i] = have_setpoint[i] ? setpoint[i] : measured_position(i);
        holding[i] = true;
      }
    }

    if (model_->family == Family::GO2) {
      unitree_go::msg::dds_::LowCmd_ out{};
      out.head()[0] = 0xFE;
      out.head()[1] = 0xEF;
      out.level_flag() = 0xFF;
      out.gpio() = 0;
      for (std::size_t i = 0; i < GO2_SLOTS; ++i) {
        auto &motor = out.motor_cmd()[i];
        /* Every slot, including the ones this body does not have. A slot left at the struct's
         * zero would carry mode 0 and `q = 0`, which is a well-formed command to a joint that is
         * not there — and on a body that does have twenty, it is a command to move it to zero. */
        motor.mode() = MOTOR_MODE_SERVO;
        motor.q() = POS_STOP_F;
        motor.dq() = VEL_STOP_F;
        motor.kp() = 0.0f;
        motor.kd() = 0.0f;
        motor.tau() = 0.0f;
      }
      for (int i = 0; i < joints; ++i) {
        auto &motor = out.motor_cmd()[model_->slots[i]];
        if (!cmd.applies(i)) {
          apply_hold(motor, hold[i]);
          continue;
        }
        if (!apply_mode(motor, mode, values[i])) return SKR_EINVAL;
      }
      out.crc() = crc32_core(reinterpret_cast<const std::uint32_t *>(&out), (sizeof(out) >> 2) - 1);
      try {
        link_.go2_publisher->Write(out);
      } catch (const std::exception &) {
        return SKR_EIO;
      }
      commit_holds(setpoint, have_setpoint, hold, holding);
      return SKR_OK;
    }

    unitree_hg::msg::dds_::LowCmd_ out{};
    /* Series joint control, the vendor example's default — and the machine code echoed from the
     * last state, read under the lock. `open()` waited for the first state, so the echo is never
     * the zero it starts at. */
    out.mode_pr() = HG_MODE_PR_SERIES;
    {
      std::lock_guard<std::mutex> guard(link_.lock);
      out.mode_machine() = link_.mode_machine;
    }
    for (std::size_t i = 0; i < HG_SLOTS; ++i) {
      auto &motor = out.motor_cmd()[i];
      motor.mode() = MOTOR_MODE_SERVO;
      motor.q() = POS_STOP_F;
      motor.dq() = VEL_STOP_F;
      motor.kp() = 0.0f;
      motor.kd() = 0.0f;
      motor.tau() = 0.0f;
    }
    for (int i = 0; i < joints; ++i) {
      auto &motor = out.motor_cmd()[model_->slots[i]];
      if (!cmd.applies(i)) {
        apply_hold(motor, hold[i]);
        continue;
      }
      if (!apply_mode(motor, mode, values[i])) return SKR_EINVAL;
    }
    out.crc() = crc32_core(reinterpret_cast<const std::uint32_t *>(&out), (sizeof(out) >> 2) - 1);
    try {
      link_.hg_publisher->Write(out);
    } catch (const std::exception &) {
      return SKR_EIO;
    }
    commit_holds(setpoint, have_setpoint, hold, holding);
    return SKR_OK;
  }

  int emergency_stop() override {
    /* §24's second rule. The latch is set first and unconditionally, so a command racing this one
     * is refused even if nothing can be published; then one message with every gain and every
     * torque at zero, which on an impedance drive is the definition of commanding nothing.
     *
     * What this is **not** is a power cut. A Unitree hardware stop is a button on the robot, and a
     * driver that reported SKR_OK for a message it merely wrote would be claiming the drives were
     * cut. The README says so under what a bench session has to measure. */
    estopped_ = true;
    if (model_ == nullptr) return SKR_OK;
    if (model_->family == Family::GO2) {
      if (!link_.go2_publisher) return SKR_OK;
      unitree_go::msg::dds_::LowCmd_ out{};
      out.head()[0] = 0xFE;
      out.head()[1] = 0xEF;
      out.level_flag() = 0xFF;
      for (std::size_t i = 0; i < GO2_SLOTS; ++i) {
        auto &motor = out.motor_cmd()[i];
        motor.mode() = MOTOR_MODE_SERVO;
        motor.q() = POS_STOP_F;
        motor.dq() = VEL_STOP_F;
        motor.kp() = 0.0f;
        motor.kd() = 0.0f;
        motor.tau() = 0.0f;
      }
      out.crc() = crc32_core(reinterpret_cast<const std::uint32_t *>(&out), (sizeof(out) >> 2) - 1);
      try {
        link_.go2_publisher->Write(out);
      } catch (const std::exception &) {
        return SKR_EIO;
      }
      return SKR_OK;
    }
    if (!link_.hg_publisher) return SKR_OK;
    unitree_hg::msg::dds_::LowCmd_ out{};
    out.mode_pr() = HG_MODE_PR_SERIES;
    {
      std::lock_guard<std::mutex> guard(link_.lock);
      out.mode_machine() = link_.mode_machine;
    }
    for (std::size_t i = 0; i < HG_SLOTS; ++i) {
      auto &motor = out.motor_cmd()[i];
      motor.mode() = MOTOR_MODE_SERVO;
      motor.q() = POS_STOP_F;
      motor.dq() = VEL_STOP_F;
      motor.kp() = 0.0f;
      motor.kd() = 0.0f;
      motor.tau() = 0.0f;
    }
    out.crc() = crc32_core(reinterpret_cast<const std::uint32_t *>(&out), (sizeof(out) >> 2) - 1);
    try {
      link_.hg_publisher->Write(out);
    } catch (const std::exception &) {
      return SKR_EIO;
    }
    return SKR_OK;
  }

  std::string describe() override {
    /* Before any `open()` there is no body — the Go2 the driver opened with, which is what an
     * unopened driver has always described. */
    const ModelSpec *model = model_ != nullptr ? model_ : &MODELS[0];
    const int joints = model->joints;
    std::string json = "{\"schema_version\":1,\"robot_slug\":\"" + slug_ +
                       "\",\"digest\":null,\"verified\":false,\"simulator\":" +
                       (simulator_ ? "true" : "false") + ",\"base\":\"legged\",\"dof\":" +
                       std::to_string(joints) + ",\"joints\":[";
    for (int i = 0; i < joints; ++i) {
      if (i) json += ",";
      /* Effort is per-joint and conditional: the H1's waist and the H1-2's waist and wrists are
       * unpublished, and a joint without a rating omits the field rather than inheriting a
       * neighbour's. Velocity is absent on every joint of every body — Unitree publishes no joint
       * speed anywhere, and the sim configs that carry one are CAD values (§63.4). */
      char effort[64] = {0};
      if (model->effort[i] >= 0.0) {
        std::snprintf(effort, sizeof effort, ",\"effort_max_nm\":%.1f", model->effort[i]);
      }
      char buffer[512];
      std::snprintf(buffer, sizeof buffer,
                    "{\"name\":\"%s\",\"role\":\"%s\",\"type\":\"revolute\","
                    "\"limits\":{\"position_min_rad\":%.5f,\"position_max_rad\":%.5f%s},"
                    "\"control_modes\":[\"position\",\"velocity\",\"torque\"]}",
                    model->names[i], model->roles[i], model->travel[i].low, model->travel[i].high,
                    effort);
      json += buffer;
    }
    char envelope[1024];
    std::snprintf(envelope, sizeof envelope,
                  "\"envelope\":{\"joint_torque_max_nm\":%.1f,\"watchdog_ms\":%.1f,"
                  "\"source\":\"%s\","
                  /* Transcribed from documentation, stated rather than defaulted (§5.3). */
                  "\"basis\":\"declared\"}",
                  model->torque_max_nm, WATCHDOG_MS, model->torque_source);
    /* Rates are conditional for the same reason as effort: the Go2's 500 Hz low-level loop is
     * documented, the humanoids' DDS rate is not, and the community's 500 Hz convention is not a
     * vendor publication. `null` is schema-valid (`Option<f64>`) and readiness treats it as
     * unknown rather than slow. */
    char rates[128];
    if (model->rate_hz_max >= 0.0) {
      std::snprintf(rates, sizeof rates, "\"rate_hz_max\":%.0f,\"rate_hz_min\":%.0f",
                    model->rate_hz_max, model->rate_hz_min);
    } else {
      std::snprintf(rates, sizeof rates, "\"rate_hz_max\":null,\"rate_hz_min\":null");
    }
    json +=
        "],\"end_effectors\":[],\"sensors\":[],"
        "\"control\":{\"modes\":[\"position\",\"velocity\",\"torque\"]," +
        std::string(rates) + ",\"command_latency_ms\":null},";
    json += envelope;
    /* `emergency_stop` latches, publishes zero gains and zero torques, and refuses every later
     * command — the suite covers all three on every body — so the software stop the driver
     * actually has is the capability it declares. The hardware stop stays undeclared for
     * `emergency_stop`'s reason: the button is on the robot and this driver cannot press it.
     *
     * `platform.onboard_compute` is the second declaration, and every body here earns it on its
     * own citation: the Go2's 8-core user-programmable CPU (unitree.com/go2, footnote [4]; the
     * secondary development manual is the programming), the G1's 8-core high-performance CPU with
     * secondary development on the G1 EDU (G1 datasheet — and low-level DDS needs EDU anyway, so
     * a base G1 answers this driver with silence, which `open()` refuses honestly), the H1 and
     * H1-2's Intel Core i5/i7 user development with optional Jetson Orin NX (H1 datasheet). A UR
     * controller or a Franka Control box is a closed computer the OS could never own, and a motor
     * bus is not a computer at all — so no other driver declares this. What the token does not
     * claim is that the OS runs there today, let alone on a real-time kernel: readiness is
     * paperwork, and the deployment and the bench are the evidence. */
    json += ",\"capabilities\":[\"safety.estop\",\"platform.onboard_compute\"],\"solvers\":[]}";
    return json;
  }

 private:
  /* One mode's setpoint into one motor slot, in either family — the member names match, so one
   * template covers both. Position carries `dq = 0` with the operator's `kd`: a setpoint with no
   * damping is an oscillation nobody asked for. */
  template <typename Motor>
  bool apply_mode(Motor &motor, int mode, float value) {
    switch (mode) {
      case SKR_MODE_POSITION:
        motor.q() = value;
        motor.kp() = static_cast<float>(kp_);
        motor.dq() = 0.0f;
        motor.kd() = static_cast<float>(kd_);
        return true;
      case SKR_MODE_VELOCITY:
        motor.dq() = value;
        motor.kd() = static_cast<float>(kd_);
        return true;
      case SKR_MODE_TORQUE:
        motor.tau() = value;
        return true;
      default:
        return false;
    }
  }

  /* §24's hold in either family: the latched target with the operator's gains and a zero velocity
   * setpoint, so the drive pulls the joint back to where it was told to stay and damps it there.
   * `write_command` has already refused a hold with no gains configured. */
  template <typename Motor>
  void apply_hold(Motor &motor, float target) {
    motor.q() = target;
    motor.kp() = static_cast<float>(kp_);
    motor.dq() = 0.0f;
    motor.kd() = static_cast<float>(kd_);
    motor.tau() = 0.0f;
  }

  /* Where the robot last said this joint was, read under the subscriber's lock. `open()` waited for
   * the first state, so this is never the zero the struct starts at. */
  float measured_position(int joint) {
    const std::uint8_t slot = model_->slots[joint];
    std::lock_guard<std::mutex> guard(link_.lock);
    return model_->family == Family::GO2 ? link_.go2_state.motor_state()[slot].q()
                                         : link_.hg_state.motor_state()[slot].q();
  }

  void commit_holds(const float *setpoint, const bool *have_setpoint, const float *hold,
                    const bool *holding) {
    std::copy(setpoint, setpoint + skynet_unitree::MAX_JOINTS, setpoint_);
    std::copy(have_setpoint, have_setpoint + skynet_unitree::MAX_JOINTS, have_setpoint_);
    std::copy(hold, hold + skynet_unitree::MAX_JOINTS, hold_);
    std::copy(holding, holding + skynet_unitree::MAX_JOINTS, holding_);
  }

  /* A legged body whose base pose nothing localises: the IMU gives orientation and nothing gives
   * position. §24 says all-NaN for a base nobody localised, and a driver that wrote the IMU
   * quaternion into a pose would be claiming a position it does not have. */
  static void set_base_pose_unknown(skynet::State &out) {
    auto pose = out.base_pose();
    for (std::size_t i = 0; i < 7; ++i) pose[i] = std::nanf("");
  }

  bool opened_ = false;
  std::atomic<bool> estopped_{false};
  std::uint32_t seq_ = 0;
  double kp_ = 0.0;
  double kd_ = 0.0;
  bool have_kp_ = false;
  bool have_kd_ = false;
  const ModelSpec *model_ = nullptr;
  std::string slug_ = "unitree-go2";
  bool simulator_ = true;

  /* §24's hold, per joint in driver order: the last position setpoint and whether there is one,
   * and the latched hold target and whether the joint is being held. */
  float setpoint_[skynet_unitree::MAX_JOINTS] = {};
  bool have_setpoint_[skynet_unitree::MAX_JOINTS] = {};
  float hold_[skynet_unitree::MAX_JOINTS] = {};
  bool holding_[skynet_unitree::MAX_JOINTS] = {};

  /* The DDS side, which outlives this object — see `Link`. */
  Link &link_ = shared_link();
};

}  // namespace

SKYNET_RHAL_EXPORT(UnitreeBridge)
