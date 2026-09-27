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
 * ── The mapping, and the place the two models used not to meet ──
 *
 * `ros2_control` declares command interfaces **per joint**, and nothing stops a URDF giving joint A
 * `position` and joint B `velocity`. §24's command used to carry **one mode for the whole body**,
 * so there was no honest way to command such a machine and this driver declared the
 * **intersection**: the modes every joint offers. A mobile manipulator — an arm on position, wheels
 * on velocity — shares none, so it was reported with no control modes at all and every command
 * against it was refused. Truthful, and it meant a perfectly ordinary robot was uncommandable.
 *
 * **§86 is the amendment that came out of this row.** The command now carries a mode per joint,
 * `control.modes` is the **union**, each joint's `control_modes` is its own, and this driver
 * declares `per_joint_modes` so transport knows the array will be read. `mixed_interfaces.urdf` was
 * the regression test for "no mode, and refused" and is now the regression test for "both modes,
 * and commanded" — the same file, asserting the opposite, which is what ratifying an amendment
 * looks like from inside a driver.
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

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#include <hardware_interface/component_parser.hpp>
#include <hardware_interface/loaned_command_interface.hpp>
#include <hardware_interface/loaned_state_interface.hpp>
#include <hardware_interface/resource_manager.hpp>
#include <hardware_interface/types/lifecycle_state_names.hpp>
#include <lifecycle_msgs/msg/state.hpp>
#include <rclcpp/rclcpp.hpp>
#include <rclcpp_lifecycle/state.hpp>

extern "C" {
#include "skynet_rhal.h"
}

// For `skynet::acquisition_ns` only: the clock every stamp is on (§24, §35.1). The rest of this
// driver speaks the C ABI directly.
#include "skynet_rhal.hpp"

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
/// driver with a dependency, and this one already has ROS 2. Five keys — `urdf`, `roles`,
/// `modes`, `simulator`, `update_rate` — and the refusal below names the one that matters.
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
  /// From the URDF's `<limit>`, where the joint declares one — narrowed by the position command
  /// interface's own `min`/`max` where those parse (see `narrow_to_command_range`).
  std::optional<double> position_min;
  std::optional<double> position_max;
  std::optional<double> velocity_max;
  /// The position command interface's `min`/`max` params, when both are numbers. A plugin may
  /// enforce them — PickNik's ABB interface throws on a command outside them (F131).
  std::optional<double> command_min;
  std::optional<double> command_max;
  /// The URDF's joint `type`, passed through. `revolute` when the file does not say — the
  /// behaviour every body had before the first continuous joint arrived (Kinova Gen3).
  std::string joint_type = "revolute";
  /// §5.1's canonical role, when the **operator** supplied one. Never inferred — see `roles=`.
  std::string role;
};

struct skr_ctx {
  bool open = false;
  // Whether the body is simulated (§35.3). The driver cannot tell a hardware plugin from a
  // simulation of one — mock_components answers identically, which is what makes it useful —
  // so the operator declares it and the default is the safe direction.
  bool simulator = true;
  /// The URDF's own `<robot name>`, slugified the way `skynet descriptor from-urdf` does, so a
  /// driver descriptor and a drafted one agree on which body they describe. Empty when the file
  /// carries no usable name, and the descriptor then falls back to the driver's own.
  std::string robot_name;
  std::shared_ptr<rclcpp::Clock> clock;
  std::unique_ptr<hardware_interface::ResourceManager> resources;
  std::vector<BridgeJoint> joints;
  /// The modes **every** joint offers — see the header.
  std::set<uint8_t> modes;
  std::string descriptor;
  uint32_t seq = 0;
  bool estop = false;
  /// The manager rate this session configured, in Hz. The first read settles against it.
  unsigned update_rate = 1000;
  /// The command interfaces the last mode switch started. A plugin that runs one controller
  /// per mode — the UR hardware interface runs a position controller, a velocity controller,
  /// or none — only drives the started set, and the bridge is the only thing here that can
  /// start one (DRY-COVERAGE-PLAN.md F96).
  std::vector<std::string> switched_interfaces;
  /// Armed by `open()` (which resets this whole struct) and consumed by the first
  /// `read_state()`: a real plugin reports NaN until its first stream sample arrives, and the
  /// first read waits for it, bounded — see `bridge_read_state`.
  bool first_read_pending = true;
  /// Commands refused because the body offers no interface for the mode asked for.
  uint32_t commands_refused = 0;
  /// When the plugins were last read and last written, for the period each call hands them —
  /// see `period_since`. Default-constructed (the epoch) until the first call of each kind.
  std::chrono::steady_clock::time_point last_read{};
  std::chrono::steady_clock::time_point last_write{};
  /// Whether the plugins' last `read` is still waiting for the `write` that answers it — see
  /// `read_in_turn` and `write_in_turn`, which are the only places that touch it.
  bool read_unanswered = false;
  /// The `<ros2_control>` components the URDF declares, by name — what `open()` held to ACTIVE.
  std::vector<std::string> components;
  /// Set by the first read or write that failed. From then on the manager is asked which
  /// components are still up — see `components_down`.
  bool hardware_failed = false;
};

namespace {

skr_ctx g_ctx;

void say(const std::string& message) {
  std::fprintf(stderr, "skynet-ros2-control: %s\n", message.c_str());
  std::fflush(stderr);
}

/// Read the URDF's `<robot name>` out of the URDF text, slugified.
///
/// The same rule `skynet descriptor from-urdf` applies (lowercase, hyphen-separated,
/// alphanumeric — `crates/skynet-urdf/src/descriptor.rs`), so the two agree on the body. Comments
/// are stripped first, because a provenance comment that names the attribute — this driver's own
/// `urdf/ur5e_mock.urdf` header does — is not the element. Empty when the file carries no usable
/// name; the descriptor then falls back to the driver's rather than refusing, because a body that
/// commands fine is not uncommandable for being unnamed.
std::string read_robot_name(const std::string& urdf) {
  std::string text;
  text.reserve(urdf.size());
  auto at = std::string::size_type{0};
  while (at < urdf.size()) {
    const auto open = urdf.find("<!--", at);
    if (open == std::string::npos) {
      text.append(urdf, at, std::string::npos);
      break;
    }
    text.append(urdf, at, open - at);
    const auto close = urdf.find("-->", open);
    if (close == std::string::npos) break;
    at = close + 3;
  }
  const auto tag = text.find("<robot");
  if (tag == std::string::npos) return "";
  const auto end = text.find('>', tag);
  const auto element = text.substr(tag, end == std::string::npos ? std::string::npos : end - tag);
  const std::string needle = "name=\"";
  const auto found = element.find(needle);
  if (found == std::string::npos) return "";
  const auto from = found + needle.size();
  const auto to = element.find('"', from);
  const std::string raw =
      element.substr(from, to == std::string::npos ? std::string::npos : to - from);
  std::string out;
  bool pending_separator = false;
  for (char ch : raw) {
    const bool alphanumeric = (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') ||
                              (ch >= '0' && ch <= '9');
    if (alphanumeric) {
      if (pending_separator && !out.empty()) out.push_back('-');
      pending_separator = false;
      out.push_back(static_cast<char>(ch >= 'A' && ch <= 'Z' ? ch - 'A' + 'a' : ch));
    } else {
      pending_separator = true;
    }
  }
  return out;
}

/// Read a joint's `<limit>` out of the URDF text.
///
/// By hand rather than through `urdf::Model`, and the reason is worth a line: parsing the URDF a
/// second time with the model library pulls in a plugin loader that scans every `urdf_parser_plugin`
/// on the machine, and a broken unrelated plugin then takes this driver down at `open()`. That
/// happened on the machine this was written on. What is needed here is three numbers from one
/// element, and `ros2_control` has already parsed the part that matters.
/// The URDF `<joint>` element for a hardware joint, up to its closing tag.
///
/// The first `<joint name="…">` in the file is not always the element: a file may list its
/// `<ros2_control>` tags before its arm (Kinova's does), and those tags carry no `type`. The
/// element is the first occurrence whose tag does. Empty when no occurrence qualifies.
std::string find_joint_element(const std::string& urdf, const std::string& name) {
  const std::string opening = "<joint name=\"" + name + "\"";
  auto at = urdf.find(opening);
  while (at != std::string::npos) {
    const auto tag_end = urdf.find('>', at);
    if (tag_end == std::string::npos) return "";
    if (urdf.substr(at, tag_end - at).find("type=\"") != std::string::npos) {
      const auto close = urdf.find("</joint>", at);
      return urdf.substr(at, close == std::string::npos ? std::string::npos : close - at);
    }
    at = urdf.find(opening, tag_end);
  }
  return "";
}

void read_limits(const std::string& urdf, BridgeJoint& joint) {
  const std::string body = find_joint_element(urdf, joint.name);
  if (body.empty()) return;
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

/// A number out of an interface parameter, or nothing. Parameters are strings in the URDF, and a
/// vendor file may carry an unexpanded xacro expression there (`{-2*pi}` in Doosan's mock) — which
/// is no number and so no bound, rather than a zero.
std::optional<double> parameter_number(const std::string& text) {
  if (text.empty()) return std::nullopt;
  try {
    std::size_t used = 0;
    const double value = std::stod(text, &used);
    if (used != text.size() || !std::isfinite(value)) return std::nullopt;
    return value;
  } catch (const std::exception&) {
    return std::nullopt;
  }
}

/// The travel the descriptor declares is the travel the plugin will take: the URDF's `<limit>`,
/// narrowed by the position command interface's `min`/`max`.
///
/// Two files can say two things about one joint, and PickNik's IRB 1200 description does — its
/// `<ros2_control>` block gives joint 6 ±2.23 rad where the joint's `<limit>` gives ±2π, joint 2
/// ±2.41 where the arm stops at −1.745 — and the plugin enforces the block's, by throwing. A
/// command inside the `<limit>` and outside the block left that component failed for the rest of
/// the session, every later write refused, back in range or not (DRY-COVERAGE-PLAN.md F131). The
/// intersection is the only travel both the plugin and the arm accept, so it is the one declared.
void narrow_to_command_range(BridgeJoint& joint) {
  if (joint.command_min) {
    joint.position_min =
        joint.position_min ? std::max(*joint.position_min, *joint.command_min) : *joint.command_min;
  }
  if (joint.command_max) {
    joint.position_max =
        joint.position_max ? std::min(*joint.position_max, *joint.command_max) : *joint.command_max;
  }
}

/// Read a joint's `type` out of the URDF text.
///
/// Beside `read_limits` rather than in it: one function per fact. The descriptor used to say
/// `revolute` for every joint, which was true of every body until the Kinova Gen3 brought four
/// continuous ones — a joint without travel limits described as a joint with them.
void read_joint_type(const std::string& urdf, BridgeJoint& joint) {
  const std::string body = find_joint_element(urdf, joint.name);
  if (body.empty()) return;
  const auto end = body.find('>');
  const auto element = body.substr(0, end);
  const std::string needle = "type=\"";
  const auto found = element.find(needle);
  if (found == std::string::npos) return;
  const auto from = found + needle.size();
  const auto to = element.find('"', from);
  if (to == std::string::npos) return;
  joint.joint_type = element.substr(from, to - from);
}

std::string build_descriptor(const skr_ctx& ctx) {
  std::ostringstream out;
  // F69's name: this driver hosts whatever body the URDF names, so the descriptor carries the
  // URDF's own `<robot name>`, slugified. The driver's own name is the fallback for a file that
  // carries no usable one — the behaviour every body in this directory had until `ur5e_mock`.
  const std::string slug = ctx.robot_name.empty() ? "ros2-control-system" : ctx.robot_name;
  out << "{\"schema_version\":1,\"robot_slug\":\"" << escape(slug) << "\",\"digest\":null,"
      << "\"verified\":false,\"simulator\":" << (ctx.simulator ? "true" : "false")
      << ",\"base\":\"fixed\",\"dof\":" << ctx.joints.size() << ",\"joints\":[";
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
    out << ",\"type\":\"" << escape(joint.joint_type) << "\","
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
    // §86.1: the joint's own interfaces, with nothing intersected away. This was already the
    // per-joint truth; what changed is that it is no longer filtered by what every *other* joint
    // also offers.
    out << "},\"control_modes\":[";
    bool first_mode = true;
    for (const ModeMap& mode : MODES) {
      if (!joint.command_interfaces.count(mode.interface)) continue;
      if (!first_mode) out << ",";
      first_mode = false;
      out << "\"" << mode.skynet_name << "\"";
    }
    out << "]}";
  }
  // §86.1: the **union** — every mode some joint offers. `ctx.modes` is that union now; it used to
  // be the intersection, and the difference is the whole amendment.
  out << "],\"end_effectors\":[],\"sensors\":[],\"control\":{\"modes\":[";
  bool first_mode = true;
  for (const ModeMap& mode : MODES) {
    if (!ctx.modes.count(mode.mode)) continue;
    if (!first_mode) out << ",";
    first_mode = false;
    out << "\"" << mode.skynet_name << "\"";
  }
  // This driver reads the per-joint array, so transport may mix modes across these joints.
  out << "],\"per_joint_modes\":true";
  // The rates are the resource manager's update rate, which this driver sets and which a caller
  // may not exceed. Not a claim about the hardware behind the plugin: that is the plugin's, and
  // `ros2_control` does not publish it.
  out << ",\"rate_hz_max\":1000,\"rate_hz_min\":1},"
      // Null. Every rating a URDF carries is a design value (§63.4).
      << "\"envelope\":null,"
      // `bridge_emergency_stop` latches and refuses every later command — the software stop
      // the driver actually has, in the tradition `drivers/ur_rtde` sets. The rated stop
      // stays undeclared for the stop's own reason (below): no interface here reaches it.
      << "\"capabilities\":[\"safety.estop\"],\"extensions\":[],\"solvers\":[]}";
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
  g_ctx.robot_name = read_robot_name(urdf);
  // Config first, then the safe default. `simulator=false` is what a real integration passes
  // deliberately; anything else, including absence, declares simulated.
  if (const auto sim = setting(config, "simulator")) {
    g_ctx.simulator = *sim != "false";
  }
  g_ctx.clock = std::make_shared<rclcpp::Clock>(RCL_SYSTEM_TIME);

  for (const hardware_interface::HardwareInfo& info : infos) {
    for (const auto& joint : info.joints) {
      BridgeJoint bridge;
      bridge.name = joint.name;
      for (const auto& interface : joint.command_interfaces) {
        bridge.command_interfaces.insert(interface.name);
        if (interface.name == "position") {
          bridge.command_min = parameter_number(interface.min);
          bridge.command_max = parameter_number(interface.max);
        }
      }
      for (const auto& interface : joint.state_interfaces) {
        bridge.state_interfaces.insert(interface.name);
      }
      read_limits(urdf, bridge);
      narrow_to_command_range(bridge);
      read_joint_type(urdf, bridge);
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

  // The modes the plugin behind the URDF actually honours, when the operator says so.
  //
  // A command interface's *name* is a convention, and the bridge maps it the way `ros2_control`'s
  // own controllers read it: `velocity` is a velocity setpoint. A plugin may use the name for
  // something else. Doosan's `DRHWInterface` exports a `velocity` command interface on every
  // joint and moves only when the `position` command changes — velocity rides along as a
  // feed-forward in real mode and is ignored in virtual mode — so a velocity command against it
  // reports success and moves nothing, which DRCF measured (DRY-COVERAGE-PLAN.md F126). Nothing in
  // the URDF says so, and nothing this side of the plugin's API can find out, so the integrator
  // declares it, as with `roles=`:
  //
  //   urdf=... modes=position
  //
  // It only narrows. Every name must be one of §24's three and offered by some joint; the
  // interfaces of the modes left out are dropped from every joint before the descriptor is built,
  // so the descriptor never offers them and `write_command` refuses them.
  if (const auto wanted = setting(config, "modes")) {
    std::set<std::string> keep;
    std::string rest = *wanted;
    while (!rest.empty()) {
      const auto cut = rest.find(';');
      const std::string name = trim(rest.substr(0, cut));
      rest = cut == std::string::npos ? "" : rest.substr(cut + 1);
      const ModeMap* found = nullptr;
      for (const ModeMap& mode : MODES) {
        if (name == mode.skynet_name) found = &mode;
      }
      if (found == nullptr) {
        say("`modes=` names `" + name +
            "`, which is not one of §24's modes. It takes `position`, `velocity` and `torque`, "
            "separated by `;` — the ones the plugin behind this URDF honours.");
        return SKR_ENODEV;
      }
      bool offered = false;
      for (const BridgeJoint& joint : g_ctx.joints) {
        offered = offered || joint.command_interfaces.count(found->interface) > 0;
      }
      if (!offered) {
        say("`modes=` names `" + name + "`, and no joint in " + *path + " declares a `" +
            found->interface +
            "` command interface. The key narrows what the URDF offers; it cannot add to it.");
        return SKR_ENODEV;
      }
      keep.insert(found->interface);
    }
    if (keep.empty()) {
      say("`modes=` is empty. Omit it to offer every mode the URDF declares, or name the ones the "
          "plugin honours.");
      return SKR_ENODEV;
    }
    for (BridgeJoint& joint : g_ctx.joints) {
      for (const ModeMap& mode : MODES) {
        if (!keep.count(mode.interface)) joint.command_interfaces.erase(mode.interface);
      }
    }
  }

  // §86.1: the **union** — every mode some joint offers. The intersection was what §24 forced
  // before a command could carry a mode per joint, and it made a mobile manipulator uncommandable.
  for (const ModeMap& mode : MODES) {
    for (const BridgeJoint& joint : g_ctx.joints) {
      if (joint.command_interfaces.count(mode.interface)) {
        g_ctx.modes.insert(mode.mode);
        break;
      }
    }
  }
  if (g_ctx.modes.empty()) {
    say(
        "no joint declares a command interface this driver maps, so there is no mode in which this "
        "body is commanded. The descriptor declares no control modes, which is the truthful shape "
        "for it, and `write_command` refuses.");
  }

  // The resource manager's update rate, in Hz. A plugin may read it as its own stream
  // rate — `ur_robot_driver` takes it as its RTDE target frequency, and 1000 is not one a
  // controller offers (DRY-COVERAGE-PLAN.md F93) — so it is configured, with the rate every
  // mock body in this directory was measured at as the default.
  g_ctx.update_rate = 1000;
  if (const auto rate = setting(config, "update_rate")) {
    bool digits = !rate->empty();
    for (char ch : *rate) digits = digits && ch >= '0' && ch <= '9';
    const unsigned long parsed = digits ? std::strtoul(rate->c_str(), nullptr, 10) : 0;
    if (!digits || parsed == 0 || parsed > 100000) {
      say("`" + *rate +
          "` is not an update rate. `update_rate=` takes a positive number of Hz — 500 for "
          "a UR controller over RTDE, 1000 where nothing else was agreed.");
      return SKR_ENODEV;
    }
    g_ctx.update_rate = static_cast<unsigned>(parsed);
  }

  try {
    g_ctx.resources = std::make_unique<hardware_interface::ResourceManager>(
        urdf, g_ctx.clock, rclcpp::get_logger("skynet-ros2-control"), /*activate_all=*/true,
        g_ctx.update_rate);
  } catch (const std::exception& error) {
    say(std::string("the hardware plugin would not load: ") + error.what() +
        ". A plugin named in <hardware><plugin> has to be on AMENT_PREFIX_PATH, and "
        "`mock_components/GenericSystem` is the one ros2_control ships for a body that is not "
        "there.");
    return SKR_ENODEV;
  }
  // `activate_all` asks; it does not promise. A plugin that refuses to configure — the UR
  // hardware interface against a rate its controller does not offer — leaves the manager
  // holding dead components whose reads are NaN and whose writes go nowhere, and `open()`
  // used to report success on exactly that (DRY-COVERAGE-PLAN.md F94). A body that is not
  // up is refused here, naming the component and the state it is in.
  //
  // The components checked are the ones the URDF declares, not the ones the manager reports.
  // A plugin whose `on_init` fails is never reported at all: the manager clears every component
  // it had loaded, its constructor ignores that it did, and a loop over the manager's own status
  // found nothing to refuse — so `open()` succeeded on a body with no plugin behind it
  // (DRY-COVERAGE-PLAN.md F137, KUKA's plugin at `master` refusing a URDF from its last release).
  const auto status = g_ctx.resources->get_components_status();
  for (const hardware_interface::HardwareInfo& declared : infos) {
    const auto found = status.find(declared.name);
    if (!g_ctx.resources->are_components_initialized() || found == status.end()) {
      say("hardware component '" + declared.name + "' (" + declared.hardware_plugin_name +
          ") did not initialize, so this body is not up. The plugin's own log above says why.");
      g_ctx.resources.reset();
      return SKR_ENODEV;
    }
    if (found->second.state.id() != lifecycle_msgs::msg::State::PRIMARY_STATE_ACTIVE) {
      say("hardware component '" + declared.name + "' is '" +
          std::string(found->second.state.label()) +
          "', not active, so this body is not up. The plugin's own log above says why.");
      g_ctx.resources.reset();
      return SKR_ENODEV;
    }
  }

  for (const hardware_interface::HardwareInfo& declared : infos) {
    g_ctx.components.push_back(declared.name);
  }
  g_ctx.descriptor = build_descriptor(g_ctx);
  g_ctx.open = true;
  *out = &g_ctx;
  return SKR_OK;
}

int bridge_close(skr_ctx* ctx) {
  if (ctx == nullptr) return SKR_EINVAL;
  if (ctx->resources) {
    // Deactivate, then shut down. `shutdown_components` alone finalizes an active component
    // straight from ACTIVE, and a plugin's `on_deactivate` never runs — which is where KUKA's
    // sends RSI's stop flag, so the controller of a closed session was never told and found out
    // by its own timeout (DRY-COVERAGE-PLAN.md F136). Every plugin gets the lifecycle's orderly
    // way out; one that refuses to deactivate is still shut down.
    for (const auto& [name, info] : ctx->resources->get_components_status()) {
      if (info.state.id() != lifecycle_msgs::msg::State::PRIMARY_STATE_ACTIVE) continue;
      rclcpp_lifecycle::State inactive(lifecycle_msgs::msg::State::PRIMARY_STATE_INACTIVE,
                                       hardware_interface::lifecycle_state_names::INACTIVE);
      if (ctx->resources->set_component_state(name, inactive) !=
          hardware_interface::return_type::OK) {
        say("hardware component '" + name + "' refused to deactivate; shutting it down anyway.");
      }
    }
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

/// Every joint holds a registered `position` state interface. Registration is settled at
/// activation; whether it has been *written* yet is the question below.
bool positions_available(skr_ctx* ctx) {
  for (const BridgeJoint& joint : ctx->joints) {
    if (!ctx->resources->state_interface_is_available(joint.name + "/position")) return false;
  }
  return true;
}

/// The `period` a hardware plugin's `read` or `write` is handed: the time since the last call of
/// the same kind, which is what `controller_manager` passes, and one configured period on the first.
///
/// It was a constant millisecond whatever the rate. `mock_components` never reads it, so no suite
/// noticed; Doosan's `DRHWInterface` is the first plugin here that does, and it skips — returning
/// OK — every write whose period is outside 0.3 to 1.5 times its own configured one. At the ten
/// hertz Doosan recommends for its emulator, every command this bridge sent reported success and
/// moved nothing (DRY-COVERAGE-PLAN.md F125).
rclcpp::Duration period_since(std::chrono::steady_clock::time_point& last, unsigned update_rate) {
  const auto now = std::chrono::steady_clock::now();
  const double seconds = last == std::chrono::steady_clock::time_point{}
                             ? 1.0 / static_cast<double>(update_rate)
                             : std::chrono::duration<double>(now - last).count();
  last = now;
  return rclcpp::Duration::from_seconds(seconds);
}

/// Every joint reads a finite position right now.
bool positions_finite(skr_ctx* ctx) {
  for (const BridgeJoint& joint : ctx->joints) {
    if (!std::isfinite(read_interface(ctx, joint.name, "position"))) return false;
  }
  return true;
}

/// Every joint reads exactly zero — the value a resource manager holds before a plugin's first
/// sample lands (DRY-COVERAGE-PLAN.md F95), and the one reading that cannot be told from it.
bool positions_all_zero(skr_ctx* ctx) {
  for (const BridgeJoint& joint : ctx->joints) {
    if (read_interface(ctx, joint.name, "position") != 0.0) return false;
  }
  return true;
}

/*
 * The plugins are read and written in turn — read, then write, once each — because that is how
 * `controller_manager` calls them, every cycle, whatever its controllers did, and a plugin may be
 * built on it.
 *
 * KUKA's is (DRY-COVERAGE-PLAN.md F135). Its RSI client is a request/reply socket: the controller
 * sends a telegram every cycle and the client must answer that telegram before it may receive the
 * next. A second receive with one unanswered fails at once, and keeps failing, because only a
 * reply clears it and the plugin sends no reply after a failed receive; a reply with no telegram
 * to answer fails too, and the plugin throws on it. The bridge used to read and write when its
 * caller did — the first read read twice, a refused command wrote nothing, and a command after
 * another wrote twice — so the first session against KUKA's own simulator wedged on its second
 * read. KUKA's `read()` returns OK after a failed receive, so the state the bridge reported from
 * then on was the first telegram's, frozen, under a fresh stamp.
 */

/*
 * A component that fails once is down for the session, and the manager's later answers do not say
 * so.
 *
 * A plugin's `read` or `write` returning ERROR takes its component out of ACTIVE — Jazzy's
 * `HardwareComponent` calls `error()`, whose `on_error` leaves it UNCONFIGURED or FINALIZED — and the
 * manager takes its interfaces off the available list; a `write` returning DEACTIVATE leaves it
 * INACTIVE, where it is no longer written. All of that is right, and nothing brings it back: a
 * plugin that fails is restarted by whoever integrates it (AUBO's own message says as much). What is
 * wrong is what follows. The manager answers every later read and write OK, so the bridge answered
 * `read_state` with SKR_OK and no fault — every position NaN, after an ERROR — and after a
 * DEACTIVATE accepted every command with SKR_OK and sent it nowhere, for the rest of the session
 * (DRY-COVERAGE-PLAN.md F142, AUBO's plugin writing to an arm ARCS had not powered on). The
 * failure is reported once, by the call it happened in; this is what makes it stay reported.
 *
 * Asked only after something has failed, so a healthy session never pays for the question.
 */
std::vector<std::string> components_down(skr_ctx* ctx) {
  std::vector<std::string> down;
  if (!ctx->hardware_failed) return down;
  const auto status = ctx->resources->get_components_status();
  for (const std::string& name : ctx->components) {
    const auto found = status.find(name);
    if (found == status.end() ||
        found->second.state.id() != lifecycle_msgs::msg::State::PRIMARY_STATE_ACTIVE) {
      down.push_back(name);
    }
  }
  return down;
}

/// Note a failed read or write, and say once which components it took down.
void note_failure(skr_ctx* ctx, const hardware_interface::HardwareReadWriteStatus& status) {
  if (status.result == hardware_interface::return_type::OK || ctx->hardware_failed) return;
  ctx->hardware_failed = true;
  for (const std::string& name : components_down(ctx)) {
    say("hardware component '" + name + "' failed a read or write and is no longer active. "
        "ros2_control does not bring it back, so every read from here reports the fault and "
        "every command is refused; close the body and open it again once the plugin's own log "
        "above is dealt with.");
  }
}

/// One turn's read. A read still waiting for its answer is answered first, with what the command
/// interfaces already hold — the last setpoint accepted, which is a hold — and a failure of that
/// answer is the one reported, because it is the one the controller saw.
hardware_interface::HardwareReadWriteStatus read_in_turn(skr_ctx* ctx) {
  std::optional<hardware_interface::HardwareReadWriteStatus> answered;
  if (ctx->read_unanswered) {
    answered =
        ctx->resources->write(ctx->clock->now(), period_since(ctx->last_write, ctx->update_rate));
    ctx->read_unanswered = false;
  }
  auto status =
      ctx->resources->read(ctx->clock->now(), period_since(ctx->last_read, ctx->update_rate));
  ctx->read_unanswered = true;
  if (answered) note_failure(ctx, *answered);
  note_failure(ctx, status);
  if (answered && answered->result != hardware_interface::return_type::OK) return *answered;
  return status;
}

/// One turn's write: the answer to the read before it. With no read waiting, nothing is written
/// now and the values already in the command interfaces go with the next turn's — a second write
/// to one read is a reply to nothing (F135).
std::optional<hardware_interface::HardwareReadWriteStatus> write_in_turn(skr_ctx* ctx) {
  if (!ctx->read_unanswered) return std::nullopt;
  ctx->read_unanswered = false;
  const auto status =
      ctx->resources->write(ctx->clock->now(), period_since(ctx->last_write, ctx->update_rate));
  note_failure(ctx, status);
  return status;
}

int bridge_read_state(skr_ctx* ctx, skr_state* out, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == nullptr || out == nullptr) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;

  // §24: the stamp is taken at acquisition — immediately before the hardware plugins are read, on
  // §35.1's clock. It was a counter that started at one second and added a millisecond a read,
  // which is a clock nobody else in the system can compare against (DRY-COVERAGE-PLAN.md F65).
  // The first read below re-stamps after its wait, for the reason beside it.
  uint64_t acquired = skynet::acquisition_ns();
  auto status = read_in_turn(ctx);

  // The session's first read, and only the first, lets the stream turn over before it
  // reports. A real plugin's state interfaces hold whatever they were initialized with —
  // NaN or zero — until the first packet lands, and `open()` returns before that packet is
  // owed to anyone. So the first read settles: it sleeps three stream periods (the manager
  // rate the plugin was configured with, floored at fifty milliseconds for a slow starter)
  // and reads again, and only then, if positions are still unwritten, waits the way
  // `drivers/ur_rtde` does where its source has a clock of its own (DRY-COVERAGE-PLAN.md
  // F27). Three things the settle is not: it is not waited when a joint has no position
  // interface at all, because nothing will ever arrive; it is not waited when the first read
  // already holds a sample — positions finite and not all exactly zero — because a plugin paced
  // by its controller has telegrams queuing behind a sleep, each one read late from then on
  // (F135); and a body that never streams still reports what the plugin holds afterwards — the
  // settle absorbs startup, and absence stays absence (DRY-COVERAGE-PLAN.md F95). The residual
  // is honest and named: a zero-initializing plugin whose stream never starts still reports
  // zeros, and no call this side of the plugin's API can tell those from a body genuinely at
  // zero.
  if (ctx->first_read_pending) {
    ctx->first_read_pending = false;
    if (positions_available(ctx) && (!positions_finite(ctx) || positions_all_zero(ctx))) {
      const auto settle = std::max(std::chrono::milliseconds(50),
                                   std::chrono::milliseconds(3000 / ctx->update_rate));
      std::this_thread::sleep_for(settle);
      status = read_in_turn(ctx);
      if (!positions_finite(ctx)) {
        const auto deadline =
            std::chrono::steady_clock::now() + std::chrono::seconds(2);
        while (std::chrono::steady_clock::now() < deadline && !positions_finite(ctx)) {
          std::this_thread::sleep_for(std::chrono::milliseconds(5));
          status = read_in_turn(ctx);
        }
      }
      // The sample was acquired during the settle, not before it: a stamp from before a
      // wait would fold the wait into every downstream budget (F65's rule, in the other
      // direction).
      acquired = skynet::acquisition_ns();
    }
  }
  ctx->seq += 1;

  out->stamp_ns = acquired;
  out->seq = ctx->seq;
  out->joint_count = static_cast<uint16_t>(ctx->joints.size());
  out->flags = 0;
  if (ctx->estop) out->flags |= SKR_FLAG_ESTOP;
  // `ros2_control` has no homing concept: a hardware plugin either reports a position or does not.
  // `unreported` is §24's value for a driver that has never heard of the question, and it is the
  // honest one here — `not_required` would be this driver answering for the plugin.
  out->flags |= SKR_HOMING_UNREPORTED;
  out->fault_code = 0;
  // A component a failed read or write took down stays a fault for the session, whatever the
  // manager answers for it afterwards (F142).
  const auto down = components_down(ctx);
  const bool faulted = status.result != hardware_interface::return_type::OK || !down.empty();
  if (faulted) {
    out->flags |= SKR_FLAG_DEGRADED | SKR_FLAG_FAULT;
    out->fault_code = static_cast<uint32_t>(
        std::max(status.failed_hardware_names.size(), down.size()));
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
  return faulted ? SKR_EIO : SKR_OK;
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

  // §86.1. Every joint's mode resolved and checked **before** any interface is claimed: a command
  // half applied is worse than one refused, and this loop is the only place that can tell.
  //
  // The two refusals are different facts and take the two codes the header defines. An entry
  // outside the vocabulary is EINVAL — a malformed request, most likely a bare SKR_MODE_* written
  // into the per-joint array, which is the transcription error that array's offset exists to catch.
  // A well-formed mode a joint does not offer is EPERM: the request is fine and this machine will
  // not do it. Before §86 both were one `!ctx->modes.count(cmd->mode)` against the intersection,
  // which refused the ordinary mixed body along with the genuinely wrong command.
  const bool per_joint = (cmd->flags & SKR_CMD_PER_JOINT_MODES) != 0 && cmd->modes != nullptr;
  std::vector<const char*> interfaces(ctx->joints.size(), nullptr);
  for (size_t index = 0; index < ctx->joints.size(); ++index) {
    uint8_t mode = cmd->mode;
    if (per_joint) {
      const uint8_t entry = cmd->modes[index];
      if (!SKR_JMODE_VALID(entry)) {
        ctx->commands_refused += 1;
        return SKR_EINVAL;
      }
      if (entry != SKR_JMODE_INHERIT) mode = SKR_MODE_OF_JMODE(entry);
    }
    // A masked joint is not commanded, so the mode it was not commanded in is not checked: the
    // mask is how the OS says nothing, and a body-wide hold has to stay expressible on a body whose
    // joints disagree about mode.
    if (cmd->mask != nullptr && cmd->mask[index] == 0) continue;
    const char* interface = nullptr;
    for (const ModeMap& candidate : MODES) {
      if (candidate.mode == mode) interface = candidate.interface;
    }
    if (interface == nullptr || !ctx->joints[index].command_interfaces.count(interface)) {
      ctx->commands_refused += 1;
      return SKR_EPERM;
    }
    interfaces[index] = interface;
  }

  if (ctx->estop) {
    ctx->commands_refused += 1;
    return SKR_EPERM;
  }
  // A component a failed read or write took down drives nothing for the rest of the session, and
  // a command to it is refused here rather than half-claimed below (F142).
  if (!components_down(ctx).empty()) return SKR_EIO;

  // A position setpoint outside the travel this driver's own descriptor declares is refused, not
  // forwarded and not clamped — before any interface is claimed. Forwarding it is how F131 was
  // found: a plugin that throws on an out-of-range command takes its component down for the rest
  // of the session, and a clamp would move the joint somewhere nobody asked for. The kernel's
  // envelope is the first line; this is the second, as in `drivers/lite3` and `drivers/fairino`.
  // The micro-radian margin is float rounding: a setpoint written as exactly the bound arrives a
  // few ulps outside it.
  for (size_t index = 0; index < ctx->joints.size(); ++index) {
    if (interfaces[index] == nullptr || std::strcmp(interfaces[index], "position") != 0) continue;
    const BridgeJoint& joint = ctx->joints[index];
    const double value = static_cast<double>(cmd->values[index]);
    if ((joint.position_min && value < *joint.position_min - 1e-6) ||
        (joint.position_max && value > *joint.position_max + 1e-6)) {
      ctx->commands_refused += 1;
      return SKR_EINVAL;
    }
  }

  // The set this write will drive, as full interface names. A plugin that runs one controller
  // per mode drives only the started set — with nothing started the UR hardware interface
  // answers every write with a keepalive and the arm never moves — and in an integration with
  // a controller manager the manager starts it. There is no manager here, so the bridge does
  // the manager's call itself, exactly when the driven set changes: first write, and any write
  // whose modes differ from the last. Both halves, in the manager's order: `prepare_` first,
  // because a plugin stages the switch there — the UR hardware interface indexes its staged
  // lists unguarded in `perform_`, and calling `perform_` alone segfaults inside the vendor's
  // code.
  //
  // A rejected switch does not refuse the write. The descriptor offers what the URDF declares
  // and the values path below was already validated — interfaces exist, the component is
  // active — so a rejection means the plugin does not do mode switches for this set (the
  // mock answers every effort switch with no and drives effort values fine), and values are
  // the whole protocol. Said aloud, once per set, because a plugin that rejects the switch
  // and then ignores values would read live and never move, and that shape must be visible.
  std::vector<std::string> driven;
  for (size_t index = 0; index < ctx->joints.size(); ++index) {
    const uint8_t applied = cmd->mask == nullptr ? 1u : cmd->mask[index];
    if (applied && interfaces[index] != nullptr) {
      driven.push_back(ctx->joints[index].name + "/" + interfaces[index]);
    }
  }
  if (driven != ctx->switched_interfaces) {
    const bool prepared =
        ctx->resources->prepare_command_mode_switch(driven, ctx->switched_interfaces);
    const bool performed =
        prepared &&
        ctx->resources->perform_command_mode_switch(driven, ctx->switched_interfaces);
    if (!prepared || !performed) {
      say("the hardware did not take the mode switch for this command, so values go without "
          "one. A plugin that runs per-mode controllers answers by driving nothing — if the "
          "body reads live and never moves, this line is why.");
    }
    ctx->switched_interfaces = driven;
  }

  for (size_t index = 0; index < ctx->joints.size(); ++index) {
    const uint8_t applied = cmd->mask == nullptr ? 1u : cmd->mask[index];
    if (!applied) continue;  // hold: the previous setpoint stands
    const float value = cmd->values[index];
    if (!std::isfinite(value)) {
      ctx->commands_refused += 1;
      return SKR_EINVAL;
    }
    const std::string key = ctx->joints[index].name + "/" + interfaces[index];
    try {
      auto loaned = ctx->resources->claim_command_interface(key);
      if (!loaned.set_value(static_cast<double>(value))) {
        return SKR_EIO;
      }
    } catch (const std::exception&) {
      return SKR_EIO;
    }
  }

  // The values are in the command interfaces. They reach the hardware with this turn's write, or,
  // when no read is waiting for one, with the next turn's (F135).
  const auto status = write_in_turn(ctx);
  if (!status) return SKR_OK;
  return (status->result == hardware_interface::return_type::OK) ? SKR_OK : SKR_EIO;
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
 * **That is a software stop and the descriptor declares it as one** — the `safety.estop`
 * capability. What it is not is a rated stop: a rated stop cuts power through a circuit, the
 * hardware plugin behind this may or may not have one, `ros2_control` has no way to say, and
 * the envelope therefore carries no rated stop.
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
