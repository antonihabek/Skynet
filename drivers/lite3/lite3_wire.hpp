/* The wire truth the driver, the fake motion host and the suite share.
 *
 * One header, included by all three, because the datagram is the thing the three must agree on:
 * the driver writes a joint command into it, the fake reads one out of it, and the suite asserts
 * the two met. If each carried its own copy, a disagreement would be three files to diff; as it
 * is, this file is the oracle and any two of the three disagreeing fails loudly.
 *
 * ── Where every constant below comes from ────────────────────────────────
 *
 * The structs are transcribed from Jueying Lite3 MotionSDK's own headers
 * (`DeepRoboticsLab/lite3_motionsdk`, MIT): `include/robot_types.h` for `ImuData`, `JointData`,
 * `JointCmd`, `RobotCmd`, `LegData`, `ContactForce` and `RobotData`, and
 * `include/common/command.h` for `EthCommand` and `CommandMessage`. Nothing here is invented and
 * nothing is reordered — `layout_check.cpp` beside this file compiles the vendor's headers next to
 * this one and static-asserts the two agree field by field, which is the strongest form the
 * "vendor's code on both sides" argument can take for a wire made of C structs.
 *
 * The **ports** are the vendor's documentation twice over: MotionSDK's README §5 prints
 * `~/jy_exe/conf/network.toml` as `target_port = 43897` / `local_port = 43893` (the motion host
 * sends state to 43897 and listens for commands on 43893), `main.cpp` constructs
 * `Sender("192.168.1.120", 43893)`, and `Lite3_rl_deploy`'s `HardwareInterface` takes
 * `local_port=43897, robot_port=43893`.
 *
 * The **command codes** are not documented. MotionSDK's README names exactly one — `0x0906`, the
 * instruction type its `RegisterCallBack` receives — and the rest live inside the prebuilt
 * `lib/libdeeprobotics_legged_sdk_x86_64.so`, whose `Sender` methods are the only published
 * account of them. They were read out of that library's disassembly (2026-09-21) rather than
 * guessed, one per vendor method:
 *
 *   `Sender::SendCmd`          Command(0x0111, 240, &robot_cmd)     the twelve-joint command
 *   `Sender::ControlGet(2)`    Command(0x0114, 0, nullptr)          take control right ("SDK")
 *   `Sender::ControlGet(1)`    damping SendCmd, sleep 2 s, then Command(0x0113, 0, nullptr)
 *   `Sender::AllJointBackZero` Command(0x31010c05, 0, nullptr)      zero every joint, take control
 *   `Sender::RobotStateInit`   AllJointBackZero, usleep(7 s), then 0x0114 twice
 *
 * Reading a constant out of a binary is weaker evidence than reading it out of a document, and
 * this file says so rather than presenting the two as equal. It is corroborated in one direction:
 * `cross_check.cpp` drives the fake with the vendor's own `Sender`, so if any code here were wrong
 * the vendor's client would be seen sending a different one and the cross-check would fail.
 *
 * The **damping shape** `kp = 0, kd = 5, everything else 0` is the vendor's own: it is the
 * `RobotCmd` `ControlGet(ROBOT)` builds on its stack (four 16-byte constants, `5.0f` in each
 * joint's `kd` slot) before handing control back. MotionSDK's README §2 lists it as "Damping
 * Control" and gives the drive law every one of these five fields feeds:
 *
 *     T = kp * (pos_goal - pos_real) + kd * (vel_goal - vel_real) + t_ff
 *
 * which the drive closes at 20 kHz into a current. The same section gives the reclaim: **one
 * second** with no command from the SDK and the underlying controller takes the robot back into
 * damping protection.
 *
 * Licence: Apache-2.0 (this file). The layouts are facts about the vendor's interface, transcribed
 * with the source beside each; nothing of the SDK is vendored here.
 */

#ifndef SKYNET_LITE3_WIRE_HPP
#define SKYNET_LITE3_WIRE_HPP

#include <cstddef>
#include <cstdint>

namespace skynet_lite3 {

/* The motion host listens for commands here, and sends state to the development host there.
 * Both are `network.toml`'s, and both are configurable on the robot — so the driver takes them
 * as config rather than hard-coding what a freshly imaged robot happens to ship with. */
constexpr std::uint16_t COMMAND_PORT = 43893;
constexpr std::uint16_t STATE_PORT = 43897;

/* Read out of the vendor's own `Sender` (see the file header). */
constexpr std::uint32_t CODE_JOINT_CMD = 0x0111;
constexpr std::uint32_t CODE_CONTROL_ROBOT = 0x0113;
constexpr std::uint32_t CODE_CONTROL_SDK = 0x0114;
constexpr std::uint32_t CODE_JOINTS_TO_ZERO = 0x31010c05;
/* The one code the vendor documents: the state push, robot to host. */
constexpr std::uint32_t CODE_STATE_PUSH = 0x0906;

/* `EthCommand`'s third word is `{ uint32_t type : 8; uint32_t count : 24; }`. The vendor's
 * `CmdDone` writes the type as a single byte at offset 8 on x86-64, so the packing is
 * little-endian with the type in the low octet — which is what these two helpers spell out
 * rather than relying on a bitfield's layout being the same in two compilers. */
constexpr std::uint32_t TYPE_SINGLE_VALUE = 0;
constexpr std::uint32_t TYPE_MESS_VALUES = 1;

constexpr std::size_t COMMAND_DATA_BUFFER = 1024;

/* The twelve joints, in the order `RobotData::joint_data` and `RobotCmd::joint_cmd` carry them:
 * front-left, front-right, hind-left, hind-right, each HipX (abduction), HipY (hip flexion),
 * Knee. The names are the vendor's own, from `Lite3_rl_deploy`'s
 * `run_policy/lite3_test_policy_runner_onnx.cpp`. */
constexpr int JOINTS = 12;

struct EthCommand {
  std::uint32_t code;
  /* A union in the vendor's header: `value` for a single-value command, `paramters_size`
   * (their spelling) for one carrying parameters. One field here, because a union of two
   * `uint32_t` is one `uint32_t` and naming it twice invited the vendor's own defect — see
   * README, "what the single-value path does". */
  std::uint32_t value_or_size;
  std::uint32_t type_and_count;
};

struct CommandMessage {
  EthCommand command;
  char data_buffer[COMMAND_DATA_BUFFER];
};

struct ImuData {
  std::int32_t timestamp;
  /* Degrees and degrees/second, not radians — `Lite3_rl_deploy`'s `HardwareInterface::GetImuRpy`
   * divides by 180 and multiplies by pi, which is the vendor stating the unit in code. */
  float angle_roll;
  float angle_pitch;
  float angle_yaw;
  float angular_velocity_roll;
  float angular_velocity_pitch;
  float angular_velocity_yaw;
  float acc_x;
  float acc_y;
  float acc_z;
};

struct JointData {
  float position;     /* radians */
  float velocity;     /* radians/second */
  float torque;       /* Nm */
  float temperature;  /* Celsius */
};

struct LegData {
  JointData joint_data[JOINTS];
};

struct ContactForce {
  /* Three components per leg, in the vendor's words "in most cases, only the z-direction force
   * is used". Doubles, where everything else on this wire is a float. */
  double leg_force[JOINTS];
};

struct JointCmd {
  float position;
  float velocity;
  float torque;
  float kp;
  float kd;
};

struct RobotCmd {
  JointCmd joint_cmd[JOINTS];
};

struct RobotData {
  std::uint32_t tick;  /* milliseconds, per `GetInterfaceTimeStamp`: `tick * 0.001` */
  ImuData imu;
  LegData joint_data;
  ContactForce contact_force;
};

/* The sizes the vendor's own compiler produces, asserted here so that a transcription slip is a
 * failed build rather than a robot reading somebody else's joint. `layout_check.cpp` re-asserts
 * them against the vendor's headers where a checkout is available. */
static_assert(sizeof(EthCommand) == 12, "EthCommand is three words");
static_assert(offsetof(CommandMessage, data_buffer) == 12, "the payload starts after the header");
static_assert(sizeof(ImuData) == 40, "ImuData is a timestamp and nine floats");
static_assert(sizeof(JointData) == 16, "JointData is four floats");
static_assert(sizeof(JointCmd) == 20, "JointCmd is five floats");
static_assert(sizeof(RobotCmd) == 240, "twelve joints of five floats — the 0x0111 payload");
static_assert(sizeof(ContactForce) == 96, "twelve doubles");
static_assert(sizeof(RobotData) == 336, "tick, imu, twelve joints, twelve forces");

/* What goes on the wire for each of the two shapes. The vendor's `CmdDone` sends
 * `sizeof(EthCommand) + size`, and for a command with no parameters `size` is the union's other
 * half and is zero — so a control-right command is exactly twelve bytes and a joint command is
 * 252. */
constexpr std::size_t CONTROL_DATAGRAM = sizeof(EthCommand);
constexpr std::size_t JOINT_DATAGRAM = sizeof(EthCommand) + sizeof(RobotCmd);
constexpr std::size_t STATE_DATAGRAM = sizeof(EthCommand) + sizeof(RobotData);

constexpr std::uint32_t pack_type_count(std::uint32_t type, std::uint32_t count) {
  return (type & 0xFFu) | ((count & 0x00FFFFFFu) << 8);
}
constexpr std::uint32_t wire_type(std::uint32_t type_and_count) { return type_and_count & 0xFFu; }
constexpr std::uint32_t wire_count(std::uint32_t type_and_count) { return type_and_count >> 8; }

/* The vendor's own damping command, the one `ControlGet(ROBOT)` sends before handing control
 * back: every setpoint and both the stiffness and the feed-forward at zero, `kd` alone non-zero.
 * On the drive law above that is "resist motion, command no position", and it is the only shape
 * on this wire that is neither a setpoint nor a limp joint. */
constexpr float VENDOR_DAMPING_KD = 5.0f;

/* The vendor's reclaim, README §2: one second without a command from the SDK and the underlying
 * controller takes the robot back. The driver's watchdog window is a fraction of it, and says so
 * in its descriptor rather than claiming the second as its own. */
constexpr double RECLAIM_MS = 1000.0;

/* The twelve joint names, the vendor's own (`Lite3_rl_deploy`,
 * `run_policy/lite3_test_policy_runner_onnx.cpp`). Shared, because the fake prints them and the
 * suite pins them. */
inline const char *const JOINT_NAMES[JOINTS] = {
    "FL_HipX_joint", "FL_HipY_joint", "FL_Knee_joint", "FR_HipX_joint",
    "FR_HipY_joint", "FR_Knee_joint", "HL_HipX_joint", "HL_HipY_joint",
    "HL_Knee_joint", "HR_HipX_joint", "HR_HipY_joint", "HR_Knee_joint",
};

}  // namespace skynet_lite3

#endif  // SKYNET_LITE3_WIRE_HPP
