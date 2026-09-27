/*
 * layout_check — this tree's transcription of the Lite3 wire, held against the vendor's own
 * headers, at compile time.
 *
 * `lite3_wire.hpp` is a transcription, and a transcription is exactly the kind of thing that is
 * right until it is not. The Unitree row could put the vendor's SDK on both ends of the wire and
 * let DDS settle the question; this wire is C structs in UDP datagrams, so the equivalent argument
 * has to be made at compile time: include the vendor's `robot_types.h` and `command.h` beside
 * this tree's header and assert every size and every offset agrees, field by field.
 *
 * It needs a checkout of MotionSDK, which is not vendored here:
 *
 *   git clone --depth 1 https://github.com/DeepRoboticsLab/lite3_motionsdk /tmp/lite3_motionsdk
 *   make layout LITE3_SDK=/tmp/lite3_motionsdk
 *
 * If this file compiles, the two layouts are the same layout. If it does not, the error names the
 * field that moved.
 *
 * Licence: Apache-2.0 (this file). It includes the vendor's MIT headers and copies none of them.
 */

#include "lite3_wire.hpp"

#include "robot_types.h"
#include "command.h"

#include <cstddef>
#include <cstdio>

namespace ours = skynet_lite3;

/* ── whole structures ─────────────────────────────────────────────────── */

static_assert(sizeof(ours::ImuData) == sizeof(::ImuData), "ImuData is a different size");
static_assert(sizeof(ours::JointData) == sizeof(::JointData), "JointData is a different size");
static_assert(sizeof(ours::JointCmd) == sizeof(::JointCmd), "JointCmd is a different size");
static_assert(sizeof(ours::LegData) == sizeof(::LegData), "LegData is a different size");
static_assert(sizeof(ours::ContactForce) == sizeof(::ContactForce),
              "ContactForce is a different size");
static_assert(sizeof(ours::RobotCmd) == sizeof(::RobotCmd), "RobotCmd is a different size");
static_assert(sizeof(ours::RobotData) == sizeof(::RobotData), "RobotData is a different size");
static_assert(sizeof(ours::EthCommand) == sizeof(::EthCommand), "EthCommand is a different size");
static_assert(sizeof(ours::CommandMessage) == sizeof(::CommandMessage),
              "CommandMessage is a different size");
static_assert(ours::COMMAND_DATA_BUFFER == kCommandDataBufferSize,
              "the data buffer is a different size");

/* ── field by field ───────────────────────────────────────────────────── */

#define SAME_OFFSET(OURS, THEIRS, FIELD)                                     \
  static_assert(offsetof(OURS, FIELD) == offsetof(THEIRS, FIELD),            \
                "field " #FIELD " of " #OURS " sits somewhere else in " #THEIRS)

SAME_OFFSET(ours::ImuData, ::ImuData, timestamp);
SAME_OFFSET(ours::ImuData, ::ImuData, angle_roll);
SAME_OFFSET(ours::ImuData, ::ImuData, angle_pitch);
SAME_OFFSET(ours::ImuData, ::ImuData, angle_yaw);
SAME_OFFSET(ours::ImuData, ::ImuData, angular_velocity_roll);
SAME_OFFSET(ours::ImuData, ::ImuData, angular_velocity_pitch);
SAME_OFFSET(ours::ImuData, ::ImuData, angular_velocity_yaw);
SAME_OFFSET(ours::ImuData, ::ImuData, acc_x);
SAME_OFFSET(ours::ImuData, ::ImuData, acc_y);
SAME_OFFSET(ours::ImuData, ::ImuData, acc_z);

SAME_OFFSET(ours::JointData, ::JointData, position);
SAME_OFFSET(ours::JointData, ::JointData, velocity);
SAME_OFFSET(ours::JointData, ::JointData, torque);
SAME_OFFSET(ours::JointData, ::JointData, temperature);

SAME_OFFSET(ours::JointCmd, ::JointCmd, position);
SAME_OFFSET(ours::JointCmd, ::JointCmd, velocity);
SAME_OFFSET(ours::JointCmd, ::JointCmd, torque);
SAME_OFFSET(ours::JointCmd, ::JointCmd, kp);
SAME_OFFSET(ours::JointCmd, ::JointCmd, kd);

SAME_OFFSET(ours::RobotData, ::RobotData, tick);
SAME_OFFSET(ours::RobotData, ::RobotData, imu);
SAME_OFFSET(ours::RobotData, ::RobotData, joint_data);
SAME_OFFSET(ours::RobotData, ::RobotData, contact_force);

SAME_OFFSET(ours::EthCommand, ::EthCommand, code);
SAME_OFFSET(ours::CommandMessage, ::CommandMessage, command);
SAME_OFFSET(ours::CommandMessage, ::CommandMessage, data_buffer);

/* The vendor's `LegData` and `RobotCmd` reach their arrays through an anonymous union, so the
 * array itself is at offset zero in both and the interesting assertion is that the *leg* views
 * land where this tree's flat twelve put them: `fr_leg` is joints 3..5, `hr_leg` is 9..11. */
static_assert(offsetof(::LegData, fl_leg) == 0, "the front-left leg is not first");
static_assert(offsetof(::LegData, fr_leg) == 3 * sizeof(::JointData), "fr_leg is not joints 3..5");
static_assert(offsetof(::LegData, hl_leg) == 6 * sizeof(::JointData), "hl_leg is not joints 6..8");
static_assert(offsetof(::LegData, hr_leg) == 9 * sizeof(::JointData), "hr_leg is not joints 9..11");
static_assert(offsetof(::RobotCmd, fr_leg) == 3 * sizeof(::JointCmd),
              "the command's front-right leg is not joints 3..5");

/* The type octet this tree packs by hand, against the vendor's bitfield. A `CommandMessage` whose
 * `type` is 1 must read as 1 through `wire_type`, on this machine's packing. */
static bool type_octet_agrees() {
  ::CommandMessage theirs{};
  theirs.command.type = command_type::kMessValues;
  theirs.command.count = 0x123456;
  ours::EthCommand mine{};
  __builtin_memcpy(&mine, &theirs.command, sizeof mine);
  return ours::wire_type(mine.type_and_count) == 1u &&
         ours::wire_count(mine.type_and_count) == 0x123456u;
}

int main() {
  if (!type_octet_agrees()) {
    std::printf(
        "layout_check: the vendor's { type : 8; count : 24 } bitfield does not pack the way "
        "`pack_type_count` spells it on this compiler.\n");
    return 1;
  }
  std::printf(
      "layout_check: this tree's wire and MotionSDK's own headers agree on every size, every "
      "offset and the type octet's packing.\n");
  return 0;
}
