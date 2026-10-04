// What Robotiq's own client writes, and what it makes of what it reads (DRY-COVERAGE-PLAN.md item 12.E.8).
//
// A small read-eval-print loop over the maker's SDK, for `layout_check.py`, which holds this repository's driver to it. It links the maker's library unmodified and drives the maker's own `GripperModbusClient`
// through a transport that records the bytes the client writes and hands back the reply it is given, so that nothing here is a second opinion of what the maker's client does: it is the maker's client, run.
//
// One request per line on stdin, one JSON object per line on stdout:
//
//   exchange <command: 12 hex> <reply: hex>      FC 0x17, as `GripperModbusClient::exchange` writes it, and the status it decodes from the reply
//   readStatus <reply: hex>                      FC 0x03
//   writeCommand <command: 12 hex> <reply: hex>  FC 0x10
//   units <speed|opening|fromregister|effort|current> <2F85|HandE> <value>
//   fault <gripper|controller> <code>            the maker's name and severity for a fault code
//   decode <status: 12 hex>                      the maker's accessors over a status block
//   profile <2F85|HandE>
//   constants

#include <algorithm>
#include <cmath>
#include <optional>
#include <cstdio>
#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include <Robotiq/detail/modbus_constants.hpp>
#include <Robotiq/gripper/command.hpp>
#include <Robotiq/gripper/connection_config.hpp>
#include <Robotiq/gripper/device_profile.hpp>
#include <Robotiq/gripper/driver_exception.hpp>
#include <Robotiq/gripper/fault_status.hpp>
#include <Robotiq/gripper/modbus_client.hpp>
#include <Robotiq/gripper/serial.hpp>
#include <Robotiq/gripper/status.hpp>
#include <Robotiq/gripper/to_string.hpp>
#include <Robotiq/gripper/units.hpp>

#include "common.hpp"

namespace {

// Records what the client writes and answers with the reply it was given.
class ScriptSerial : public Robotiq::Serial
{
public:
   explicit ScriptSerial(std::vector<uint8_t>* wrote, std::vector<uint8_t> reply)
      : _wrote(wrote)
      , _reply(std::move(reply))
   {
   }
   void open() override { _open = true; }
   [[nodiscard]] bool isOpen() const override { return _open; }
   void close() override { _open = false; }
   [[nodiscard]] std::vector<uint8_t> read(size_t size, std::chrono::milliseconds) override
   {
      if(!_written)
      {
         return {};      // the client drains the line before a request (a zero-timeout read): a reply exists only after one, as it does on a wire
      }
      const size_t count = std::min(size, _reply.size());
      std::vector<uint8_t> data(_reply.begin(), _reply.begin() + static_cast<long>(count));
      _reply.erase(_reply.begin(), _reply.begin() + static_cast<long>(count));
      return data;
   }
   void write(const std::vector<uint8_t>& data) override
   {
      _wrote->insert(_wrote->end(), data.begin(), data.end());
      _written = true;
   }
   [[nodiscard]] std::chrono::milliseconds getTimeout() const override { return std::chrono::milliseconds(50); }

private:
   std::vector<uint8_t>* _wrote;
   std::vector<uint8_t> _reply;
   bool _open = false;
   bool _written = false;
};

Robotiq::GripperCommand commandFrom(const std::string& hexBlock)
{
   Robotiq::GripperCommand command;
   const auto bytes = fromHex(hexBlock);
   for(size_t i = 0; i < bytes.size() && i < 6; ++i)
   {
      command.data()[i] = bytes[i];
   }
   return command;
}

const Robotiq::DeviceProfile& profileNamed(const std::string& name)
{
   return name == "HandE" ? Robotiq::profiles::kHandE : Robotiq::profiles::k2F85;
}

template <typename T>
std::string optionalNumber(const std::optional<T>& value)
{
   if(!value)
   {
      return "null";
   }
   std::ostringstream out;
   out.precision(17);
   out << static_cast<double>(*value);
   return out.str();
}

void clientCall(const std::string& op, const std::string& commandHex, const std::string& replyHex)
{
   std::vector<uint8_t> wrote;
   auto serial = std::make_unique<ScriptSerial>(&wrote, fromHex(replyHex));
   Robotiq::GripperModbusClient client(std::move(serial), Robotiq::kDefaultModbusSlaveAddress, std::make_shared<SilentLogger>());
   std::string result;
   try
   {
      if(op == "exchange")
      {
         result = statusJson(client.exchange(commandFrom(commandHex)));
      }
      else if(op == "readStatus")
      {
         result = statusJson(client.readStatus());
      }
      else
      {
         client.writeCommand(commandFrom(commandHex));
         result = "null";
      }
   }
   catch(const std::exception& error)
   {
      result = "{\"error\":" + quoted(error.what()) + "}";
   }
   std::printf("{\"op\":\"%s\",\"request\":\"%s\",\"result\":%s}\n", op.c_str(), toHex(wrote.data(), wrote.size()).c_str(), result.c_str());
}

} // namespace

int main()
{
   namespace mc = Robotiq::detail::modbus_constants;
   std::string line;
   while(std::getline(std::cin, line))
   {
      std::istringstream words(line);
      std::string op;
      words >> op;
      if(op == "exchange" || op == "writeCommand")
      {
         std::string command, reply;
         words >> command >> reply;
         clientCall(op, command, reply);
      }
      else if(op == "readStatus")
      {
         std::string reply;
         words >> reply;
         clientCall(op, "", reply);
      }
      else if(op == "units")
      {
         std::string which, profile;
         double value = 0;
         words >> which >> profile >> value;
         const auto& p = profileNamed(profile);
         std::string out = "null";
         if(which == "speed")
         {
            out = optionalNumber(Robotiq::units::speedToRegister(value, p));
         }
         else if(which == "opening")
         {
            out = optionalNumber(Robotiq::units::openingToRegister(value, p));
         }
         else if(which == "fromregister")
         {
            out = optionalNumber(Robotiq::units::openingFromRegister(static_cast<uint8_t>(value), p));
         }
         else if(which == "effort")
         {
            out = optionalNumber(Robotiq::units::effortToRegister(value));
         }
         else if(which == "current")
         {
            std::ostringstream o;
            o.precision(17);
            o << Robotiq::units::motorCurrentFromRegister(static_cast<uint8_t>(value));
            out = o.str();
         }
         std::printf("{\"op\":\"units\",\"which\":\"%s\",\"value\":%s}\n", which.c_str(), out.c_str());
      }
      else if(op == "fault")
      {
         std::string which;
         int code = 0;
         words >> which >> code;
         if(which == "gripper")
         {
            const auto fault = static_cast<Robotiq::GripperFault>(code);
            std::printf("{\"op\":\"fault\",\"which\":\"gripper\",\"code\":%d,\"name\":\"%s\",\"severity\":\"%s\"}\n", code, std::string(Robotiq::toString(fault)).c_str(),
                        std::string(Robotiq::toString(Robotiq::severity(fault))).c_str());
         }
         else
         {
            const auto fault = static_cast<Robotiq::ControllerFault>(code);
            std::printf("{\"op\":\"fault\",\"which\":\"controller\",\"code\":%d,\"name\":\"%s\",\"severity\":\"%s\"}\n", code, std::string(Robotiq::toString(fault)).c_str(),
                        std::string(Robotiq::toString(Robotiq::severity(fault))).c_str());
         }
      }
      else if(op == "decode")
      {
         std::string block;
         words >> block;
         Robotiq::GripperStatus status;
         const auto bytes = fromHex(block);
         for(size_t i = 0; i < bytes.size() && i < 6; ++i)
         {
            status.data()[i] = bytes[i];
         }
         std::printf("{\"op\":\"decode\",\"status\":%s}\n", statusJson(status).c_str());
      }
      else if(op == "profile")
      {
         std::string name;
         words >> name;
         const auto& p = profileNamed(name);
         std::printf("{\"op\":\"profile\",\"minSpeed\":%.17g,\"maxSpeed\":%.17g,\"minOpening\":%.17g,\"maxOpening\":%.17g,\"openPosition\":%d,\"closedPosition\":%d}\n", p.minSpeed, p.maxSpeed, p.minOpening,
                    p.maxOpening, p.openPosition, p.closedPosition);
      }
      else if(op == "constants")
      {
         std::printf("{\"op\":\"constants\",\"commandAddress\":%d,\"statusAddress\":%d,\"commandRegisters\":%d,\"statusRegisters\":%d,\"slave\":%d,\"actionBits\":[%d,%d,%d,%d]}\n", mc::kCommandAddress,
                     mc::kStatusAddress, mc::kCommandRegisterCount, mc::kStatusRegisterCount, Robotiq::kDefaultModbusSlaveAddress, static_cast<int>(Robotiq::ActionRequestBit::Activate),
                     static_cast<int>(Robotiq::ActionRequestBit::GoTo), static_cast<int>(Robotiq::ActionRequestBit::AutoRelease), static_cast<int>(Robotiq::ActionRequestBit::AutoReleaseOpenDirection));
      }
      else if(!op.empty())
      {
         std::printf("{\"error\":\"unknown op %s\"}\n", op.c_str());
      }
      std::fflush(stdout);
   }
   return 0;
}
