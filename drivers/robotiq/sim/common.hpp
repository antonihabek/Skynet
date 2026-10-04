// Helpers shared by the harnesses in this directory: hex, a JSON line for a status block, a logger that says nothing.
#pragma once

#include <cstdint>
#include <cstdio>
#include <sstream>
#include <string>
#include <vector>

#include <Robotiq/gripper/logger.hpp>
#include <Robotiq/gripper/status.hpp>
#include <Robotiq/gripper/to_string.hpp>

inline std::vector<uint8_t> fromHex(const std::string& text)
{
   std::vector<uint8_t> out;
   for(size_t i = 0; i + 1 < text.size(); i += 2)
   {
      out.push_back(static_cast<uint8_t>(std::stoi(text.substr(i, 2), nullptr, 16)));
   }
   return out;
}

inline std::string toHex(const uint8_t* data, size_t size)
{
   std::string out;
   char buffer[3];
   for(size_t i = 0; i < size; ++i)
   {
      std::snprintf(buffer, sizeof buffer, "%02x", data[i]);
      out += buffer;
   }
   return out;
}

class SilentLogger : public Robotiq::Logger
{
public:
   void log(Level, std::string_view) override {}
};

inline std::string statusJson(const Robotiq::GripperStatus& status)
{
   std::ostringstream out;
   out << "{\"block\":\"" << toHex(status.data(), 6) << "\",\"gACT\":" << status.gripperStatus.activated() << ",\"gGTO\":" << status.gripperStatus.goToEnabled() << ",\"gSTA\":"
       << static_cast<int>(status.gripperStatus.activationState()) << ",\"gOBJ\":" << static_cast<int>(status.gripperStatus.objectDetection()) << ",\"gFLT\":"
       << static_cast<int>(status.faultStatus.gripperFault()) << ",\"kFLT\":" << static_cast<int>(status.faultStatus.controllerFault()) << ",\"gPR\":" << static_cast<int>(status.positionRequestEcho)
       << ",\"gPO\":" << static_cast<int>(status.position) << ",\"gCU\":" << static_cast<int>(status.current) << ",\"text\":\"" << Robotiq::toString(status) << "\"}";
   return out.str();
}

inline std::string quoted(const std::string& text)
{
   std::string out = "\"";
   for(char c : text)
   {
      if(c == '"' || c == '\\')
      {
         out += '\\';
      }
      if(c == '\n')
      {
         out += "\\n";
         continue;
      }
      out += c;
   }
   return out + "\"";
}

