// The maker's whole client, `Robotiq::Gripper`, driving a gripper through a TCP gateway (DRY-COVERAGE-PLAN.md item 12.E.8).
//
//   robotiq-client-probe HOST PORT SLAVE [POSITION_REGISTER ...]
//
// What `layout_check.py` runs against the simulated gripper written in this repository, to show that the maker's own client, as shipped, can drive it: construct the `Gripper` (which reads the gripper and starts the
// exchange cycle), `activate()` it (which is a no-op on an activated gripper and the reset-and-sweep handshake on one that is not), and for each POSITION_REGISTER command a go-to and wait for the fingers to stop
// (`waitForMotionEnd`). Prints one JSON line per step with the status the maker's client read. If the simulated gripper were not what the maker's client expects of one, this is where it would say so.

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <string>

#include <Robotiq/gripper.hpp>
#include <Robotiq/gripper/command.hpp>
#include <Robotiq/gripper/platform.hpp>
#include <Robotiq/gripper/wait.hpp>

#include "common.hpp"
#include "tcp_serial.hpp"

int main(int argc, char** argv)
{
   if(argc < 4)
   {
      std::fprintf(stderr, "usage: robotiq-client-probe HOST PORT SLAVE [POSITION_REGISTER ...]\n");
      return 2;
   }
   try
   {
      Robotiq::Gripper gripper(std::make_unique<TcpSerial>(argv[1], std::atoi(argv[2]), std::chrono::milliseconds(500)), static_cast<uint8_t>(std::atoi(argv[3])), std::chrono::milliseconds(10),
                               Robotiq::makeDefaultPlatform(), std::make_shared<SilentLogger>());
      std::printf("{\"step\":\"connected\",\"status\":%s}\n", statusJson(gripper.getStatus()).c_str());
      const auto activation = Robotiq::activate(gripper, std::chrono::seconds(15));
      std::printf("{\"step\":\"activate\",\"result\":\"%s\",\"status\":%s}\n", std::string(Robotiq::toString(activation)).c_str(), statusJson(gripper.getStatus()).c_str());
      for(int i = 4; i < argc; ++i)
      {
         Robotiq::GripperCommand command = Robotiq::GripperCommand::defaults();
         command.action.set(Robotiq::ActionRequestBit::GoTo);
         command.positionRequest = static_cast<uint8_t>(std::atoi(argv[i]));
         command.speed = 0xFF;
         command.force = 0x80;
         const auto sent = Robotiq::setCommandAndWaitForExchange(gripper, command, std::chrono::seconds(5));
         const auto stopped = Robotiq::waitForMotionEnd(gripper, std::chrono::seconds(10));
         std::printf("{\"step\":\"go_to\",\"position_request\":%s,\"sent\":%s,\"stopped\":%s,\"status\":%s}\n", argv[i], sent ? "true" : "false", stopped ? "true" : "false",
                     statusJson(gripper.getStatus()).c_str());
      }
      std::fflush(stdout);
   }
   catch(const std::exception& error)
   {
      std::printf("{\"error\":%s}\n", quoted(error.what()).c_str());
      return 1;
   }
   return 0;
}
