// A Robotiq::Serial over a TCP socket: the transport extension point the maker's SDK documents for "a TCP-serial bridge". Used by the harnesses in this directory, which are the maker's own client driving a gripper
// (the maker's fake server, or the simulated gripper written in this repository) through a gateway that forwards the line raw.
#pragma once

#include <arpa/inet.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

#include <chrono>
#include <cstdint>
#include <string>
#include <vector>

#include <Robotiq/gripper/serial.hpp>
#include <Robotiq/gripper/serial_io_exception.hpp>

class TcpSerial : public Robotiq::Serial
{
public:
   TcpSerial(std::string host, int port, std::chrono::milliseconds timeout = std::chrono::milliseconds(500))
      : _host(std::move(host))
      , _port(port)
      , _timeout(timeout)
   {
   }

   ~TcpSerial() override { close(); }

   void open() override
   {
      _fd = ::socket(AF_INET, SOCK_STREAM, 0);
      if(_fd < 0)
      {
         throw Robotiq::SerialIOException("socket() failed");
      }
      sockaddr_in address{};
      address.sin_family = AF_INET;
      address.sin_port = htons(static_cast<uint16_t>(_port));
      if(::inet_pton(AF_INET, _host.c_str(), &address.sin_addr) != 1 || ::connect(_fd, reinterpret_cast<sockaddr*>(&address), sizeof address) != 0)
      {
         close();
         throw Robotiq::SerialIOException("could not connect to " + _host + ":" + std::to_string(_port));
      }
      int one = 1;
      ::setsockopt(_fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
   }

   [[nodiscard]] bool isOpen() const override { return _fd >= 0; }

   void close() override
   {
      if(_fd >= 0)
      {
         ::close(_fd);
         _fd = -1;
      }
   }

   [[nodiscard]] std::vector<uint8_t> read(size_t size, std::chrono::milliseconds timeout) override
   {
      fd_set readable;
      FD_ZERO(&readable);
      FD_SET(_fd, &readable);
      timeval wait{static_cast<time_t>(timeout.count() / 1000), static_cast<suseconds_t>((timeout.count() % 1000) * 1000)};
      if(::select(_fd + 1, &readable, nullptr, nullptr, &wait) <= 0)
      {
         return {};
      }
      std::vector<uint8_t> data(size);
      const ssize_t got = ::recv(_fd, data.data(), size, 0);
      if(got <= 0)
      {
         throw Robotiq::SerialIOException("the gateway closed the connection");
      }
      data.resize(static_cast<size_t>(got));
      return data;
   }

   void write(const std::vector<uint8_t>& data) override
   {
      size_t sent = 0;
      while(sent < data.size())
      {
         const ssize_t put = ::send(_fd, data.data() + sent, data.size() - sent, MSG_NOSIGNAL);
         if(put <= 0)
         {
            throw Robotiq::SerialIOException("the gateway did not take the request");
         }
         sent += static_cast<size_t>(put);
      }
   }

   [[nodiscard]] std::chrono::milliseconds getTimeout() const override { return _timeout; }

private:
   std::string _host;
   int _port;
   std::chrono::milliseconds _timeout;
   int _fd = -1;
};
