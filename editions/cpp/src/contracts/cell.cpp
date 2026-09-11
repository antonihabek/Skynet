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
bool is_derived_fact(std::string_view key) { return key == "cell" || key == "fixtures"; }

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
bool truthy(const Json& value) {
  switch (value.kind()) {
    case Json::Kind::Null: return false;
    case Json::Kind::Bool: return value.is_true();
    case Json::Kind::Number: {
      const double held = value.number_or(0);
      return !(held == 0 || std::isnan(held));
    }
    case Json::Kind::String: return !value.string_or("").empty();
    default: return true;
  }
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

}  // namespace

Json cell(const std::string& label, const Json& input) {
  const Json& document = input["cell"];

  if (label.find("validate") != std::string::npos) {
    Json rows = Json::array();
    for (const Fault& fault : validate_cell(document)) {
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
    Json out = Json::object();
    out.set("faults", rows);
    return out;
  }

  if (label.find("context") == std::string::npos) {
    throw Unimplemented("cell", label);
  }

  // The `env.*` branch of a precondition context (§54.2): two derived facts and then the operator's
  // own. Derived first and unshadowable, which `validate_cell` enforces rather than this function
  // silently arbitrating: a cell whose facts overwrote its own name would answer a precondition
  // truthfully about the wrong thing.
  //
  // A cell with faults still produces a context. Refusing to would mean an operator's typo in a
  // fixture name took away the battery check as well, and §54.2 already has an answer for a fact
  // that cannot be read: the precondition naming it is refused, individually, with the path named.
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

  // One layer of §38.2's stack, named after the cell so an operator reading a workspace trip can
  // tell which layer stopped the robot. Layers compose by conjunction — a frame must be inside at
  // least one volume of *every* layer that names it — so adding this one can only narrow what the
  // site above it allowed, which is §38.2's rule holding by construction rather than by a check.
  //
  // **A cell that draws nothing adds no layer**, and that is not the same as a cell that allows
  // nothing. An empty layer would forbid every frame it named, and a site that has not yet measured
  // its cell would find the robot refusing to move with no volume anywhere to explain why.
  Json volumes = Json::array();
  for (const Json& volume : document["allowed"].items()) {
    if (shape_is_usable(volume) && frame_is_named(volume)) {
      Json row = Json::object();
      row.set("frame", volume["frame"]);
      row.set("shape", shape_json(*shape_of(volume)));
      volumes.push(row);
    }
  }

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
