// Robotiq's own fake gripper, served on the two lines a driver reaches a gripper by (DRY-COVERAGE-PLAN.md item 12.E.8).
//
// The maker's SDK (`github.com/Robotiq/grippers`, BSD-3-Clause) ships `makeFakeGripper()`: a register model (`fake::RegisterModel`) behind a Modbus RTU server (`fake::GripperServer`, nanomodbus underneath), both
// meant for the SDK's own tests and in-process. This file puts that model and that server on a TCP port (an RS-485 gateway that forwards the line raw) and on a pseudo-terminal (the gripper's USB adapter), unmodified, so
// that a driver written elsewhere speaks to the maker's own register map through the maker's own Modbus implementation. The model is **minimal on purpose** (the maker says so): activation completes instantly, the fingers
// are wherever they were told, there is no travel time, no object detection and no fault. That is a gripper's *protocol*, and not a gripper.
//
//   robotiq-fake-server [--slave 9] [--tcp PORT] [--pty] [--log FILE]
//
// Prints `tcp 127.0.0.1:PORT` and `pty /dev/pts/N` on stdout once it is listening. `--log` writes one JSON line per request and per reply, in hex: the record of what the driver put on the wire.

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <termios.h>
#include <unistd.h>

#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#include <Robotiq/gripper/fault_status.hpp>
#include <Robotiq/gripper/status.hpp>

#include "fake/gripper_server.hpp"
#include "fake/register_model.hpp"

namespace {

std::mutex g_model_lock;
Robotiq::fake::RegisterModel* g_model = nullptr;
std::FILE* g_log = nullptr;
std::mutex g_log_lock;
uint8_t g_slave = 9;

std::string hex(const std::vector<uint8_t>& bytes)
{
   std::string out;
   char buffer[3];
   for(uint8_t byte : bytes)
   {
      std::snprintf(buffer, sizeof buffer, "%02x", byte);
      out += buffer;
   }
   return out;
}

void record(const char* line, int peer, const std::vector<uint8_t>& request, const std::vector<uint8_t>& reply)
{
   if(g_log == nullptr)
   {
      return;
   }
   const auto now = std::chrono::duration<double>(std::chrono::system_clock::now().time_since_epoch()).count();
   std::lock_guard<std::mutex> guard(g_log_lock);
   std::fprintf(g_log, "{\"t\":%.6f,\"line\":\"%s\",\"peer\":%d,\"request\":\"%s\",\"reply\":\"%s\"}\n", now, line, peer, hex(request).c_str(), hex(reply).c_str());
   std::fflush(g_log);
}

// One client of the line: its own Modbus server state (a connection of its own), the one shared register model underneath.
struct Client
{
   Robotiq::fake::GripperServer server{*g_model, g_slave};
};

// What a Modbus RTU slave does: a request is what has arrived once the line has been quiet for a moment.
template <typename Ready, typename Read, typename Write>
void pump(const char* line, int peer, Ready ready, Read read, Write write)
{
   Client client;
   std::vector<uint8_t> pending;
   while(true)
   {
      if(!ready(pending.empty() ? 200 : 4))
      {
         if(!pending.empty())
         {
            std::vector<uint8_t> reply;
            {
               std::lock_guard<std::mutex> guard(g_model_lock);
               client.server.deliver(pending);
               reply = client.server.drain(512);
            }
            record(line, peer, pending, reply);
            pending.clear();
            if(!reply.empty())
            {
               write(reply);
            }
         }
         continue;
      }
      uint8_t chunk[512];
      const ssize_t got = read(chunk, sizeof chunk);
      if(got == 0 || (got < 0 && errno != EINTR && errno != EAGAIN && errno != EIO))
      {
         return;
      }
      if(got < 0)
      {
         std::this_thread::sleep_for(std::chrono::milliseconds(5));
         continue;
      }
      pending.insert(pending.end(), chunk, chunk + got);
   }
}

bool readable(int fd, int milliseconds)
{
   fd_set set;
   FD_ZERO(&set);
   FD_SET(fd, &set);
   timeval wait{milliseconds / 1000, (milliseconds % 1000) * 1000};
   return ::select(fd + 1, &set, nullptr, nullptr, &wait) > 0;
}

void serve_connection(int fd)
{
   sockaddr_in from{};
   socklen_t length = sizeof from;
   ::getpeername(fd, reinterpret_cast<sockaddr*>(&from), &length);      // the client's port: which of the clients on the gateway sent a request
   pump("tcp", ntohs(from.sin_port), [fd](int ms) { return readable(fd, ms); }, [fd](uint8_t* buffer, size_t size) { return ::recv(fd, buffer, size, 0); },
        [fd](const std::vector<uint8_t>& data) { ::send(fd, data.data(), data.size(), MSG_NOSIGNAL); });
   ::close(fd);
}

void serve_pty(int master)
{
   pump("pty", 0, [master](int ms) { return readable(master, ms); }, [master](uint8_t* buffer, size_t size) { return ::read(master, buffer, size); },
        [master](const std::vector<uint8_t>& data) { ssize_t ignored = ::write(master, data.data(), data.size()); (void)ignored; });
}

// What a test bench does to a device it cannot walk up to: stage a state in the maker's own register model, the way its header says a caller staging one does (`RegisterModel::status()` and `setStatus()`).
//
//   fault <HEX>      the FAULT STATUS byte of the status block (gFLT low nibble, kFLT high). The model keeps it across the next commands, "as on the real device", until rACT falls.
//   status           the six documented bytes of the status block, as hex
void control()
{
   std::string line;
   while(std::getline(std::cin, line))
   {
      std::istringstream words(line);
      std::string op;
      words >> op;
      std::lock_guard<std::mutex> guard(g_model_lock);
      if(op == "fault")
      {
         unsigned value = 0;
         words >> std::hex >> value;
         Robotiq::GripperStatus status = g_model->status();
         status.faultStatus = Robotiq::FaultStatus::fromRaw(static_cast<uint8_t>(value));
         g_model->setStatus(status);
         std::printf("ok fault %02x\n", value & 0xFF);
      }
      else if(op == "status")
      {
         const Robotiq::GripperStatus status = g_model->status();
         std::printf("status %s\n", hex(std::vector<uint8_t>(status.data(), status.data() + 6)).c_str());
      }
      else if(!op.empty())
      {
         std::printf("unknown %s\n", op.c_str());
      }
      std::fflush(stdout);
   }
}

} // namespace

int main(int argc, char** argv)
{
   int tcp_port = -1;
   bool want_pty = false;
   for(int i = 1; i < argc; ++i)
   {
      if(!std::strcmp(argv[i], "--slave") && i + 1 < argc)
      {
         g_slave = static_cast<uint8_t>(std::atoi(argv[++i]));
      }
      else if(!std::strcmp(argv[i], "--tcp") && i + 1 < argc)
      {
         tcp_port = std::atoi(argv[++i]);
      }
      else if(!std::strcmp(argv[i], "--pty"))
      {
         want_pty = true;
      }
      else if(!std::strcmp(argv[i], "--log") && i + 1 < argc)
      {
         g_log = std::fopen(argv[++i], "w");
      }
      else
      {
         std::fprintf(stderr, "usage: robotiq-fake-server [--slave N] [--tcp PORT] [--pty] [--log FILE]\n");
         return 2;
      }
   }
   Robotiq::fake::RegisterModel model;
   g_model = &model;

   std::vector<std::thread> threads;
   int listener = -1;
   if(tcp_port >= 0)
   {
      listener = ::socket(AF_INET, SOCK_STREAM, 0);
      int one = 1;
      ::setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
      sockaddr_in address{};
      address.sin_family = AF_INET;
      address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
      address.sin_port = htons(static_cast<uint16_t>(tcp_port));
      if(::bind(listener, reinterpret_cast<sockaddr*>(&address), sizeof address) != 0 || ::listen(listener, 8) != 0)
      {
         std::perror("bind/listen");
         return 1;
      }
      socklen_t length = sizeof address;
      ::getsockname(listener, reinterpret_cast<sockaddr*>(&address), &length);
      std::printf("tcp 127.0.0.1:%d\n", ntohs(address.sin_port));
   }
   if(want_pty)
   {
      const int master = ::posix_openpt(O_RDWR | O_NOCTTY);
      if(master < 0 || ::grantpt(master) != 0 || ::unlockpt(master) != 0)
      {
         std::perror("pty");
         return 1;
      }
      const char* name = ::ptsname(master);
      const int slave = ::open(name, O_RDWR | O_NOCTTY);     // held open, so that the line outlives the driver's own opens
      termios attributes{};
      ::tcgetattr(slave, &attributes);
      ::cfmakeraw(&attributes);
      attributes.c_cflag |= CLOCAL | CREAD;
      ::tcsetattr(slave, TCSANOW, &attributes);
      std::printf("pty %s\n", name);
      threads.emplace_back(serve_pty, master);
   }
   std::fflush(stdout);
   std::thread(control).detach();
   if(listener >= 0)
   {
      while(true)
      {
         const int connection = ::accept(listener, nullptr, nullptr);
         if(connection < 0)
         {
            if(errno == EINTR)
            {
               continue;
            }
            break;
         }
         int one = 1;
         ::setsockopt(connection, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
         std::thread(serve_connection, connection).detach();
      }
   }
   for(auto& thread : threads)
   {
      thread.join();
   }
   return 0;
}
