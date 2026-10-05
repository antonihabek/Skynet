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

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
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

std::size_t count_of(const std::string &haystack, const std::string &needle) {
  std::size_t found = 0;
  for (std::size_t at = haystack.find(needle); at != std::string::npos;
       at = haystack.find(needle, at + needle.size())) {
    found += 1;
  }
  return found;
}

/* What the driver says on stderr while the suite runs something: descriptor 2 is pointed at a temporary file until `finish()`, which puts it back and returns what was written. Always
 * finished on the path that started it — a descriptor left pointing at a file would swallow what the rest of the suite says about itself. */
class StderrCapture {
 public:
  StderrCapture() {
    std::fflush(stderr);
    saved_ = ::dup(STDERR_FILENO);
    file_ = std::tmpfile();
    ::dup2(::fileno(file_), STDERR_FILENO);
  }

  std::string finish() {
    std::fflush(stderr);
    ::dup2(saved_, STDERR_FILENO);
    ::close(saved_);
    std::string text;
    std::rewind(file_);
    char chunk[4096];
    std::size_t got = 0;
    while ((got = std::fread(chunk, 1, sizeof chunk, file_)) > 0) text.append(chunk, got);
    std::fclose(file_);
    return text;
  }

 private:
  int saved_ = -1;
  std::FILE *file_ = nullptr;
};

/* CLOCK_MONOTONIC_RAW, the clock `skynet::acquisition_ns()` stamps on, so that a stamp can be aged
 * against the instant a read returned. */
std::uint64_t raw_ns() {
  timespec now{};
  clock_gettime(CLOCK_MONOTONIC_RAW, &now);
  return static_cast<std::uint64_t>(now.tv_sec) * 1000000000ull +
         static_cast<std::uint64_t>(now.tv_nsec);
}

/* What a read owes a caller who comes back before the next frame has arrived: a frame it has not
 * returned before, or — when the stream is late — the previous sample **after waiting the bound the
 * driver documents** (twice the stream's period, 16 ms at the vendor's default; `read_state`).
 *
 * Not "a new frame, always". The fake sends every 8 ms, and on a machine with every core busy it is
 * sometimes a period late; a driver that then reports the previous sample is giving a late producer
 * what §24 says it gets (a repeated `seq`), and a suite that called that a failure failed one run in
 * twenty-five under load — and, run by the seeding campaign's parallel workers, would have counted
 * the flake as a defect noticed. What this still refuses is a read that returns the previous sample
 * *without waiting*: a driver whose bound is zero returns in microseconds and fails here every time.
 * The two milliseconds of slack are the clock's and the sleep's granularity, not the driver's. */
constexpr std::uint64_t WAIT_BOUND_NS = 2ull * CNDE_DEFAULT_PERIOD_MS * 1000000ull;
bool fresh_or_waited(std::uint32_t before, std::uint32_t after, std::uint64_t took_ns) {
  return after > before || took_ns + 2000000ull >= WAIT_BOUND_NS;
}

/* Each joint as the descriptor writes it, worked out by hand from FAIR INNOVATION's page and from
 * the fake's own soft limits, in radians to five places — written here, not read back from the
 * driver, so that a unit or an index slipped in the driver cannot also slip in the expectation. */
struct JointText {
  const char *name;
  const char *role;
  const char *catalogue_low;
  const char *catalogue_high;
  const char *robot_low;
  const char *robot_high;
};

constexpr JointText JOINT_TEXT[JOINTS] = {
    {"j1", "right_arm.shoulder.yaw", "-3.05433", "3.05433", "-3.05433", "3.05433"},
    {"j2", "right_arm.shoulder.pitch", "-4.62512", "1.48353", "-4.62512", "1.48353"},
    {"j3", "right_arm.elbow.flex", "-2.61799", "2.61799", "-2.61799", "2.61799"},
    {"j4", "right_arm.wrist.pitch", "-4.62512", "1.48353", "-4.62512", "1.48353"},
    {"j5", "right_arm.wrist.yaw", "-3.05433", "3.05433", "-3.05433", "3.05433"},
    {"j6", "right_arm.wrist.roll", "-3.05433", "3.05433", "-1.57080", "1.57080"},
};

std::string joint_entry(const JointText &joint, const char *low, const char *high) {
  return std::string("\"name\":\"") + joint.name + "\",\"role\":\"" + joint.role +
         "\",\"type\":\"revolute\",\"limits\":{\"position_min_rad\":" + low +
         ",\"position_max_rad\":" + high + ",\"velocity_max_rad_s\":3.14159}";
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
    bool entries_right = true;
    for (const JointText &joint : JOINT_TEXT) {
      if (!has(descriptor, joint_entry(joint, joint.catalogue_low, joint.catalogue_high))) {
        entries_right = false;
        std::printf("        (no entry for %s)\n", joint.name);
      }
    }
    check(entries_right,
          "every joint's entry — name, role, travel in both directions and the 180 deg/s "
          "ceiling — is what the vendor's page says, in radians");
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
    bool entries_right = true;
    for (const JointText &joint : JOINT_TEXT) {
      if (!has(descriptor, joint_entry(joint, joint.robot_low, joint.robot_high))) {
        entries_right = false;
        std::printf("        (no entry for %s)\n", joint.name);
      }
    }
    check(entries_right,
          "and every joint carries the travel this controller reported, in both directions");
  }

  std::printf("\n== what the body reports ==\n");
  {
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.position_deg[0] = 12.0;
      body.position_deg[5] = -45.0;
      /* A number wider than sixteen bits: an error number read through a narrower field would
       * come back as 4464 and look like a different error. */
      body.main_code = 70000;
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
    check(state.raw.fault_code == 70000,
          "the controller's own main error code is reported as the fault word, all thirty-two bits "
          "of it and not the sub code beside it");
    /* DRY-COVERAGE-PLAN.md F206. The word alone is not read: §27's `rhal` condition reads DEGRADED
     * (`blackbox.rs`, `RhalSighting::of`), so a controller holding an error that set no flag left
     * the operating state at `present` for as long as the error stood. */
    check((state.raw.flags & SKR_FLAG_DEGRADED) != 0,
          "a controller that holds an error reads DEGRADED: the bit §27 reads");
    check((state.raw.flags & SKR_FLAG_FAULT) != 0,
          "and FAULT: it is not doing what it is told (§24)");
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
    const std::uint64_t again_from = raw_ns();
    check(driver->read_state(ctx, again.build(), 0) == SKR_OK, "a second read succeeds");
    check(fresh_or_waited(first, again.raw.seq, raw_ns() - again_from),
          "seq counts arrivals, not reads");

    body.paused.store(true);
    std::this_thread::sleep_for(std::chrono::milliseconds(120));
    StateBuffer stalled;
    const std::uint64_t stalled_from = raw_ns();
    check(driver->read_state(ctx, stalled.build(), 0) == SKR_OK,
          "a read against a stalled stream still returns the last sample");
    const std::uint64_t waited_ns = raw_ns() - stalled_from;
    check(waited_ns < 400ull * 1000000ull,
          "and returns inside a fifth of a second: a stalled stream is reported, not waited on "
          "while the loop that is reading it starves");
    check(raw_ns() - stalled.raw.stamp_ns >= 80ull * 1000000ull,
          "the sample's stamp is the instant its frame arrived, a hundred milliseconds ago, and "
          "not the instant the read returned (§24, §35.1)");
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
    /* A caller faster than the stream is slowed to it (`fresh_or_waited` says what that owes). The
     * first form of this check asked for five fresh frames and failed two runs in six on a loaded
     * machine. */
    std::uint32_t last_seq = recovered.raw.seq;
    int fresh_reads = 0;
    int unwaited_repeats = 0;
    for (int n = 0; n < 5; ++n) {
      StateBuffer quick;
      const std::uint64_t from = raw_ns();
      const int quick_rc = driver->read_state(ctx, quick.build(), 0);
      const std::uint64_t took = raw_ns() - from;
      if (quick_rc != SKR_OK || !fresh_or_waited(last_seq, quick.raw.seq, took)) {
        ++unwaited_repeats;
      } else if (quick.raw.seq > last_seq) {
        ++fresh_reads;
      }
      last_seq = quick.raw.seq;
    }
    check(unwaited_repeats == 0,
          "five reads in a row with nothing between them: each returns a frame not returned before, "
          "or waited the driver's bound for one first — a caller faster than the stream is slowed to "
          "it, because a repeated seq is how §24 recognises a stalled producer");
    check(fresh_reads >= 1,
          "and a stream that is on time gives at least one of them a frame of its own");
  }

  /* F206's other half (F198's class). The controller still holds the error the block above set, so
   * a command here is a command sent to a robot that says it is not doing what it is told — and
   * the driver's own check, the one that says *why*, is what stands between that and the wire. */
  std::printf("\n== a controller that holds an error ==\n");
  {
    std::uint64_t servo_j_before = 0;
    std::uint64_t starts_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      servo_j_before = body.servo_j;
      starts_before = body.move_starts;
    }
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = static_cast<float>(i) * 0.1f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_EPERM,
          "a command is refused while the controller holds an error");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.servo_j == servo_j_before && body.move_starts == starts_before,
            "and nothing reached the controller: no servo window opened, no ServoJ sent");
    }
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.main_code = 0;
      body.sub_code = 0;
    }
    check(wait_frames(server, 3), "the operator clears the error at the controller");
    StateBuffer cleared;
    check(driver->read_state(ctx, cleared.build(), 0) == SKR_OK, "state reads");
    check(cleared.raw.fault_code == 0 &&
              (cleared.raw.flags & (SKR_FLAG_FAULT | SKR_FLAG_DEGRADED | SKR_FLAG_ESTOP)) == 0,
          "and the driver reads healthy again: a controller's error is not sticky here");
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
    check(body.mode_calls == 0,
          "a controller already in auto mode is left there: Mode is for one found in manual");
    check(near(body.last_cmd_t, 0.008, 1e-9),
          "ServoJ's command period is the stream's 8 ms written in seconds, the vendor's unit");
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
  std::uint64_t before_starts = 0;
  {
    Cmd velocity;
    /* A different rate on every joint, so that a sign or an index wrong on one of them cannot hide
     * behind five that are right. */
    for (int i = 0; i < JOINTS; ++i) velocity.values[i] = 0.02f * static_cast<float>(i + 1);
    velocity.mask[2] = 0;
    check(driver->write_command(ctx, velocity.build(SKR_MODE_VELOCITY, 5), 0) == SKR_OK,
          "a velocity command is accepted");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      Call call;
      check(body.last_call(RPC_SERVO_JV, &call), "and goes out as ServoJV");
      bool every_joint = call.numbers.size() == JOINTS;
      for (int i = 0; i < JOINTS && every_joint; ++i) {
        const double wanted = i == 2 ? 0.0 : 0.02 * (i + 1) * RAD_TO_DEG;
        if (!near(call.numbers[i], wanted, 1e-3)) every_joint = false;
      }
      check(every_joint,
            "every joint's rate goes out in degrees per second, this wire's unit, with its own "
            "sign and its own slot; the masked joint is commanded zero velocity, which is that "
            "mode's way of saying nothing");
      check(near(body.last_cmd_t, 0.008, 1e-9), "and ServoJV's command period is 8 ms in seconds");
      check(body.move_starts == 1, "velocity shares the servo window position opened");
    }
    /* The one place the velocity conversion is exercised on a number that is not zero: the plant
     * takes up the commanded rate, and what comes back out of `read_state` has to be the radians
     * per second it was commanded in. */
    check(wait_frames(server, 3), "the plant takes up the commanded velocity");
    {
      StateBuffer moving;
      check(driver->read_state(ctx, moving.build(), 0) == SKR_OK, "state reads while moving");
      bool back = true;
      for (int i = 0; i < JOINTS; ++i) {
        if (!near(moving.velocity[i], i == 2 ? 0.0 : 0.02 * (i + 1), 1e-3)) back = false;
      }
      check(back,
            "and each joint's velocity comes back in radians per second, the unit it was "
            "commanded in, and the masked one is not moving");
    }

    Cmd torque;
    for (int i = 0; i < JOINTS; ++i) torque.values[i] = 1.5f;
    torque.mask[4] = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      before_starts = body.torque_starts;
    }
    check(driver->write_command(ctx, torque.build(SKR_MODE_TORQUE, 6), 0) == SKR_EPERM,
          "a torque command that masks a joint is refused: in torque a hold has no value, and "
          "zero is a joint that gives way (F121)");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.torque_starts == before_starts,
            "and nothing reached the controller — not even the window");
    }
    torque.mask[4] = 1;
    check(driver->write_command(ctx, torque.build(SKR_MODE_TORQUE, 7), 0) == SKR_OK,
          "a torque command on every joint is accepted");
    std::lock_guard<std::mutex> guard(body.lock);
    check(body.move_ends == 1, "switching to torque closed the position window");
    check(body.torque_starts == 1, "and opened the torque one, which is a different verb");
    Call call;
    check(body.last_call(RPC_SERVO_JT, &call), "the command goes out as ServoJT");
    check(near(call.numbers[0], 1.5, 1e-6) && near(call.numbers[4], 1.5, 1e-6),
          "newton-metres, unconverted, on every joint");
    check(near(body.last_cmd_t, 0.008, 1e-9), "and ServoJT's period is 8 ms in seconds");
  }

  std::printf("\n== the first command of a window, with a joint masked ==\n");
  {
    /* The torque window is still open, so a position command closes it and opens a new one, and
     * the held setpoints start again from where the robot is. A joint masked in that very first
     * command has no setpoint of this driver's to repeat: it is held where it was measured, and
     * a driver that started from zero would send it to zero degrees. */
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.position_deg[1] = 30.0;
    }
    check(wait_frames(server, 3), "the controller reports joint two at thirty degrees");
    Cmd hold;
    for (int i = 0; i < JOINTS; ++i) hold.values[i] = 0.1f;
    hold.mask[1] = 0;
    check(driver->write_command(ctx, hold.build(SKR_MODE_POSITION, 8), 0) == SKR_OK,
          "a position command that masks a joint is accepted as the first of a new window");
    std::lock_guard<std::mutex> guard(body.lock);
    Call call;
    check(body.last_call(RPC_SERVO_J, &call) && call.numbers.size() == JOINTS,
          "and it reached the controller");
    check(near(call.numbers[1], 30.0, 1.0),
          "the masked joint is held where the robot was when the window opened, not at zero "
          "degrees");
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

    /* The second input on its own: the vendor keeps SI0 and SI1 independent and refuses motion on
     * either, so a driver that read one of them twice would see a stop on SI0 and a healthy arm
     * with SI1 asserted. */
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.safety_stop1 = true;
      before = body.xmlrpc_requests;
    }
    check(wait_frames(server, 3), "the controller reports SI1 asserted, and SI0 not");
    StateBuffer second;
    check(driver->read_state(ctx, second.build(), 0) == SKR_OK, "state reads");
    check((second.raw.flags & SKR_FLAG_ESTOP) != 0,
          "SI1 alone shows in the state flags: either input stops the arm");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 12), 0) == SKR_EPERM,
          "and a command into SI1 is refused as well");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.xmlrpc_requests == before, "with nothing sent");
      body.safety_stop1 = false;
    }
    check(wait_frames(server, 3), "SI1 clears");

    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.estop_button = true;
      before = body.xmlrpc_requests;
    }
    check(wait_frames(server, 3), "the emergency stop button is pressed");
    StateBuffer pressed;
    check(driver->read_state(ctx, pressed.build(), 0) == SKR_OK, "state reads with it pressed");
    check((pressed.raw.flags & SKR_FLAG_ESTOP) != 0, "and the flag is set");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 13), 0) == SKR_EPERM,
          "and a command with the button pressed is refused, with nothing sent");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.xmlrpc_requests == before, "the controller saw no request");
    }
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

  std::printf("\n== releasing the latch without closing the session (§48.2) ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 0.05f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "and takes a command, which opens its servo window");
    std::uint64_t stop_frames_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      stop_frames_before = body.stop_frames;
    }
    check(driver->emergency_stop(ctx) == SKR_OK, "the driver's own stop is sent");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 2), 0) == SKR_EPERM,
          "and the latch refuses what follows");
    /* The stop frame goes to port 8080 and a servo window is opened over the XML-RPC port: two
     * sockets, two threads in the controller, and no order between them. A real operator's
     * power-on self-test comes seconds after a stop, so the order never matters on a robot; here
     * the next call is microseconds behind, and on a loaded machine the controller sometimes
     * handles the new window first and the old stop after it, which closes the window the command
     * is about to use. So the test waits for the controller to have taken the stop, which is the
     * state a self-test begins from. */
    bool stop_taken = false;
    for (int i = 0; i < 400 && !stop_taken; ++i) {
      {
        std::lock_guard<std::mutex> guard(body.lock);
        stop_taken = body.stop_frames > stop_frames_before;
      }
      if (!stop_taken) std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    check(stop_taken, "and the controller has taken the stop frame");
    check(driver->release_stop(ctx) == SKR_OK,
          "a power-on self-test releases the latch without closing the session");
    StateBuffer released;
    check(driver->read_state(ctx, released.build(), 0) == SKR_OK, "state reads");
    check((released.raw.flags & SKR_FLAG_ESTOP) == 0,
          "and no longer carries the stop flag: it was this driver's latch, and the controller's "
          "own stop circuit is not asserted");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 3), 0) == SKR_OK,
          "a command is accepted again — the stop ended the controller's servo window, so the "
          "driver opens one rather than sending ServoJ into none");
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  std::printf("\n== a second open on an open session ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    std::uint64_t clients_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      clients_before = body.cnde_clients;
    }
    /* Two reads straight after open, before the second frame has arrived and so before the driver
     * has measured an interval: the wait for a new frame is then the period it asked for, and the
     * second read must get the next frame rather than the first one again. */
    StateBuffer first_read;
    StateBuffer second_read;
    const int first_rc = driver->read_state(ctx, first_read.build(), 0);
    const std::uint64_t second_from = raw_ns();
    const int second_rc = driver->read_state(ctx, second_read.build(), 0);
    const std::uint64_t second_took = raw_ns() - second_from;
    check(first_rc == SKR_OK && second_rc == SKR_OK, "two reads straight after open succeed");
    /* What a read must never do here is return the previous sample *without waiting*, which is
     * what a bound of zero — the interval not yet measured — would do. */
    check(fresh_or_waited(first_read.raw.seq, second_read.raw.seq, second_took),
          "and the second is a new frame, or waited the period asked for twice over before saying "
          "there was none: the wait is the period asked for until an interval has been measured");
    skr_ctx *second = nullptr;
    check(driver->open(config.c_str(), &second) == SKR_EPERM,
          "a second open on the same driver is refused while one session is open");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      check(body.cnde_clients == clients_before,
            "and it did not connect a second state stream, which would orphan the first "
            "session's stop socket and reader");
    }
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  std::printf("\n== a controller that sends something else first ==\n");
  {
    /* A real CNDE stream interleaves frames of other types with the replies; the vendor's own
     * client skips them while it waits for the message it asked about. A driver that took the
     * first frame to be the reply would read a stray frame's first byte as a refusal. */
    body.stray_before_reply.store(true);
    check(driver->open(config.c_str(), &ctx) == SKR_OK,
          "a session opens although a frame of another type arrives ahead of each reply");
    body.stray_before_reply.store(false);
    check(driver->close(ctx) == SKR_OK, "and closes");
  }

  std::printf("\n== a controller that stops taking commands under an open window ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 0.05f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "and takes a command");
    std::uint64_t refusals_before = 0;
    char buffer[8192];
    auto refusals_now = [&]() {
      const int written = driver->extension(ctx, "vendor.fairino.telemetry", nullptr, 0, buffer,
                                            sizeof buffer);
      const std::string telemetry(buffer, written > 0 ? written : 0);
      const std::size_t at = telemetry.find("\"refusals\":");
      return at == std::string::npos ? ~0ull : std::strtoull(telemetry.c_str() + at + 11, nullptr, 10);
    };
    refusals_before = refusals_now();
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.enabled = false;
    }
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 2), 0) == SKR_EIO,
          "a ServoJ the controller refuses (its drives went down under the window) is reported "
          "as a failure, not as sent");
    check(refusals_now() == refusals_before + 1,
          "and counted among the refusals the telemetry reports");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.enabled = true;
    }
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  std::printf("\n== a command is bounded by the control loop's, whatever the controller does (F274) ==\n");
  {
    /* The loop waits for `write_command`, and a quarter of a second is its bound (`tools/faultproxy/chaos.py`). Every servo verb used to be given the driver's whole two-second RPC
     * timeout, so against a controller that answered late a command took as long as the answer did (1,497 ms under a stall, 2,028 ms under a blackhole). A whole command — the calls that
     * open a servo window included — is now given 200 ms, and what is not done in that is EIO at once and not a late write: a setpoint that arrives a second late is a different one. */
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 0.05f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "and takes a command");

    body.rpc_reply_delay_ms.store(1500);
    StderrCapture late;
    std::uint64_t slowest_ns = 0;
    int refused = 0;
    for (std::uint32_t seq = 2; seq < 8; ++seq) {
      const std::uint64_t from = raw_ns();
      if (driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, seq), 0) == SKR_EIO) refused += 1;
      slowest_ns = std::max(slowest_ns, raw_ns() - from);
    }
    const std::string said_late = late.finish();
    body.rpc_reply_delay_ms.store(0);
    check(refused == 6,
          "six commands to a controller whose answers are 1.5 s late are each EIO, and not late "
          "successes");
    check(slowest_ns < 250ull * 1000000ull,
          "the slowest returned inside the loop's quarter of a second (it took " +
              std::to_string(slowest_ns / 1000000ull) + " ms)");
    check(count_of(said_late, "did not answer ServoJ") == 1,
          "and said so once: a failure that goes on is one sentence, not one per period (F266)");
    check(has(said_late, "200 ms"), "naming the 200 ms a command is given");
    /* The fake's accept loop joins its connection threads when it has collected thirty-three, so while a reply it is holding is still to be sent a new connection waits for it. The six late
     * answers were due by 1.5 s after the last of the six requests; this is the wait that leaves the fake free to serve, and not a property of the driver. */
    std::this_thread::sleep_for(std::chrono::milliseconds(1600));
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 8), 0) == SKR_OK,
          "and once the controller answers in time the next command is taken");

    /* The other way a call fails: at once, so that it can repeat at the loop's own rate. */
    body.rpc_hang_up.store(true);
    StderrCapture hung;
    int failed = 0;
    for (std::uint32_t seq = 10; seq < 210; ++seq) {
      if (driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, seq), 0) == SKR_EIO) failed += 1;
    }
    const std::string said_hung = hung.finish();
    body.rpc_hang_up.store(false);
    check(failed == 200, "two hundred commands to a controller that hangs up on each connection "
                         "are each EIO");
    const bool said_once = count_of(said_hung, "ServoJ") == 1;
    check(said_once, "and are said once, not two hundred times: the same cause is not repeated "
                     "until a command has been taken in between");
    if (!said_once) std::printf("        (what was said:\n%s)\n", said_hung.c_str());
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 300), 0) == SKR_OK,
          "a command is taken when the controller serves again");
    body.rpc_hang_up.store(true);
    StderrCapture again;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 301), 0) == SKR_EIO,
          "and one that fails after that");
    const std::string said_again = again.finish();
    body.rpc_hang_up.store(false);
    check(count_of(said_again, "ServoJ") == 1,
          "is said again: the same cause after the entry point worked is news");

    /* A reply that arrives in pieces: a wait restarted by every piece never ends while the pieces keep coming, and a controller answering a byte every 40 ms has a hundred and more of them to
     * send. The budget is for the whole reply. */
    body.rpc_reply_trickle_ms.store(40);
    const std::uint64_t trickle_from = raw_ns();
    const int trickled = driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 400), 0);
    const std::uint64_t trickle_ns = raw_ns() - trickle_from;
    body.rpc_reply_trickle_ms.store(0);
    check(trickled == SKR_EIO && trickle_ns < 250ull * 1000000ull,
          "a reply that arrives a byte at a time is EIO inside the quarter of a second too (it "
          "took " + std::to_string(trickle_ns / 1000000ull) + " ms)");
    check(driver->close(ctx) == SKR_OK, "the session closes");

    /* A command that has a servo window to open is given the 200 ms for all of it: not 200 ms for each call, and not none for the first. Two controllers show the two ways it goes wrong. One
     * answers late (1.5 s), where a call that is not under the command's budget holds the loop for as long as the controller likes. The other answers every call in 150 ms, where RobotEnable,
     * ServoMoveStart and ServoJ are 450 ms if each is given a budget of its own, and 300 ms if only the call that opens the window is (a margin of 50 ms past the quarter of a second: a
     * mutant with that one call on its own budget passed at 120 ms, where it took 240). */
    for (const int answer_ms : {1500, 150}) {
      {
        std::lock_guard<std::mutex> guard(body.lock);
        body.enabled = false;
      }
      check(driver->open(config.c_str(), &ctx) == SKR_OK,
            "a session opens against a controller whose drives are off");
      check(wait_frames(server, 3), "and sees that");
      body.rpc_reply_delay_ms.store(answer_ms);
      const std::uint64_t opening_from = raw_ns();
      const int opening = driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0);
      const std::uint64_t opening_ns = raw_ns() - opening_from;
      body.rpc_reply_delay_ms.store(0);
      check(opening == SKR_EIO && opening_ns < 250ull * 1000000ull,
            "a command that has to enable the drives and open a window, each call answered in " +
                std::to_string(answer_ms) + " ms, is EIO inside the quarter of a second: the "
                "200 ms are the command's (it took " + std::to_string(opening_ns / 1000000ull) +
                " ms)");
      std::this_thread::sleep_for(std::chrono::milliseconds(answer_ms + 100));
      check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 2), 0) == SKR_OK,
            "and the next, to a controller that answers in time, opens the window and is taken");
      check(driver->close(ctx) == SKR_OK, "the session closes");
    }

    /* A controller whose XML-RPC service has hung: it is listening and not accepting, its queue is full, and a new connection waits for a SYN nobody answers. A connect given the two seconds of
     * `rpc_timeout_ms` holds the loop for two; given what is left of the command's budget it is EIO at 200 ms. A connect on loopback completes at once otherwise, so this is the one way the time a
     * connection is allowed is ever used here. */
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "and takes a command");
    body.rpc_accept_paused.store(true);
    std::vector<int> queued;
    for (int i = 0; i < 24; ++i) {
      const int fd = ::socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
      sockaddr_in remote{};
      remote.sin_family = AF_INET;
      remote.sin_port = htons(xmlrpc_port);
      ::inet_pton(AF_INET, "127.0.0.1", &remote.sin_addr);
      /* Whether this one completes or stays in progress, it is a client of the queue that fills. */
      ::connect(fd, reinterpret_cast<const sockaddr *>(&remote), sizeof remote);
      queued.push_back(fd);
    }
    StderrCapture stuck;
    const std::uint64_t stuck_from = raw_ns();
    const int stuck_rc = driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 2), 0);
    const std::uint64_t stuck_ns = raw_ns() - stuck_from;
    const std::string said_stuck = stuck.finish();
    body.rpc_accept_paused.store(false);
    for (const int fd : queued) ::close(fd);
    check(stuck_rc == SKR_EIO && stuck_ns < 250ull * 1000000ull,
          "a command to a service that is not accepting connections is EIO inside the quarter of "
          "a second (it took " + std::to_string(stuck_ns / 1000000ull) + " ms)");
    check(has(said_stuck, "was not accepted in time"),
          "and says the connection was not accepted in time, not that Linux's connect is in "
          "progress");
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 3), 0) == SKR_OK,
          "and when the service accepts again the next command is taken");

    /* A controller that refuses every command with the same error says so once: an arm whose drives went down under the window is refused at the loop's rate. */
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.enabled = false;
    }
    StderrCapture refusing;
    int refusals = 0;
    for (std::uint32_t seq = 10; seq < 60; ++seq) {
      if (driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, seq), 0) == SKR_EIO) refusals += 1;
    }
    const std::string said_refusing = refusing.finish();
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.enabled = true;
    }
    check(refusals == 50, "fifty commands a controller refuses are each EIO");
    check(count_of(said_refusing, "refused the command") == 1,
          "and the refusal is said once, not fifty times");
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  std::printf("\n== the stop never waits on the controller (F275) ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    /* `emergency_stop` wrote the vendor's frame to a socket that has a two-second send timeout, so a stop written into a full buffer waited for room: 211 ms, and 2,013 ms in another run, after
     * some tens of thousands of stops to a controller that had stopped reading. It never blocks now, and a stop that cannot be written is EIO at once. The loop is bounded, and the second check
     * is what says the buffers did fill: a host whose TCP buffers are larger than the loop can fill would otherwise pass the first for the wrong reason. */
    body.command_deaf.store(true);
    std::uint64_t slowest_ns = 0;
    long calls = 0;
    long unwritten = 0;
    const auto give_up = std::chrono::steady_clock::now() + std::chrono::seconds(30);
    while (calls < 1000000 && std::chrono::steady_clock::now() < give_up) {
      const std::uint64_t from = raw_ns();
      if (driver->emergency_stop(ctx) != SKR_OK) unwritten += 1;
      slowest_ns = std::max(slowest_ns, raw_ns() - from);
      calls += 1;
      if (slowest_ns > 100ull * 1000000ull || unwritten >= 100) break;
    }
    body.command_deaf.store(false);
    std::printf("        (%ld stops, %ld not written, the slowest %llu ms)\n", calls, unwritten,
                static_cast<unsigned long long>(slowest_ns / 1000000ull));
    check(slowest_ns < 50ull * 1000000ull,
          "a stop written to a controller that has stopped reading its instant-command port "
          "returns inside the 50 ms the RHAL gives it, buffers full or not");
    check(unwritten > 0,
          "and the buffers did fill, and a stop that could not be written said so (EIO) instead "
          "of waiting for room");
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  std::printf("\n== a controller left in manual mode ==\n");
  {
    check(driver->open(config.c_str(), &ctx) == SKR_OK, "a session opens");
    std::uint64_t modes_before = 0;
    {
      std::lock_guard<std::mutex> guard(body.lock);
      body.robot_mode = 1;
      modes_before = body.mode_calls;
    }
    check(wait_frames(server, 3), "the controller reports itself in manual mode");
    Cmd cmd;
    for (int i = 0; i < JOINTS; ++i) cmd.values[i] = 0.05f;
    check(driver->write_command(ctx, cmd.build(SKR_MODE_POSITION, 1), 0) == SKR_OK,
          "the first command is accepted");
    {
      std::lock_guard<std::mutex> guard(body.lock);
      Call call;
      check(body.mode_calls == modes_before + 1 && body.last_call(RPC_MODE, &call) &&
                call.numbers.size() == 1 && near(call.numbers[0], 0.0, 1e-9),
            "after one Mode(0): the switch to auto the pendant's mode needs, announced");
      check(body.robot_mode == 0, "and the controller is in auto now");
    }
    check(driver->close(ctx) == SKR_OK, "the session closes");
  }

  std::printf("\n== a stream that is slow, and then stops ==\n");
  {
    /* The wait for a new frame is twice the period the driver asked for, but never more than a
     * fifth of a second: at a 600 ms period twice the period would hold the loop that is reading
     * for over a second to learn what a stalled stream already says. */
    const std::string slow =
        config_for(xmlrpc_port, command_port, state_port, ",\"state_period_ms\":600");
    check(driver->open(slow.c_str(), &ctx) == SKR_OK, "a session opens against a 600 ms stream");
    check(wait_frames(server, 2), "and sees frames");
    StateBuffer fresh;
    check(driver->read_state(ctx, fresh.build(), 0) == SKR_OK, "it reads");
    body.paused.store(true);
    std::this_thread::sleep_for(std::chrono::milliseconds(700));
    StateBuffer stalled;
    const std::uint64_t from = raw_ns();
    check(driver->read_state(ctx, stalled.build(), 0) == SKR_OK,
          "and a read after the stream stops returns the last sample");
    check(raw_ns() - from < 500ull * 1000000ull,
          "inside half a second: the wait is capped at a fifth of one however long the period");
    body.paused.store(false);
    check(driver->close(ctx) == SKR_OK, "the session closes");
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
