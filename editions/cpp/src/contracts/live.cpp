// §54.2's live precondition context, assembled — C++ edition.
//
// Transcribed from `reference/typescript/live.ts`. Every rule and message string is read from that
// file, never from `corpus/`.
//
// §54.2 turns `SkillManifest.preconditions` from prose into predicates and then names the seven
// branches those predicates may read:
//
//     robot.state.*        joint positions, gripper, battery percent, base pose
//     robot.guards.active  which guards are actually running (§7.2)
//     robot.calibration.*  which parameter sets are valid, and their age (§45)
//     robot.frames.*       whether the frames the skill needs resolve (§44)
//     robot.tier           guest | host | certified
//     skill.grants.*       capability tokens the operator granted this package
//     env.*                operator-declared facts about the cell
//
// ── The rule that decides every field: absent is not null ──────────────────
//
// §54.2's first rule is the one with teeth:
//
//     **A precondition that cannot be evaluated refuses the skill**, naming the path that was
//     unresolvable. Not "assume true", which is what free-text preconditions do by construction.
//
// So a fact this robot does not know must be **absent** from the context, never present as `null`
// and never present as a plausible zero. A robot with no battery sensor reporting
// `robot.state.battery_percent = 0` would refuse every skill; one reporting `100` would run every
// skill; one that omits the path refuses only the skills that asked, and names the path in the
// refusal. Only the third is honest, and only the third is useful.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `if (anyJoint) state.joints = joints`
//     A flag rather than a size test, and they are the same here only because a joint's name is its
//     key. The flag is what the reference wrote and it is the one that stays correct if two joints
//     ever share a name.
//
// `if (pose)`
//     Truthy on an object, so a `base_pose` of `null` publishes nothing and raises nothing —
//     distinct from a pose whose numbers are not finite, which lands in `unreadable`. An empty
//     object must reach the finite check and be reported unreadable.
//
// `if (inputs.guards_active)`
//     Truthy again, and this one carries the whole contract in a field. An empty array is a kernel
//     reporting **no guard is running**; absent is nobody having asked a kernel. JavaScript's `[]`
//     is truthy, so a host whose empty container is falsy would silently drop the first case — the
//     one §33.1 exists to make observable.
//
// `typeof inputs.tier === 'string'`
//     A type test, not a truthiness test: an empty-string tier is published, because a tier of `""`
//     is a value somebody supplied and §54.2's rule is about facts nobody knows.
//
// `new Set(values)` then `.sort()`
//     Insertion order then UTF-16 order, which is neither byte order nor code-point order.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/text.hpp"

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// A key a dot path can address: non-empty, no dot, and no surrounding whitespace.
///
/// The same rule as the cell's usable-key test, **deliberately duplicated rather than shared**,
/// because they are two rules that happen to agree: this one is about a name the *body* chose and
/// that one is about a name the *operator* chose, and the two could reasonably diverge about
/// whitespace without either being wrong.
bool addressable(const Json& key) {
  if (!key.is_string()) {
    return false;
  }
  const std::string value(key.string_or(""));
  return !value.empty() && value.find('.') == std::string::npos && js::trim(value) == value;
}

/// `typeof value === 'number' && Number.isFinite(value)`. A gripper of `true` is not a gripper at 1.
bool finite(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

/// `[...new Set(values)].sort()` — deduplicated in insertion order, then in UTF-16 order.
Json string_list(const std::vector<std::string>& values) {
  std::vector<std::string> unique;
  for (const std::string& value : values) {
    if (std::find(unique.begin(), unique.end(), value) == unique.end()) {
      unique.push_back(value);
    }
  }
  std::sort(unique.begin(), unique.end(),
            [](const std::string& left, const std::string& right) {
              return js::utf16_less(left, right);
            });
  Json out = Json::array();
  for (const std::string& value : unique) {
    out.push(Json::string(value));
  }
  return out;
}

std::vector<std::string> strings_of(const Json& value) {
  std::vector<std::string> out;
  for (const Json& item : value.items()) {
    out.emplace_back(item.string_or(""));
  }
  return out;
}

}  // namespace

Json live(const std::string& /*label*/, const Json& input) {
  std::vector<std::string> unaddressable;
  std::vector<std::string> unreadable;

  // ── robot.state ──────────────────────────────────────────────────────────
  Json joints = Json::object();
  bool any_joint = false;
  for (const Json& reading : input["joints"].items()) {
    const Json& name = reading["name"];
    if (!addressable(name)) {
      unaddressable.emplace_back(js::interpolate(name.is_null() ? nullptr : &name));
      continue;
    }
    if (!finite(reading["position"])) {
      unreadable.emplace_back(name.string_or(""));
      continue;
    }
    joints.set(std::string(name.string_or("")), reading["position"]);
    any_joint = true;
  }

  Json state = Json::object();
  if (any_joint) {
    state.set("joints", joints);
  }
  if (finite(input["gripper"])) {
    state.set("gripper", input["gripper"]);
  }
  if (finite(input["battery_percent"])) {
    state.set("battery_percent", input["battery_percent"]);
  }
  if (input.has("base_pose") && !input["base_pose"].is_null()) {
    const Json& pose = input["base_pose"];
    const JsonArray& xyz = pose["xyz"].items();
    const JsonArray& quaternion = pose["quaternion"].items();
    const bool readable =
        xyz.size() == 3 && quaternion.size() == 4 &&
        std::all_of(xyz.begin(), xyz.end(), [](const Json& one) { return finite(one); }) &&
        std::all_of(quaternion.begin(), quaternion.end(),
                    [](const Json& one) { return finite(one); });
    if (readable) {
      Json base = Json::object();
      base.set("x", xyz[0]);
      base.set("y", xyz[1]);
      base.set("z", xyz[2]);
      base.set("qx", quaternion[0]);
      base.set("qy", quaternion[1]);
      base.set("qz", quaternion[2]);
      base.set("qw", quaternion[3]);
      state.set("base_pose", base);
    } else {
      unreadable.emplace_back("base_pose");
    }
  }
  // §54.2's table says `robot.state.*` and then lists four examples; §27's state is inside that
  // branch and is the fact a precondition most obviously wants — *"only when the robot is
  // running"* is the first precondition anybody writes. It is published under its own name rather
  // than inside a status object so the path stays stable if §27 ever grows a second field.
  if (input["operating_state"].is_string() && !input["operating_state"].string_or("").empty()) {
    state.set("operating", input["operating_state"]);
  }

  // ── robot.guards ─────────────────────────────────────────────────────────
  Json guards = Json::object();
  // `[]` is a kernel reporting that no guard is running, and absent is nobody having asked a
  // kernel — §33.1 keeps `skynetd` on the far side of that line.
  if (input.has("guards_active") && !input["guards_active"].is_null()) {
    guards.set("active", string_list(strings_of(input["guards_active"])));
  }
  if (!input["guards_unavailable"].items().empty()) {
    guards.set("unavailable", string_list(strings_of(input["guards_unavailable"])));
  }

  // ── robot.calibration ────────────────────────────────────────────────────
  Json calibration = Json::object();
  bool any_calibration = false;
  for (const Json& fact : input["calibration"].items()) {
    const Json& parameter_set = fact["set"];
    if (!addressable(parameter_set)) {
      unaddressable.emplace_back(
          js::interpolate(parameter_set.is_null() ? nullptr : &parameter_set));
      continue;
    }
    const Json& validity = fact["validity"];
    Json row = Json::object();
    row.set("valid", Json::boolean(validity["state"].is_string() &&
                                   validity["state"].string_or("") == "valid"));
    row.set("state", validity["state"]);
    if (finite(fact["age_s"])) {
      row.set("age_s", fact["age_s"]);
    }
    calibration.set(std::string(parameter_set.string_or("")), row);
    any_calibration = true;
  }

  // ── robot ────────────────────────────────────────────────────────────────
  Json robot = Json::object();
  if (!state.entries().empty()) {
    robot.set("state", state);
  }
  if (!guards.entries().empty()) {
    robot.set("guards", guards);
  }
  if (any_calibration) {
    robot.set("calibration", calibration);
  }
  Json frames = Json::object();
  if (!input["frames_resolved"].items().empty()) {
    frames.set("resolved", string_list(strings_of(input["frames_resolved"])));
  }
  if (input.has("frames_fault") && !input["frames_fault"].is_null()) {
    const Json& fault = input["frames_fault"];
    Json row = Json::object();
    row.set("code", fault["code"]);
    row.set("detail", fault["detail"]);
    row.set("subjects", string_list(strings_of(fault["subjects"])));
    frames.set("fault", row);
  }
  if (!frames.entries().empty()) {
    robot.set("frames", frames);
  }
  if (input["tier"].is_string()) {
    robot.set("tier", input["tier"]);
  }

  // ── skill.grants ─────────────────────────────────────────────────────────
  //
  // A list rather than an object, and the reason is a dot. The cell contract found, from the other
  // side, that a fact key containing one is a key no precondition can address — and a **capability
  // token contains a dot by construction** (`arm.left`, `sensing.camera`), so `skill.grants.<token>`
  // is unaddressable for every token that exists and nobody can rename them. The shape changes
  // rather than the names.
  //
  // It is the one branch emitted empty rather than omitted: a grant is a **file the daemon owns**,
  // so "this package was granted nothing" is an answer the OS always has. Every other branch is
  // something the OS learns from somewhere else — a driver, a kernel, a clock, an operator's survey
  // of a room — and any of them can be missing, at which point §54.2's first rule is the honest
  // answer and an empty array is a claim.
  Json grants = Json::object();
  grants.set("tokens", string_list(strings_of(input["grants"])));
  Json skill = Json::object();
  skill.set("grants", grants);

  Json context = Json::object();
  context.set("robot", robot);
  context.set("skill", skill);
  // `if (env)` — an object is truthy, including an empty one, so the only thing this omits is a
  // null or absent `env`.
  if (input.has("env") && !input["env"].is_null()) {
    context.set("env", input["env"]);
  }

  // The two omission lists sit **beside** the context, because a path under `robot.` describing the
  // context itself would be a path a precondition could read — and a skill whose precondition
  // asserts something about the completeness of its own evaluation context is a skill asserting
  // something about the OS rather than about the robot.
  Json out = Json::object();
  out.set("context", context);
  out.set("unaddressable", string_list(unaddressable));
  out.set("unreadable", string_list(unreadable));
  return out;
}

}  // namespace skynet::contracts
