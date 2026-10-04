// The maker's own client, reading a gripper while a driver written elsewhere commands it (DRY-COVERAGE-PLAN.md item 12.E.8).
//
//   robotiq-second-opinion HOST PORT SLAVE PERIOD_MS DURATION_S
//
// Reads the status block (Modbus function 0x03, `GripperModbusClient::readStatus`: it writes nothing, so it cannot disturb what the driver commands) every PERIOD_MS milliseconds for DURATION_S seconds through a TCP
// gateway, and prints one JSON line per read: the time, the block, and the maker's own decoding of it. `live.py` holds the driver's state to this, and the record of what each side saw to each other.

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <thread>

#include <Robotiq/gripper/modbus_client.hpp>

#include "common.hpp"
#include "tcp_serial.hpp"

int main(int argc, char** argv)
{
   if(argc != 6)
   {
      std::fprintf(stderr, "usage: robotiq-second-opinion HOST PORT SLAVE PERIOD_MS DURATION_S\n");
      return 2;
   }
   const std::string host = argv[1];
   const int port = std::atoi(argv[2]);
   const int slave = std::atoi(argv[3]);
   const auto period = std::chrono::milliseconds(std::atoi(argv[4]));
   const auto duration = std::chrono::duration<double>(std::atof(argv[5]));
   try
   {
      Robotiq::GripperModbusClient client(std::make_unique<TcpSerial>(host, port, std::chrono::milliseconds(500)), static_cast<uint8_t>(slave), std::make_shared<SilentLogger>());
      const auto started = std::chrono::steady_clock::now();
      while(std::chrono::steady_clock::now() - started < duration)
      {
         const double at = std::chrono::duration<double>(std::chrono::steady_clock::now() - started).count();
         try
         {
            std::printf("{\"t\":%.4f,\"status\":%s}\n", at, statusJson(client.readStatus()).c_str());
         }
         catch(const std::exception& error)
         {
            std::printf("{\"t\":%.4f,\"error\":%s}\n", at, quoted(error.what()).c_str());
         }
         std::fflush(stdout);
         std::this_thread::sleep_for(period);
      }
   }
   catch(const std::exception& error)
   {
      std::printf("{\"error\":%s}\n", quoted(error.what()).c_str());
      return 1;
   }
   return 0;
}
