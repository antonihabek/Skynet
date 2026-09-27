/*
 * cross_check — Deep Robotics' own MotionSDK client, unmodified, against this tree's fake motion
 * host.
 *
 * F92 specifies this row as *"Fourier-shaped, SDK client unmodified against a fake motion host"*,
 * and this is the half that makes the shape worth having. `layout_check.cpp` settles the structs
 * at compile time; this settles the *behaviour*: the vendor's `Sender` builds the datagrams and
 * the vendor's `Receiver` parses the pushes, so if this tree had a command code wrong, a header
 * field in the wrong place or a push the SDK will not accept, the vendor's own code says so here
 * rather than a robot saying it later.
 *
 * Nothing of the SDK is modified and nothing of it is vendored. It needs a checkout:
 *
 *   git clone --depth 1 https://github.com/DeepRoboticsLab/lite3_motionsdk /tmp/lite3_motionsdk
 *   make cross LITE3_SDK=/tmp/lite3_motionsdk
 *
 * **The ports are not negotiable and that is the vendor's doing.** `Sender` takes an address and a
 * port; `Receiver` does not — its listening port is fixed inside the prebuilt library at 43897 —
 * so this check runs on the real ports over loopback rather than on a pair chosen to be free. The
 * suite beside it, which is its own motion host, is the one that takes ports as arguments.
 *
 * What it does **not** do is exercise `RobotStateInit()`, which moves every joint to zero and then
 * sleeps seven seconds. `Lite3_rl_deploy`'s own `HardwareInterface::Start()` takes control with
 * `ControlGet(2)`, which is the call this driver's arrangement mirrors, and a cross-check that
 * commanded a zeroing motion would be exercising a path no driver here takes.
 *
 * Licence: Apache-2.0 (this file). It links MotionSDK, which is MIT, and copies none of it.
 */

#include "sender.h"
#include "receiver.h"
#include "robot_types.h"

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

namespace {

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

bool near(double a, double b, double tolerance = 1e-3) { return std::fabs(a - b) <= tolerance; }

std::atomic<int> UPDATES{0};
std::atomic<uint32_t> LAST_CODE{0};

void on_message(uint32_t code) {
  LAST_CODE.store(code);
  if (code == 0x0906) UPDATES.fetch_add(1);
}

/* The fake's rest pose, which is the vendor's own `standup_state.hpp` rule — the midpoint of each
 * published range — and the constants it pushes for the parts with no plant behind them. */
constexpr double REST[3] = {(-0.530 + 0.530) / 2.0, (-3.50 + 0.320) / 2.0, (0.349 + 2.80) / 2.0};
constexpr float REST_TEMPERATURE = 30.0f;
constexpr float GRAVITY = 9.81f;

}  // namespace

int main(int argc, char **argv) {
  const std::string host = argc > 1 ? argv[1] : "127.0.0.1";
  const int port = argc > 2 ? std::atoi(argv[2]) : 43893;

  std::printf("\n== the vendor's Receiver against this tree's push ==\n");

  Receiver *receiver = new Receiver();
  receiver->RegisterCallBack(on_message);
  RobotData *data = &receiver->GetState();
  receiver->StartWork();

  /* The vendor's own signal that a push arrived and parsed: `OnMessageUpdate(0x0906)`, which their
   * `main.cpp` gates every command on. */
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
  while (UPDATES.load() == 0 && std::chrono::steady_clock::now() < deadline) {
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  check(UPDATES.load() > 0,
        "the vendor's Receiver accepted this tree's state push and called its own callback");
  check(LAST_CODE.load() == 0x0906,
        "with the instruction type their README documents, and no other");

  if (UPDATES.load() == 0) {
    std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
    for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
    return 1;
  }

  check(near(data->joint_data.joint_data[0].position, REST[0]) &&
            near(data->joint_data.joint_data[2].position, REST[2]),
        "and decoded the joint positions where this tree put them");
  check(near(data->joint_data.joint_data[3].position, REST[0]) &&
            near(data->joint_data.joint_data[11].position, REST[2]),
        "including the legs behind the first, so the twelve are not off by a leg");
  check(near(data->joint_data.fr_leg[0].position, REST[0]) &&
            near(data->joint_data.hr_leg[2].position, REST[2]),
        "read through their own per-leg view as well as the flat twelve");
  check(near(data->joint_data.joint_data[5].temperature, REST_TEMPERATURE),
        "the temperature field is where this tree thinks it is");
  check(near(data->imu.acc_z, GRAVITY) && near(data->imu.acc_x, 0.0),
        "the IMU accelerations decode, which puts the whole 40-byte IMU block in the right place");
  check(data->tick > 0, "the tick is counting");

  std::printf("\n== the vendor's Sender against this tree's motion host ==\n");
  Sender *sender = new Sender(host, static_cast<uint16_t>(port));

  /* `ControlGet(2)` — `SDK` — is what `Lite3_rl_deploy`'s `HardwareInterface::Start` sends. */
  sender->ControlGet(2);
  std::this_thread::sleep_for(std::chrono::milliseconds(200));
  check(true, "ControlGet(SDK) sent — the fake's own count is what says it arrived as 0x0114");

  RobotCmd command;
  std::memset(&command, 0, sizeof command);
  for (int i = 0; i < 12; ++i) {
    /* A command inside the published travel and with gains on it, so the fake's `beyond_travel`
     * and its control-right checks are both exercised by the vendor's construction rather than
     * only by this tree's. */
    command.joint_cmd[i].position = static_cast<float>(REST[i % 3]);
    command.joint_cmd[i].kp = 30.0f;
    command.joint_cmd[i].kd = 1.0f;
  }
  for (int i = 0; i < 5; ++i) {
    sender->SendCmd(command);
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
  }
  check(true, "five SendCmd datagrams sent — the fake counts them as 0x0111 with 240 bytes");

  sender->AllJointBackZero();
  std::this_thread::sleep_for(std::chrono::milliseconds(100));
  check(true, "AllJointBackZero sent — the fake counts it as 0x31010c05");

  /* Hands control back the vendor's way, which sends a damping command, sleeps two seconds and
   * then sends 0x0113. */
  sender->ControlGet(1);
  std::this_thread::sleep_for(std::chrono::milliseconds(200));
  check(true, "ControlGet(ROBOT) sent — damping, then 0x0113");

  /* The push kept arriving through all of it: a wire that stopped being read halfway would make
   * everything above a one-way claim. */
  const int updates_before = UPDATES.load();
  std::this_thread::sleep_for(std::chrono::milliseconds(200));
  check(UPDATES.load() > updates_before,
        "and the state push was still arriving and parsing at the end of the exchange");

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
