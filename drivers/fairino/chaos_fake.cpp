/*
 * The fake FR controller as a shared object, so that `chaos.py` can run it in its own process, put a fault-injecting relay in front of
 * each of its three sockets, and still ask it where the arm really is (DRY-COVERAGE-PLAN.md item 12.C.4).
 *
 * `fake_controller.cpp` wraps the same `FakeServer` in a process, and that is what the cross-check and `skynet driver check` use. A
 * process cannot be asked where its plant is, and the contract's last question — is an OK read as fresh as the driver says it is —
 * is a comparison of what the driver reports with where the arm is. So the plant is read here, under the lock the stream's own
 * thread takes, and nothing about it is changed: this file is a window on the controller the suite runs, not a second controller.
 *
 * One thing the window shows that matters to how the answer is read: **this plant advances when the stream sends a frame**
 * (`serve_cnde` calls `integrate` once per frame it writes), not on a clock of its own. A link that stops the stream from sending
 * — a relay that is not reading, a full socket buffer — stops the arm too, where a real arm goes on moving. The contract's age
 * check is therefore exact for a link that delays what the controller has sent, and says nothing about the instant a stream is
 * cut off; the outage check, which asks the driver to report a stream that has stopped, does not depend on the plant at all.
 *
 * Licence: Apache-2.0.
 */

#include "fake_controller.hpp"

extern "C" {

/* Listens on the three ports and returns a handle, or null if one could not be bound (and nothing is left listening). */
void *fairino_fake_start(unsigned xmlrpc_port, unsigned command_port, unsigned state_port) {
  FakeServer *server = new FakeServer();
  if (!server->start(static_cast<std::uint16_t>(xmlrpc_port), static_cast<std::uint16_t>(command_port),
                     static_cast<std::uint16_t>(state_port))) {
    delete server;
    return nullptr;
  }
  return server;
}

/* Where the six joints are, in degrees: the plant's own values, not anything a client was told. */
void fairino_fake_positions_deg(void *handle, double *out) {
  FakeServer *server = static_cast<FakeServer *>(handle);
  std::lock_guard<std::mutex> guard(server->controller.lock);
  for (int i = 0; i < JOINTS; ++i) out[i] = server->controller.position_deg[i];
}

/* How many servo commands the controller has accepted, so that a harness can tell a command that arrived from one that was taken. */
unsigned long long fairino_fake_servo_commands(void *handle) {
  FakeServer *server = static_cast<FakeServer *>(handle);
  std::lock_guard<std::mutex> guard(server->controller.lock);
  return server->controller.servo_j + server->controller.servo_jv + server->controller.servo_jt;
}

/* How many STOP frames the instant-command port has received. */
unsigned long long fairino_fake_stop_frames(void *handle) {
  FakeServer *server = static_cast<FakeServer *>(handle);
  std::lock_guard<std::mutex> guard(server->controller.lock);
  return server->controller.stop_frames;
}

void fairino_fake_stop(void *handle) { delete static_cast<FakeServer *>(handle); }

}  // extern "C"
