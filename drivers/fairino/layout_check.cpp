/*
 * This tree's reading of the FR wire, held against FAIR INNOVATION's own source.
 *
 * Two kinds of check, because the wire has two kinds of fact in it:
 *
 *   **Compile time.** The fields this driver asks the CNDE stream for are named in the vendor's
 *   `InitAllStates()` and carried in `ROBOT_STATE_PKG`. The sizes in `fairino_wire.hpp` are
 *   asserted here against `sizeof` of the vendor's own struct members, so a field this tree
 *   thought was six doubles and is not fails the build rather than a robot.
 *
 *   **Read from the source.** The ports and the stop frame are not in any header — they are
 *   `#define`s and a `sprintf` inside `robot.cpp`. Rather than transcribe them and hope, this
 *   program opens the vendor's own file and requires the text to be there. The stop frame
 *   matters most: `emergency_stop()` sends thirty-three characters from a signal handler, and
 *   this is what says those characters are the vendor's.
 *
 * Needs a checkout:
 *
 *   git clone --depth 1 https://github.com/FAIR-INNOVATION/fairino-cpp-sdk /tmp/fairino-cpp-sdk
 *   git -C /tmp/fairino-cpp-sdk checkout v2.3.9-robot3.9.9
 *   make layout FAIRINO_SDK=/tmp/fairino-cpp-sdk
 *
 * Licence: Apache-2.0.
 */

#include "fairino_wire.hpp"

#include "robot_types.h"
#include "CNDEFrameHandle.h"
#include "Utility.h"

#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>

/* ── compile time: the packet the names refer to ──────────────────────── */

static_assert(sizeof(ROBOT_STATE_PKG) == 2075,
              "the fixed-layout packet is 2075 bytes at robot 3.9.9 — the legacy port 20004 "
              "carries this, and its data_len is this minus seven");

#define SAME_SIZE(index, member)                                                             \
  static_assert(skynet_fairino::STATE_FIELDS[index].bytes == sizeof(ROBOT_STATE_PKG::member), \
                "the stream's size for this field is the packet's size for " #member)

SAME_SIZE(skynet_fairino::IDX_PROGRAM_STATE, program_state);
SAME_SIZE(skynet_fairino::IDX_ROBOT_STATE, robot_state);
SAME_SIZE(skynet_fairino::IDX_MAIN_CODE, main_code);
SAME_SIZE(skynet_fairino::IDX_SUB_CODE, sub_code);
SAME_SIZE(skynet_fairino::IDX_ROBOT_MODE, robot_mode);
SAME_SIZE(skynet_fairino::IDX_JOINT_POS, jt_cur_pos);
SAME_SIZE(skynet_fairino::IDX_JOINT_VEL, actual_qd);
SAME_SIZE(skynet_fairino::IDX_JOINT_TORQUE, jt_cur_tor);
SAME_SIZE(skynet_fairino::IDX_EMERGENCY_STOP, EmergencyStop);
SAME_SIZE(skynet_fairino::IDX_SAFETY_STOP0, safety_stop0_state);
SAME_SIZE(skynet_fairino::IDX_SAFETY_STOP1, safety_stop1_state);
SAME_SIZE(skynet_fairino::IDX_COLLISION, collisionState);
SAME_SIZE(skynet_fairino::IDX_MOTION_DONE, motion_done);
SAME_SIZE(skynet_fairino::IDX_QUEUE_LEN, mc_queue_len);
SAME_SIZE(skynet_fairino::IDX_ENABLE_STATE, rbtEnableState);
SAME_SIZE(skynet_fairino::IDX_DRIVER_TORQUE, jointDriverTorque);
SAME_SIZE(skynet_fairino::IDX_JOINT_TEMP, jointDriverTemperature);
SAME_SIZE(skynet_fairino::IDX_LAST_SERVO_TARGET, lastServoTarget);

static_assert(sizeof(ROBOT_STATE_PKG::jt_cur_pos) == sizeof(double) * skynet_fairino::JOINTS,
              "six joints, six doubles — an FR is a six-axis arm on this wire");

/* The CNDE frame's own constants, against the vendor's header rather than against this tree's
 * memory of it. */
static_assert(skynet_fairino::CNDE_TYPE_OUTPUT_CONFIG == CNDE_FRAME_TYPE_OUTPUT_CONFIG, "type 1");
static_assert(skynet_fairino::CNDE_TYPE_START == CNDE_FRAME_TYPE_START, "type 2");
static_assert(skynet_fairino::CNDE_TYPE_STOP == CNDE_FRAME_TYPE_STOP, "type 3");
static_assert(skynet_fairino::CNDE_TYPE_OUTPUT_STATE == CNDE_FRAME_TYPE_OUTPUT_STATE, "type 4");
static_assert(skynet_fairino::CNDE_TYPE_MESSAGE == CNDE_FRAME_TYPE_MESSAGE, "type 6");
static_assert(skynet_fairino::CNDE_MAX_FRAME == CNDE_MAX_PKG_SIZE, "the vendor's frame ceiling");

/* `CNDE_PKG`'s head and tail are defaults on the struct, which is the vendor stating them. */
static_assert(sizeof(CNDE_PKG::head) == 2 && sizeof(CNDE_PKG::end) == 2,
              "two bytes of head and two of tail");

namespace {

int CHECKS = 0;
int FAILURES = 0;

void check(bool held, const char *what) {
  CHECKS += 1;
  if (held) {
    std::printf("  ok    %s\n", what);
  } else {
    FAILURES += 1;
    std::printf("  FAIL  %s\n", what);
  }
}

std::string slurp(const std::string &path) {
  std::ifstream file(path, std::ios::binary);
  if (!file) return std::string();
  std::ostringstream out;
  out << file.rdbuf();
  return out.str();
}

bool has(const std::string &haystack, const std::string &needle) {
  return haystack.find(needle) != std::string::npos;
}

}  // namespace

int main(int argc, char **argv) {
  const std::string sdk = argc > 1 ? argv[1] : FAIRINO_SDK_PATH;
  const std::string robot_cpp = slurp(sdk + "/libfairino/src/src/Robot/robot.cpp");
  const std::string cnde_cpp = slurp(sdk + "/libfairino/src/src/CNDE/FRCNDEClient.cpp");
  const std::string tcp_cpp = slurp(sdk + "/libfairino/src/src/ComClient/FRTcpClient.cpp");
  const std::string cnde_hpp = slurp(sdk + "/libfairino/src/include/CNDE/FRCNDEClient.h");
  /* The servo verbs live in their own translation unit, which is the vendor's split and not this
   * tree's business — both files are searched rather than assuming which holds what. */
  const std::string motion_cpp = slurp(sdk + "/libfairino/src/src/Robot/robotMotion.cpp");
  const std::string commands = robot_cpp + motion_cpp;

  std::printf("\n== the ports, read out of the vendor's own source ==\n");
  check(!robot_cpp.empty(), "robot.cpp is where the SDK path says it is");
  check(has(robot_cpp, "#define ROBOT_CNDE_TCP_PORT 20005"), "the CNDE state port is 20005");
  check(has(robot_cpp, "#define ROBOT_CMD_PORT 8080"), "the instant-command port is 8080");
  check(has(robot_cpp, "#define ROBOT_UDP_CMD_PORT 20007"), "the UDP command port is 20007");
  check(has(robot_cpp, "XmlRpcClient c(serverUrl, 20003)"), "every verb goes to XML-RPC on 20003");
  check(has(robot_cpp, "20004"), "and 20004 is still named, as the port whose period is settable");

  std::printf("\n== the stop this driver sends from a signal handler ==\n");
  check(has(robot_cpp, "/f/bIII44III102III4IIISTOPIII/b/f"),
        "the stop frame is in the vendor's source, character for character");
  check(std::string(skynet_fairino::STOP_FRAME) == "/f/bIII44III102III4IIISTOPIII/b/f",
        "and it is the one this tree sends");
  check(has(robot_cpp, "errno_t FRRobot::StopMotion()"),
        "StopMotion is the verb it belongs to, and it is not an XML-RPC one");

  std::printf("\n== the verbs ==\n");
  const char *const verbs[] = {
      "\"Mode\"",         "\"RobotEnable\"",   "\"ServoMoveStart\"", "\"ServoMoveEnd\"",
      "\"ServoJ\"",       "\"ServoJV\"",       "\"ServoJTStart\"",   "\"ServoJT\"",
      "\"ServoJTEnd\"",   "\"GetSoftwareVersion\"", "\"GetRobotSN\"",
      "\"GetJointSoftLimitDeg\""};
  for (const char *verb : verbs) {
    const std::string needle = std::string("c.execute(") + verb;
    check(has(commands, needle), (std::string("the vendor executes ") + verb).c_str());
  }

  std::printf("\n== the field names this driver asks the stream for ==\n");
  check(!cnde_cpp.empty(), "FRCNDEClient.cpp is where the SDK path says it is");
  for (std::size_t i = 0; i < skynet_fairino::STATE_FIELD_COUNT; ++i) {
    const std::string needle = std::string("\"") + skynet_fairino::STATE_FIELDS[i].name + "\"";
    check(has(cnde_cpp, needle),
          (std::string("the controller knows the name ") + skynet_fairino::STATE_FIELDS[i].name)
              .c_str());
  }

  std::printf("\n== the framing ==\n");
  check(has(cnde_cpp, "SetCNDEStart") && has(cnde_cpp, "SendCNDEOutputConfig"),
        "a CNDE session is a configuration frame and then a start frame");
  check(has(tcp_cpp, "allRecvBuf[0] == 0x5A && allRecvBuf[1] == 0x5A"),
        "the frame head the vendor's reader looks for is 5A 5A");
  check(has(cnde_hpp, "robotStatePeriod = 8"),
        "eight milliseconds is the vendor's own default stream period");
  check(has(motion_cpp, "recommended range [0.001~0.0016]") ||
            has(motion_cpp, "0.001~0.0016"),
        "and ServoJ's own command period is a recommendation, not a body rate");

  std::printf("\n%d checks, %d failed\n", CHECKS, FAILURES);
  return FAILURES == 0 ? 0 : 1;
}
