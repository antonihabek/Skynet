// Forward kinematics and the arithmetic underneath it (§44).
//
// # Why the trigonometry is written out
//
// `std::sin` is not required to be correctly rounded, and neither is ECMAScript's, and neither is
// a C library's, and a Cortex-M without double-precision hardware runs a fourth routine again.
// §31.4 requires two editions to emit **byte-identical** canonical JSON, so a pose computed
// through `std::sin` here and through fdlibm there would diverge on the first revolute joint — and
// the divergence would look like a specification disagreement rather than what it is.
//
// So the specification defines its own, and §31.2 puts `sin`, `cos` and `sqrt` in the
// single-source column for exactly this reason. Addition, subtraction, multiplication and division
// *are* correctly rounded on every conforming platform, so the same sequence of operations
// produces the same bits here as in the reference. What follows is that sequence, matching it
// operation for operation.
//
// Where a line looks redundant — an unconditional second Cody–Waite step, a fixed five Newton
// iterations — it is because a branch or an early exit would make the number of operations depend
// on the input, and an edition could then take a different number of them without noticing.

#ifndef SKYNET_OS_KINEMATICS_HPP
#define SKYNET_OS_KINEMATICS_HPP

#include "skynet_os/json.hpp"

#include <array>
#include <optional>
#include <string>
#include <vector>

namespace skynet::kinematics {

using Vec3 = std::array<double, 3>;
/// A quaternion ordered `xyzw` (§44.4).
using Quat = std::array<double, 4>;

/// A rigid transform: metres, and a quaternion ordered `xyzw`.
struct Transform {
  Vec3 translation{0.0, 0.0, 0.0};
  Quat rotation{0.0, 0.0, 0.0, 1.0};
};

struct Mimic {
  std::string joint;
  double multiplier = 1.0;
  double offset = 0.0;
};

struct Joint {
  std::string name;
  std::string kind;
  Vec3 axis{1.0, 0.0, 0.0};
  std::optional<Mimic> mimic;
};

struct Capsule {
  Vec3 a{0.0, 0.0, 0.0};
  Vec3 b{0.0, 0.0, 0.0};
  double radius_m = 0.0;
};

/// One frame of the flattened set. The ordering invariant — frame `i`'s parent index is `< i` — is
/// what makes forward kinematics a single forward pass and makes a cycle in the graph
/// unrepresentable rather than a hang in a control loop.
struct Frame {
  std::string name;
  std::optional<std::size_t> parent;
  Transform local;
  std::vector<std::size_t> ignore;
  std::optional<Joint> joint;
  std::optional<Capsule> collision;
};

/// One channel binding: which joint a command channel drives, and what it last measured.
struct Binding {
  std::string joint;
  std::optional<std::string> channel;
  const Json* measured = nullptr;
};

struct Solved {
  /// One entry per frame; empty where the frame's pose is unknown.
  std::vector<std::optional<Transform>> poses;
  std::vector<std::size_t> unresolved;
  bool lookahead = true;
};

/// A flatten that could not produce a frame set. Thrown rather than returned: every caller in this
/// edition treats it as a defect in the document rather than as a verdict.
struct FlattenFault {
  std::string code;
};

constexpr std::size_t kMaxFrames = 128;

// The specified arithmetic. NaN for a non-finite angle and for one outside the range the reduction
// is exact over — the NaN propagates into the pose, and `solve` reads a non-finite pose as an
// unresolved frame and names it (§7.2's rule applied to arithmetic).
[[nodiscard]] double sqrt_(double x);
[[nodiscard]] double sin_(double x);
[[nodiscard]] double cos_(double x);

[[nodiscard]] Quat quaternion_product(const Quat& a, const Quat& b);
[[nodiscard]] Vec3 rotate(const Quat& q, const Vec3& v);
[[nodiscard]] Transform compose(const Transform& parent, const Transform& local);

/// Whether a frame's pose is computable at all, whatever the joint values are. False when the walk
/// to the root passes through a joint one scalar cannot describe (§44.5).
[[nodiscard]] bool statically_resolvable(const std::vector<Frame>& frames, std::size_t index);
/// How many frames carry collision geometry this kernel can both read and place.
[[nodiscard]] std::size_t collision_bodies(const std::vector<Frame>& frames);
/// How many pairs the self-collision guard tests each cycle.
[[nodiscard]] std::size_t collision_pairs(const std::vector<Frame>& frames);

[[nodiscard]] bool adjacent(const std::vector<Frame>& frames, std::size_t a, std::size_t b);
[[nodiscard]] bool ignores(const std::vector<Frame>& frames, std::size_t a, std::size_t b);

/// Compute every frame's pose in the root frame: one forward pass.
[[nodiscard]] Solved solve(const std::vector<Frame>& frames, const std::vector<Binding>& bindings,
                           const Json& command);

/// Flatten a §44.3 frame graph into the order the kernel reads. Throws `FlattenFault`.
[[nodiscard]] std::vector<Frame> flatten(const Json& graph);

/// Signed distance from a point to a volume's boundary: **positive inside**.
[[nodiscard]] double margin(const Json& shape, const Vec3& point);

/// Closest distance between two segments, **squared** — so no square root can move a verdict.
[[nodiscard]] double segment_distance_squared(const Vec3& p1, const Vec3& q1, const Vec3& p2,
                                              const Vec3& q2);

/// A capsule's two endpoints, in the root frame.
[[nodiscard]] std::pair<Vec3, Vec3> capsule_in_world(const Transform& pose, const Capsule& capsule);

/// The specified square root, for a consumer reporting a distance.
[[nodiscard]] double distance(double squared);

/// Read a three-vector out of a JSON value, NaN where a component is not a number. NaN rather than
/// a refusal because every consumer already treats a non-finite pose as unresolved, and a second
/// way to say "this frame has no pose" is a second thing to keep in step.
[[nodiscard]] Vec3 vec3(const Json& value);

}  // namespace skynet::kinematics

#endif  // SKYNET_OS_KINEMATICS_HPP
