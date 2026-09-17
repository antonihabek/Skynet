// The cell descriptor (§55.2) — the physical counterpart of an environment, C++ edition.
//
// Transcribed from `reference/typescript/cell.ts` and `reference/typescript/shape.ts`. Every rule
// and message string is read from those files, never from `corpus/`.
//
// §55.1 makes an environment a simulation artifact that only a simulator driver can load. §55.2
// names its real-world analogue and then refuses to make it a repo kind:
//
//     The real-world analogue of an environment — where the fixtures are, which volumes are
//     forbidden, where the safe parking pose is, what the operator has declared about the space —
//     is a **cell descriptor**, and it belongs in the operator profile (§38), not in the repository
//     catalogue.
//
//     The reason is ownership. An environment is published, versioned and shared; a cell is
//     site-specific, changes when somebody moves a table, and is nobody's intellectual property.
//
// So there is no publishing here, no versioning and no digest. A cell does exactly two jobs: it
// supplies `env.*` to §54.2's precondition context, and it supplies allowed volumes to §7.1's
// fourth guard as one layer of §38.2's stack — where layers compose by conjunction and can
// therefore only narrow.
//
// ── The check that only exists because §54.2 does ──────────────────────────
//
// `env.*` is addressed by a **dot path**, so a fact whose key contains a dot is a fact no
// precondition can ever name: `{"door.closed": true}` and `{"door": {"closed": true}}` are the same
// path and different documents, and the first will never match. That is not a hypothetical typo —
// it is the obvious way to write a hierarchical key for anybody who has not read `readPath`.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `key.trim() === key`
//     `String.prototype.trim` removes a **smaller** set than most hosts' trim, so a key wrapped in
//     U+001C is unusable to some hosts and perfectly addressable to the reference. Every trim here
//     is `js::trim`.
//
// `Object.keys(facts).sort()`
//     UTF-16 code-unit order, and the sort is a **compatibility decision rather than a tidy one**:
//     the Rust edition holds the facts in a `BTreeMap`, which has no insertion order to offer, so
//     reporting in the order somebody typed would make the two editions name the same faults in
//     different orders.
//
// `fixture.volume && !shapeIsUsable(...)`
//     Truthy, and it has to be: a `volume` that is present and `null` is a fixture nobody measured,
//     not a fixture measured wrongly.
//
// `Number.isFinite`
//     Rejects NaN and the infinities, and rejects booleans. A `true` in a coordinate is not a
//     coordinate of 1.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"

#include <algorithm>
#include <cmath>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// Facts the cell derives, which its own `facts` may not shadow. A precondition reading `env.cell`
/// must be reading the cell's name, on every robot, whatever somebody typed into a configuration.
bool is_derived_fact(std::string_view key) {
  return key == "cell" || key == "fixtures" || key == "robot" || key == "robots";
}

struct Fault {
  std::string code;
  std::vector<std::string> subjects;
  std::string detail;
};

/// The cell's name, or the empty string. A document whose `name` is missing or is not a string has
/// no name, and the empty string is how every edition spells that.
std::string cell_name(const Json& cell) {
  return cell["name"].is_string() ? std::string(cell["name"].string_or("")) : "";
}

/// A key a dot path can address: non-empty, no dot, and no leading or trailing whitespace.
bool usable_key(const std::string& key) {
  return !key.empty() && key.find('.') == std::string::npos && js::trim(key) == key;
}

/// Exactly three numbers, or nothing.
///
/// The finiteness test is deliberately **not** here: a coordinate of Infinity is decoded and then
/// reported as unusable geometry, which is a fault an operator can act on, rather than dropped at
/// the boundary into a shape this edition does not recognise.
std::optional<std::vector<double>> triple(const Json& value) {
  if (!value.is_array() || value.items().size() != 3) {
    return std::nullopt;
  }
  std::vector<double> out;
  for (const Json& item : value.items()) {
    if (!item.is_number()) {
      return std::nullopt;
    }
    out.push_back(item.number_or(0));
  }
  return out;
}

struct Shape {
  std::string kind;
  std::vector<double> a;  //< `min` for a box, `centre` for a sphere
  std::vector<double> b;  //< `max` for a box; unused for a sphere
  double radius_m = 0;
  bool radius_finite = false;
};

/// Decode one shape, or nothing when this edition does not recognise it.
std::optional<Shape> shape_of(const Json& volume) {
  if (!volume.is_object()) {
    return std::nullopt;
  }
  const Json& shape = volume["shape"];
  if (!shape.is_object()) {
    return std::nullopt;
  }
  const std::string kind(shape["kind"].string_or(""));
  if (kind == "box") {
    const auto minimum = triple(shape["min"]);
    const auto maximum = triple(shape["max"]);
    if (!minimum.has_value() || !maximum.has_value()) {
      return std::nullopt;
    }
    return Shape{"box", *minimum, *maximum, 0, false};
  }
  if (kind == "sphere") {
    const auto centre = triple(shape["centre"]);
    if (!centre.has_value() || !shape["radius_m"].is_number()) {
      return std::nullopt;
    }
    return Shape{"sphere", *centre, {}, shape["radius_m"].number_or(0),
                 std::isfinite(shape["radius_m"].number_or(0))};
  }
  return std::nullopt;
}

bool shape_is_usable(const Json& volume) {
  const auto shape = shape_of(volume);
  if (!shape.has_value()) {
    return false;
  }
  const auto all_finite = [](const std::vector<double>& values) {
    return std::all_of(values.begin(), values.end(),
                       [](double value) { return std::isfinite(value); });
  };
  if (shape->kind == "sphere") {
    return shape->radius_finite && shape->radius_m > 0 && all_finite(shape->a);
  }
  if (!all_finite(shape->a) || !all_finite(shape->b)) {
    return false;
  }
  // A box whose min exceeds its max on any axis is empty, and an empty allowed volume is a cell the
  // robot may never be in — a configuration mistake rather than a lock-out to enforce silently.
  for (std::size_t axis = 0; axis < 3; ++axis) {
    if (shape->a[axis] > shape->b[axis]) {
      return false;
    }
  }
  return true;
}

bool frame_is_named(const Json& volume) {
  return volume.is_object() && volume["frame"].is_string() &&
         !js::trim(volume["frame"].string_or("")).empty();
}

/// Whether `value` is truthy in the JavaScript sense — the test the fixture volume needs.
using js::truthy;

// ── §87.1's intersection test ────────────────────────────────────────────────

/// A shape moved into the cell's frame by its robot's placement, with the frame it constrains.
///
/// Translation only. §44.4's transforms carry a rotation and this deliberately does not read one: a
/// rotated box is not a box, so the test would need an oriented-box routine whose disagreements
/// between eight editions would be in the last bits of a square root. A cell whose robots are
/// mounted at an angle is one whose operator draws axis-aligned volumes large enough to hold the
/// rotation, which over-reports sharing and never under-reports it.
struct Placed {
  std::string frame;
  Shape shape;
};

Shape place(const Shape& shape, const std::vector<double>& at) {
  Shape out = shape;
  for (std::size_t axis = 0; axis < 3; ++axis) {
    out.a[axis] += at[axis];
    if (out.kind == "box") {
      out.b[axis] += at[axis];
    }
  }
  return out;
}

/// The placement, or nothing when the operator wrote none the arithmetic can use.
std::optional<std::vector<double>> read_placement(const Json& robot) {
  const auto at = triple(robot["at"]);
  if (!at.has_value()) {
    return std::nullopt;
  }
  for (const double value : *at) {
    if (!std::isfinite(value)) {
      return std::nullopt;
    }
  }
  return at;
}

/// Squared distance from a point to a box, zero inside.
double box_point_distance_squared(const Shape& box, const std::vector<double>& point) {
  double total = 0;
  for (std::size_t axis = 0; axis < 3; ++axis) {
    const double low = box.a[axis];
    const double high = box.b[axis];
    const double value = point[axis];
    const double outside = value < low ? low - value : (value > high ? value - high : 0.0);
    total += outside * outside;
  }
  return total;
}

/// Whether two placed shapes share a point.
///
/// **Inclusive, and that is the decision rather than an off-by-one.** `margin` counts zero as
/// inside — a workspace trip is `margin < 0` — so two robots whose boxes meet on a face can each
/// legally be at the point where they meet. A strict test would call that configuration disjoint,
/// and the platform would have certified the one arrangement where both arms are allowed into the
/// same millimetre.
bool intersects(const Shape& a, const Shape& b) {
  if (a.kind == "box" && b.kind == "box") {
    for (std::size_t axis = 0; axis < 3; ++axis) {
      if (a.a[axis] > b.b[axis] || b.a[axis] > a.b[axis]) {
        return false;
      }
    }
    return true;
  }
  if (a.kind == "sphere" && b.kind == "sphere") {
    const double reach = a.radius_m + b.radius_m;
    double squared = 0;
    for (std::size_t axis = 0; axis < 3; ++axis) {
      const double difference = a.a[axis] - b.a[axis];
      squared += difference * difference;
    }
    return squared <= reach * reach;
  }
  const Shape& box = a.kind == "box" ? a : b;
  const Shape& sphere = a.kind == "sphere" ? a : b;
  return box_point_distance_squared(box, sphere.a) <= sphere.radius_m * sphere.radius_m;
}

/// A robot's usable volumes, placed. A volume with no usable shape is not compared.
std::vector<Placed> placed_volumes(const Json& robot) {
  std::vector<Placed> placed;
  const auto at = read_placement(robot);
  if (!at.has_value()) {
    return placed;
  }
  for (const Json& volume : robot["allowed"].items()) {
    if (!shape_is_usable(volume)) {
      continue;
    }
    placed.push_back({std::string(volume["frame"].string_or("")), place(*shape_of(volume), *at)});
  }
  return placed;
}

/// A robot's slug, or the empty string — `cell_name`'s rule, one level down.
std::string robot_slug(const Json& robot) {
  return robot["slug"].is_string() ? std::string(robot["slug"].string_or("")) : "";
}

/// §87.1's five faults, or none.
///
/// Split out rather than inlined because all five are conditional on the same thing — **two or more
/// robots** — and a reader has to be able to see that condition once. One robot is §55.2's cell and
/// has no pair to be disjoint from; the placement rule would otherwise refuse every cell anybody has
/// ever written.
///
/// Reported in declaration order, then the pairs in index order, so every edition names the same
/// fault first. The disjointness walk skips a robot that already has a fault of its own, for
/// `validate_cell`'s duplicate-fixture reason.
std::vector<Fault> robot_faults(const Json& cell) {
  std::vector<Fault> faults;
  const auto& robots = cell["robots"].items();
  const bool several = robots.size() >= 2;
  std::set<std::string> seen;
  std::vector<std::pair<std::string, std::vector<Placed>>> comparable;

  std::size_t index = 0;
  for (const Json& robot : robots) {
    const std::string slug = robot_slug(robot);
    const std::size_t at = index++;
    if (js::trim(slug).empty()) {
      faults.push_back({"robot_unnamed",
                        {"robots[" + std::to_string(at) + "]"},
                        "A robot a precondition cannot name is a robot no skill can ask about. "
                        "`env.robot` is its slug."});
      continue;
    }
    if (seen.count(slug) != 0) {
      faults.push_back({"duplicate_robot",
                        {slug},
                        "Two entries with one slug: a precondition asking about it would be asking "
                        "about whichever the reader happened to reach."});
      continue;
    }
    seen.insert(slug);

    // Everything below is about the pair, so a cell with one robot stops here.
    if (!several) {
      continue;
    }

    if (!read_placement(robot).has_value()) {
      faults.push_back({"robot_unplaced",
                        {slug},
                        "This cell holds more than one robot, and this one carries no `at`. Its "
                        "volumes are in its own root frame (§44.2), so nothing can say whether "
                        "they overlap another robot's until somebody says where it stands."});
      continue;
    }
    auto volumes = placed_volumes(robot);
    if (volumes.empty()) {
      faults.push_back({"robot_unbounded",
                        {slug},
                        "This cell holds more than one robot, and this one draws no volume the "
                        "workspace guard can evaluate. A robot that may be anywhere is not "
                        "disjoint from anything."});
      continue;
    }
    comparable.emplace_back(slug, std::move(volumes));
  }

  for (std::size_t a = 0; a < comparable.size(); ++a) {
    for (std::size_t b = a + 1; b < comparable.size(); ++b) {
      for (const Placed& one : comparable[a].second) {
        for (const Placed& other : comparable[b].second) {
          if (!intersects(one.shape, other.shape)) {
            continue;
          }
          faults.push_back(
              {"shared_volume",
               {comparable[a].first, comparable[b].first, one.frame, other.frame},
               comparable[a].first + "'s " + one.frame + " and " + comparable[b].first + "'s " +
                   other.frame +
                   " share a point once both robots are placed. §87.1 gives each robot its "
                   "own kernel, so nothing on this machine evaluates the pair — the overlap "
                   "is a claim the platform cannot enforce rather than a hazard it has measured."});
        }
      }
    }
  }

  return faults;
}

/// Every fault in a cell descriptor.
///
/// A **list** rather than the first one, unlike `validate_frames` next door, and the difference is
/// about who is reading. A frame graph that does not form a tree cannot be analysed further, so one
/// fault is all there is to say. A cell descriptor's problems are independent, an operator is
/// editing it by hand, and admission's reason applies unchanged: fixing four problems one
/// deployment at a time is how people end up giving up and granting everything.
std::vector<Fault> validate_cell(const Json& cell) {
  std::vector<Fault> faults;

  if (!cell["name"].is_string() || js::trim(cell["name"].string_or("")).empty()) {
    faults.push_back({"unnamed", {}, "A cell has no identity but its name, and this one has none."});
  }

  // Fact keys are walked in sorted order; everything else in the order it was written.
  std::vector<std::string> keys;
  for (const auto& entry : cell["facts"].entries()) {
    keys.push_back(entry.first);
  }
  std::sort(keys.begin(), keys.end(), [](const std::string& left, const std::string& right) {
    return js::utf16_less(left, right);
  });
  for (const std::string& key : keys) {
    if (!usable_key(key)) {
      faults.push_back({"fact_key_unusable",
                        {key},
                        "A precondition addresses this as env." + key +
                            ", and a dot path cannot name it. Nest the object instead of putting a "
                            "dot in the key."});
      continue;
    }
    if (is_derived_fact(key)) {
      faults.push_back({"fact_key_reserved",
                        {key},
                        "env." + key +
                            " is derived from the cell itself, so a fact of that name would be read "
                            "by nobody and would make a precondition true for the wrong reason."});
    }
  }

  const JsonArray& allowed = cell["allowed"].items();
  for (std::size_t index = 0; index < allowed.size(); ++index) {
    const std::string where = "allowed[" + std::to_string(index) + "]";
    if (!frame_is_named(allowed[index])) {
      faults.push_back({"volume_frame_missing",
                        {where},
                        "A volume constrains a frame, and this one names none. The workspace guard "
                        "would have nothing to check it against."});
    }
    if (!shape_is_usable(allowed[index])) {
      faults.push_back({"volume_shape_invalid",
                        {where},
                        "This volume has no usable geometry — a box whose minimum exceeds its "
                        "maximum, a sphere with no radius, or a shape this edition does not know."});
    }
  }

  std::set<std::string> seen_fixtures;
  const JsonArray& fixtures = cell["fixtures"].items();
  for (std::size_t index = 0; index < fixtures.size(); ++index) {
    const Json& fixture = fixtures[index];
    const Json& fixture_name = fixture["name"];
    if (!fixture_name.is_string() || js::trim(fixture_name.string_or("")).empty()) {
      faults.push_back({"fixture_unnamed",
                        {"fixtures[" + std::to_string(index) + "]"},
                        "A fixture a precondition cannot name is a fixture the OS cannot check "
                        "for."});
      continue;
    }
    const std::string name(fixture_name.string_or(""));
    if (seen_fixtures.count(name) != 0) {
      faults.push_back({"duplicate_fixture",
                        {name},
                        "Two fixtures with one name: a precondition asking for it would be asking "
                        "about whichever the reader happened to reach."});
      continue;
    }
    seen_fixtures.insert(name);
    // Truthy, deliberately: a `volume` that is present and null is a fixture nobody measured.
    if (truthy(fixture["volume"]) && !shape_is_usable(fixture["volume"])) {
      faults.push_back({"volume_shape_invalid",
                        {name},
                        "This fixture has been given a volume with no usable geometry. A fixture "
                        "may be named without being measured; it may not be measured wrongly."});
    }
  }

  std::set<std::string> seen_channels;
  const JsonArray& parking = cell["parking_pose"].items();
  for (std::size_t index = 0; index < parking.size(); ++index) {
    const Json& channel_field = parking[index]["channel"];
    if (!channel_field.is_string() || js::trim(channel_field.string_or("")).empty()) {
      faults.push_back({"parking_channel_unnamed",
                        {"parking_pose[" + std::to_string(index) + "]"},
                        "A parking pose is a command, and a command names its channel."});
      continue;
    }
    const std::string channel(channel_field.string_or(""));
    if (seen_channels.count(channel) != 0) {
      faults.push_back({"duplicate_parking_channel",
                        {channel},
                        "One channel, two parking positions. Which one the robot drives to would "
                        "depend on the order a reader happened to walk the list."});
      continue;
    }
    seen_channels.insert(channel);
  }

  const auto robots = robot_faults(cell);
  faults.insert(faults.end(), robots.begin(), robots.end());

  return faults;
}

Json shape_json(const Shape& shape) {
  Json out = Json::object();
  const auto vector_json = [](const std::vector<double>& values) {
    Json list = Json::array();
    for (const double value : values) {
      list.push(Json::number(value));
    }
    return list;
  };
  if (shape.kind == "sphere") {
    out.set("kind", Json::string("sphere"));
    out.set("centre", vector_json(shape.a));
    out.set("radius_m", Json::number(shape.radius_m));
    return out;
  }
  out.set("kind", Json::string("box"));
  out.set("min", vector_json(shape.a));
  out.set("max", vector_json(shape.b));
  return out;
}


/// Every slug the cell names, sorted and without the ones it got wrong (§87.1).
///
/// Sorted for `validate_cell`'s reason one field along: a precondition asks `includes`, so order is
/// not semantic — but two editions emitting one list in two orders is a corpus failure, and sorted
/// is the order every edition can produce. A duplicate appears once, because `env.robots` answers
/// *which robots*, and `duplicate_robot` is where the document's mistake is reported.
std::vector<std::string> cell_robot_slugs(const Json& cell) {
  std::vector<std::string> slugs;
  for (const Json& robot : cell["robots"].items()) {
    const std::string slug = robot_slug(robot);
    if (js::trim(slug).empty() ||
        std::find(slugs.begin(), slugs.end(), slug) != slugs.end()) {
      continue;
    }
    slugs.push_back(slug);
  }
  std::sort(slugs.begin(), slugs.end(), [](const std::string& left, const std::string& right) {
    return js::utf16_less(left, right);
  });
  return slugs;
}

/// The volumes of one layer, on the wire. Shared by the cell's own layer and each robot's.
Json layer_volumes(const Json& allowed) {
  Json volumes = Json::array();
  for (const Json& volume : allowed.items()) {
    if (shape_is_usable(volume) && frame_is_named(volume)) {
      Json row = Json::object();
      row.set("frame", volume["frame"]);
      row.set("shape", shape_json(*shape_of(volume)));
      volumes.push(row);
    }
  }
  return volumes;
}

/// The layer one robot of a shared cell contributes, or null (§87.1).
///
/// A **second** layer rather than a replacement for the cell's own, because §38.2's layers compose
/// by conjunction and two layers is how a cell-wide volume and a per-robot one both hold. The
/// volumes are **unplaced**, in the robot's own root frame, because that is the frame the
/// `workspace` guard resolves poses in (§44.2).
Json cell_robot_layer(const Json& cell, const std::string& slug) {
  if (js::trim(slug).empty()) {
    return Json::null();
  }
  for (const Json& robot : cell["robots"].items()) {
    if (robot_slug(robot) != slug) {
      continue;
    }
    Json volumes = layer_volumes(robot["allowed"]);
    if (volumes.items().empty()) {
      return Json::null();
    }
    Json layer = Json::object();
    layer.set("name", Json::string("cell:" + cell_name(cell) + ":" + slug));
    layer.set("volumes", volumes);
    return layer;
  }
  return Json::null();
}

/// The `env.*` branch of a precondition context (§54.2).
///
/// `subject` is which robot the context is being assembled for (§87.1); a slug the cell has not been
/// told about is reported as the empty string rather than echoed back, because an edition that
/// echoed it would let a precondition assert membership of a cell by asserting it.
Json cell_environment(const Json& document, const std::string& subject, bool has_subject) {
  Json environment = Json::object();
  for (const auto& entry : document["facts"].entries()) {
    if (usable_key(entry.first) && !is_derived_fact(entry.first)) {
      environment.set(entry.first, entry.second);
    }
  }
  environment.set("cell", Json::string(cell_name(document)));
  Json fixture_names = Json::array();
  for (const Json& fixture : document["fixtures"].items()) {
    if (fixture["name"].is_string() && !js::trim(fixture["name"].string_or("")).empty()) {
      fixture_names.push(fixture["name"]);
    }
  }
  environment.set("fixtures", fixture_names);

  const auto declared = cell_robot_slugs(document);
  const bool mine = has_subject &&
                    std::find(declared.begin(), declared.end(), subject) != declared.end();
  environment.set("robot", Json::string(mine ? subject : ""));
  Json robots = Json::array();
  for (const std::string& slug : declared) {
    robots.push(Json::string(slug));
  }
  environment.set("robots", robots);
  return environment;
}

Json fault_rows(const std::vector<Fault>& faults) {
  Json rows = Json::array();
  for (const Fault& fault : faults) {
    Json row = Json::object();
    row.set("code", Json::string(fault.code));
    Json subjects = Json::array();
    for (const std::string& subject : fault.subjects) {
      subjects.push(Json::string(subject));
    }
    row.set("subjects", subjects);
    row.set("detail", Json::string(fault.detail));
    rows.push(row);
  }
  return rows;
}

}  // namespace

Json cell(const std::string& label, const Json& input) {
  const Json& document = input["cell"];

  if (label.find("validate") != std::string::npos) {
    Json out = Json::object();
    out.set("faults", fault_rows(validate_cell(document)));
    return out;
  }

  // §87.1's three projections of one document. One document rather than three, because they are one
  // question — *what does this cell say about its robots* — and an edition could agree about the
  // disjointness arithmetic while disagreeing about which layer a robot contributes, which is the
  // half a workspace guard reads.
  if (label.find("robots") != std::string::npos) {
    Json layers = Json::array();
    for (const Json& robot : document["robots"].items()) {
      const std::string slug = robot_slug(robot);
      Json row = Json::object();
      row.set("slug", Json::string(slug));
      row.set("layer", cell_robot_layer(document, slug));
      layers.push(row);
    }
    Json slugs = Json::array();
    for (const std::string& slug : cell_robot_slugs(document)) {
      slugs.push(Json::string(slug));
    }
    Json out = Json::object();
    out.set("env", cell_environment(document, std::string(input["robot"].string_or("")),
                                    input["robot"].is_string()));
    out.set("slugs", slugs);
    out.set("layers", layers);
    out.set("faults", fault_rows(validate_cell(document)));
    return out;
  }

  if (label.find("context") == std::string::npos) {
    throw Unimplemented("cell", label);
  }

  // No subject slug: these cases predate §87.1 and are about a cell with one robot, so `env.robot`
  // is the empty string and `env.robots` is empty — which is what every cell written before §87
  // answers, and the property that makes the field additive.
  Json environment = cell_environment(document, "", false);

  // One layer of §38.2's stack, named after the cell so an operator reading a workspace trip can
  // tell which layer stopped the robot. Layers compose by conjunction — a frame must be inside at
  // least one volume of *every* layer that names it — so adding this one can only narrow what the
  // site above it allowed, which is §38.2's rule holding by construction rather than by a check.
  //
  // **A cell that draws nothing adds no layer**, and that is not the same as a cell that allows
  // nothing. An empty layer would forbid every frame it named, and a site that has not yet measured
  // its cell would find the robot refusing to move with no volume anywhere to explain why.
  Json volumes = layer_volumes(document["allowed"]);

  Json layer = Json::null();
  if (!volumes.items().empty()) {
    layer = Json::object();
    layer.set("name", Json::string("cell:" + cell_name(document)));
    layer.set("volumes", volumes);
  }

  // `layer` is nullable rather than absent, and the distinction is §55.2's: a cell nobody has
  // measured and a cell that permits nothing are different facts, and only one of them should stop
  // a robot.
  Json out = Json::object();
  out.set("env", environment);
  out.set("layer", layer);
  return out;
}

}  // namespace skynet::contracts
