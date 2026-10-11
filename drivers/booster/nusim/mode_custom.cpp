/*
 * mode_custom — ask a Booster robot (or NUbots' simulator of a K1) to enter CUSTOM mode, with the vendor SDK's own high-level client.
 *
 * `rt/joint_ctrl` takes effect only in CUSTOM mode ("the low level publish interface will only take effect when the robot is in custom mode", T1 manual p55), and entering it is the operator's setup,
 * which `drivers/booster` never does (its file header, fact 1). The live run against NUSim (`run-nusim.sh`) is the operator, so it does it here, the way the maker's `b1_loco_example_client` does:
 * `B1LocoClient::ChangeMode`, a request on `rt/LocoApiTopicReq` that the robot answers on `rt/LocoApiTopicResp`.
 *
 *   mode_custom [interface]      exit 0 when the robot answered 0, 1 when it answered anything else or nothing
 *
 * Licence: Apache-2.0 (this file). Links booster_robotics_sdk (Apache-2.0); not vendored here.
 */

#include <booster/robot/b1/b1_loco_client.hpp>
#include <booster/robot/channel/channel_factory.hpp>

#include <chrono>
#include <cstdio>
#include <string>
#include <thread>

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
  booster::robot::ChannelFactory::Instance()->Init(0, interface);
  booster::robot::b1::B1LocoClient client;
  client.Init();
  /* The request goes to a robot that may not have discovered this client yet: a few tries, a second apart, and the last answer is the exit status. */
  int answer = -1;
  for (int attempt = 0; attempt < 10 && answer != 0; ++attempt) {
    answer = client.ChangeMode(booster::robot::RobotMode::kCustom);
    std::fprintf(stderr, "mode_custom: ChangeMode(kCustom) -> %d\n", answer);
    if (answer != 0) std::this_thread::sleep_for(std::chrono::seconds(1));
  }
  return answer == 0 ? 0 : 1;
}
