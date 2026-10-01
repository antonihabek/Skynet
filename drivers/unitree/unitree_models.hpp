/* The wire truth the driver, the fake and the suite share.
 *
 * One header, included by all three, because the slot map is the thing the three must agree on:
 * the driver writes joint i into a slot, the fake publishes joint i from a slot, and the suite
 * asserts the two met. If each carried its own copy, a disagreement would be three files to diff;
 * as it is, the maps below are the oracle and any two of the three disagreeing fails loudly.
 *
 * What is shared is ONLY the wire: the family, the joint count and the slot map. Names, roles,
 * travel and ratings are the driver's descriptor content and stay in `unitree_bridge.cpp`; the
 * suite pins them from the model's expectations instead.
 *
 * Every map is transcribed from the vendor's published low-level examples (unitree_sdk2,
 * BSD-3-Clause), and each says which:
 *
 *   go2  — twelve joints, slots 0..11 of the twenty-slot `go2` message. The vendor's Go2 example
 *          addresses the legs in order; the eight slots above 11 belong to larger bodies.
 *   b2   — the same twelve joints in the same slots 0..11 of the `go2` message: the vendor's B2
 *          example (`example/b2/b2_stand_example.cpp`) addresses twelve motors in that message, and
 *          `unitree_mujoco`'s `b2.xml` lists its twelve actuators in the Go2's leg order
 *          (front-right, front-left, rear-right, rear-left, each hip/thigh/calf), which `sim_check`
 *          holds the slots to by name. The wire is the Go2's; what differs is the machine behind
 *          it (travel and ratings, in the bridge).
 *   h1   — nineteen joints on the `go2`-shaped message, per `example/h1/low_level/motors.hpp`:
 *          legs at 0..5, 7, 8, 10, 11, waist yaw at 6, arms at 12..19, and slot 9 is the
 *          vendor's own `kNotUsedJoint`.
 *   g1   — twenty-three joints on the thirty-five-slot `hg` message, per `G1JointIndex` in
 *          `example/g1/low_level/g1_ankle_swing_example.cpp`: legs 0..11, waist yaw 12, arms
 *          15..19 and 22..26. Sparse — slots 13, 14, 20, 21, 27, 28 are the 29-DoF body's and
 *          are marked "NOTE INVALID for g1 23dof" in the example. See the bridge's header for why
 *          the simulator's contiguous packing is not followed.
 *   h1_2 — twenty-seven joints on `hg`, per `H1JointIndex` in
 *          `example/h1/low_level/h1_27dof_example.cpp`: legs 0..11, waist 12, arms 13..26.
 *   r1   — twenty-six motors on `hg`, per `joint_idx_in_idl` in
 *          `example/r1/low_level/r1_ankle_swing_example.cpp`: legs 0..11, waist roll and yaw 12, 13,
 *          the left arm's five 15..19, the right arm's five 22..26, head pitch and yaw 29, 30. Sparse:
 *          slots 14, 20, 21, 27 and 28 are the 29-joint layout's waist pitch and wrist pitch and yaw,
 *          which the standard R1 does not have, and `unitree_mujoco`'s `R1_C++.xml` carries them as
 *          placeholder joints in those slots. It carries no head at all.
 *   h2   — thirty-one joints on `hg`, per `H2JointIndex` in
 *          `example/h2/low_level/h2_ankle_swing_example.cpp`: legs 0..11 (the ankle's roll at 4 and
 *          10 *before* its pitch at 5 and 11, the other way round from the H1-2's), waist yaw, roll
 *          and pitch 12..14, arms 15..28 (seven each), head pitch and yaw 29, 30. Contiguous, and
 *          `unitree_mujoco`'s `h2_mujoco.xml` lists its thirty-one actuators in the same order.
 *
 * Licence: Apache-2.0 (this file). The maps are facts about the vendor's interface, transcribed
 * with the source beside each.
 */

#ifndef SKYNET_UNITREE_MODELS_HPP
#define SKYNET_UNITREE_MODELS_HPP

#include <cstddef>
#include <cstdint>
#include <string>

namespace skynet_unitree {

/* Twenty motor slots for `go2`, thirty-five for `hg`. */
enum class Family { GO2, HG };
constexpr std::size_t GO2_SLOTS = 20;
constexpr std::size_t HG_SLOTS = 35;
/* The largest joint count below, so the fake and the suite can size static buffers. */
constexpr int MAX_JOINTS = 31;

struct WireModel {
  const char *model;
  Family family;
  int joints;
  const std::uint8_t *slots;
};

constexpr std::uint8_t GO2_SLOTS_MAP[12] = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11};

constexpr std::uint8_t H1_SLOTS[19] = {
    7, 4, 3, 5, 10, 8, 1, 0, 2, 11, 6, 16, 17, 18, 19, 12, 13, 14, 15,
};

constexpr std::uint8_t G1_SLOTS[23] = {
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 15, 16, 17, 18, 19, 22, 23, 24, 25, 26,
};

constexpr std::uint8_t H1_2_SLOTS[27] = {
    0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15, 16, 17, 18,
    19, 20, 21, 22, 23, 24, 25, 26,
};

constexpr std::uint8_t H2_SLOTS[31] = {
    0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15,
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30,
};

constexpr std::uint8_t R1_SLOTS[26] = {
    0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13,
    15, 16, 17, 18, 19, 22, 23, 24, 25, 26, 29, 30,
};

constexpr WireModel WIRE_MODELS[7] = {
    {"go2", Family::GO2, 12, GO2_SLOTS_MAP},
    {"b2", Family::GO2, 12, GO2_SLOTS_MAP},
    {"h1", Family::GO2, 19, H1_SLOTS},
    {"g1", Family::HG, 23, G1_SLOTS},
    {"h1_2", Family::HG, 27, H1_2_SLOTS},
    {"h2", Family::HG, 31, H2_SLOTS},
    {"r1", Family::HG, 26, R1_SLOTS},
};

inline const WireModel *find_wire_model(const std::string &model) {
  for (const WireModel &wire : WIRE_MODELS) {
    if (model == wire.model) return &wire;
  }
  return nullptr;
}

inline std::size_t family_slots(Family family) { return family == Family::GO2 ? GO2_SLOTS : HG_SLOTS; }

}  // namespace skynet_unitree

#endif  // SKYNET_UNITREE_MODELS_HPP
