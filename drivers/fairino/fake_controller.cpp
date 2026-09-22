/*
 * The fake FR controller as a process: `fake_controller.hpp` with a `main` around it.
 *
 * Two pieces of this row run it this way — the wire cross-check, which needs the vendor's own
 * client to connect to something over real sockets, and `skynet driver check`, which loads the
 * driver into its own process. The suite does not: it runs the same controller in-process, which
 * is what lets it assert the parameters of the exact calls the driver made.
 *
 *   ./fake_controller [xmlrpc_port] [command_port] [state_port] [seconds]
 *
 * It announces itself on stderr, prints one line of JSON on `SIGTERM`, and models nothing it does
 * not say it models — see the header.
 *
 * Licence: Apache-2.0.
 */

#include "fake_controller.hpp"

#include <csignal>

namespace {
std::atomic<bool> RUNNING{true};
void stop(int) { RUNNING.store(false); }
}  // namespace

int main(int argc, char **argv) {
  const std::uint16_t xmlrpc_port =
      static_cast<std::uint16_t>(argc > 1 ? std::atoi(argv[1]) : XMLRPC_PORT);
  const std::uint16_t command_port =
      static_cast<std::uint16_t>(argc > 2 ? std::atoi(argv[2]) : COMMAND_PORT);
  const std::uint16_t state_port =
      static_cast<std::uint16_t>(argc > 3 ? std::atoi(argv[3]) : STATE_PORT);
  const double seconds = argc > 4 ? std::atof(argv[4]) : 0.0;

  std::signal(SIGINT, stop);
  std::signal(SIGTERM, stop);
  /* A client that goes away mid-reply must not take the controller with it. */
  std::signal(SIGPIPE, SIG_IGN);

  FakeServer server;
  if (!server.start(xmlrpc_port, command_port, state_port)) {
    std::fprintf(stderr,
                 "fake-fairino: cannot bind %u (XML-RPC), %u (instant command) or %u (CNDE).\n",
                 static_cast<unsigned>(xmlrpc_port), static_cast<unsigned>(command_port),
                 static_cast<unsigned>(state_port));
    return 2;
  }

  std::fprintf(stderr,
               "fake-fairino: listening for commands on XML-RPC %u, instant commands on %u, CNDE "
               "state on %u. This is not an FR controller: no kinematics, no collision model, no "
               "safety system, and the dynamics are one integrator per joint.\n",
               static_cast<unsigned>(xmlrpc_port), static_cast<unsigned>(command_port),
               static_cast<unsigned>(state_port));

  const auto started = std::chrono::steady_clock::now();
  while (RUNNING.load()) {
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    if (seconds > 0.0 &&
        std::chrono::steady_clock::now() - started > std::chrono::duration<double>(seconds)) {
      break;
    }
  }

  server.stop();
  std::lock_guard<std::mutex> guard(server.controller.lock);
  std::printf("%s\n", server.controller.report().c_str());
  return 0;
}
