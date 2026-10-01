/*
 * A fake FAIRINO FR controller: the robot's side of the two wires `fairino_bridge.cpp` speaks,
 * with a plant behind it.
 *
 * FAIR INNOVATION ship a simulator (`FAIRINO SimMachine`) and it is a Windows virtual machine
 * behind a registration wall, so it is not something a CI job can clone. What is public and
 * complete is the SDK's **source** (`FAIR-INNOVATION/fairino-cpp-sdk`, Apache-2.0), and a
 * controller written from it is a substrate this repository can own, run in a pipeline and
 * measure a driver against — the Fourier and Lite3 arrangement, one vendor further along because
 * here the wire came from source rather than from a disassembly.
 *
 * What it is:
 *
 *   * an **XML-RPC server** on 20003 answering the verbs `robot.cpp` sends, with the vendor's own
 *     reply shapes — a bare `<i4>` for a command, an array beginning with the error code for a
 *     query;
 *   * a **CNDE server** on 20005: it takes a period and a field list, refuses a field it does not
 *     know rather than padding the stream, and then streams exactly the fields asked for, in
 *     order, at the period asked for;
 *   * an **instant-command server** on 8080, which is where `StopMotion()`'s frame arrives;
 *   * a **plant**: six joints with soft limits, a velocity ceiling and three integrators, one per
 *     servo mode.
 *
 * What it is not: an FR controller. It has no dynamics worth the name, no collision model, no
 * kinematics (the TCP pose it streams is zeros, and is not among the fields the driver asks for),
 * no drag teaching, no Lua interpreter and no safety system. It exists to be commanded and to say
 * truthfully what it was commanded with.
 *
 * On `SIGTERM` it prints one line of JSON: everything it counted. `run-check.sh` reads that line
 * and fails the row on the counters that mean a defect — a command sent outside a servo window, a
 * setpoint past a soft limit, a malformed request — because a conformance run that passed while
 * commanding a controller that was refusing everything would otherwise be a green tick.
 *
 * Licence: Apache-2.0.
 */

#ifndef SKYNET_FAIRINO_FAKE_HPP
#define SKYNET_FAIRINO_FAKE_HPP

#include "fairino_wire.hpp"
#include "fairino_xmlrpc.hpp"

#include <arpa/inet.h>
#include <cerrno>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

using namespace skynet_fairino;

namespace {

/* ── what the controller can be asked to stream ───────────────────────── */

/* The vendor's own field table, name and streamed size, transcribed from
 * `libfairino/src/src/CNDE/FRCNDEClient.cpp`'s `InitAllStates()` — the seventy-six names its
 * default configuration asks for, in that order. The sizes are the CNDE types' (the fourth tuple
 * element), which for eight of these are not the packet struct's own: `gripper_fault` is
 * `UINT16` in `ROBOT_STATE_PKG` and `INT32` on the stream, and the analogue channels are integers
 * in the struct and doubles on the stream. Getting that wrong would put every later field at the
 * wrong offset, which is exactly what `cross_check.cpp` would see. */
struct Field {
  const char *name;
  std::size_t bytes;
};

const Field FIELDS[] = {
    {"program_state", 1},
    {"robot_state", 1},
    {"main_code", 4},
    {"sub_code", 4},
    {"robot_mode", 1},
    {"actual_joint_pos", 48},
    {"actual_TCP_pos", 48},
    {"actual_flange_pos", 48},
    {"actual_joint_vel", 48},
    {"actual_joint_acc", 48},
    {"target_TCP_cmpvel", 16},
    {"target_TCP_vel", 48},
    {"actual_TCP_cmpvel", 16},
    {"actual_TCP_vel", 48},
    {"actual_joint_torque", 48},
    {"tool_id", 4},
    {"wobj_id", 4},
    {"cfg_DO_box", 1},
    {"std_DO_box", 1},
    {"cfg_DO_tool", 1},
    {"cfg_DI_box", 1},
    {"std_DI_box", 1},
    {"cfg_DI_tool", 1},
    {"std_AI0_box", 8},
    {"std_AI1_box", 8},
    {"std_AI_tool", 8},
    {"ft_sensor_raw_data", 48},
    {"ft_sensor_data", 48},
    {"ft_sensor_active", 1},
    {"emergency_stop", 1},
    {"motion_done", 4},
    {"gripper_motion_done", 1},
    {"motion_queue_len", 4},
    {"collision_state", 1},
    {"trajectory_pnum", 4},
    {"safety_stop0_state", 1},
    {"safety_stop1_state", 1},
    {"gripper_fault_id", 1},
    {"gripper_fault", 4},
    {"gripper_active", 4},
    {"gripper_position", 1},
    {"gripper_speed", 4},
    {"gripper_current", 4},
    {"gripper_temp", 4},
    {"gripper_voltage", 4},
    {"aux_axis_state", 25},
    {"exaxis_status", 116},
    {"ext_DI_state", 16},
    {"ext_DO_state", 16},
    {"ext_AI_state", 16},
    {"ext_AO_state", 16},
    {"rbt_enable_state", 4},
    {"joint_driver_torque", 48},
    {"actual_joint_temp", 48},
    {"robot_time", 28},
    {"software_upgrade_state", 4},
    {"end_lua_err_code", 4},
    {"std_AO0_box", 8},
    {"std_AO1_box", 8},
    {"std_AO_tool", 8},
    {"rotating_gripper_num", 8},
    {"rotating_gripper_speed", 1},
    {"rotating_gripper_tor", 1},
    {"weld_break_off_state", 1},
    {"weld_arc_state", 1},
    {"target_joint_torque", 48},
    {"smarttool_state", 4},
    {"wide_voltage_ctrl_box_temp", 8},
    {"wide_voltage_ctrl_box_fan_current", 4},
    {"tool_coord", 48},
    {"wobj_coord", 48},
    {"exTool_coord", 48},
    {"exAxis_coord", 48},
    {"payload", 8},
    {"pay_cog", 24},
    {"last_servoJ_target", 48},
};

constexpr std::size_t FIELD_COUNT = sizeof(FIELDS) / sizeof(FIELDS[0]);

const Field *find_field(const std::string &name) {
  for (std::size_t i = 0; i < FIELD_COUNT; ++i) {
    if (name == FIELDS[i].name) return &FIELDS[i];
  }
  return nullptr;
}

/* ── the plant ────────────────────────────────────────────────────────── */

/* The soft limits *this* controller is configured with, which are deliberately **not** the
 * catalogue's: FAIR INNOVATION publish ±175° for wrist 3 and this one answers ±90°, the way a real
 * arm reads when something is bolted to its flange. The driver asks
 * `GetJointSoftLimitDeg` at `open()` and publishes what it is told; the suite asserts the
 * descriptor carries 90 and not 175, which is the whole claim about where those numbers come
 * from. */
constexpr double SOFT_LOW_DEG[JOINTS] = {-175.0, -265.0, -150.0, -265.0, -175.0, -90.0};
constexpr double SOFT_HIGH_DEG[JOINTS] = {175.0, 85.0, 150.0, 85.0, 175.0, 90.0};

/* The vendor's published per-axis ceiling. The plant clamps to it and counts when it does. */
constexpr double VELOCITY_MAX_DEG_S = 180.0;

/* Enough of a body to be commanded: a rotational inertia per joint for the torque mode, and a
 * constant load torque so that a joint told to hold still still reports something. Neither is a
 * measurement of anything — see the file header. */
constexpr double INERTIA[JOINTS] = {1.20, 1.50, 0.90, 0.40, 0.30, 0.20};
constexpr double LOAD_NM[JOINTS] = {0.0, 8.0, 4.0, 0.5, 0.2, 0.0};

enum class Window { None, Move, Torque };
enum class Moving { None, Position, Velocity };

struct Call {
  std::string method;
  std::vector<double> numbers;
};

class FakeController {
 public:
  /* ── counters, read by the suite and printed by the process wrapper ── */
  std::uint64_t xmlrpc_requests = 0;
  std::uint64_t unknown_method = 0;
  std::uint64_t malformed_request = 0;
  std::uint64_t servo_j = 0;
  std::uint64_t servo_jv = 0;
  std::uint64_t servo_jt = 0;
  std::uint64_t move_starts = 0;
  std::uint64_t move_ends = 0;
  std::uint64_t torque_starts = 0;
  std::uint64_t torque_ends = 0;
  std::uint64_t enables = 0;
  std::uint64_t mode_calls = 0;
  std::uint64_t identity_queries = 0;
  std::uint64_t limit_queries = 0;
  std::uint64_t commanded_without_window = 0;
  std::uint64_t commanded_while_disabled = 0;
  std::uint64_t beyond_travel = 0;
  std::uint64_t clamped_velocity = 0;
  std::uint64_t clamped_position = 0;
  std::uint64_t stop_frames = 0;
  std::uint64_t frames_8080 = 0;
  std::uint64_t unknown_frame_id = 0;
  std::uint64_t cnde_configs = 0;
  std::uint64_t cnde_unknown_field = 0;
  std::uint64_t cnde_starts = 0;
  std::uint64_t cnde_stops = 0;
  std::uint64_t cnde_frames_sent = 0;
  std::uint64_t cnde_clients = 0;
  std::uint64_t cnde_malformed = 0;
  int requested_period_ms = 0;
  double last_cmd_t = -1.0;
  std::size_t requested_fields = 0;
  std::string requested_list;

  /* ── the body ── */
  double position_deg[JOINTS] = {0, -90.0, 90.0, -90.0, 0, 0};
  double velocity_deg_s[JOINTS] = {0, 0, 0, 0, 0, 0};
  double torque_nm[JOINTS] = {0, 0, 0, 0, 0, 0};
  double target_deg[JOINTS] = {0, -90.0, 90.0, -90.0, 0, 0};
  double target_velocity_deg_s[JOINTS] = {0, 0, 0, 0, 0, 0};
  double target_torque_nm[JOINTS] = {0, 0, 0, 0, 0, 0};
  double last_servo_target_deg[JOINTS] = {0, 0, 0, 0, 0, 0};
  Window window = Window::None;
  Moving moving = Moving::None;
  bool enabled = false;
  int robot_mode = 0;   /* 0 auto, 1 manual */
  int program_state = 1;
  int robot_state = 1;
  int main_code = 0;
  int sub_code = 0;
  int queue_len = 0;
  int motion_done = 1;
  /* Settable by the suite, because a body whose stop can never assert is a body whose stop was
   * never tested. */
  bool estop_button = false;
  bool safety_stop0 = false;
  bool safety_stop1 = false;
  bool collision = false;

  std::vector<Call> calls;
  /* The exact frame the last stop arrived as, so a suite can hold it against the vendor's own
   * literal rather than against the fact that something arrived. */
  std::string last_stop_frame;
  /* Set by the suite to make the controller go quiet without closing the socket: a stream that
   * stops is how §24's stalled producer is tested, and a closed socket is a different failure. */
  std::atomic<bool> paused{false};
  /* Set by the suite to make the controller send a frame of another type ahead of each reply to a
   * configuration or a start, which a real stream does (the vendor's own client skips frames of
   * other types while it waits for the message it asked about). */
  std::atomic<bool> stray_before_reply{false};
  mutable std::mutex lock;

  void record(const std::string &method, std::vector<double> numbers) {
    calls.push_back(Call{method, std::move(numbers)});
    if (calls.size() > 512) calls.erase(calls.begin());
  }

  std::size_t count_calls(const char *method) const {
    std::size_t seen = 0;
    for (const Call &call : calls) {
      if (call.method == method) ++seen;
    }
    return seen;
  }

  bool last_call(const char *method, Call *out) const {
    for (std::size_t i = calls.size(); i > 0; --i) {
      if (calls[i - 1].method == method) {
        *out = calls[i - 1];
        return true;
      }
    }
    return false;
  }

  /* ── the plant ── */

  void integrate(double dt) {
    for (int i = 0; i < JOINTS; ++i) {
      if (window == Window::Move && moving == Moving::Position) {
        const double want = target_deg[i] - position_deg[i];
        const double step_limit = VELOCITY_MAX_DEG_S * dt;
        double step = want;
        if (step > step_limit) {
          step = step_limit;
          clamped_velocity += 1;
        } else if (step < -step_limit) {
          step = -step_limit;
          clamped_velocity += 1;
        }
        position_deg[i] += step;
        velocity_deg_s[i] = step / dt;
        torque_nm[i] = LOAD_NM[i] + 5.0 * (target_deg[i] - position_deg[i]) * DEG_TO_RAD;
      } else if (window == Window::Move && moving == Moving::Velocity) {
        double velocity = target_velocity_deg_s[i];
        if (velocity > VELOCITY_MAX_DEG_S) {
          velocity = VELOCITY_MAX_DEG_S;
          clamped_velocity += 1;
        } else if (velocity < -VELOCITY_MAX_DEG_S) {
          velocity = -VELOCITY_MAX_DEG_S;
          clamped_velocity += 1;
        }
        position_deg[i] += velocity * dt;
        velocity_deg_s[i] = velocity;
        torque_nm[i] = LOAD_NM[i];
      } else if (window == Window::Torque) {
        const double acceleration = (target_torque_nm[i] - LOAD_NM[i]) / INERTIA[i] * RAD_TO_DEG;
        velocity_deg_s[i] += acceleration * dt;
        if (velocity_deg_s[i] > VELOCITY_MAX_DEG_S) {
          velocity_deg_s[i] = VELOCITY_MAX_DEG_S;
          clamped_velocity += 1;
        } else if (velocity_deg_s[i] < -VELOCITY_MAX_DEG_S) {
          velocity_deg_s[i] = -VELOCITY_MAX_DEG_S;
          clamped_velocity += 1;
        }
        position_deg[i] += velocity_deg_s[i] * dt;
        torque_nm[i] = target_torque_nm[i];
      } else {
        velocity_deg_s[i] = 0.0;
        torque_nm[i] = LOAD_NM[i];
      }

      if (position_deg[i] < SOFT_LOW_DEG[i]) {
        position_deg[i] = SOFT_LOW_DEG[i];
        velocity_deg_s[i] = 0.0;
        clamped_position += 1;
      } else if (position_deg[i] > SOFT_HIGH_DEG[i]) {
        position_deg[i] = SOFT_HIGH_DEG[i];
        velocity_deg_s[i] = 0.0;
        clamped_position += 1;
      }
    }
    motion_done = window == Window::None ? 1 : 0;
    robot_state = window == Window::None ? 1 : 2;
  }

  /* ── the state stream ── */

  /* One field, into the stream, in the CNDE type's own size. Everything this controller does not
   * model is zeros of the right length — which is honest (it is a controller with no gripper, no
   * force sensor and no external axes) and keeps every later field at the offset the client
   * computed from its own list. */
  void write_field(const std::string &name, unsigned char *out, std::size_t bytes) const {
    std::memset(out, 0, bytes);
    auto put_doubles = [&](const double *values) {
      for (int i = 0; i < JOINTS; ++i) std::memcpy(out + i * sizeof(double), &values[i],
                                                   sizeof(double));
    };
    auto put_int32 = [&](std::int32_t value) { std::memcpy(out, &value, sizeof value); };
    if (name == "actual_joint_pos") return put_doubles(position_deg);
    if (name == "actual_joint_vel") return put_doubles(velocity_deg_s);
    if (name == "actual_joint_torque") return put_doubles(torque_nm);
    if (name == "last_servoJ_target") return put_doubles(last_servo_target_deg);
    if (name == "joint_driver_torque") {
      double drive[JOINTS];
      /* The drives' own torque, which on a real arm is not the joint's: it is the joint's plus
       * whatever the gearbox is eating. A constant ratio here, so that a suite can tell the two
       * fields apart. */
      for (int i = 0; i < JOINTS; ++i) drive[i] = torque_nm[i] * 1.1;
      return put_doubles(drive);
    }
    if (name == "actual_joint_temp") {
      double temperature[JOINTS];
      for (int i = 0; i < JOINTS; ++i) temperature[i] = 30.0 + i;
      return put_doubles(temperature);
    }
    if (name == "program_state") { out[0] = static_cast<unsigned char>(program_state); return; }
    if (name == "robot_state") { out[0] = static_cast<unsigned char>(robot_state); return; }
    if (name == "robot_mode") { out[0] = static_cast<unsigned char>(robot_mode); return; }
    if (name == "main_code") return put_int32(main_code);
    if (name == "sub_code") return put_int32(sub_code);
    if (name == "motion_done") return put_int32(motion_done);
    if (name == "motion_queue_len") return put_int32(queue_len);
    if (name == "rbt_enable_state") return put_int32(enabled ? 1 : 0);
    if (name == "emergency_stop") { out[0] = estop_button ? 1 : 0; return; }
    if (name == "safety_stop0_state") { out[0] = safety_stop0 ? 1 : 0; return; }
    if (name == "safety_stop1_state") { out[0] = safety_stop1 ? 1 : 0; return; }
    if (name == "collision_state") { out[0] = collision ? 1 : 0; return; }
  }

  bool build_payload(const std::vector<std::string> &names, std::vector<unsigned char> *out) const {
    std::size_t total = 0;
    for (const std::string &name : names) {
      const Field *field = find_field(name);
      if (field == nullptr) return false;
      total += field->bytes;
    }
    out->assign(total, 0);
    std::size_t at = 0;
    for (const std::string &name : names) {
      const Field *field = find_field(name);
      write_field(name, out->data() + at, field->bytes);
      at += field->bytes;
    }
    return true;
  }

  /* ── the verbs ── */

  /* Returns the XML-RPC reply for one call. The shapes are the vendor's: a command answers with a
   * bare integer error code, a query answers with an array whose first element is one. */
  std::string dispatch(const std::string &method, const std::vector<RpcValue> &params) {
    std::lock_guard<std::mutex> guard(lock);
    xmlrpc_requests += 1;

    auto numbers_of = [](const RpcValue &value) {
      std::vector<double> out;
      if (value.kind == RpcValue::Kind::Array) {
        for (const RpcValue &item : value.items) {
          double number = 0.0;
          item.as_double(&number);
          out.push_back(number);
        }
      } else {
        double number = 0.0;
        value.as_double(&number);
        out.push_back(number);
      }
      return out;
    };

    if (method == RPC_MODE) {
      long value = 0;
      if (params.empty() || !params[0].as_int(&value)) return fail(method, params);
      robot_mode = static_cast<int>(value);
      mode_calls += 1;
      record(method, {static_cast<double>(value)});
      return rpc_response(RpcValue::of_int(0));
    }
    if (method == RPC_ROBOT_ENABLE) {
      long value = 0;
      if (params.empty() || !params[0].as_int(&value)) return fail(method, params);
      enabled = value != 0;
      enables += 1;
      record(method, {static_cast<double>(value)});
      return rpc_response(RpcValue::of_int(0));
    }
    if (method == RPC_SERVO_MOVE_START || method == RPC_SERVO_JT_START) {
      const bool torque = method == RPC_SERVO_JT_START;
      record(method, {});
      if (torque) {
        torque_starts += 1;
      } else {
        move_starts += 1;
      }
      if (!enabled) {
        /* A real controller refuses to open a servo window on drives that are not energised.
         * `14` is this fake's stand-in for that refusal and the suite reads the refusal, not the
         * number. */
        commanded_while_disabled += 1;
        return rpc_response(RpcValue::of_int(14));
      }
      window = torque ? Window::Torque : Window::Move;
      moving = Moving::None;
      for (int i = 0; i < JOINTS; ++i) {
        target_deg[i] = position_deg[i];
        target_velocity_deg_s[i] = 0.0;
        target_torque_nm[i] = 0.0;
      }
      return rpc_response(RpcValue::of_int(0));
    }
    if (method == RPC_SERVO_MOVE_END || method == RPC_SERVO_JT_END) {
      record(method, {});
      if (method == RPC_SERVO_JT_END) {
        torque_ends += 1;
      } else {
        move_ends += 1;
      }
      window = Window::None;
      moving = Moving::None;
      return rpc_response(RpcValue::of_int(0));
    }
    if (method == RPC_SERVO_J || method == RPC_SERVO_JV || method == RPC_SERVO_JT) {
      if (params.empty() || params[0].kind != RpcValue::Kind::Array ||
          params[0].items.size() != static_cast<std::size_t>(JOINTS)) {
        return fail(method, params);
      }
      const std::vector<double> values = numbers_of(params[0]);
      record(method, values);
      /* `cmdT`, the period each setpoint is told to cover: the fifth parameter of ServoJ and ServoJV
       * and the second of ServoJT (the vendor's own signatures). Kept so a suite can hold it to
       * seconds. */
      const std::size_t cmd_t_at = method == RPC_SERVO_JT ? 1 : 4;
      if (params.size() > cmd_t_at) params[cmd_t_at].as_double(&last_cmd_t);
      const bool torque = method == RPC_SERVO_JT;
      if (window != (torque ? Window::Torque : Window::Move)) {
        /* The vendor's own sequencing: `ServoJ` outside a `ServoMoveStart` window is refused by
         * the controller. A driver that skipped the window would otherwise present as a robot
         * that silently ignores commands. */
        commanded_without_window += 1;
        return rpc_response(RpcValue::of_int(16));
      }
      if (!enabled) {
        commanded_while_disabled += 1;
        return rpc_response(RpcValue::of_int(14));
      }
      if (method == RPC_SERVO_J) {
        for (int i = 0; i < JOINTS; ++i) {
          if (values[i] < SOFT_LOW_DEG[i] || values[i] > SOFT_HIGH_DEG[i]) {
            beyond_travel += 1;
            return rpc_response(RpcValue::of_int(18));
          }
        }
        for (int i = 0; i < JOINTS; ++i) {
          target_deg[i] = values[i];
          last_servo_target_deg[i] = values[i];
        }
        moving = Moving::Position;
        servo_j += 1;
      } else if (method == RPC_SERVO_JV) {
        for (int i = 0; i < JOINTS; ++i) target_velocity_deg_s[i] = values[i];
        moving = Moving::Velocity;
        servo_jv += 1;
      } else {
        for (int i = 0; i < JOINTS; ++i) target_torque_nm[i] = values[i];
        servo_jt += 1;
      }
      queue_len += 1;
      return rpc_response(RpcValue::of_int(0));
    }
    if (method == RPC_GET_JOINT_SOFT_LIMIT) {
      limit_queries += 1;
      record(method, {});
      std::vector<RpcValue> items;
      items.push_back(RpcValue::of_int(0));
      for (int i = 0; i < JOINTS; ++i) {
        items.push_back(RpcValue::of_double(SOFT_LOW_DEG[i]));
        items.push_back(RpcValue::of_double(SOFT_HIGH_DEG[i]));
      }
      return rpc_response(RpcValue::of_array(std::move(items)));
    }
    if (method == RPC_GET_SOFTWARE_VERSION) {
      identity_queries += 1;
      record(method, {});
      return rpc_response(RpcValue::of_array({RpcValue::of_int(0), RpcValue::of_string("FR3"),
                                              RpcValue::of_string("fake-web-1.0"),
                                              RpcValue::of_string("fake-controller-3.9.9")}));
    }
    if (method == RPC_GET_ROBOT_SN) {
      identity_queries += 1;
      record(method, {});
      return rpc_response(
          RpcValue::of_array({RpcValue::of_int(0), RpcValue::of_string("FR3-FAKE-000001")}));
    }
    unknown_method += 1;
    record(method, {});
    /* Including `StopMotion`, deliberately: it is *not* an XML-RPC verb on a real controller, and
     * a fake that answered it here would let a driver's stop pass a test while going out on the
     * wrong wire. */
    return rpc_response_fault(-1, "no such method: " + method);
  }

  std::string fail(const std::string &method, const std::vector<RpcValue> &) {
    malformed_request += 1;
    return rpc_response_fault(-2, "malformed parameters for " + method);
  }

  /* ── the instant-command channel ── */

  void feed_frames(const char *bytes, std::size_t length) {
    std::lock_guard<std::mutex> guard(lock);
    frame_buffer_.append(bytes, length);
    for (;;) {
      const std::size_t start = frame_buffer_.find(FRAME_HEAD);
      if (start == std::string::npos) break;
      const std::size_t end = frame_buffer_.find(FRAME_TAIL, start + 4);
      if (end == std::string::npos) break;
      const std::string frame = frame_buffer_.substr(start, end + 4 - start);
      frame_buffer_.erase(0, end + 4);
      frames_8080 += 1;
      /* `/f/b` `III` count `III` id `III` length `III` content `III` `/b/f`, the vendor's
       * `UnpacketFrame` shape. Only the id matters here. */
      std::vector<std::string> parts;
      std::size_t at = 4;
      while (at < frame.size() - 4) {
        const std::size_t next = frame.find(FRAME_SEPARATOR, at);
        if (next == std::string::npos || next > frame.size() - 4) {
          parts.push_back(frame.substr(at, frame.size() - 4 - at));
          break;
        }
        parts.push_back(frame.substr(at, next - at));
        at = next + 3;
      }
      int id = -1;
      if (parts.size() >= 3) id = std::atoi(parts[2].c_str());
      if (id == FRAME_ID_STOP) {
        stop_frames += 1;
        last_stop_frame = frame;
        window = Window::None;
        moving = Moving::None;
        for (int i = 0; i < JOINTS; ++i) {
          target_deg[i] = position_deg[i];
          target_velocity_deg_s[i] = 0.0;
          target_torque_nm[i] = 0.0;
          velocity_deg_s[i] = 0.0;
        }
        program_state = 1;
        robot_state = 1;
      } else {
        unknown_frame_id += 1;
      }
    }
  }

  std::string report() const {
    char buffer[2048];
    std::snprintf(
        buffer, sizeof buffer,
        "{\"xmlrpc_requests\":%llu,\"unknown_method\":%llu,\"malformed_request\":%llu,"
        "\"servo_j\":%llu,\"servo_jv\":%llu,\"servo_jt\":%llu,\"move_starts\":%llu,"
        "\"move_ends\":%llu,\"torque_starts\":%llu,\"torque_ends\":%llu,\"enables\":%llu,"
        "\"mode_calls\":%llu,\"identity_queries\":%llu,\"limit_queries\":%llu,"
        "\"commanded_without_window\":%llu,\"commanded_while_disabled\":%llu,"
        "\"beyond_travel\":%llu,\"clamped_velocity\":%llu,\"clamped_position\":%llu,"
        "\"stop_frames\":%llu,\"frames_8080\":%llu,\"unknown_frame_id\":%llu,"
        "\"cnde_configs\":%llu,\"cnde_unknown_field\":%llu,\"cnde_starts\":%llu,"
        "\"cnde_stops\":%llu,\"cnde_frames_sent\":%llu,\"cnde_clients\":%llu,"
        "\"cnde_malformed\":%llu,\"requested_period_ms\":%d,\"requested_fields\":%zu}",
        static_cast<unsigned long long>(xmlrpc_requests),
        static_cast<unsigned long long>(unknown_method),
        static_cast<unsigned long long>(malformed_request),
        static_cast<unsigned long long>(servo_j), static_cast<unsigned long long>(servo_jv),
        static_cast<unsigned long long>(servo_jt), static_cast<unsigned long long>(move_starts),
        static_cast<unsigned long long>(move_ends),
        static_cast<unsigned long long>(torque_starts),
        static_cast<unsigned long long>(torque_ends), static_cast<unsigned long long>(enables),
        static_cast<unsigned long long>(mode_calls),
        static_cast<unsigned long long>(identity_queries),
        static_cast<unsigned long long>(limit_queries),
        static_cast<unsigned long long>(commanded_without_window),
        static_cast<unsigned long long>(commanded_while_disabled),
        static_cast<unsigned long long>(beyond_travel),
        static_cast<unsigned long long>(clamped_velocity),
        static_cast<unsigned long long>(clamped_position),
        static_cast<unsigned long long>(stop_frames),
        static_cast<unsigned long long>(frames_8080),
        static_cast<unsigned long long>(unknown_frame_id),
        static_cast<unsigned long long>(cnde_configs),
        static_cast<unsigned long long>(cnde_unknown_field),
        static_cast<unsigned long long>(cnde_starts), static_cast<unsigned long long>(cnde_stops),
        static_cast<unsigned long long>(cnde_frames_sent),
        static_cast<unsigned long long>(cnde_clients),
        static_cast<unsigned long long>(cnde_malformed), requested_period_ms, requested_fields);
    return buffer;
  }

 private:
  std::string frame_buffer_;
};

/* ── the sockets ──────────────────────────────────────────────────────── */

inline int listen_on(std::uint16_t port) {
  const int fd = ::socket(AF_INET, SOCK_STREAM, 0);
  if (fd < 0) return -1;
  int reuse = 1;
  ::setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof reuse);
  sockaddr_in local{};
  local.sin_family = AF_INET;
  local.sin_addr.s_addr = htonl(INADDR_ANY);
  local.sin_port = htons(port);
  if (::bind(fd, reinterpret_cast<const sockaddr *>(&local), sizeof local) < 0) {
    ::close(fd);
    return -2;
  }
  if (::listen(fd, 8) < 0) {
    ::close(fd);
    return -3;
  }
  /* An accept timeout so every loop below wakes to notice that it is being stopped. */
  timeval timeout{};
  timeout.tv_sec = 0;
  timeout.tv_usec = 50000;
  ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
  return fd;
}

inline bool send_all(int fd, const void *bytes, std::size_t length) {
  const unsigned char *at = static_cast<const unsigned char *>(bytes);
  std::size_t left = length;
  while (left > 0) {
    const ssize_t sent = ::send(fd, at, left, MSG_NOSIGNAL);
    if (sent <= 0) return false;
    at += sent;
    left -= static_cast<std::size_t>(sent);
  }
  return true;
}

/* The whole controller as one object: three listeners, their threads, and the body they share.
 * `fake_controller.cpp` wraps it in a process; `selftest.cpp` runs it in-process, which is what
 * lets the suite assert the exact parameters of the exact calls the driver made rather than
 * inferring them from a report. */
class FakeServer {
 public:
  FakeController controller;

  bool start(std::uint16_t xmlrpc_port, std::uint16_t command_port, std::uint16_t state_port) {
    xmlrpc_fd_ = listen_on(xmlrpc_port);
    command_fd_ = listen_on(command_port);
    state_fd_ = listen_on(state_port);
    if (xmlrpc_fd_ < 0 || command_fd_ < 0 || state_fd_ < 0) {
      stop();
      return false;
    }
    running_.store(true);
    threads_.emplace_back([this] { xmlrpc_loop(); });
    threads_.emplace_back([this] { command_loop(); });
    threads_.emplace_back([this] { state_loop(); });
    return true;
  }

  void stop() {
    running_.store(false);
    for (int *fd : {&xmlrpc_fd_, &command_fd_, &state_fd_}) {
      if (*fd >= 0) {
        ::shutdown(*fd, SHUT_RDWR);
        ::close(*fd);
        *fd = -1;
      }
    }
    for (std::thread &thread : threads_) {
      if (thread.joinable()) thread.join();
    }
    threads_.clear();
  }

  ~FakeServer() { stop(); }

 private:
  /* One XML-RPC connection, handled to EOF. The vendor's client opens one per verb and closes it;
   * this driver does the same; a client that kept it open and pipelined would also work. */
  void serve_xmlrpc(int fd) {
    std::string buffer;
    char chunk[4096];
    timeval timeout{};
    timeout.tv_sec = 1;
    ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
    while (running_.load()) {
      std::size_t header_bytes = 0;
      std::size_t content_length = 0;
      if (rpc_content_length(buffer, &header_bytes, &content_length) &&
          buffer.size() >= header_bytes + content_length) {
        const std::string body = buffer.substr(header_bytes, content_length);
        buffer.erase(0, header_bytes + content_length);
        std::string method;
        std::vector<RpcValue> params;
        std::string reply;
        if (!rpc_parse_request(body, &method, &params)) {
          std::lock_guard<std::mutex> guard(controller.lock);
          controller.malformed_request += 1;
          reply = rpc_response_fault(-2, "unparseable methodCall");
        } else {
          reply = controller.dispatch(method, params);
        }
        if (!send_all(fd, reply.data(), reply.size())) break;
        continue;
      }
      const ssize_t got = ::recv(fd, chunk, sizeof chunk, 0);
      if (got == 0) break;
      if (got < 0) {
        if (errno == EAGAIN || errno == EWOULDBLOCK) continue;
        break;
      }
      buffer.append(chunk, static_cast<std::size_t>(got));
      if (buffer.size() > (1u << 20)) break;
    }
    ::close(fd);
  }

  void xmlrpc_loop() {
    std::vector<std::thread> connections;
    while (running_.load()) {
      const int fd = ::accept(xmlrpc_fd_, nullptr, nullptr);
      if (fd < 0) continue;
      connections.emplace_back([this, fd] { serve_xmlrpc(fd); });
      /* Join the finished ones rather than detaching, so a run ends with nothing in flight and
       * the report is the same twice. */
      if (connections.size() > 32) {
        for (std::thread &thread : connections) {
          if (thread.joinable()) thread.join();
        }
        connections.clear();
      }
    }
    for (std::thread &thread : connections) {
      if (thread.joinable()) thread.join();
    }
  }

  void command_loop() {
    while (running_.load()) {
      const int fd = ::accept(command_fd_, nullptr, nullptr);
      if (fd < 0) continue;
      timeval timeout{};
      timeout.tv_sec = 0;
      timeout.tv_usec = 50000;
      ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
      char chunk[1024];
      while (running_.load()) {
        const ssize_t got = ::recv(fd, chunk, sizeof chunk, 0);
        if (got == 0) break;
        if (got < 0) {
          if (errno == EAGAIN || errno == EWOULDBLOCK) continue;
          break;
        }
        controller.feed_frames(chunk, static_cast<std::size_t>(got));
      }
      ::close(fd);
    }
  }

  /* CNDE: configure, start, stream. One client at a time, which is what a controller with one
   * state port has. */
  void serve_cnde(int fd) {
    {
      std::lock_guard<std::mutex> guard(controller.lock);
      controller.cnde_clients += 1;
    }
    std::vector<std::string> names;
    bool streaming = false;
    int period_ms = CNDE_DEFAULT_PERIOD_MS;
    std::string buffer;
    char chunk[2048];
    timeval timeout{};
    timeout.tv_sec = 0;
    timeout.tv_usec = 1000;
    ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
    auto next_send = std::chrono::steady_clock::now();
    std::uint8_t count = 0;

    while (running_.load()) {
      const ssize_t got = ::recv(fd, chunk, sizeof chunk, 0);
      if (got == 0) break;
      if (got > 0) buffer.append(chunk, static_cast<std::size_t>(got));
      if (got < 0 && errno != EAGAIN && errno != EWOULDBLOCK) break;

      /* Frames, length-prefixed, as many as arrived. */
      for (;;) {
        if (buffer.size() < 6) break;
        const unsigned char *head = reinterpret_cast<const unsigned char *>(buffer.data());
        if (head[0] != CNDE_HEAD_0 || head[1] != CNDE_HEAD_1) {
          std::lock_guard<std::mutex> guard(controller.lock);
          controller.cnde_malformed += 1;
          buffer.clear();
          break;
        }
        const std::size_t payload =
            static_cast<std::size_t>(head[4]) | (static_cast<std::size_t>(head[5]) << 8);
        if (buffer.size() < payload + CNDE_OVERHEAD) break;
        const std::uint8_t type = head[3];
        const std::string data = buffer.substr(6, payload);
        const bool tail_ok = static_cast<unsigned char>(buffer[payload + 6]) == CNDE_TAIL_0 &&
                             static_cast<unsigned char>(buffer[payload + 7]) == CNDE_TAIL_1;
        buffer.erase(0, payload + CNDE_OVERHEAD);
        if (!tail_ok) {
          std::lock_guard<std::mutex> guard(controller.lock);
          controller.cnde_malformed += 1;
          continue;
        }
        std::string reply_text;
        std::uint8_t reply_code = CNDE_MESSAGE_OK;
        if (type == CNDE_TYPE_OUTPUT_CONFIG) {
          if (data.size() < 2) {
            reply_code = CNDE_MESSAGE_FAIL;
            reply_text = "SHORT_CONFIG";
          } else {
            period_ms = static_cast<unsigned char>(data[0]) |
                        (static_cast<unsigned char>(data[1]) << 8);
            const std::string list = data.substr(2);
            std::vector<std::string> wanted;
            std::size_t at = 0;
            while (at <= list.size()) {
              const std::size_t comma = list.find(',', at);
              const std::string name =
                  list.substr(at, comma == std::string::npos ? std::string::npos : comma - at);
              if (!name.empty()) wanted.push_back(name);
              if (comma == std::string::npos) break;
              at = comma + 1;
            }
            std::string unknown;
            for (const std::string &name : wanted) {
              if (find_field(name) == nullptr) {
                unknown = name;
                break;
              }
            }
            std::lock_guard<std::mutex> guard(controller.lock);
            controller.cnde_configs += 1;
            if (!unknown.empty()) {
              /* The vendor's own client reads `NOT_FOUND` in the reply text as
               * `ERR_STATE_INVALID`. A fake that padded an unknown field instead would put every
               * later field at the wrong offset and call it a stream. */
              controller.cnde_unknown_field += 1;
              reply_code = CNDE_MESSAGE_FAIL;
              reply_text = "NOT_FOUND " + unknown;
            } else if (period_ms < CNDE_MIN_PERIOD_MS || period_ms > CNDE_MAX_PERIOD_MS) {
              reply_code = CNDE_MESSAGE_FAIL;
              reply_text = "PERIOD_OUT_OF_RANGE";
            } else {
              names = wanted;
              controller.requested_period_ms = period_ms;
              controller.requested_fields = wanted.size();
              controller.requested_list = list;
            }
          }
        } else if (type == CNDE_TYPE_START) {
          std::lock_guard<std::mutex> guard(controller.lock);
          controller.cnde_starts += 1;
          if (names.empty()) {
            reply_code = CNDE_MESSAGE_FAIL;
            reply_text = "NO_CONFIG";
          } else {
            streaming = true;
            next_send = std::chrono::steady_clock::now();
          }
        } else if (type == CNDE_TYPE_STOP) {
          std::lock_guard<std::mutex> guard(controller.lock);
          controller.cnde_stops += 1;
          streaming = false;
        } else {
          std::lock_guard<std::mutex> guard(controller.lock);
          controller.cnde_malformed += 1;
          continue;
        }

        if (controller.stray_before_reply.load()) {
          const unsigned char stray[4] = {1, 2, 3, 4};
          std::vector<unsigned char> stray_frame(sizeof stray + CNDE_OVERHEAD);
          const std::size_t stray_length =
              build_cnde_frame(stray_frame.data(), count++, 5, stray, sizeof stray);
          if (!send_all(fd, stray_frame.data(), stray_length)) {
            ::close(fd);
            return;
          }
        }
        std::vector<unsigned char> message;
        message.push_back(reply_code);
        message.insert(message.end(), reply_text.begin(), reply_text.end());
        std::vector<unsigned char> frame(message.size() + CNDE_OVERHEAD);
        const std::size_t length = build_cnde_frame(frame.data(), count++, CNDE_TYPE_MESSAGE,
                                                    message.data(), message.size());
        if (!send_all(fd, frame.data(), length)) {
          ::close(fd);
          return;
        }
      }

      if (!streaming || controller.paused.load()) continue;
      const auto now = std::chrono::steady_clock::now();
      if (now < next_send) continue;
      next_send = now + std::chrono::milliseconds(period_ms);
      std::vector<unsigned char> payload;
      {
        std::lock_guard<std::mutex> guard(controller.lock);
        controller.integrate(period_ms / 1000.0);
        if (!controller.build_payload(names, &payload)) continue;
        controller.cnde_frames_sent += 1;
      }
      std::vector<unsigned char> frame(payload.size() + CNDE_OVERHEAD);
      const std::size_t length = build_cnde_frame(frame.data(), count++, CNDE_TYPE_OUTPUT_STATE,
                                                  payload.data(), payload.size());
      if (!send_all(fd, frame.data(), length)) break;
    }
    ::close(fd);
  }

  void state_loop() {
    while (running_.load()) {
      const int fd = ::accept(state_fd_, nullptr, nullptr);
      if (fd < 0) continue;
      serve_cnde(fd);
    }
  }

  std::atomic<bool> running_{false};
  int xmlrpc_fd_ = -1;
  int command_fd_ = -1;
  int state_fd_ = -1;
  std::vector<std::thread> threads_;
};

}  // namespace

#endif  // SKYNET_FAIRINO_FAKE_HPP
