/* The wire truth the driver, the fake controller and the suite share.
 *
 * One header, included by all three, for the reason `drivers/lite3/lite3_wire.hpp` gives: the
 * frame is the thing the three have to agree on, and if each carried its own copy a disagreement
 * would be three files to diff instead of one file to fail.
 *
 * ── Where every constant below comes from ────────────────────────────────
 *
 * FAIRINO (FAIR INNOVATION) publish their C++ SDK **with its source**, Apache-2.0, at
 * `FAIR-INNOVATION/fairino-cpp-sdk` (tag `v2.3.9-robot3.9.9`, read 2026-09-21). That is a
 * different class of evidence from the Lite3 row, where four command codes had to be read out of a
 * disassembly: here every number below was read out of the vendor's own source, and the file and
 * line are named beside each one.
 *
 *   ports            `libfairino/src/src/Robot/robot.cpp`: `ROBOT_CMD_PORT 8080`,
 *                    `ROBOT_CNDE_TCP_PORT 20005`, `ROBOT_UDP_CMD_PORT 20007`, and
 *                    `XmlRpcClient c(serverUrl, 20003)` at every command site. Port 20004 is the
 *                    controller's older fixed-layout state push, still settable through
 *                    `SetRobotRealtimeStateSamplePeriod` and no longer read by this SDK — this
 *                    driver speaks the one the vendor's own client speaks, which is 20005.
 *   CNDE framing     `libfairino/src/src/CNDE/CNDEFrameHandle.cpp` and its header:
 *                    `5A 5A | count | type | len (LE16) | data | A5 A5`, total `len + 8`, and the
 *                    five frame types.
 *   the state names  `libfairino/src/src/CNDE/FRCNDEClient.cpp`, `InitAllStates()`. A CNDE stream
 *                    carries exactly the fields the client asked for, in the order it asked for
 *                    them, so the names *are* the layout.
 *   the stop         `libfairino/src/src/Robot/robot.cpp`, `FRRobot::StopMotion()`, which writes
 *                    the literal frame `/f/bIII44III102III4IIISTOPIII/b/f` to port 8080. The
 *                    driver's `emergency_stop()` sends those bytes — the vendor's own, character
 *                    for character, rather than this tree's idea of a stop.
 *   the method names `robot.cpp` again, one `c.execute("Name", param, result)` per verb.
 *
 * `layout_check.cpp` beside this file compiles the vendor's own headers next to this one and
 * static-asserts the field sizes and the frame constants agree; `cross_check.cpp` drives the fake
 * with the vendor's own `FRRobot`, unmodified, so a name or a size that were wrong here would be
 * seen as the vendor's client failing to configure, to command or to read back.
 *
 * Licence: Apache-2.0 (this file), the same licence the SDK it describes carries. Nothing of the
 * SDK is vendored here; what is here is a description of an interface, with the vendor's file
 * named beside every line of it.
 */

#ifndef SKYNET_FAIRINO_WIRE_HPP
#define SKYNET_FAIRINO_WIRE_HPP

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string_view>

namespace skynet_fairino {

/* ── the four sockets an FR controller answers on ─────────────────────── */

/* XML-RPC over HTTP/1.1. Every verb in `robot.cpp` opens one of these. */
constexpr std::uint16_t XMLRPC_PORT = 20003;
/* The instant-command channel: ASCII frames, and the only path `StopMotion()` takes. */
constexpr std::uint16_t COMMAND_PORT = 8080;
/* CNDE — the configurable state stream this driver reads. */
constexpr std::uint16_t STATE_PORT = 20005;
/* The older fixed-layout push, `sizeof(ROBOT_STATE_PKG)` bytes a period. Named here because the
 * SDK still sets its period and because `layout_check.cpp` asserts what it would have carried;
 * this driver does not read it. */
constexpr std::uint16_t LEGACY_STATE_PORT = 20004;
/* Servo commands may go by UDP instead of XML-RPC (`comType == 1`). Not used here: a datagram
 * path with no reply cannot report a refusal, and §24 needs the refusal. */
constexpr std::uint16_t UDP_COMMAND_PORT = 20007;

/* The controller's factory address, `FRRobot::FRRobot()`. Config, never assumed. */
constexpr const char *DEFAULT_HOST = "192.168.58.2";

/* ── CNDE frames ──────────────────────────────────────────────────────── */

constexpr std::uint8_t CNDE_HEAD_0 = 0x5A;
constexpr std::uint8_t CNDE_HEAD_1 = 0x5A;
constexpr std::uint8_t CNDE_TAIL_0 = 0xA5;
constexpr std::uint8_t CNDE_TAIL_1 = 0xA5;

constexpr std::uint8_t CNDE_TYPE_OUTPUT_CONFIG = 1;
constexpr std::uint8_t CNDE_TYPE_START = 2;
constexpr std::uint8_t CNDE_TYPE_STOP = 3;
constexpr std::uint8_t CNDE_TYPE_OUTPUT_STATE = 4;
constexpr std::uint8_t CNDE_TYPE_MESSAGE = 6;

/* head, count, type, len, tail — everything that is not payload. */
constexpr std::size_t CNDE_OVERHEAD = 8;
/* `CNDE_MAX_PKG_SIZE`, `libfairino/src/include/Base/Utility.h`. */
constexpr std::size_t CNDE_MAX_FRAME = 4096;
/* A reply's first byte: zero is "done", anything else is a failure whose text follows. */
constexpr std::uint8_t CNDE_MESSAGE_OK = 0x00;
constexpr std::uint8_t CNDE_MESSAGE_FAIL = 0x01;

/* `FRCNDEClient`'s own default, and the period this driver asks for unless told otherwise.
 * `SetRobotRealtimeStatePeriod` documents the range as 4..1000 ms. */
constexpr int CNDE_DEFAULT_PERIOD_MS = 8;
constexpr int CNDE_MIN_PERIOD_MS = 4;
constexpr int CNDE_MAX_PERIOD_MS = 1000;

/* Write a CNDE frame into `out`, returning its length. `out` must hold `payload_len + 8`. */
inline std::size_t build_cnde_frame(unsigned char *out, std::uint8_t count, std::uint8_t type,
                                    const void *payload, std::size_t payload_len) {
  out[0] = CNDE_HEAD_0;
  out[1] = CNDE_HEAD_1;
  out[2] = count;
  out[3] = type;
  out[4] = static_cast<unsigned char>(payload_len & 0xFFu);
  out[5] = static_cast<unsigned char>((payload_len >> 8) & 0xFFu);
  if (payload_len > 0 && payload != nullptr) std::memcpy(out + 6, payload, payload_len);
  out[6 + payload_len] = CNDE_TAIL_0;
  out[7 + payload_len] = CNDE_TAIL_1;
  return payload_len + CNDE_OVERHEAD;
}

/* ── the instant-command channel, port 8080 ───────────────────────────── */

/* `/f/b` … `III`-separated … `/b/f`, per `FrameHandle.cpp`'s `PackFrame`/`UnpacketFrame`. */
constexpr const char *FRAME_HEAD = "/f/b";
constexpr const char *FRAME_TAIL = "/b/f";
constexpr const char *FRAME_SEPARATOR = "III";

/* The vendor's own stop, byte for byte: `FRRobot::StopMotion()` formats exactly this and hands it
 * to the thread that owns the 8080 socket. Command 102, content "STOP", and the count field is
 * the literal 44 in the vendor's `sprintf` — see the README, which counts that among the defects
 * this row found rather than copying it silently. */
constexpr const char *STOP_FRAME = "/f/bIII44III102III4IIISTOPIII/b/f";
constexpr int FRAME_ID_STOP = 102;
/* Thirty-three characters, asserted rather than counted by hand: `emergency_stop()` sends exactly
 * this many bytes from a signal handler and a length that disagreed with the literal would be a
 * truncated stop. */
constexpr std::size_t STOP_FRAME_BYTES = 33;
static_assert(sizeof("/f/bIII44III102III4IIISTOPIII/b/f") - 1 == STOP_FRAME_BYTES,
              "the vendor's own StopMotion frame, character for character");

/* ── the XML-RPC verbs this driver uses ───────────────────────────────── */

constexpr const char *RPC_MODE = "Mode";                      /* 0 auto, 1 manual */
constexpr const char *RPC_ROBOT_ENABLE = "RobotEnable";       /* 1 enable, 0 disable */
constexpr const char *RPC_SERVO_MOVE_START = "ServoMoveStart";
constexpr const char *RPC_SERVO_MOVE_END = "ServoMoveEnd";
constexpr const char *RPC_SERVO_J = "ServoJ";
constexpr const char *RPC_SERVO_JV = "ServoJV";
constexpr const char *RPC_SERVO_JT_START = "ServoJTStart";
constexpr const char *RPC_SERVO_JT = "ServoJT";
constexpr const char *RPC_SERVO_JT_END = "ServoJTEnd";
constexpr const char *RPC_GET_SOFTWARE_VERSION = "GetSoftwareVersion";
constexpr const char *RPC_GET_ROBOT_SN = "GetRobotSN";
constexpr const char *RPC_GET_JOINT_SOFT_LIMIT = "GetJointSoftLimitDeg";

/* ── the body ─────────────────────────────────────────────────────────── */

constexpr int JOINTS = 6;

/* The names the vendor's own description uses (`fairino_description`'s URDFs call the six joints
 * `j1`..`j6`), and the names its catalogue page uses for the same six axes. */
inline const char *const JOINT_NAMES[JOINTS] = {"j1", "j2", "j3", "j4", "j5", "j6"};
inline const char *const AXIS_NAMES[JOINTS] = {"base",    "shoulder", "elbow",
                                               "wrist_1", "wrist_2",  "wrist_3"};

/* Everything on this wire is degrees, degrees per second, millimetres and newton-metres — the
 * SDK's own doc comments say so on every entry point. §24 is radians, so the conversion happens
 * once, at the edge, and both directions are named. */
constexpr double DEG_TO_RAD = 3.14159265358979323846 / 180.0;
constexpr double RAD_TO_DEG = 180.0 / 3.14159265358979323846;

/* ── the state fields this driver asks the controller for ─────────────── */

/* CNDE carries what the client asked for, in the order it asked for it, so a field list is a
 * layout. These eighteen are what §24's `State` needs plus what its flags need plus what the
 * telemetry extension reports; asking for the SDK's default seventy-three would move about four
 * times the bytes for fields nothing here reads.
 *
 * Every `name` is the vendor's, from `InitAllStates()`, and every `bytes` is the size that entry's
 * CNDE type has there (its fourth tuple element, which is *not* always the struct's own type —
 * `gripper_fault` is `UINT16` in the packet and `INT32` on the stream, which is why the SDK has a
 * conversion table and why this one carries the stream's size rather than the struct's). */
struct StateField {
  const char *name;
  std::size_t bytes;
};

constexpr StateField STATE_FIELDS[] = {
    {"program_state", 1},        /* 1 stop, 2 run, 3 pause */
    {"robot_state", 1},          /* 1 stop, 2 run, 3 pause, 4 drag */
    {"main_code", 4},            /* the controller's fault word, §24's `fault` */
    {"sub_code", 4},
    {"robot_mode", 1},           /* 0 auto, 1 manual */
    {"actual_joint_pos", 48},    /* six doubles, degrees */
    {"actual_joint_vel", 48},    /* six doubles, degrees/second */
    {"actual_joint_torque", 48}, /* six doubles, N.m */
    {"emergency_stop", 1},       /* the hardware button, 1 while pressed */
    {"safety_stop0_state", 1},   /* SI0 — the pair `GetSafetyCode()` refuses motion on */
    {"safety_stop1_state", 1},   /* SI1 */
    {"collision_state", 1},
    {"motion_done", 4},
    {"motion_queue_len", 4},
    {"rbt_enable_state", 4},
    {"joint_driver_torque", 48},
    {"actual_joint_temp", 48},   /* the drives' temperatures, six doubles, Celsius */
    {"last_servoJ_target", 48},  /* the controller's own last ServoJ setpoint */
};

constexpr std::size_t STATE_FIELD_COUNT = sizeof(STATE_FIELDS) / sizeof(STATE_FIELDS[0]);

/* Offsets into the payload, computed from the list above rather than written down twice. */
constexpr std::size_t state_offset(std::size_t index) {
  std::size_t at = 0;
  for (std::size_t i = 0; i < index; ++i) at += STATE_FIELDS[i].bytes;
  return at;
}

constexpr std::size_t state_payload_bytes() { return state_offset(STATE_FIELD_COUNT); }

enum StateIndex {
  IDX_PROGRAM_STATE = 0,
  IDX_ROBOT_STATE,
  IDX_MAIN_CODE,
  IDX_SUB_CODE,
  IDX_ROBOT_MODE,
  IDX_JOINT_POS,
  IDX_JOINT_VEL,
  IDX_JOINT_TORQUE,
  IDX_EMERGENCY_STOP,
  IDX_SAFETY_STOP0,
  IDX_SAFETY_STOP1,
  IDX_COLLISION,
  IDX_MOTION_DONE,
  IDX_QUEUE_LEN,
  IDX_ENABLE_STATE,
  IDX_DRIVER_TORQUE,
  IDX_JOINT_TEMP,
  IDX_LAST_SERVO_TARGET,
};

static_assert(STATE_FIELD_COUNT == 18, "the enum above and the table are one list");
static_assert(STATE_FIELDS[IDX_JOINT_POS].bytes == sizeof(double) * JOINTS,
              "six joints of double, the vendor's DOUBLE_6");
static_assert(state_payload_bytes() == 315, "eighteen fields, 315 bytes on the stream");

/* The field list as the config frame carries it: the names in order, comma-separated. Built once
 * so the driver, the fake and the suite cannot disagree about it. */
inline std::string_view state_field_list(char *buffer, std::size_t capacity) {
  std::size_t at = 0;
  for (std::size_t i = 0; i < STATE_FIELD_COUNT; ++i) {
    const std::size_t len = std::strlen(STATE_FIELDS[i].name);
    if (at + len + (i ? 1u : 0u) + 1u > capacity) return std::string_view();
    if (i) buffer[at++] = ',';
    std::memcpy(buffer + at, STATE_FIELDS[i].name, len);
    at += len;
  }
  buffer[at] = '\0';
  return std::string_view(buffer, at);
}

/* Little-endian doubles off the stream. The controller is x86-64 and so is every host this
 * driver runs on, and the SDK `memcpy`s straight into its struct — but the read is spelled out
 * rather than aliased, because a `reinterpret_cast` over a possibly-unaligned payload offset is
 * undefined behaviour even where it happens to work. */
inline double read_double(const unsigned char *payload, std::size_t at) {
  double value = 0.0;
  std::memcpy(&value, payload + at, sizeof value);
  return value;
}

inline std::int32_t read_int32(const unsigned char *payload, std::size_t at) {
  std::int32_t value = 0;
  std::memcpy(&value, payload + at, sizeof value);
  return value;
}

inline std::uint8_t read_uint8(const unsigned char *payload, std::size_t at) {
  return payload[at];
}

}  // namespace skynet_fairino

#endif  // SKYNET_FAIRINO_WIRE_HPP
