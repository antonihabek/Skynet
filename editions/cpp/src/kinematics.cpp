#include "skynet_os/kinematics.hpp"

#include "skynet_os/embodiment.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>

namespace skynet::kinematics {
namespace {

constexpr double kNaN = std::numeric_limits<double>::quiet_NaN();

/// A scalar joint — one the pose solver can drive from one number.
bool is_scalar_joint(const std::string& kind) {
  return kind == "revolute" || kind == "continuous" || kind == "prismatic";
}

bool is_joint_kind(const std::string& kind) {
  return is_scalar_joint(kind) || kind == "ball" || kind == "floating";
}

// ── Range reduction ────────────────────────────────────────────────────────

/// 2/pi, to the nearest double.
constexpr double kTwoOverPi = 6.36619772367581382433e-1;
/// pi/2 with its low 33 bits cleared, so `k * kPio2_1` is exact for every admitted `k`.
constexpr double kPio2_1 = 1.57079632673412561417e0;
/// pi/2 - kPio2_1, split so the second step recovers the bits the first dropped.
constexpr double kPio2_2 = 6.0771005063039659766e-11;
/// The remainder of that split.
constexpr double kPio2_2t = 2.02226624879595063154e-21;

/// kPio2_1 carries 33 significant bits, so `k * kPio2_1` is exact while `k` fits in 20 — about
/// 1.65e6 radians, a quarter of a million revolutions. Past that the caller is told rather than
/// handed a pose computed from a reduction that has lost its low bits.
constexpr double kMaxQuadrants = 1048576.0;

/// Round half away from zero. ECMAScript's `Math.round` sends -0.5 to -0; this does not.
double round_half_away(double v) {
  const double shifted = v >= 0 ? v + 0.5 : v - 0.5;
  return std::trunc(shifted);
}

struct Reduced {
  double remainder = 0.0;
  int quadrant = 0;
  bool exact = false;
};

Reduced reduce(double x) {
  const double k = round_half_away(x * kTwoOverPi);
  if (std::isnan(k) || std::fabs(k) >= kMaxQuadrants) {
    return {};
  }
  const double t = x - k * kPio2_1;
  const double w0 = k * kPio2_2;
  const double r0 = t - w0;
  const double w1 = k * kPio2_2t - (t - r0 - w0);

  Reduced out;
  out.remainder = r0 - w1;
  out.quadrant = static_cast<int>(std::fmod(std::fmod(k, 4.0) + 4.0, 4.0));
  out.exact = true;
  return out;
}

// fdlibm's minimax coefficients, with fdlibm's own digits, so a reviewer can diff them against the
// published table without converting anything. The Rust, TypeScript and Python editions write the
// same characters.
constexpr double kS1 = -1.66666666666666324348e-1;
constexpr double kS2 = 8.33333333332248946124e-3;
constexpr double kS3 = -1.98412698298579493134e-4;
constexpr double kS4 = 2.75573137070700676789e-6;
constexpr double kS5 = -2.50507602534068634195e-8;
constexpr double kS6 = 1.58969099521155010221e-10;

constexpr double kC1 = 4.16666666666666019037e-2;
constexpr double kC2 = -1.38888888888741095749e-3;
constexpr double kC3 = 2.48015872894767294178e-5;
constexpr double kC4 = -2.75573143513906633035e-7;
constexpr double kC5 = 2.0875723212981748279e-9;
constexpr double kC6 = -1.13596475577881948265e-11;

double kernel_sin(double x) {
  const double z = x * x;
  const double poly = kS1 + z * (kS2 + z * (kS3 + z * (kS4 + z * (kS5 + z * kS6))));
  return x + x * z * poly;
}

double kernel_cos(double x) {
  const double z = x * x;
  const double poly = kC1 + z * (kC2 + z * (kC3 + z * (kC4 + z * (kC5 + z * kC6))));
  return 1 - 0.5 * z + z * z * poly;
}

Vec3 sub(const Vec3& a, const Vec3& b) { return {a[0] - b[0], a[1] - b[1], a[2] - b[2]}; }
double dot(const Vec3& a, const Vec3& b) { return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]; }

double clamp01(double v) {
  if (v > 1) {
    return 1.0;
  }
  if (v > 0) {
    return v;
  }
  return 0.0;
}

bool finite_transform(const Transform& t) {
  return std::isfinite(t.translation[0]) && std::isfinite(t.translation[1]) &&
         std::isfinite(t.translation[2]) && std::isfinite(t.rotation[0]) &&
         std::isfinite(t.rotation[1]) && std::isfinite(t.rotation[2]) &&
         std::isfinite(t.rotation[3]);
}

/// The transform a joint value produces, in the joint's own frame.
///
/// Empty where a scalar cannot describe the motion, or where the axis is degenerate. A zero-length
/// axis is a body-description defect and **not** a fixed joint: reading it as one would silently
/// weld a limb in the guard's model of the robot while it moved in the world.
std::optional<Vec3> unit(const Vec3& v) {
  const double squared = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
  if (!std::isfinite(squared) || squared <= 0) {
    return std::nullopt;
  }
  const double length = sqrt_(squared);
  if (!std::isfinite(length) || length <= 0) {
    return std::nullopt;
  }
  return Vec3{v[0] / length, v[1] / length, v[2] / length};
}

std::optional<Transform> motion(const Joint& joint, double value) {
  if (!std::isfinite(value) || !is_scalar_joint(joint.kind)) {
    return std::nullopt;
  }
  const std::optional<Vec3> axis = unit(joint.axis);
  if (!axis.has_value()) {
    return std::nullopt;
  }
  Transform out;
  if (joint.kind == "prismatic") {
    out.translation = {(*axis)[0] * value, (*axis)[1] * value, (*axis)[2] * value};
    out.rotation = {0.0, 0.0, 0.0, 1.0};
    return out;
  }
  const double half = 0.5 * value;
  const double s = sin_(half);
  const double c = cos_(half);
  if (!std::isfinite(s) || !std::isfinite(c)) {
    return std::nullopt;
  }
  out.translation = {0.0, 0.0, 0.0};
  out.rotation = {(*axis)[0] * s, (*axis)[1] * s, (*axis)[2] * s, c};
  return out;
}

/// A number out of a JSON value, NaN for anything that is not one. Booleans coerce the way the
/// reference's `Number()` does for the one place that reaches it — a mimic coefficient times a
/// measured value — and produce a non-finite pose everywhere else, which reads as unresolved.
double as_number(const Json* value, double fallback) {
  if (value == nullptr) {
    return fallback;
  }
  if (value->is_number()) {
    return value->number_or(fallback);
  }
  if (value->is_bool()) {
    return value->is_true() ? 1.0 : 0.0;
  }
  return kNaN;
}

}  // namespace

Vec3 vec3(const Json& value) {
  Vec3 out{kNaN, kNaN, kNaN};
  const JsonArray& items = value.items();
  for (std::size_t at = 0; at < 3; ++at) {
    if (at < items.size() && items[at].is_number()) {
      out[at] = items[at].number_or(kNaN);
    }
  }
  return out;
}

double sqrt_(double x) {
  // Newton's iteration from a bit-pattern seed, a fixed five times. `std::sqrt` is correctly
  // rounded and would agree — but `core` has no square root at all, the Rust edition therefore
  // computes one, and an oracle that took a shortcut the edition cannot take would be checking the
  // wrong function.
  if (std::isnan(x) || x < 0) {
    return kNaN;
  }
  if (x == 0 || std::isinf(x)) {
    return x;
  }

  std::uint64_t bits = 0;
  static_assert(sizeof(bits) == sizeof(x), "a double is not 64 bits on this platform");
  std::memcpy(&bits, &x, sizeof(bits));
  const std::uint64_t seed_bits = (bits >> 1) + (static_cast<std::uint64_t>(1023) << 51);
  double y = 0.0;
  std::memcpy(&y, &seed_bits, sizeof(y));
  if (y == 0 || !std::isfinite(y)) {
    y = x;
  }
  for (int step = 0; step < 5; ++step) {
    y = 0.5 * (y + x / y);
  }
  return y;
}

double sin_(double x) {
  if (!std::isfinite(x)) {
    return kNaN;
  }
  const Reduced reduced = reduce(x);
  if (!reduced.exact) {
    return kNaN;
  }
  switch (reduced.quadrant) {
    case 0: return kernel_sin(reduced.remainder);
    case 1: return kernel_cos(reduced.remainder);
    case 2: return -kernel_sin(reduced.remainder);
    default: return -kernel_cos(reduced.remainder);
  }
}

double cos_(double x) {
  if (!std::isfinite(x)) {
    return kNaN;
  }
  const Reduced reduced = reduce(x);
  if (!reduced.exact) {
    return kNaN;
  }
  switch (reduced.quadrant) {
    case 0: return kernel_cos(reduced.remainder);
    case 1: return -kernel_sin(reduced.remainder);
    case 2: return -kernel_cos(reduced.remainder);
    default: return kernel_sin(reduced.remainder);
  }
}

Quat quaternion_product(const Quat& a, const Quat& b) {
  // Hamilton product, xyzw in and xyzw out.
  return {
      a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
      a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
      a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
      a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
  };
}

Vec3 rotate(const Quat& q, const Vec3& v) {
  // Active rotation, per §44.4.
  const double tx = 2 * (q[1] * v[2] - q[2] * v[1]);
  const double ty = 2 * (q[2] * v[0] - q[0] * v[2]);
  const double tz = 2 * (q[0] * v[1] - q[1] * v[0]);
  return {
      v[0] + q[3] * tx + (q[1] * tz - q[2] * ty),
      v[1] + q[3] * ty + (q[2] * tx - q[0] * tz),
      v[2] + q[3] * tz + (q[0] * ty - q[1] * tx),
  };
}

Transform compose(const Transform& parent, const Transform& local) {
  const Vec3 rotated = rotate(parent.rotation, local.translation);
  Transform out;
  out.translation = {parent.translation[0] + rotated[0], parent.translation[1] + rotated[1],
                     parent.translation[2] + rotated[2]};
  out.rotation = quaternion_product(parent.rotation, local.rotation);
  return out;
}

bool statically_resolvable(const std::vector<Frame>& frames, std::size_t index) {
  std::size_t here = index;
  for (;;) {
    if (here >= frames.size()) {
      return false;
    }
    const Frame& frame = frames[here];
    if (frame.joint.has_value() && !is_scalar_joint(frame.joint->kind)) {
      return false;
    }
    if (!frame.parent.has_value()) {
      return true;
    }
    here = *frame.parent;
  }
}

std::size_t collision_bodies(const std::vector<Frame>& frames) {
  std::size_t count = 0;
  for (std::size_t at = 0; at < frames.size(); ++at) {
    if (frames[at].collision.has_value() && statically_resolvable(frames, at)) {
      ++count;
    }
  }
  return count;
}

std::size_t collision_pairs(const std::vector<Frame>& frames) {
  const std::size_t n = collision_bodies(frames);
  return (n * (n - 1)) / 2;
}

bool adjacent(const std::vector<Frame>& frames, std::size_t a, std::size_t b) {
  // Whether one frame is the other's parent. Excluded from the collision test unconditionally.
  return (frames[a].parent.has_value() && *frames[a].parent == b) ||
         (frames[b].parent.has_value() && *frames[b].parent == a);
}

bool ignores(const std::vector<Frame>& frames, std::size_t a, std::size_t b) {
  // Whether `a` lists `b`, or `b` lists `a`. Symmetric on purpose.
  const auto& first = frames[a].ignore;
  const auto& second = frames[b].ignore;
  return std::find(first.begin(), first.end(), b) != first.end() ||
         std::find(second.begin(), second.end(), a) != second.end();
}

Solved solve(const std::vector<Frame>& frames, const std::vector<Binding>& bindings,
             const Json& command) {
  Solved out;
  out.poses.reserve(frames.size());

  // A frame is **unresolved** when its parent is, when its joint has no value, when its joint kind
  // needs more than a scalar, or when the arithmetic produced something that is not finite — and
  // unresolved propagates down the chain, because a pose relative to an unknown pose is unknown.
  const auto value_of = [&](const std::string& joint_name) -> const Json* {
    const auto found = std::find_if(bindings.begin(), bindings.end(),
                                    [&](const Binding& one) { return one.joint == joint_name; });
    if (found == bindings.end()) {
      return nullptr;
    }
    if (!found->channel.has_value()) {
      out.lookahead = false;
      return found->measured;
    }
    if (!command.has(*found->channel)) {
      out.lookahead = false;
      return found->measured;
    }
    return &command[*found->channel];
  };

  for (std::size_t index = 0; index < frames.size(); ++index) {
    const Frame& frame = frames[index];
    Transform parent_pose;
    if (frame.parent.has_value()) {
      const std::optional<Transform>& resolved = out.poses[*frame.parent];
      if (!resolved.has_value()) {
        out.poses.emplace_back(std::nullopt);
        out.unresolved.push_back(index);
        continue;
      }
      parent_pose = *resolved;
    }

    Transform local = frame.local;
    if (frame.joint.has_value()) {
      const Joint& joint = *frame.joint;
      // A mimic is followed exactly one step. URDF permits no more, and a chain of them would need
      // a traversal with its own cycle check inside the control loop.
      const Json* raw =
          joint.mimic.has_value() ? value_of(joint.mimic->joint) : value_of(joint.name);
      if (raw == nullptr || raw->is_null()) {
        out.poses.emplace_back(std::nullopt);
        out.unresolved.push_back(index);
        continue;
      }
      const double value = joint.mimic.has_value()
                               ? joint.mimic->multiplier * as_number(raw, kNaN) + joint.mimic->offset
                               : (raw->is_number() ? raw->number_or(kNaN) : kNaN);
      const std::optional<Transform> moved = motion(joint, value);
      if (!moved.has_value()) {
        out.poses.emplace_back(std::nullopt);
        out.unresolved.push_back(index);
        continue;
      }
      local = compose(local, *moved);
    }

    const Transform pose = compose(parent_pose, local);
    if (!finite_transform(pose)) {
      out.poses.emplace_back(std::nullopt);
      out.unresolved.push_back(index);
      continue;
    }
    out.poses.emplace_back(pose);
  }

  return out;
}

std::vector<Frame> flatten(const Json& graph) {
  // Flatten a §44.3 frame graph into the order the kernel reads.
  //
  // Breadth-first from the root, so a parent is always emitted before its children and the
  // ordering invariant holds by construction rather than by a sort that has to be checked
  // afterwards. The output order is a function of the tree and of the descriptor's own node order
  // among siblings, which makes it stable across editions — a requirement, because verdicts name
  // frames and the corpus compares those names in order.
  if (const std::optional<embodiment::Fault> invalid = embodiment::validate_frames(graph);
      invalid.has_value()) {
    throw FlattenFault{invalid->code};
  }

  const JsonArray& nodes = graph["nodes"].items();
  if (nodes.size() > kMaxFrames) {
    // Half a frame graph is a robot with a limb missing from its own collision model, so the graph
    // is refused rather than truncated.
    throw FlattenFault{"too_large"};
  }

  const std::string root_name(graph["root"].string_or(""));
  const auto name_of = [&](const Json& node) { return std::string(node["name"].string_or("")); };
  const auto index_of = [&](const std::string& name) -> std::optional<std::size_t> {
    for (std::size_t at = 0; at < nodes.size(); ++at) {
      if (name_of(nodes[at]) == name) {
        return at;
      }
    }
    return std::nullopt;
  };

  const std::optional<std::size_t> root_index = index_of(root_name);
  if (!root_index.has_value()) {
    throw FlattenFault{"root_missing"};
  }

  std::vector<std::size_t> order{*root_index};
  for (std::size_t cursor = 0; cursor < order.size(); ++cursor) {
    const std::string parent_name = name_of(nodes[order[cursor]]);
    for (std::size_t at = 0; at < nodes.size(); ++at) {
      const Json& parent_field = nodes[at]["parent"];
      if (parent_field.is_string() && parent_field.string_or("") == parent_name) {
        order.push_back(at);
      }
    }
  }
  if (order.size() != nodes.size()) {
    // Its parent chain is valid and leads somewhere else, which is a second tree in one document.
    throw FlattenFault{"unreachable"};
  }

  std::vector<std::size_t> position(nodes.size(), 0);
  for (std::size_t slot = 0; slot < order.size(); ++slot) {
    position[order[slot]] = slot;
  }

  std::vector<Frame> frames;
  frames.reserve(order.size());
  for (const std::size_t index : order) {
    const Json& node = nodes[index];
    Frame frame;
    frame.name = name_of(node);

    if (node["parent"].is_string()) {
      const std::optional<std::size_t> parent = index_of(std::string(node["parent"].string_or("")));
      if (!parent.has_value()) {
        throw FlattenFault{"parent_missing"};
      }
      frame.parent = position[*parent];
    }

    frame.local.translation =
        node.has("translation") && !node["translation"].is_null() ? vec3(node["translation"])
                                                                  : Vec3{0.0, 0.0, 0.0};
    // Absent means no rotation, and the identity quaternion is `xyzw = 0 0 0 1` — not `0 0 0 0`,
    // which is not a rotation at all and would collapse every pose below it.
    if (node.has("rotation") && !node["rotation"].is_null()) {
      const JsonArray& items = node["rotation"].items();
      for (std::size_t at = 0; at < 4; ++at) {
        frame.local.rotation[at] =
            at < items.size() && items[at].is_number() ? items[at].number_or(kNaN) : kNaN;
      }
    } else {
      frame.local.rotation = {0.0, 0.0, 0.0, 1.0};
    }

    if (node.has("joint") && !node["joint"].is_null()) {
      const Json& joint_type = node["joint_type"];
      if (joint_type.is_null()) {
        // Nothing can tell whether it turns or slides.
        throw FlattenFault{"unknown_joint_type"};
      }
      const std::string kind(joint_type.string_or(""));
      if (!is_joint_kind(kind)) {
        // Reading it as revolute is how a prismatic axis becomes a rotation.
        throw FlattenFault{"unknown_joint_type"};
      }
      Joint joint;
      joint.name = std::string(node["joint"].string_or(""));
      joint.kind = kind;
      // URDF's default, stated: the axis is `1 0 0` when the file is silent.
      joint.axis = node.has("axis") && !node["axis"].is_null() ? vec3(node["axis"])
                                                              : Vec3{1.0, 0.0, 0.0};
      if (node.has("mimics") && !node["mimics"].is_null()) {
        Mimic mimic;
        mimic.joint = std::string(node["mimics"].string_or(""));
        mimic.multiplier = node.has("mimic_multiplier") && !node["mimic_multiplier"].is_null()
                               ? as_number(&node["mimic_multiplier"], kNaN)
                               : 1.0;
        mimic.offset = node.has("mimic_offset") && !node["mimic_offset"].is_null()
                           ? as_number(&node["mimic_offset"], kNaN)
                           : 0.0;
        joint.mimic = mimic;
      }
      frame.joint = joint;
    }

    for (const Json& ignored : node["collision_ignore"].items()) {
      if (const std::optional<std::size_t> found = index_of(std::string(ignored.string_or("")));
          found.has_value()) {
        frame.ignore.push_back(position[*found]);
      }
    }
    std::sort(frame.ignore.begin(), frame.ignore.end());

    if (node.has("collision_shape") && !node["collision_shape"].is_null()) {
      const Json& shape = node["collision_shape"];
      Capsule capsule;
      capsule.a = vec3(shape["a"]);
      capsule.b = vec3(shape["b"]);
      capsule.radius_m = as_number(&shape["radius_m"], kNaN);
      frame.collision = capsule;
    }

    frames.push_back(std::move(frame));
  }

  return frames;
}

double margin(const Json& shape, const Vec3& point) {
  // A layer of the operator's workspace configuration (§38.2) is a set of these, and layers compose
  // by conjunction — a frame must be inside at least one volume of every layer that names it. That
  // is "each layer may only narrow" as a predicate rather than as geometry: intersecting a box with
  // a sphere is a shape neither of them is, and a composition rule producing a third shape would be
  // one no operator could check by reading their own configuration.
  if (shape["kind"].string_or("") == "sphere") {
    const Vec3 centre = vec3(shape["centre"]);
    const double dx = point[0] - centre[0];
    const double dy = point[1] - centre[1];
    const double dz = point[2] - centre[2];
    return as_number(&shape["radius_m"], kNaN) - sqrt_(dx * dx + dy * dy + dz * dz);
  }

  const Vec3 low = vec3(shape["min"]);
  const Vec3 high = vec3(shape["max"]);
  Vec3 outside{0.0, 0.0, 0.0};
  bool any_outside = false;
  double inside_margin = std::numeric_limits<double>::infinity();
  for (std::size_t axis = 0; axis < 3; ++axis) {
    const double below = low[axis] - point[axis];
    const double above = point[axis] - high[axis];
    if (below > 0) {
      outside[axis] = below;
      any_outside = true;
    } else if (above > 0) {
      outside[axis] = above;
      any_outside = true;
    }
    const double to_low = point[axis] - low[axis];
    const double to_high = high[axis] - point[axis];
    const double nearest = to_low < to_high ? to_low : to_high;
    if (nearest < inside_margin) {
      inside_margin = nearest;
    }
  }
  if (any_outside) {
    return -sqrt_(outside[0] * outside[0] + outside[1] * outside[1] + outside[2] * outside[2]);
  }
  return inside_margin;
}

double segment_distance_squared(const Vec3& p1, const Vec3& q1, const Vec3& p2, const Vec3& q2) {
  const Vec3 d1 = sub(q1, p1);
  const Vec3 d2 = sub(q2, p2);
  const Vec3 r = sub(p1, p2);
  const double a = dot(d1, d1);
  const double e = dot(d2, d2);
  const double f = dot(d2, r);

  constexpr double kEpsilon = 1e-18;
  double s = 0.0;
  double t = 0.0;
  if (a <= kEpsilon && e <= kEpsilon) {
    return dot(r, r);
  }
  if (a <= kEpsilon) {
    s = 0.0;
    t = clamp01(f / e);
  } else {
    const double c = dot(d1, r);
    if (e <= kEpsilon) {
      t = 0.0;
      s = clamp01(-c / a);
    } else {
      const double b = dot(d1, d2);
      const double denominator = a * e - b * b;
      s = denominator > kEpsilon ? clamp01((b * f - c * e) / denominator) : 0.0;
      t = (b * s + f) / e;
      if (t < 0) {
        t = 0.0;
        s = clamp01(-c / a);
      } else if (t > 1) {
        t = 1.0;
        s = clamp01((b - c) / a);
      }
    }
  }

  const Vec3 c1{p1[0] + d1[0] * s, p1[1] + d1[1] * s, p1[2] + d1[2] * s};
  const Vec3 c2{p2[0] + d2[0] * t, p2[1] + d2[1] * t, p2[2] + d2[2] * t};
  const Vec3 difference = sub(c1, c2);
  return dot(difference, difference);
}

std::pair<Vec3, Vec3> capsule_in_world(const Transform& pose, const Capsule& capsule) {
  const Vec3 a = rotate(pose.rotation, capsule.a);
  const Vec3 b = rotate(pose.rotation, capsule.b);
  return {
      Vec3{pose.translation[0] + a[0], pose.translation[1] + a[1], pose.translation[2] + a[2]},
      Vec3{pose.translation[0] + b[0], pose.translation[1] + b[1], pose.translation[2] + b[2]},
  };
}

double distance(double squared) { return sqrt_(std::fabs(squared)); }

}  // namespace skynet::kinematics
