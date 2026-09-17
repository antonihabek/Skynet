/*
 * skynet-ros2-control — a body behind `ros2_control`, as one Skynet RHAL driver.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as a *"fake
 * `hardware_interface`"*, and `ros2_control` ships one: `mock_components/GenericSystem`, the fake
 * its own test suite runs against. So the whole of this driver is exercised against the real
 * framework with no robot — and, unlike a fake written here, one nobody in this repository wrote.
 *
 * ── It hosts a ResourceManager, and does not talk to a controller_manager ──
 *
 * The obvious shape is a ROS 2 node that publishes to `controller_manager` and subscribes to a
 * state broadcaster. This is not that, and the reason is §11:
 *
 *   No control decision may require a network round trip.
 *
 * A setpoint that crosses a DDS domain on its way from the Safety Kernel to a drive is a control
 * loop with a network in it, whatever the network is made of — and §25's ring exists precisely so
 * that the loop does not have one. `hardware_interface::ResourceManager` is the layer *underneath*
 * `controller_manager`: it loads the hardware plugins, owns their lifecycle, and hands out state
 * and command interfaces. That is the whole of what a Skynet driver needs, it is in-process, and
 * it works on a machine with no ROS graph running at all.
 *
 * What this costs is stated rather than absorbed: a robot driven this way is **not** also driven by
 * ROS 2 controllers. `ros2_control` allows one claim on a command interface, and this driver takes
 * it. That is the same exclusivity §25.2 gives a capability lease, arriving from the other side.
 *
 * ── The mapping, and the place the two models do not meet ──
 *
 * §24's command carries **one mode for the whole body**. `ros2_control` declares command interfaces
 * **per joint**, and nothing stops a URDF giving joint A `position` and joint B `velocity`.
 *
 * There is no honest way to command such a body through §24, so the descriptor declares the
 * **intersection**: the modes every joint offers. A body whose joints share none is a body this
 * driver reports with no control modes at all — which is the truthful descriptor for it, and which
 * `skynet driver check` then reads exactly as it reads `drivers/pendant` (§59.2): the joints are
 * reported and there is no mode in which they are commanded.
 *
 * ── What it will not declare ──
 *
 * A **torque rating**. A URDF's `<limit effort="...">` is a design value from a CAD model and not a
 * datasheet safety rating — §63.4, and the same rule that makes `skynet-urdf` write a literal null
 * envelope rather than copy one across. Position and velocity limits are declared because they are
 * what they say they are; `effort_max_nm` is absent and `envelope` is null, so the guards that need
 * one report unavailable.
 *
 * Build: see Makefile. Needs ROS 2 with `ros2_control` installed.
 * Licence: Apache-2.0.
 */

#include <cmath>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <sstream>
#include <string>
#include <vector>

#include <hardware_interface/component_parser.hpp>
#include <hardware_interface/loaned_command_interface.hpp>
#include <hardware_interface/loaned_state_interface.hpp>
#include <hardware_interface/resource_manager.hpp>
#include <rclcpp/rclcpp.hpp>

extern "C" {
#include "skynet_rhal.h"
}

namespace {

/// The three §24 modes, and the `ros2_control` interface name each maps to.
struct ModeMap {
  uint8_t mode;
  const char* interface;
  const char* skynet_name;
};

constexpr ModeMap MODES[] = {
    {SKR_MODE_POSITION, "position", "position"},
    {SKR_MODE_VELOCITY, "velocity", "velocity"},
    {SKR_MODE_TORQUE, "effort", "torque"},
};

std::string trim(const std::string& text) {
  const auto first = text.find_first_not_of(" \t\r\n\"");
  if (first == std::string::npos) return {};
  const auto last = text.find_last_not_of(" \t\r\n\"");
  return text.substr(first, last - first + 1);
}

/// `key=value` out of the configuration string.
///
/// Not JSON, for `drivers/mock`'s reason: a driver that needed a parser to be configured would be a
/// driver with a dependency, and this one already has ROS 2. One key today — `urdf` — and the
/// refusal below names it.
std::optional<std::string> setting(const std::string& config, const std::string& key) {
  const std::string needle = key + "=";
  auto at = config.find(needle);
  while (at != std::string::npos) {
    const bool starts_word = at == 0 || config[at - 1] == ' ' || config[at - 1] == ',';
    if (starts_word) {
      const auto from = at + needle.size();
      const auto to = config.find_first_of(" ,", from);
      return trim(config.substr(from, to == std::string::npos ? std::string::npos : to - from));
    }
    at = config.find(needle, at + needle.size());
  }
  return std::nullopt;
}

std::string escape(const std::string& text) {
  std::string out;
  for (char one : text) {
    if (one == '"' || one == '\\') out.push_back('\\');
    out.push_back(one);
  }
  return out;
}

std::string number(double value) {
  std::ostringstream out;
  out.precision(12);
  out << value;
  return out.str();
}

}  // namespace

/// One joint, as this driver sees it from both sides.
struct BridgeJoint {
  std::string name;
  /// `ros2_control` interface names this joint accepts a command on.
  std::set<std::string> command_interfaces;
  /// …and reports state on.
  std::set<std::string> state_interfaces;
  /// From the URDF's `<limit>`, where the joint declares one.
  std::optional<double> position_min;
  std::optional<double> position_max;
  std::optional<double> velocity_max;
  /// §5.1's canonical role, when the **operator** supplied one. Never inferred — see `roles=`.
  std::string role;
};

struct skr_ctx {
  bool open = false;
  std::shared_ptr<rclcpp::Clock> clock;
  std::unique_ptr<hardware_interface::ResourceManager> resources;
  std::vector<BridgeJoint> joints;
  /// The modes **every** joint offers — see the header.
  std::set<uint8_t> modes;
  std::string descriptor;
  uint32_t seq = 0;
  bool estop = false;
  uint64_t clock_ns = 0;
  /// Commands refused because the body offers no interface for the mode asked for.
  uint32_t commands_refused = 0;
};

namespace {

skr_ctx g_ctx;

void say(const std::string& message) {
  std::fprintf(stderr, "skynet-ros2-control: %s\n", message.c_str());
  std::fflush(stderr);
}

/// Read a joint's `<limit>` out of the URDF text.
///
/// By hand rather than through `urdf::Model`, and the reason is worth a line: parsing the URDF a
/// second time with the model library pulls in a plugin loader that scans every `urdf_parser_plugin`
/// on the machine, and a broken unrelated plugin then takes this driver down at `open()`. That
/// happened on the machine this was written on. What is needed here is three numbers from one
/// element, and `ros2_control` has already parsed the part that matters.
void read_limits(const std::string& urdf, BridgeJoint& joint) {
  const std::string opening = "<joint name=\"" + joint.name + "\"";
  auto at = urdf.find(opening);
  if (at == std::string::npos) return;
  const auto end = urdf.find("</joint>", at);
  const auto body = urdf.substr(at, end == std::string::npos ? std::string::npos : end - at);
  const auto limit = body.find("<limit");
  if (limit == std::string::npos) return;
  const auto limit_end = body.find('>', limit);
  const auto element = body.substr(limit, limit_end - limit);

  const auto attribute = [&element](const char* name) -> std::optional<double> {
    const std::string needle = std::string(name) + "=\"";
    const auto found = element.find(needle);
    if (found == std::string::npos) return std::nullopt;
    const auto from = found + needle.size();
    const auto to = element.find('"', from);
    try {
      return std::stod(element.substr(from, to - from));
    } catch (const std::exception&) {
      return std::nullopt;
    }
  };
  joint.position_min = attribute("lower");
  joint.position_max = attribute("upper");
  joint.velocity_max = attribute("velocity");
  // `effort` is deliberately not read. §63.4: a URDF effort limit is a design value from a CAD
  // model, not a datasheet safety rating, and copying one into a field the Safety Kernel enforces
  // would have it enforcing a rumour.
}

std::string build_descriptor(const skr_ctx& ctx) {
  std::ostringstream out;
  out << "{\"schema_version\":1,\"robot_slug\":\"ros2-control-system\",\"digest\":null,"
      << "\"verified\":false,\"simulator\":true,\"base\":\"fixed\",\"dof\":" << ctx.joints.size()
      << ",\"joints\":[";
  bool first_joint = true;
  for (const BridgeJoint& joint : ctx.joints) {
    if (!first_joint) out << ",";
    first_joint = false;
    out << "{\"name\":\"" << escape(joint.name) << "\",\"role\":";
    if (joint.role.empty()) {
      out << "null";
    } else {
      out << "\"" << escape(joint.role) << "\"";
    }
    out << ",\"type\":\"revolute\","
        << "\"limits\":{";
    bool first_limit = true;
    const auto emit = [&](const char* key, const std::optional<double>& value) {
      if (!value) return;
      if (!first_limit) out << ",";
      first_limit = false;
      out << "\"" << key << "\":" << number(*value);
    };
    emit("position_min_rad", joint.position_min);
    emit("position_max_rad", joint.position_max);
    emit("velocity_max_rad_s", joint.velocity_max);
    out << "},\"control_modes\":[";
    bool first_mode = true;
    for (const ModeMap& mode : MODES) {
      if (!ctx.modes.count(mode.mode)) continue;
      if (!joint.command_interfaces.count(mode.interface)) continue;
      if (!first_mode) out << ",";
      first_mode = false;
      out << "\"" << mode.skynet_name << "\"";
    }
    out << "]}";
  }
  out << "],\"end_effectors\":[],\"sensors\":[],\"control\":{\"modes\":[";
  bool first_mode = true;
  for (const ModeMap& mode : MODES) {
    if (!ctx.modes.count(mode.mode)) continue;
    if (!first_mode) out << ",";
    first_mode = false;
    out << "\"" << mode.skynet_name << "\"";
  }
  // The rates are the resource manager's update rate, which this driver sets and which a caller
  // may not exceed. Not a claim about the hardware behind the plugin: that is the plugin's, and
  // `ros2_control` does not publish it.
  out << "],\"rate_hz_max\":1000,\"rate_hz_min\":1},"
      // Null. Every rating a URDF carries is a design value (§63.4).
      << "\"envelope\":null,\"capabilities\":[],\"extensions\":[],\"solvers\":[]}";
  return out.str();
}

int bridge_open(const char* config_json, skr_ctx** out) {
  if (out == nullptr) return SKR_EINVAL;
  const std::string config = config_json == nullptr ? "" : config_json;
  const auto path = setting(config, "urdf");
  if (!path) {
    say(
        "no `urdf=` in the configuration. This driver has nothing to discover: `ros2_control` "
        "describes a body in a URDF's <ros2_control> element, and which URDF is a question only "
        "the operator can answer. Pass `urdf=/path/to/robot.urdf`.");
    return SKR_ENODEV;
  }

  std::ifstream file(*path);
  if (!file) {
    say("cannot read " + *path);
    return SKR_ENODEV;
  }
  std::stringstream buffer;
  buffer << file.rdbuf();
  const std::string urdf = buffer.str();

  std::vector<hardware_interface::HardwareInfo> infos;
  try {
    infos = hardware_interface::parse_control_resources_from_urdf(urdf);
  } catch (const std::exception& error) {
    say(std::string("this URDF has no usable <ros2_control> element: ") + error.what());
    return SKR_ENODEV;
  }
  if (infos.empty()) {
    say(*path +
        " parses and declares no <ros2_control> element. A URDF describes a body's geometry; the "
        "element is what says which hardware plugin drives it, and without one there is nothing "
        "to command.");
    return SKR_ENODEV;
  }

  g_ctx = skr_ctx{};
  g_ctx.clock = std::make_shared<rclcpp::Clock>(RCL_SYSTEM_TIME);

  for (const hardware_interface::HardwareInfo& info : infos) {
    for (const auto& joint : info.joints) {
      BridgeJoint bridge;
      bridge.name = joint.name;
      for (const auto& interface : joint.command_interfaces) {
        bridge.command_interfaces.insert(interface.name);
      }
      for (const auto& interface : joint.state_interfaces) {
        bridge.state_interfaces.insert(interface.name);
      }
      read_limits(urdf, bridge);
      // §5.1's roles, from the configuration and from nowhere else.
      //
      // A `ros2_control` URDF names joints and says nothing about what they are: `shoulder` is a
      // string, and whether it is `right_arm.shoulder.yaw` or the pan axis of a turret is a fact
      // about the machine that no file here carries. `skynet descriptor from-urdf` *suggests*
      // roles and waits for a human (§63.4), and a driver that inferred them would be making the
      // same guess with nobody to check it — so an unconfigured joint declares `role: null`,
      // `embodiment.roles.coverage` fails, and the report says which joints nobody has named.
      //
      //   urdf=... roles=shoulder:right_arm.shoulder.yaw;elbow:right_arm.elbow.flex
      if (const auto roles = setting(config, "roles")) {
        const std::string wanted = bridge.name + ":";
        auto at = roles->find(wanted);
        while (at != std::string::npos) {
          const bool starts_word = at == 0 || (*roles)[at - 1] == ';';
          if (starts_word) {
            const auto from = at + wanted.size();
            const auto to = roles->find(';', from);
            bridge.role = trim(
                roles->substr(from, to == std::string::npos ? std::string::npos : to - from));
            break;
          }
          at = roles->find(wanted, at + wanted.size());
        }
      }
      g_ctx.joints.push_back(std::move(bridge));
    }
  }
  if (g_ctx.joints.empty()) {
    say(*path +
        " declares a <ros2_control> element with no joints. A sensor-only component is a body "
        "with nothing to command, and this driver has no way to report that as anything but an "
        "empty robot.");
    return SKR_ENODEV;
  }

  // §24's one mode per body against `ros2_control`'s per-joint interfaces: the intersection, and
  // the header says why there is no honest alternative.
  for (const ModeMap& mode : MODES) {
    bool every = true;
    for (const BridgeJoint& joint : g_ctx.joints) {
      if (!joint.command_interfaces.count(mode.interface)) {
        every = false;
        break;
      }
    }
    if (every) g_ctx.modes.insert(mode.mode);
  }
  if (g_ctx.modes.empty()) {
    say(
        "no control mode is offered by every joint, so this body cannot be commanded through §24 "
        "— which carries one mode for the whole body. The descriptor declares no control modes, "
        "which is the truthful shape for it, and `write_command` refuses.");
  }

  try {
    g_ctx.resources = std::make_unique<hardware_interface::ResourceManager>(
        urdf, g_ctx.clock, rclcpp::get_logger("skynet-ros2-control"), /*activate_all=*/true,
        /*update_rate=*/1000);
  } catch (const std::exception& error) {
    say(std::string("the hardware plugin would not load: ") + error.what() +
        ". A plugin named in <hardware><plugin> has to be on AMENT_PREFIX_PATH, and "
        "`mock_components/GenericSystem` is the one ros2_control ships for a body that is not "
        "there.");
    return SKR_ENODEV;
  }

  g_ctx.descriptor = build_descriptor(g_ctx);
  g_ctx.open = true;
  g_ctx.clock_ns = 1000000000ull;
  *out = &g_ctx;
  return SKR_OK;
}

int bridge_close(skr_ctx* ctx) {
  if (ctx == nullptr) return SKR_EINVAL;
  if (ctx->resources) {
    ctx->resources->shutdown_components();
    ctx->resources.reset();
  }
  ctx->open = false;
  return SKR_OK;
}

double read_interface(skr_ctx* ctx, const std::string& joint, const char* interface) {
  const std::string key = joint + "/" + interface;
  if (!ctx->resources->state_interface_is_available(key)) return NAN;
  try {
    auto loaned = ctx->resources->claim_state_interface(key);
    const auto value = loaned.get_optional();
    return value.has_value() ? *value : NAN;
  } catch (const std::exception&) {
    // A state interface that is declared and cannot be read is **NaN**, never zero: §24's rule,
    // and a zero would be a joint reported as still on a body whose encoder has stopped answering.
    return NAN;
  }
}

int bridge_read_state(skr_ctx* ctx, skr_state* out, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == nullptr || out == nullptr) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;

  const rclcpp::Time now = ctx->clock->now();
  const rclcpp::Duration period = rclcpp::Duration::from_seconds(0.001);
  const auto status = ctx->resources->read(now, period);
  ctx->clock_ns += 1000000ull;
  ctx->seq += 1;

  out->stamp_ns = ctx->clock_ns;
  out->seq = ctx->seq;
  out->joint_count = static_cast<uint16_t>(ctx->joints.size());
  out->flags = 0;
  if (ctx->estop) out->flags |= SKR_FLAG_ESTOP;
  // `ros2_control` has no homing concept: a hardware plugin either reports a position or does not.
  // `unreported` is §24's value for a driver that has never heard of the question, and it is the
  // honest one here — `not_required` would be this driver answering for the plugin.
  out->flags |= SKR_HOMING_UNREPORTED;
  out->fault_code = 0;
  if (status.result != hardware_interface::return_type::OK) {
    out->flags |= SKR_FLAG_DEGRADED | SKR_FLAG_FAULT;
    out->fault_code = static_cast<uint32_t>(status.failed_hardware_names.size());
  }

  for (size_t index = 0; index < ctx->joints.size(); ++index) {
    const BridgeJoint& joint = ctx->joints[index];
    if (out->position_rad != nullptr) {
      out->position_rad[index] = static_cast<float>(read_interface(ctx, joint.name, "position"));
    }
    if (out->velocity_rad_s != nullptr) {
      out->velocity_rad_s[index] =
          static_cast<float>(read_interface(ctx, joint.name, "velocity"));
    }
    if (out->effort_nm != nullptr) {
      out->effort_nm[index] = static_cast<float>(read_interface(ctx, joint.name, "effort"));
    }
  }
  for (int i = 0; i < 7; ++i) out->base_pose[i] = NAN;
  for (int i = 0; i < 6; ++i) out->base_twist[i] = 0.0f;
  return (status.result == hardware_interface::return_type::OK) ? SKR_OK : SKR_EIO;
}

int bridge_write_command(skr_ctx* ctx, const skr_command* cmd, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == nullptr || cmd == nullptr) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;
  if (cmd->joint_count != ctx->joints.size()) {
    ctx->commands_refused += 1;
    return SKR_EINVAL;
  }
  if (cmd->values == nullptr) {
    ctx->commands_refused += 1;
    return SKR_EINVAL;
  }
  if (cmd->mode > SKR_MODE_TORQUE) {
    ctx->commands_refused += 1;
    return SKR_EINVAL;
  }
  if (!ctx->modes.count(cmd->mode)) {
    // A mode some joints offer and not all of them is refused rather than applied to the ones that
    // can take it: a body half commanded in torque and half holding is a trajectory nobody asked
    // for, which is §25.2's argument about two policies arriving one layer down.
    ctx->commands_refused += 1;
    return SKR_EINVAL;
  }
  if (ctx->estop) {
    ctx->commands_refused += 1;
    return SKR_EPERM;
  }

  const char* interface = nullptr;
  for (const ModeMap& mode : MODES) {
    if (mode.mode == cmd->mode) interface = mode.interface;
  }
  if (interface == nullptr) return SKR_EINVAL;

  for (size_t index = 0; index < ctx->joints.size(); ++index) {
    const uint8_t applied = cmd->mask == nullptr ? 1u : cmd->mask[index];
    if (!applied) continue;  // hold: the previous setpoint stands
    const float value = cmd->values[index];
    if (!std::isfinite(value)) {
      ctx->commands_refused += 1;
      return SKR_EINVAL;
    }
    const std::string key = ctx->joints[index].name + "/" + interface;
    try {
      auto loaned = ctx->resources->claim_command_interface(key);
      if (!loaned.set_value(static_cast<double>(value))) {
        return SKR_EIO;
      }
    } catch (const std::exception&) {
      return SKR_EIO;
    }
  }

  const rclcpp::Time now = ctx->clock->now();
  const auto status = ctx->resources->write(now, rclcpp::Duration::from_seconds(0.001));
  return (status.result == hardware_interface::return_type::OK) ? SKR_OK : SKR_EIO;
}

/*
 * Callable from a signal handler and from a dying process, which is what rules out the obvious
 * implementation.
 *
 * Deactivating the hardware components is what a `ros2_control` integration would do, and it
 * allocates, takes locks and calls into a plugin — none of which is safe from a signal handler. So
 * this latches a flag and nothing else: every subsequent `write_command` is refused, which stops
 * the setpoints at the boundary this driver owns.
 *
 * **That is not a rated stop and this driver does not report it as one.** A rated stop cuts power
 * through a circuit; the hardware plugin behind this may or may not have one, `ros2_control` has no
 * way to say, and the descriptor therefore declares no `estop` rather than `true`.
 */
int bridge_emergency_stop(skr_ctx* ctx) {
  if (ctx == nullptr) return SKR_EINVAL;
  ctx->estop = true;
  return SKR_OK;
}

int bridge_describe(skr_ctx* ctx, char* json_out, size_t len) {
  if (ctx == nullptr || json_out == nullptr) return SKR_EINVAL;
  const size_t needed = ctx->descriptor.size() + 1;
  if (len < needed) return SKR_EINVAL;
  std::memcpy(json_out, ctx->descriptor.c_str(), needed);
  return static_cast<int>(needed - 1);
}

const skr_driver BRIDGE_DRIVER = {
    SKYNET_RHAL_ABI_VERSION,
    "skynet-ros2-control",
    "0.1.0",
    bridge_open,
    bridge_close,
    bridge_read_state,
    bridge_write_command,
    bridge_emergency_stop,
    bridge_describe,
    nullptr,  // read_sensor: ros2_control's sensors are a separate component kind, not wired here
    nullptr,  // extension:   no vendor escape hatch
};

}  // namespace

extern "C" const skr_driver* skynet_rhal_entry(void) { return &BRIDGE_DRIVER; }

/* Test-only accessors, as `drivers/mock` has. */
extern "C" uint32_t ros2_commands_refused(const skr_ctx* ctx) { return ctx->commands_refused; }
extern "C" size_t ros2_joint_count(const skr_ctx* ctx) { return ctx->joints.size(); }
extern "C" size_t ros2_mode_count(const skr_ctx* ctx) { return ctx->modes.size(); }
