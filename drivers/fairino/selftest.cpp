/*
 * skynet-fairino's own suite: the wire, the mode mapping, the mask, every refusal, and the exact
 * XML-RPC calls each one makes.
 *
 * Linked against `fairino_bridge.cpp` rather than dlopening it, so a refusal can be asserted
 * through the ABI directly, and **it runs the fake controller in-process** — the same
 * `FakeController` `fake_controller.cpp` wraps in a process for the cross-check and for
 * `skynet driver check`. In-process is what lets this suite assert the *parameters of the calls
 * the driver made* rather than a return code: when it says a masked joint held its setpoint, what
 * it read was the six doubles inside the `ServoJ` the controller received.
 *
 * Held to controls. Each of these single-line edits fails a named check rather than passing
 * quietly:
 *
 *   * writing the measured position into a masked joint instead of the held setpoint fails "a
 *     masked joint holds the setpoint this driver last issued";
 *   * commanding before `ServoMoveStart` fails "the first command of a session opens a servo
 *     window first";
 *   * clamping an out-of-travel setpoint instead of refusing it fails "a setpoint outside the
 *     controller's own soft limit is refused" *and* "nothing reached the controller";
 *   * publishing the catalogue's travel when the controller answered fails "the descriptor
 *     carries this controller's wrist limit, not the catalogue's";
 *   * advancing `seq` per read rather than per arrival fails "seq counts arrivals, not reads";
 *   * sending the stop by XML-RPC rather than as the vendor's 8080 frame fails "the stop is the
 *     vendor's own frame, byte for byte".
 *
 * Licence: Apache-2.0.
 */

#include "skynet_rhal.h"

#include "fairino_wire.hpp"
#include "fairino_xmlrpc.hpp"
#include "fake_controller.hpp"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

extern "C" const skr_driver *skynet_rhal_entry(void);

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

bool near(double a, double b, double tolerance = 1e-4) { return std::fabs(a - b) <= tolerance; }

bool has(const std::string &haystack, const std::string &needle) {
  return haystack.find(needle) != std::string::npos;
}

/* ── driving the ABI ──────────────────────────────────────────────────── */

struct Cmd {
  float values[JOINTS] = {0, 0, 0, 0, 0, 0};
  std::uint8_t mask[JOINTS] = {1, 1, 1, 1, 1, 1};
  std::uint8_t modes[JOINTS] = {0, 0, 0, 0, 0, 0};
  skr_command raw{};

  skr_command *build(std::uint8_t mode, std::uint32_t seq, bool per_joint = false,
                     std::uint16_t joints = JOINTS) {
    raw.stamp_ns = 1;
    raw.seq = seq;
    raw.mode = mode;
    raw.flags = per_joint ? SKR_CMD_PER_JOINT_MODES : 0;
    raw.joint_count = joints;
    raw.values = values;
    raw.mask = mask;
    raw.modes = per_joint ? modes : nullptr;
    return &raw;
  }
};

struct StateBuffer {
  float position[JOINTS] = {0, 0, 0, 0, 0, 0};
  float velocity[JOINTS] = {0, 0, 0, 0, 0, 0};
  float effort[JOINTS] = {0, 0, 0, 0, 0, 0};
  skr_state raw{};

  skr_state *build(std::uint16_t joints = JOINTS) {
    raw.joint_count = joints;
    raw.position_rad = position;
    raw.velocity_rad_s = velocity;
    raw.effort_nm = effort;
    return &raw;
  }
};

std::string config_for(std::uint16_t xmlrpc, std::uint16_t command, std::uint16_t state,
                       const std::string &extra = "") {
  return "{\"host\":\"127.0.0.1\",\"xmlrpc_port\":" + std::to_string(xmlrpc) +
         ",\"command_port\":" + std::to_string(command) +
         ",\"state_port\":" + std::to_string(state) + extra + "}";
}

/* Wait until the controller has streamed `count` more frames, so that whatever the suite just set
 * on the body has reached the driver. */
bool wait_frames(FakeServer &server, std::uint64_t count) {
  std::uint64_t start = 0;
  {
    std::lock_guard<std::mutex> guard(server.controller.lock);
    start = server.controller.cnde_frames_sent;
  }
  for (int i = 0; i < 400; ++i) {
    {
      std::lock_guard<std::mutex> guard(server.controller.lock);
      if (server.controller.cnde_frames_sent >= start + count) return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
  }
  return false;
}

/* A raw CNDE client, for the three things only a malformed client can prove. */
int cnde_connect(std::uint16_t port) {
  const int fd = ::socket(AF_INET, SOCK_STREAM, 0);
  if (fd < 0) return -1;
  timeval timeout{};
  timeout.tv_sec = 2;
  ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
  sockaddr_in remote{};
  remote.sin_family = AF_INET;
  remote.sin_port = htons(port);
  ::inet_pton(AF_INET, "127.0.0.1", &remote.sin_addr);
  if (::connect(fd, reinterpret_cast<const sockaddr *>(&remote), sizeof remote) < 0) {
    ::close(fd);
    return -1;
  }
  return fd;
}

bool cnde_exchange(int fd, std::uint8_t type, const std::string &payload, std::string *reply) {
  std::vector<unsigned char> frame(payload.size() + CNDE_OVERHEAD);
  const std::size_t length =
      build_cnde_frame(frame.data(), 0, type, payload.data(), payload.size());
  if (!send_all(fd, frame.data(), length)) return false;
  unsigned char head[6];
  std::size_t got = 0;
  while (got < sizeof head) {
    const ssize_t chunk = ::recv(fd, head + got, sizeof head - got, 0);
    if (chunk <= 0) return false;
    got += static_cast<std::size_t>(chunk);
  }
  const std::size_t body =
      static_cast<std::size_t>(head[4]) | (static_cast<std::size_t>(head[5]) << 8);
  std::vector<unsigned char> rest(body + 2);
  got = 0;
  while (got < rest.size()) {
    const ssize_t chunk = ::recv(fd, rest.data() + got, rest.size() - got, 0);
    if (chunk <= 0) return false;
    got += static_cast<std::size_t>(chunk);
  }
  *reply = std::string(reinterpret_cast<const char *>(rest.data()), body);
  return head[3] == CNDE_TYPE_MESSAGE;
}

}  // namespace

int main(int argc, char **argv) {
  const std::uint16_t xmlrpc_port =
      static_cast<std::uint16_t>(argc > 1 ? std::atoi(argv[1]) : 31003);
  const std::uint16_t command_port =
      static_cast<std::uint16_t>(argc > 2 ? std::atoi(argv[2]) : 31080);
  const std::uint16_t state_port =
      static_cast<std::uint16_t>(argc > 3 ? std::atoi(argv[3]) : 31005);

  std::signal(SIGPIPE, SIG_IGN);
  const skr_driver *driver = skynet_rhal_entry();
  skr_ctx *ctx = nullptr;

  std::printf("\n== the wire, before any socket ==\n");
  {
    unsigned char frame[64];
    const char payload[] = {1, 2, 3, 4, 5};
    const std::size_t length = build_cnde_frame(frame, 0x2A, CNDE_TYPE_OUTPUT_CONFIG, payload, 5);
    check(length == 5 + CNDE_OVERHEAD, "a CNDE frame is its payload plus eight");
    check(frame[0] == 0x5A && frame[1] == 0x5A, "the head is 5A 5A");
    check(frame[2] == 0x2A, "the third byte is the frame count");
    check(frame[3] == CNDE_TYPE_OUTPUT_CONFIG, "the fourth byte is the frame type");
    check(frame[4] == 5 && frame[5] == 0, "the length is two bytes, little-endian");
    check(std::memcmp(frame + 6, payload, 5) == 0, "the payload follows the header");
    check(frame[11] == 0xA5 && frame[12] == 0xA5, "the tail is A5 A5");
    check(CNDE_TYPE_OUTPUT_CONFIG == 1 && CNDE_TYPE_START == 2 && CNDE_TYPE_STOP == 3 &&
              CNDE_TYPE_OUTPUT_STATE == 4 && CNDE_TYPE_MESSAGE == 6,
          "the five frame types are the vendor's");
    check(std::string(STOP_FRAME).size() == STOP_FRAME_BYTES,
          "the stop frame is thirty-three characters");
    check(std::string(STOP_FRAME) == "/f/bIII44III102III4IIISTOPIII/b/f",
          "the stop frame is FRRobot::StopMotion()'s own literal");
    check(XMLRPC_PORT == 20003 && COMMAND_PORT == 8080 && STATE_PORT == 20005 &&
              LEGACY_STATE_PORT == 20004 && UDP_COMMAND_PORT == 20007,
          "the five ports are the ones robot.cpp names");
    check(state_payload_bytes() == 315, "the eighteen fields this driver asks for are 315 bytes");
    check(STATE_FIELD_COUNT == 18, "eighteen fields");
    char names[1024];
    const std::string list(state_field_list(names, sizeof names));
    check(!list.empty(), "the field list builds");
    check(list.back() != ',', "the field list has no trailing comma");
    std::size_t commas = 0;
    for (const char c : list) {
      if (c == ',') commas += 1;
    }
    check(commas == STATE_FIELD_COUNT - 1, "the field list is comma-separated, one per field");
    bool all_known = true;
    for (std::size_t i = 0; i < STATE_FIELD_COUNT; ++i) {
      const Field *field = find_field(STATE_FIELDS[i].name);
      if (field == nullptr || field->bytes != STATE_FIELDS[i].bytes) all_known = false;
    }
    /* The strongest of the constant checks: every name and size this driver asks for is in the
     * table transcribed from the vendor's own `InitAllStates()`, at the vendor's own size. */
    check(all_known, "every field this driver asks for is the vendor's, at the vendor's size");
    check(state_offset(IDX_JOINT_POS) == 11 && STATE_FIELDS[IDX_JOINT_POS].bytes == 48,
          "joint positions are six doubles, eleven bytes into the payload");
    bool increasing = true;
    for (std::size_t i = 1; i < STATE_FIELD_COUNT; ++i) {
      if (state_offset(i) <= state_offset(i - 1)) increasing = false;
    }
    check(increasing, "the offsets are the running sum of the sizes");
    check(near(180.0 * DEG_TO_RAD, 3.14159265, 1e-6), "degrees convert to radians");
    check(near(3.14159265358979 * RAD_TO_DEG, 180.0, 1e-6), "and back again");
  }

  std::printf("\n== XML-RPC, both directions ==\n");
  {
    const double joints[JOINTS] = {1.5, -2.5, 3.5, -4.5, 5.5, -6.5};
    const double exaxis[4] = {0, 0, 0, 0};
    const std::vector<RpcValue> params = {
        RpcValue::of_doubles(joints, JOINTS), RpcValue::of_doubles(exaxis, 4),
        RpcValue::of_double(0.0),             RpcValue::of_double(0.0),
        RpcValue::of_double(0.008),           RpcValue::of_double(0.0),
        RpcValue::of_double(0.0),             RpcValue::of_int(0)};
    const std::string request = rpc_request("127.0.0.1", 20003, "ServoJ", params);
    check(has(request, "POST /RPC2 HTTP/1.1\r\n"), "the request is an HTTP/1.1 POST to /RPC2");
    check(has(request, "Content-length: "),
          "the header spells Content-length the way XmlRpc++ looks for it");
    std::size_t header_bytes = 0;
    std::size_t content_length = 0;
    check(rpc_content_length(request, &header_bytes, &content_length),
          "the content length parses out of the header");
    check(request.size() == header_bytes + content_length,
          "the content length is the body's length");
    std::string method;
    std::vector<RpcValue> parsed;
    check(rpc_parse_request(request.substr(header_bytes), &method, &parsed),
          "the request body parses back");
    check(method == "ServoJ", "the method name survives the round trip");
    check(parsed.size() == 8, "eight parameters, the vendor's ServoJ shape");
    check(parsed[0].kind == RpcValue::Kind::Array && parsed[0].items.size() == JOINTS,
          "the first parameter is the six joints");
    bool values_match = true;
    for (int i = 0; i < JOINTS; ++i) {
      double value = 0.0;
      if (!parsed[0].items[i].as_double(&value) || !near(value, joints[i], 1e-5)) {
        values_match = false;
      }
    }
    check(values_match, "the six joint values survive the round trip");
    RpcValue result;
    bool faulted = true;
    check(rpc_parse_response(rpc_response(RpcValue::of_int(0)), &result, &faulted),
          "a response parses");
    long code = 1;
    check(!faulted && result.as_int(&code) && code == 0, "a zero error code reads as zero");
    check(rpc_parse_response(rpc_response_fault(-1, "no such method"), &result, &faulted) &&
              faulted,
          "a fault is read as a fault, not as a value");
    std::size_t at = 0;
    const std::string bare = "<value>FR3</value>";
    RpcValue string_value;
    check(rpc_parse_value(bare, &at, &string_value) &&
              string_value.kind == RpcValue::Kind::String && string_value.text == "FR3",
          "a bare <value>text</value> parses as a string, which is what XmlRpc++ emits");
    at = 0;
    const std::string tagged = "<value><string>FR3</string></value>";
    check(rpc_parse_value(tagged, &at, &string_value) && string_value.text == "FR3",
          "a tagged string parses too");
    at = 0;
    const std::string nested =
        "<value><array><data><value><i4>7</i4></value>"
        "<value><array><data><value><double>1.5</double></value></data></array></value>"
        "</data></array></value>";
    RpcValue tree;
    check(rpc_parse_value(nested, &at, &tree) && tree.items.size() == 2 &&
              tree.items[1].items.size() == 1,
          "nested arrays parse");
  }

  std::printf("\n== the descriptor, before any body ==\n");
  {
    char buffer[16384];
    const int written = driver->describe(nullptr, buffer, sizeof buffer);
    const std::string descriptor(buffer, written > 0 ? written : 0);
    check(written > 0, "an unopened driver describes its body");
    check(has(descriptor, "\"base\":\"fixed\""), "the base is fixed: this arm is bolted down");
    check(has(descriptor, "\"dof\":6"), "six degrees of freedom");
    bool names_present = true;
    for (int i = 0; i < JOINTS; ++i) {
      if (!has(descriptor, std::string("\"name\":\"") + JOINT_NAMES[i] + "\"")) {
        names_present = false;
      }
    }
    check(names_present, "the joints carry the vendor's own j1..j6");
    check(has(descriptor, "\"role\":\"right_arm.shoulder.yaw\"") &&
              has(descriptor, "\"role\":\"right_arm.wrist.roll\""),
          "the canonical roles are the six-axis arm's, the same ones the UR row declares");
    check(has(descriptor, "\"per_joint_modes\":false"),
          "per-joint modes are declared false: no verb on this wire mixes them");
    check(has(descriptor, "\"rate_hz_max\":null"),
          "no control rate is claimed: the vendor publishes none for the body");
    check(!has(descriptor, "effort_max_nm"),
          "no joint torque rating is declared, because FAIR INNOVATION publish none");
    check(!has(descriptor, "joint_torque_max_nm"),
          "and the envelope carries no torque ceiling either");
    check(has(descriptor, "\"joint_velocity_max_rad_s\":3.14159"),
          "the velocity ceiling is the catalogue's 180 deg/s");
    check(has(descriptor, "\"watchdog_ms\":null"),
          "no watchdog window is claimed: no command-loss timeout is published");
    check(has(descriptor, "\"estop\":true"),
          "a hardware stop is declared: the controller streams the button and two safety inputs");
    check(has(descriptor, "frtech.fr/FR/5.html"), "the envelope cites the vendor's own page");
    check(has(descriptor, "\"capabilities\":[\"safety.estop\"]"),
          "the software stop is declared and nothing else is");
    check(!has(descriptor, "platform.onboard_compute"),
          "onboard compute is not claimed: the controller runs the vendor's software");
    check(has(descriptor, "vendor.fairino.telemetry"),
          "the telemetry token is declared as a vendor extension");
    check(has(descriptor, "\"position_min_rad\":-3.05433"),
          "before any controller has answered, the travel is the catalogue's");
  }

  std::printf("\n== refusals that need no body ==\n");
  {
    check(driver->open("{\"model\":\"fr5\"}", &ctx) == SKR_EINVAL,
          "an FR5 is refused by name: same wire, different published figures");
    check(driver->open(config_for(xmlrpc_port, command_port, state_port,
                                  ",\"state_period_ms\":2")
                           .c_str(),
                       &ctx) == SKR_EINVAL,
          "a stream period under the vendor's 4 ms minimum is refused");
    check(driver->open(config_for(xmlrpc_port, command_port, state_port,
                                  ",\"servo_cmd_t_s\":-1")
                           .c_str(),
                       &ctx) == SKR_EINVAL,
          "a negative servo command period is refused");
    check(driver->open("{\"host\":\"not-an-address\"}", &ctx) == SKR_EIO,
          "a host that is not an address cannot be reached, and open says so");
    StateBuffer state;
    check(driver->read_state(nullptr, state.build(), 0) == SKR_EPERM,
          "reading state before open is refused");
    Cmd cmd;
    check(driver->write_command(nullptr, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_EPERM,
          "commanding before open is refused");
  }

  std::printf("\n== the controller refuses what it cannot stream ==\n");
  {
    FakeServer server;
    if (!server.start(xmlrpc_port, command_port, state_port)) {
      std::fprintf(stderr, "selftest: cannot bind %u/%u/%u\n", xmlrpc_port, command_port,
                   state_port);
      return 2;
    }
    {
      const int fd = cnde_connect(state_port);
      check(fd >= 0, "the CNDE port accepts a connection");
      std::string reply;
      std::string payload;
      payload.push_back(static_cast<char>(CNDE_DEFAULT_PERIOD_MS));
      payload.push_back('\0');
      payload += "actual_joint_pos,not_a_field";
      check(cnde_exchange(fd, CNDE_TYPE_OUTPUT_CONFIG, payload, &reply),
            "a configuration frame is answered with a message frame");
      check(!reply.empty() && static_cast<unsigned char>(reply[0]) == CNDE_MESSAGE_FAIL,
            "a field the controller does not know is refused, not padded");
      check(has(reply, "NOT_FOUND"),
            "and the refusal says NOT_FOUND, which is what the vendor's client reads");
      ::close(fd);
    }
    {
      const int fd = cnde_connect(state_port);
      std::string reply;
      std::string payload;
      payload.push_back(static_cast<char>(1));
      payload.push_back('\0');
      payload += "actual_joint_pos";
      check(cnde_exchange(fd, CNDE_TYPE_OUTPUT_CONFIG, payload, &reply) && !reply.empty() &&
                static_cast<unsigned char>(reply[0]) == CNDE_MESSAGE_FAIL,
            "a one-millisecond period is refused: the vendor documents 4 ms as the floor");
      ::close(fd);
    }
    {
      const int fd = cnde_connect(state_port);
      std::string reply;
      check(cnde_exchange(fd, CNDE_TYPE_START, "", &reply) && !reply.empty() &&
                static_cast<unsigned char>(reply[0]) == CNDE_MESSAGE_FAIL,
            "a start before a configuration is refused");
      ::close(fd);
    }
    std::size_t unknown = 0;
    {
      std::lock_guard<std::mutex> guard(server.controller.lock);
      unknown = server.controller.cnde_unknown_field;
    }
    check(unknown == 1, "the controller counted the one unknown field it was asked for");
    server.stop();
  }

  std::printf("\n== open against a controller that configures and then says nothing ==\n");
  {
    FakeServer server;
    server.controller.paused.store(true);
    if (!server.start(xmlrpc_port, command_port, state_port)) return 2;
    const std::string config =
        config_for(xmlrpc_port, command_port, state_port, ",\"open_timeout_ms\":400");
    check(driver->open(config.c_str(), &ctx) == SKR_ETIMEDOUT,
          "a stream that is configured and then silent is a failed open, not a silent one");
    std::uint64_t starts = 0;
    {
      std::lock_guard<std::mutex> guard(server.controller.lock);
      starts = server.controller.cnde_starts;
    }
    check(starts == 1, "the driver did configure and start before it gave up");
    server.stop();
  }

  FakeServer server;
  if (!server.start(xmlrpc_port, command_port, state_port)) {
    std::fprintf(stderr, "selftest: cannot bind %u/%u/%u\n", xmlrpc_port, command_port, state_port);
    return 2;
  }
  FakeController &body = server.controller;

  std::printf("\n== a session against a live controller ==\n");
  const std::string config = config_for(xmlrpc_port, command_port, state_port);
  check(driver->open(config.c_str(), &ctx) == SKR_OK, "the session opens");
  {
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.requested_fields == STATE_FIELD_COUNT,
          "the driver asked for exactly the eighteen fields it reads");
    check(body.requested_period_ms == CNDE_DEFAULT_PERIOD_MS,
          "it asked for the vendor's own default period, 8 ms");
    check(body.identity_queries == 2,
          "it asked the robot what it is: software version and serial number");
    check(body.limit_queries == 1, "and what its joints may do: GetJointSoftLimitDeg");
    check(body.cnde_starts == 1 && body.cnde_configs == 1,
          "one configuration and one start, not a retry loop");
  }
  {
    char buffer[16384];
    const int written = driver->describe(ctx, buffer, sizeof buffer);
    const std::string descriptor(buffer, written > 0 ? written : 0);
    /* The claim this row is built on: the travel in the descriptor is the controller's own soft
     * limit, which for wrist 3 on this fake is ±90° rather than the catalogue's ±175°. */
    check(has(descriptor, "\"position_min_rad\":-1.57080"),
          "the descriptor carries this controller's wrist limit, not the catalogue's");
    check(!has(descriptor, "\"name\":\"j6\",\"role\":\"right_arm.wrist.roll\",\"type\":\"revolute\","
                           "\"limits\":{\"position_min_rad\":-3.05433"),
          "the catalogue's wrist limit is gone from the joint it was a fallback for");
    check(has(descriptor, "superseded in this descriptor by the soft limits this controller"),
          "and the envelope's source says where the numbers came from");
  }

  std::printf("\n== what the body reports ==\n");
  {
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.position_deg[0] = 12.0;
      body.position_deg[5] = -45.0;
      body.main_code = 42;
      body.sub_code = 7;
    }
    check(wait_frames(server, 3), "the controller keeps streaming");
    StateBuffer state;
    const int read = driver->read_state(ctx, state.build(), 0);
    check(read == SKR_OK, "state reads");
    check(state.raw.joint_count == JOINTS, "six joints are reported");
    check(near(state.position[0], 12.0 * DEG_TO_RAD, 1e-4),
          "joint positions arrive in degrees and are reported in radians");
    check(near(state.position[5], -45.0 * DEG_TO_RAD, 1e-4), "including the negative ones");
    /* With no servo window open the plant holds still and reports the load torque it was built
     * with, so these two are the units rather than the dynamics: zero is zero in both, and the
     * load is newton-metres on both sides. The velocity conversion is exercised under a velocity
     * command further down, where the number is not zero. */
    check(near(state.velocity[1], 0.0, 1e-6), "a joint that is not moving reports zero velocity");
    check(near(state.effort[2], LOAD_NM[2], 1e-4), "joint torque is newton-metres on both sides");
    check(state.raw.fault_code == 42,
          "the controller's own main error code is reported as the fault word");
    check((state.raw.flags & SKR_HOMING_NOT_REQUIRED) != 0,
          "homing is not required: 599 API entry points and no verb that homes a joint");
    check((state.raw.flags & SKR_FLAG_ESTOP) == 0, "and no stop is asserted");
    bool pose_nan = true;
    for (int i = 0; i < 7; ++i) {
      if (!std::isnan(state.raw.base_pose[i])) pose_nan = false;
    }
    check(pose_nan, "the base pose is NaN: nothing on a bolted-down arm measures it");
    bool twist_nan = true;
    for (int i = 0; i < 6; ++i) {
      if (!std::isnan(state.raw.base_twist[i])) twist_nan = false;
    }
    check(twist_nan, "and so is the base twist");
    check(state.raw.stamp_ns > 0, "the stamp is the arrival's, taken in the reader");

    const std::uint32_t first = state.raw.seq;
    StateBuffer again;
    check(driver->read_state(ctx, again.build(), 0) == SKR_OK, "a second read succeeds");
    check(again.raw.seq > first, "seq counts arrivals, not reads");

    body.paused.store(true);
    std::this_thread::sleep_for(std::chrono::milliseconds(40));
    StateBuffer stalled;
    check(driver->read_state(ctx, stalled.build(), 0) == SKR_OK,
          "a read against a stalled stream still returns the last sample");
    const std::uint32_t held = stalled.raw.seq;
    StateBuffer stalled_again;
    check(driver->read_state(ctx, stalled_again.build(), 0) == SKR_OK, "and again");
    check(stalled_again.raw.seq == held,
          "a stream that has stopped holds seq, which is 24's stalled producer");
    body.paused.store(false);
    check(wait_frames(server, 3), "the stream resumes");
    StateBuffer recovered;
    check(driver->read_state(ctx, recovered.build(), 0) == SKR_OK, "a read after the gap succeeds");
    check(recovered.raw.seq > held, "and seq advances again");
  }

  std::printf("\n== position commands ==\n");
  {
    std::uint64_t servo_j_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      servo_j_before = body.servo_j;
      body.enabled = false;
    }
    check(wait_frames(server, 2), "the controller reports its drives disabled");

    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = static_cast<float>(i) * 0.1f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "a position command is accepted");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.enables == 1, "the first command enabled the drives, and said so");
    check(body.move_starts == 1, "the first command of a session opens a servo window first");
    check(body.servo_j == servo_j_before + 1, "and then sends exactly one ServoJ");
    check(body.commanded_without_window == 0,
          "nothing was commanded outside a window, which the controller would have refused");
    Call call;
    check(body.last_call(RPC_SERVO_J, &call) && call.numbers.size() == JOINTS,
          "the ServoJ carried six joint values");
    bool degrees_match = true;
    for (int i = 0; i < JOINTS; ++i) {
      if (!near(call.numbers[i], static_cast<double>(i) * 0.1 * RAD_TO_DEG, 1e-3)) {
        degrees_match = false;
      }
    }
    check(degrees_match, "the values went out in degrees, which is what this wire carries");
  }

  std::printf("\n== the mask, on a wire that carries all six joints ==\n");
  {
    {
      /* Put joint 1 a long way from where it is about to be commanded, so that the plant cannot
       * arrive within the frames this section waits — otherwise "the setpoint this driver issued"
       * and "the position it happens to be at" would be the same number and the check below
       * would prove nothing. */
      std::lock_guard<std::mutex> guard(body.lock);
      body.position_deg[0] = 150.0;
    }
    Cmd first;
    for (int i = 0; i < JOINTS; ++i) first.values[i] = 0.2f;
    check(driver->write_command(ctx, first.build(SKR_MODE_POSITION, 2), 0) == SKR_OK,
          "every joint is commanded to the same setpoint");
    /* Let the plant move, so that the measured position is no longer the setpoint: that is the
     * difference the next check is about. */
    check(wait_frames(server, 4), "the plant moves toward it");
    Cmd masked;
    for (int i = 0; i < JOINTS; ++i) masked.values[i] = 0.4f;
    masked.mask[0] = 0;
    check(driver->write_command(ctx, masked.build(SKR_MODE_POSITION, 3), 0) == SKR_OK,
          "a command with one joint masked is accepted");
    std::lock_guard<std::mutex> guard(body.lock);
    Call call;
    check(body.last_call(RPC_SERVO_J, &call), "it reached the controller");
    check(near(call.numbers[0], 0.2 * RAD_TO_DEG, 1e-3),
          "a masked joint holds the setpoint this driver last issued");
    check(!near(call.numbers[0], body.position_deg[0], 1e-3),
          "and not the position it happens to be at, which would creep every cycle");
    check(near(call.numbers[1], 0.4 * RAD_TO_DEG, 1e-3), "the unmasked joints carry the new one");
  }

  std::printf("\n== travel, and what is refused before the wire ==\n");
  {
    std::uint64_t before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      before = body.servo_j;
    }
    Cmd beyond;
    for (int i = 0; i < JOINTS; ++i) beyond.values[i] = 0.1f;
    /* Wrist 3's soft limit on this controller is ±90°, and 100° is outside it — but inside the
     * ±175° the catalogue publishes. A driver that had kept the catalogue's numbers would send
     * this. */
    beyond.values[5] = static_cast<float>(100.0 * DEG_TO_RAD);
    check(driver->write_command(ctx, beyond.build(SKR_MODE_POSITION, 4), 0) == SKR_EINVAL,
          "a setpoint outside the controller's own soft limit is refused");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.servo_j == before, "nothing reached the controller");
    check(body.beyond_travel == 0,
          "so the controller never had to refuse it, which is where a clamp would have shown up");
  }

  std::printf("\n== velocity and torque ==\n");
  {
    Cmd velocity;
    for (int i = 0; i < JOINTS; ++i) velocity.values[i] = 0.05f;
    velocity.mask[2] = 0;
    check(driver->write_command(ctx, velocity.build(SKR_MODE_VELOCITY, 5), 0) == SKR_OK,
          "a velocity command is accepted");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      Call call;
      check(body.last_call(RPC_SERVO_JV, &call), "and goes out as ServoJV");
      check(near(call.numbers[0], 0.05 * RAD_TO_DEG, 1e-3),
            "in degrees per second, which is this wire's unit");
      check(near(call.numbers[2], 0.0, 1e-9),
            "a masked joint is commanded zero velocity, which is that mode's way of saying "
            "nothing");
      check(body.move_starts == 1, "velocity shares the servo window position opened");
    }
    /* The one place the velocity conversion is exercised on a number that is not zero: the plant
     * takes up the commanded rate, and what comes back out of `read_state` has to be the radians
     * per second it was commanded in. */
    check(wait_frames(server, 3), "the plant takes up the commanded velocity");
    {
      StateBuffer moving;
      check(driver->read_state(ctx, moving.build(), 0) == SKR_OK, "state reads while moving");
      check(near(moving.velocity[0], 0.05, 1e-3),
            "and the velocity comes back in radians per second, the unit it was commanded in");
      check(near(moving.velocity[2], 0.0, 1e-6), "the masked joint is not moving");
    }

    Cmd torque;
    for (int i = 0; i < JOINTS; ++i) torque.values[i] = 1.5f;
    torque.mask[4] = 0;
    check(driver->write_command(ctx, torque.build(SKR_MODE_TORQUE, 6), 0) == SKR_OK,
          "a torque command is accepted");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.move_ends == 1, "switching to torque closed the position window");
    check(body.torque_starts == 1, "and opened the torque one, which is a different verb");
    Call call;
    check(body.last_call(RPC_SERVO_JT, &call), "the command goes out as ServoJT");
    check(near(call.numbers[0], 1.5, 1e-6), "newton-metres, unconverted");
    check(near(call.numbers[4], 0.0, 1e-9), "a masked joint is commanded zero torque");
  }

  std::printf("\n== commands this wire cannot express ==\n");
  {
    std::uint64_t before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      before = body.xmlrpc_requests;
    }
    Cmd mixed;
    for (int i = 0; i < JOINTS; ++i) {
      mixed.values[i] = 0.1f;
      mixed.modes[i] = SKR_JMODE_POSITION;
    }
    mixed.modes[3] = SKR_JMODE_TORQUE;
    check(driver->write_command(ctx, mixed.build(SKR_MODE_POSITION, 7, /*per_joint=*/true), 0) ==
              SKR_EINVAL,
          "a command whose joints are in two modes is refused: no verb here mixes them");
    Cmd bogus;
    for (int i = 0; i < JOINTS; ++i) {
      bogus.values[i] = 0.1f;
      bogus.modes[i] = SKR_JMODE_POSITION;
    }
    bogus.modes[2] = 200;
    check(driver->write_command(ctx, bogus.build(SKR_MODE_POSITION, 8, /*per_joint=*/true), 0) ==
              SKR_EINVAL,
          "a per-joint mode this ABI does not define is refused, not guessed");
    Cmd wrong;
    check(driver->write_command(ctx, wrong.build(SKR_MODE_POSITION, 9, false, 4), 0) == SKR_EINVAL,
          "a command for four joints is refused by a six-joint body");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.xmlrpc_requests == before, "and none of the three reached the controller");
  }

  std::printf("\n== the robot's own stop, which is not this driver's ==\n");
  {
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.safety_stop0 = true;
    }
    check(wait_frames(server, 3), "the controller reports SI0 asserted");
    StateBuffer state;
    check(driver->read_state(ctx, state.build(), 0) == SKR_OK, "state still reads");
    check((state.raw.flags & SKR_FLAG_ESTOP) != 0,
          "an asserted safety stop shows in the state flags");
    std::uint64_t before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      before = body.xmlrpc_requests;
    }
    Cmd cmd;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 10), 0) == SKR_EPERM,
          "a command into an asserted safety stop is refused, as the vendor's own client does");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.xmlrpc_requests == before, "and nothing was sent");
      body.safety_stop0 = false;
    }
    check(wait_frames(server, 3), "the stop clears");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 11), 0) == SKR_OK,
          "and commands are accepted again");

    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.estop_button = true;
    }
    check(wait_frames(server, 3), "the emergency stop button is pressed");
    StateBuffer pressed;
    check(driver->read_state(ctx, pressed.build(), 0) == SKR_OK, "state reads with it pressed");
    check((pressed.raw.flags & SKR_FLAG_ESTOP) != 0, "and the flag is set");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.estop_button = false;
    }
    check(wait_frames(server, 3), "the button is released");
  }

  std::printf("\n== this driver's stop ==\n");
  {
    check(driver->emergency_stop(ctx) == SKR_OK, "the stop returns");
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.stop_frames == 1, "one frame arrived on the instant-command port");
      check(body.last_stop_frame == STOP_FRAME,
            "the stop is the vendor's own frame, byte for byte");
      check(body.unknown_method == 0,
            "and it did not go out as XML-RPC, which the controller would have faulted");
      check(body.window == Window::None, "the controller left its servo window");
    }
    Cmd cmd;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 12), 0) == SKR_EPERM,
          "every later command in this session is refused");
    StateBuffer state;
    check(driver->read_state(ctx, state.build(), 0) == SKR_OK, "state still reads after a stop");
    check((state.raw.flags & SKR_FLAG_ESTOP) != 0, "and carries the stop flag");
    std::uint64_t ends_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      ends_before = body.move_ends + body.torque_ends;
    }
    check(driver->close(ctx) == SKR_OK, "the session closes");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.move_ends + body.torque_ends == ends_before,
          "a stopped session does not reopen the conversation to tidy up");
  }

  std::printf("\n== a second session ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK,
          "a new session opens after a stopped one");
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 0.05f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "and the latch does not carry into it");
    char buffer[8192];
    const int written = driver->extension(ctx, "vendor.fairino.telemetry", nullptr, 0, buffer,
                                          sizeof buffer);
    const std::string telemetry(buffer, written > 0 ? written : 0);
    check(written > 0, "the telemetry extension answers");
    check(has(telemetry, "\"robot_model\":\"FR3\""),
          "it carries what the controller said it is");
    check(has(telemetry, "\"serial\":\"FR3-FAKE-000001\""), "and its serial number");
    check(has(telemetry, "\"limits_from_robot\":true"),
          "and says the travel came from the robot rather than from a catalogue");
    check(has(telemetry, "\"joint_driver_temperature_c\":[30.000,31.000"),
          "the drives' temperatures are reported, which 24's State has no field for");
    check(has(telemetry, "\"servo_window\":\"move\""), "and which window is open");
    check(!has(telemetry, "\"arrivals\":0"), "frames have arrived");
    check(driver->extension(ctx, "vendor.unitree.telemetry", nullptr, 0, buffer, sizeof buffer) ==
              SKR_ENOSYS,
          "another vendor's token is not answered");
    std::uint64_t ends_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      ends_before = body.move_ends;
    }
    check(driver->close(ctx) == SKR_OK, "the session closes cleanly");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.move_ends == ends_before + 1,
          "and a clean close ends the servo window it opened");
  }

  std::printf("\n== a session that is told not to energise the arm ==\n");
  {
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.enabled = false;
      body.enables = 0;
    }
    const std::string careful =
        config_for(xmlrpc_port, command_port, state_port, ",\"enable\":false");
    check(driver->open(careful.c_str(), &ctx) == SKR_OK, "the session opens");
    check(wait_frames(server, 3), "and sees the drives disabled");
    Cmd cmd;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_EPERM,
          "a command is refused rather than quietly energising six joints");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.enables == 0, "RobotEnable was never sent");
    }
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  server.stop();

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const std::string &failure : FAILURES) std::printf("  FAILED: %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
