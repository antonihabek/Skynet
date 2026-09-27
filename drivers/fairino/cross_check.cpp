/*
 * The wire cross-check: FAIR INNOVATION's own client, unmodified, against this tree's fake
 * controller.
 *
 * Everything else in this row is this tree on both ends — the driver writes a frame and the fake
 * reads it, and if both were wrong in the same way nothing would notice. This program puts the
 * **vendor's** `FRRobot` on one end. It links the prebuilt `libfairino.so` FAIR INNOVATION ship
 * (Apache-2.0) and calls the same entry points their examples call:
 *
 *   `RPC()`                    connects all three sockets, configures the CNDE stream with the
 *                              SDK's own default seventy-six fields, starts it, and only reports
 *                              success if its state thread is still healthy two seconds later
 *   `GetSoftwareVersion`       an XML-RPC query answered as an array
 *   `GetJointSoftLimitDeg`     the interleaved twelve-value reply this driver also parses
 *   `ServoMoveStart`/`ServoJ`  the position path
 *   `ServoJTStart`/`ServoJT`   the torque path, a different window
 *   `GetRobotRealTimeState`    the state the SDK assembled **from our stream** into its own
 *                              `ROBOT_STATE_PKG` — which is the strongest single check here,
 *                              because every field it reads is at an offset the vendor computed
 *                              from its own table and ours filled from ours
 *   `StopMotion`               the 8080 frame
 *
 * If a field name, a size, a frame layout or a reply shape in `fairino_wire.hpp` were wrong, the
 * vendor's client would fail to configure, fail to parse, or read a number from the wrong place —
 * and every one of those lands here as a failed check.
 *
 * It runs against the **real ports** (20003, 8080, 20005) because the vendor's library has them
 * compiled in, the way the Lite3 row's cross-check runs on MotionSDK's own ports. Everything else
 * in this row runs on ports beside them.
 *
 * Licence: Apache-2.0. Needs a checkout and the prebuilt library; see the Makefile.
 */

#include "robot.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>

namespace {

int CHECKS = 0;
int FAILURES = 0;

void check(bool held, const std::string &what) {
  CHECKS += 1;
  if (held) {
    std::printf("  ok    %s\n", what.c_str());
  } else {
    FAILURES += 1;
    std::printf("  FAIL  %s\n", what.c_str());
  }
}

bool near(double a, double b, double tolerance) { return std::fabs(a - b) <= tolerance; }

}  // namespace

int main(int argc, char **argv) {
  const char *host = argc > 1 ? argv[1] : "127.0.0.1";

  FRRobot robot;
  std::printf("\n== the vendor's own client, against this tree's controller ==\n");
  const errno_t connected = robot.RPC(host);
  check(connected == 0,
        "FRRobot::RPC connects: XML-RPC, the instant-command socket and a configured, started "
        "CNDE stream, with its own state thread still healthy two seconds later");
  if (connected != 0) {
    std::printf("\n%d checks, %d failed\n", CHECKS, FAILURES + 1);
    return 1;
  }

  char model[64] = {0};
  char web[64] = {0};
  char controller[64] = {0};
  check(robot.GetSoftwareVersion(model, web, controller) == 0, "GetSoftwareVersion answers");
  check(std::string(model) == "FR3", "and the model comes back as the controller reported it");
  check(std::string(controller) == "fake-controller-3.9.9",
        "and so does the controller version, from the third element of the same array");

  float negative[6] = {0};
  float positive[6] = {0};
  check(robot.GetJointSoftLimitDeg(0, negative, positive) == 0, "GetJointSoftLimitDeg answers");
  check(near(negative[0], -175.0, 1e-3) && near(positive[0], 175.0, 1e-3),
        "the first joint's pair is read from the interleaved reply in the right order");
  check(near(negative[5], -90.0, 1e-3) && near(positive[5], 90.0, 1e-3),
        "and the sixth's is this controller's reduced wrist, not the catalogue's");

  check(robot.Mode(0) == 0, "Mode(0) puts the controller in automatic");
  check(robot.RobotEnable(1) == 0, "RobotEnable(1) energises the drives");

  ROBOT_STATE_PKG pkg{};
  std::this_thread::sleep_for(std::chrono::milliseconds(120));
  check(robot.GetRobotRealTimeState(&pkg) == 0, "the SDK hands back the state it assembled");
  check(pkg.rbtEnableState == 1,
        "the enable state this tree streamed landed in the field the vendor reads it from");
  check(near(pkg.jointDriverTemperature[0], 30.0, 1e-6) &&
            near(pkg.jointDriverTemperature[5], 35.0, 1e-6),
        "and so did the six drive temperatures, 823 bytes into the vendor's own struct");
  check(pkg.EmergencyStop == 0 && pkg.safety_stop0_state == 0,
        "the stop fields are where the vendor expects them, and are clear");

  check(robot.ServoMoveStart() == 0, "ServoMoveStart opens the position window");
  JointPos target;
  target.jPos[0] = 10.0;
  target.jPos[1] = -90.0;
  target.jPos[2] = 90.0;
  target.jPos[3] = -90.0;
  target.jPos[4] = 0.0;
  target.jPos[5] = 20.0;
  ExaxisPos exaxis;
  for (int i = 0; i < 4; ++i) exaxis.ePos[i] = 0.0;
  bool every_servo_accepted = true;
  for (int i = 0; i < 20; ++i) {
    if (robot.ServoJ(&target, &exaxis, 0.0f, 0.0f, 0.008f, 0.0f, 0.0f) != 0) {
      every_servo_accepted = false;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(8));
  }
  check(every_servo_accepted, "twenty ServoJ commands are accepted, parameters and all");
  std::this_thread::sleep_for(std::chrono::milliseconds(200));
  check(robot.GetRobotRealTimeState(&pkg) == 0, "state reads while it moves");
  check(near(pkg.lastServoTarget[5], 20.0, 1e-6),
        "the controller's record of the last setpoint is the one the vendor's client sent");
  check(std::fabs(pkg.jt_cur_pos[0] - 10.0) < std::fabs(0.0 - 10.0),
        "and the joint has moved toward it, so the loop closed through both wires");
  check(robot.ServoMoveEnd() == 0, "ServoMoveEnd closes the window");

  check(robot.ServoJTStart() == 0, "ServoJTStart opens the torque window, a different verb");
  float torque[6] = {0.5f, 8.5f, 4.5f, 0.6f, 0.3f, 0.1f};
  check(robot.ServoJT(torque, 0.008) == 0, "ServoJT is accepted with its five parameters");
  check(robot.ServoJTEnd() == 0, "ServoJTEnd closes it");

  check(robot.StopMotion() == 0, "StopMotion returns");
  /* `StopMotion` hands the frame to the SDK's own send thread rather than writing it, so the
   * frame arrives a moment later. The fake's report is what counts it; this only waits. */
  std::this_thread::sleep_for(std::chrono::milliseconds(300));
  check(robot.GetRobotRealTimeState(&pkg) == 0, "state still reads after the stop");
  check(pkg.robot_state == 1, "and the controller reports itself stopped");

  robot.CloseRPC();
  std::printf("\n%d checks, %d failed\n", CHECKS, FAILURES);
  return FAILURES == 0 ? 0 : 1;
}
