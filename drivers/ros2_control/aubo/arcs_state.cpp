// arcs_state — what ARCS says about itself, for `run-arcs.sh` (DRY-COVERAGE-PLAN.md Phase 8, 8.15).
//
// The one program in the AUBO row written here, and it only reads: over AUBO's own SDK, the robot's
// mode, its safety mode, whether servo mode is on, whether it is steady, and its joints — the
// controller's account, beside the one the bridge gives, so the two can be compared. One line,
// `key=value` pairs, so a shell can read it. Exit 2 when no controller answers.
//
//   arcs_state [host]

#include <cstdio>
#include <iostream>
#include <memory>
#include <string>

#include "aubo_sdk/rpc.h"

using namespace arcs::common_interface;
using namespace arcs::aubo_sdk;

int main(int argc, char** argv) {
  const std::string host = argc > 1 ? argv[1] : "127.0.0.1";
  auto rpc = std::make_shared<RpcClient>();
  rpc->setRequestTimeout(1000);
  if (rpc->connect(host, 30004) != 0 || !rpc->hasConnected()) {
    std::fprintf(stderr, "arcs_state: no controller answers RPC at %s:30004\n", host.c_str());
    return 2;
  }
  rpc->login("aubo", "123456");
  const auto names = rpc->getRobotNames();
  if (names.empty()) {
    std::fprintf(stderr, "arcs_state: the controller at %s names no robot\n", host.c_str());
    return 2;
  }
  auto robot = rpc->getRobotInterface(names.front());
  auto state = robot->getRobotState();
  std::cout << "robot=" << names.front() << " mode=" << state->getRobotModeType()
            << " safety=" << state->getSafetyModeType()
            << " servo=" << (robot->getMotionControl()->isServoModeEnabled() ? 1 : 0)
            << " steady=" << (state->isSteady() ? 1 : 0) << " q=";
  const auto q = state->getJointPositions();
  for (std::size_t i = 0; i < q.size(); ++i) {
    std::printf("%s%.5f", i == 0 ? "" : ",", q[i]);
    std::fflush(stdout);
  }
  std::cout << std::endl;
  rpc->logout();
  rpc->disconnect();
  return 0;
}
