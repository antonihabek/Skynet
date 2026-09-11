// The black box — §13 and §26. C++ edition.
//
// Transcribed from `reference/typescript/blackbox.ts`: the ring's record layout, the ring header, the
// six MCAP channel schemas, and the sealed file's framing. Every offset, schema and description is
// read from that file, never from `corpus/`.
//
// §31.2 puts the MCAP channel schemas in the short list of artifacts that stay single-source across
// editions, because *a black box sealed by one edition must be readable by every other* — and a
// format with one implementation has not been specified, only written. So there is a third.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// 64-bit fields
//     `bigint` in the reference, and decimal strings in a case's input: a wall-clock epoch in
//     nanoseconds is past 2^53 and would not survive a JSON number. Parsed straight into a
//     `uint64_t`, wrapping modulo 2^64 the way `setBigUint64` reduces a `bigint` — never through a
//     double.
//
// `DataView.setUint16` and friends
//     Apply ECMAScript's `ToUint16`/`ToUint32`/`ToInt32` to their argument: truncate toward zero,
//     reduce modulo 2^n, and write 0 for NaN or an infinity. A `static_cast` from a double does none
//     of that and is undefined outside the target's range.
//
// `setFloat32`
//     Rounds a double to the nearest float, ties to even, which is what a C++ conversion does in the
//     default floating-point environment. A NaN is the quiet NaN `0x7fc00000` in both.
//
// `TextEncoder`, and a cut inside a character
//     A name that does not fit is truncated on a character boundary: the cut walks back off any
//     continuation byte (`10xxxxxx`) it lands on, so the ring never holds invalid UTF-8.
//
// `GUARDS.indexOf(guard) + 1`
//     A name the table does not hold is -1 + 1: code 0. Reproduced rather than refused, because the
//     encoder's answer for an unknown name is part of the format.
//
// `Map` iteration
//     Insertion order. The sealed file's per-channel counts are written in the order the channels
//     were declared, which is id order here.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/sha256.hpp"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace skynet::contracts {
namespace {

using Bytes = std::vector<std::uint8_t>;

// ── ECMAScript's integer conversions, as `DataView` applies them ─────────────

/// `ToUint32`: truncate toward zero and reduce modulo 2^32. NaN and the infinities are 0.
std::uint32_t to_uint32(double value) {
  if (!std::isfinite(value)) {
    return 0;
  }
  const double truncated = std::trunc(value);
  const double modulus = 4294967296.0;
  double reduced = std::fmod(truncated, modulus);
  if (reduced < 0) {
    reduced += modulus;
  }
  return static_cast<std::uint32_t>(reduced);
}

std::uint16_t to_uint16(double value) { return static_cast<std::uint16_t>(to_uint32(value) & 0xFFFFu); }
std::uint8_t to_uint8(double value) { return static_cast<std::uint8_t>(to_uint32(value) & 0xFFu); }

/// `ToInt32`: `ToUint32`, then the upper half of the range read as negative.
std::int32_t to_int32(double value) {
  const std::uint32_t bits = to_uint32(value);
  std::int32_t out = 0;
  std::memcpy(&out, &bits, sizeof out);
  return out;
}

/// A decimal string as the reference's `BigInt(…)` then `setBigUint64` reduces it: modulo 2^64.
/// Wrapping arithmetic on a `uint64_t` is exactly that reduction, digit by digit.
std::uint64_t u64_from(const Json& value) {
  if (!value.is_string()) {
    // The corpus writes every 64-bit field as a decimal string, because a JSON number cannot carry
    // one intact. A number here is a value nobody received whole, and reading it would be a guess.
    throw std::invalid_argument("black box: a 64-bit field is not a decimal string");
  }
  const std::string text(value.string_or(""));
  std::size_t at = 0;
  const bool negative = !text.empty() && text[0] == '-';
  if (negative) {
    at = 1;
  }
  if (at >= text.size()) {
    throw std::invalid_argument("black box: a 64-bit field is not a decimal string: " + text);
  }
  std::uint64_t out = 0;
  for (; at < text.size(); ++at) {
    const char digit = text[at];
    if (digit < '0' || digit > '9') {
      throw std::invalid_argument("black box: a 64-bit field is not a decimal string: " + text);
    }
    out = out * 10u + static_cast<std::uint64_t>(digit - '0');
  }
  return negative ? static_cast<std::uint64_t>(0) - out : out;
}

/// A number from a case's input, with `null` standing for NaN: every float here is a bare `number`
/// in the reference, so a null is how `JSON.stringify` wrote a NaN (corpus/README.md).
double float_from(const Json& value) {
  return value.is_number() ? value.number_or(0) : std::numeric_limits<double>::quiet_NaN();
}

Bytes bytes_from_hex(std::string_view hex) {
  const auto nibble = [&](char ch) -> std::uint8_t {
    if (ch >= '0' && ch <= '9') return static_cast<std::uint8_t>(ch - '0');
    if (ch >= 'a' && ch <= 'f') return static_cast<std::uint8_t>(ch - 'a' + 10);
    if (ch >= 'A' && ch <= 'F') return static_cast<std::uint8_t>(ch - 'A' + 10);
    throw std::invalid_argument("black box: not a hex string");
  };
  if (hex.size() % 2 != 0) {
    throw std::invalid_argument("black box: a hex string of odd length");
  }
  Bytes out;
  for (std::size_t at = 0; at < hex.size(); at += 2) {
    out.push_back(static_cast<std::uint8_t>((nibble(hex[at]) << 4) | nibble(hex[at + 1])));
  }
  return out;
}

std::string hex_of(const Bytes& bytes) {
  static constexpr char kDigits[] = "0123456789abcdef";
  std::string out;
  out.reserve(bytes.size() * 2);
  for (const std::uint8_t byte : bytes) {
    out.push_back(kDigits[byte >> 4]);
    out.push_back(kDigits[byte & 0x0F]);
  }
  return out;
}

// ── CRC-32 ───────────────────────────────────────────────────────────────────

const std::array<std::uint32_t, 256>& crc_table() {
  static const std::array<std::uint32_t, 256> table = [] {
    std::array<std::uint32_t, 256> out{};
    for (std::uint32_t index = 0; index < 256; ++index) {
      std::uint32_t crc = index;
      for (int bit = 0; bit < 8; ++bit) {
        crc = (crc & 1u) != 0 ? 0xEDB88320u ^ (crc >> 1) : crc >> 1;
      }
      out[index] = crc;
    }
    return out;
  }();
  return table;
}

/// The running state over `bytes`, from `seed`.
std::uint32_t crc32(const std::uint8_t* bytes, std::size_t length, std::uint32_t seed = 0xFFFFFFFFu) {
  std::uint32_t state = seed;
  for (std::size_t at = 0; at < length; ++at) {
    state = crc_table()[(state ^ bytes[at]) & 0xFFu] ^ (state >> 8);
  }
  return state;
}

std::uint32_t crc32_final(std::uint32_t state) { return state ^ 0xFFFFFFFFu; }

std::uint32_t crc32_of(const std::uint8_t* bytes, std::size_t length) {
  return crc32_final(crc32(bytes, length));
}

// ── A little-endian writer over a fixed buffer ──────────────────────────────

class Cursor {
 public:
  explicit Cursor(std::size_t size) : bytes_(size, 0) {}

  void u8(std::size_t at, std::uint8_t value) { bytes_.at(at) = value; }
  void u16(std::size_t at, std::uint16_t value) { little(at, value, 2); }
  void u32(std::size_t at, std::uint32_t value) { little(at, value, 4); }
  void i32(std::size_t at, std::int32_t value) {
    std::uint32_t bits = 0;
    std::memcpy(&bits, &value, sizeof bits);
    u32(at, bits);
  }
  void u64(std::size_t at, std::uint64_t value) { little(at, value, 8); }
  void f32(std::size_t at, double value) {
    const float narrowed = static_cast<float>(value);
    std::uint32_t bits = 0;
    std::memcpy(&bits, &narrowed, sizeof bits);
    u32(at, bits);
  }
  void f64(std::size_t at, double value) {
    std::uint64_t bits = 0;
    std::memcpy(&bits, &value, sizeof bits);
    u64(at, bits);
  }
  void raw(std::size_t at, const Bytes& source) {
    if (at + source.size() > bytes_.size()) {
      throw std::invalid_argument("black box: a raw write runs past the slot");
    }
    std::copy(source.begin(), source.end(), bytes_.begin() + static_cast<std::ptrdiff_t>(at));
  }
  void or_bit(std::size_t at, std::uint8_t mask) { bytes_.at(at) = static_cast<std::uint8_t>(bytes_.at(at) | mask); }

  /// Write `value` into `width` bytes, NUL-padded, truncated on a character boundary.
  void text(std::size_t at, std::size_t width, std::string_view value) {
    std::size_t end = std::min(value.size(), width);
    while (end > 0 && end < value.size() &&
           (static_cast<std::uint8_t>(value[end]) & 0xC0u) == 0x80u) {
      --end;
    }
    std::fill(bytes_.begin() + static_cast<std::ptrdiff_t>(at),
              bytes_.begin() + static_cast<std::ptrdiff_t>(at + width), std::uint8_t{0});
    std::copy(value.begin(), value.begin() + static_cast<std::ptrdiff_t>(end),
              bytes_.begin() + static_cast<std::ptrdiff_t>(at));
  }

  [[nodiscard]] Bytes& bytes() { return bytes_; }

 private:
  void little(std::size_t at, std::uint64_t value, std::size_t width) {
    for (std::size_t index = 0; index < width; ++index) {
      bytes_.at(at + index) = static_cast<std::uint8_t>((value >> (8 * index)) & 0xFFu);
    }
  }

  Bytes bytes_;
};

// ── The record ───────────────────────────────────────────────────────────────

constexpr std::size_t kPrefix = 24;
/// The per-record schema version: **2** puts `read_state`'s outcome in bytes 172..176.
constexpr std::uint8_t kRecordSchema = 2;
constexpr std::size_t kCycleFixed = 176;
constexpr std::size_t kMaxTripped = 8;
constexpr std::size_t kMaxStreams = 6;
constexpr std::size_t kStreamName = 16;
constexpr std::size_t kEventSubject = 64;
constexpr std::size_t kEventDetail = 96;
constexpr std::size_t kFrameBody = 52 + kMaxStreams * (kStreamName + 8);
constexpr std::size_t kEventBody = 4 + kEventSubject + kEventDetail;
constexpr std::size_t kGapBody = 24;
/// `worst_skew_us` when fewer than two streams contributed: the spread of one instant is undefined.
constexpr std::uint32_t kSkewUnmeasurable = 4294967295u;

constexpr std::array<std::string_view, 8> kGuards = {
    "watchdog", "envelope", "rate_limit", "workspace", "self_collision", "balance", "proximity", "mode_gate",
};
constexpr std::array<std::string_view, 5> kResponses = {"clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0"};
constexpr std::array<std::string_view, 15> kEventCodes = {
    "lease_granted", "lease_released", "lease_preempted", "estop", "estop_cleared",
    "package_started", "package_stopped", "grant_changed", "ota", "clock_step",
    "recording_started", "sealed", "config_changed", "admission", "skill_transition",
};
constexpr std::array<std::string_view, 4> kGapReasons = {"writer_behind", "encoder_behind", "not_recording", "write_failed"};

/// `TABLE.indexOf(word) + 1`: a one-based wire code, and 0 for a word the table does not hold.
template <std::size_t N>
std::uint8_t code_of(const std::array<std::string_view, N>& table, const Json& word) {
  if (!word.is_string()) {
    return 0;
  }
  const auto found = std::find(table.begin(), table.end(), word.string_or(""));
  return found == table.end() ? 0 : static_cast<std::uint8_t>(found - table.begin() + 1);
}

struct Shape {
  std::size_t joints;
  std::size_t channels;
};

std::size_t div_ceil(std::size_t value, std::size_t by) { return (value + by - 1) / by; }

std::size_t record_size(const Shape& shape) {
  const std::size_t cycle = kCycleFixed + shape.channels * 13 + div_ceil(shape.channels, 8) + shape.joints * 12;
  const std::size_t largest = std::max({cycle, kPrefix + kFrameBody, kPrefix + kEventBody, kPrefix + kGapBody});
  return div_ceil(largest, 8) * 8;
}

Shape shape_from(const Json& shape) {
  return {static_cast<std::size_t>(to_uint32(shape["joints"].number_or(0))),
          static_cast<std::size_t>(to_uint32(shape["channels"].number_or(0)))};
}

void encode_cycle(const Shape& shape, Cursor& slot, const Json& cycle) {
  const auto check = [](const char* field, std::size_t found, std::size_t expected) {
    if (found != expected) {
      throw std::invalid_argument(std::string("black box: ") + field + " holds " + std::to_string(found) +
                                  " values, and the ring's shape says " + std::to_string(expected));
    }
  };
  check("desired", cycle["desired"].items().size(), shape.channels);
  check("command", cycle["command"].items().size(), shape.channels);
  check("mask", cycle["mask"].items().size(), shape.channels);
  check("clamped", cycle["clamped"].items().size(), shape.channels);
  check("position", cycle["position"].items().size(), shape.joints);
  check("velocity", cycle["velocity"].items().size(), shape.joints);
  check("effort", cycle["effort"].items().size(), shape.joints);

  slot.u64(24, u64_from(cycle["tick"]));
  slot.raw(32, bytes_from_hex(cycle["observation"].string_or("")));
  slot.u16(64, to_uint16(cycle["policy"].number_or(0)));
  slot.u16(66, to_uint16(cycle["state_flags"].number_or(0)));
  slot.u32(68, to_uint32(cycle["fault_code"].number_or(0)));
  slot.i32(72, to_int32(cycle["rhal_result"].number_or(0)));
  slot.u32(76, to_uint32(cycle["state_seq"].number_or(0)));
  slot.u64(80, u64_from(cycle["state_stamp_ns"]));
  slot.u64(88, u64_from(cycle["command_stamp_ns"]));
  slot.u32(96, to_uint32(cycle["command_seq"].number_or(0)));
  slot.u8(100, to_uint8(cycle["mode"].number_or(0)));
  slot.u8(101, cycle["advisory"].is_true() ? 1 : 0);
  slot.u8(102, cycle["response"].is_null() ? 0 : code_of(kResponses, cycle["response"]));
  const JsonArray& tripped = cycle["tripped"].items();
  slot.u8(103, static_cast<std::uint8_t>(std::min(tripped.size(), kMaxTripped)));
  for (std::size_t index = 0; index < std::min(tripped.size(), kMaxTripped); ++index) {
    slot.u8(104 + index, code_of(kGuards, tripped[index]));
  }
  slot.u16(112, to_uint16(cycle["capability"].number_or(0)));
  slot.f32(116, float_from(cycle["nearest_person_m"]));
  const JsonArray& pose = cycle["base_pose"].items();
  for (std::size_t index = 0; index < pose.size(); ++index) slot.f32(120 + index * 4, float_from(pose[index]));
  const JsonArray& twist = cycle["base_twist"].items();
  for (std::size_t index = 0; index < twist.size(); ++index) slot.f32(148 + index * 4, float_from(twist[index]));
  // 172..176 was reserved padding; the schema byte separates "reserved zero" from "the read succeeded".
  slot.i32(172, to_int32(cycle["read_result"].number_or(0)));

  const std::size_t desired_at = kCycleFixed;
  const std::size_t command_at = desired_at + shape.channels * 4;
  const std::size_t mask_at = command_at + shape.channels * 8;
  const std::size_t clamped_at = mask_at + shape.channels;
  const std::size_t position_at = clamped_at + div_ceil(shape.channels, 8);
  const std::size_t velocity_at = position_at + shape.joints * 4;
  const std::size_t effort_at = velocity_at + shape.joints * 4;

  for (std::size_t index = 0; index < shape.channels; ++index) {
    slot.f32(desired_at + index * 4, float_from(cycle["desired"].items()[index]));
    slot.f64(command_at + index * 8, float_from(cycle["command"].items()[index]));
    slot.u8(mask_at + index, to_uint8(cycle["mask"].items()[index].number_or(0)));
    if (cycle["clamped"].items()[index].is_true()) {
      slot.or_bit(clamped_at + index / 8, static_cast<std::uint8_t>(1u << (index % 8)));
    }
  }
  for (std::size_t index = 0; index < shape.joints; ++index) {
    slot.f32(position_at + index * 4, float_from(cycle["position"].items()[index]));
    slot.f32(velocity_at + index * 4, float_from(cycle["velocity"].items()[index]));
    slot.f32(effort_at + index * 4, float_from(cycle["effort"].items()[index]));
  }
}

void encode_frame(Cursor& slot, const Json& frame) {
  slot.raw(kPrefix, bytes_from_hex(frame["observation"].string_or("")));
  slot.u64(kPrefix + 32, u64_from(frame["target_ns"]));
  // `worstSkewUs` admits null, and there it means *not measurable* rather than NaN.
  const Json& skew = frame["worst_skew_us"];
  slot.u32(kPrefix + 40, skew.is_null() ? kSkewUnmeasurable : to_uint32(skew.number_or(0)));
  const JsonArray& streams = frame["streams"].items();
  const std::size_t written = std::min(streams.size(), kMaxStreams);
  slot.u16(kPrefix + 44, static_cast<std::uint16_t>(written));
  slot.u16(kPrefix + 46,
           to_uint16(frame["streams_omitted"].number_or(0) +
                     static_cast<double>(streams.size() > kMaxStreams ? streams.size() - kMaxStreams : 0)));
  slot.u16(kPrefix + 48, to_uint16(frame["streams_absent"].number_or(0)));
  // kPrefix + 50 is reserved and left zero, so the stream table starts on a four-byte boundary.
  for (std::size_t index = 0; index < written; ++index) {
    const Json& stream = streams[index];
    const std::size_t at = kPrefix + 52 + index * (kStreamName + 8);
    slot.text(at, kStreamName, stream["name"].string_or(""));
    slot.u32(at + kStreamName, to_uint32(stream["age_us"].number_or(0)));
    slot.u8(at + kStreamName + 4, stream["stale"].is_true() ? 1 : 0);
  }
}

Json encode_record(const Json& input) {
  const Shape shape = shape_from(input["shape"]);
  const Json& entry = input["entry"];
  const std::string kind(entry["kind"].string_or(""));
  Cursor slot(record_size(shape));
  std::uint8_t kind_code = 0;
  if (kind == "cycle") kind_code = 1;
  else if (kind == "frame") kind_code = 2;
  else if (kind == "event") kind_code = 3;
  else if (kind == "gap") kind_code = 4;
  else throw std::invalid_argument("black box: a record of no kind the format has: " + kind);

  slot.u8(4, kind_code);
  slot.u8(5, kRecordSchema);
  slot.u64(8, u64_from(input["sequence"]));
  slot.u64(16, u64_from(input["mono_ns"]));

  if (kind == "cycle") {
    encode_cycle(shape, slot, entry);
  } else if (kind == "frame") {
    encode_frame(slot, entry);
  } else if (kind == "event") {
    slot.u16(kPrefix, code_of(kEventCodes, entry["code"]));
    slot.text(kPrefix + 4, kEventSubject, entry["subject"].string_or(""));
    slot.text(kPrefix + 4 + kEventSubject, kEventDetail, entry["detail"].string_or(""));
  } else {
    slot.u64(kPrefix, u64_from(entry["dropped"]));
    slot.u64(kPrefix + 8, u64_from(entry["first_tick"]));
    slot.u16(kPrefix + 16, code_of(kGapReasons, entry["reason"]));
  }

  Bytes& bytes = slot.bytes();
  slot.u32(0, crc32_of(bytes.data() + 4, bytes.size() - 4));

  Json out = Json::object();
  out.set("size", Json::number(static_cast<double>(bytes.size())));
  out.set("hex", Json::string(hex_of(bytes)));
  return out;
}

// ── The ring header ──────────────────────────────────────────────────────────

constexpr std::size_t kHeaderFixed = 184;
constexpr std::size_t kHeaderAlign = 4096;
constexpr std::size_t kRobotId = 64;
constexpr std::size_t kTokenName = 48;
constexpr std::size_t kChannelName = 48;
/// The ring's on-disk format. **2** adds `streams_absent` and the unmeasurable skew.
constexpr std::uint16_t kFormatVersion = 2;

Json encode_header(const Json& input) {
  const Json& geometry = input["geometry"];
  const Json& meta = input["meta"];
  const Shape shape = shape_from(geometry["shape"]);
  const std::size_t digests = to_uint32(geometry["digests"].number_or(0));
  const std::size_t capabilities = to_uint32(geometry["capabilities"].number_or(0));
  const std::size_t raw_length = kHeaderFixed + digests * 32 + capabilities * kTokenName + shape.channels * kChannelName;
  const std::size_t length = div_ceil(raw_length, kHeaderAlign) * kHeaderAlign;

  Cursor header(length);
  header.raw(0, Bytes{'S', 'K', 'Y', 'N', 'E', 'T', 'B', 'B'});
  header.u16(8, kFormatVersion);
  header.u32(12, static_cast<std::uint32_t>(length));
  header.u32(16, static_cast<std::uint32_t>(record_size(shape)));
  header.u32(20, to_uint32(geometry["capacity"].number_or(0)));
  header.u16(24, static_cast<std::uint16_t>(shape.joints));
  header.u16(26, static_cast<std::uint16_t>(shape.channels));
  header.u8(28, to_uint8(meta["tier"].number_or(0)));
  header.u8(29, to_uint8(meta["enforcement"].number_or(0)));
  header.u8(30, meta["simulator"].is_true() ? 1 : 0);
  header.raw(32, bytes_from_hex(meta["embodiment"].string_or("")));
  header.raw(64, bytes_from_hex(meta["os_image"].string_or("")));
  header.text(96, kRobotId, meta["robot_id"].string_or(""));
  header.u16(160, static_cast<std::uint16_t>(digests));
  header.u16(162, static_cast<std::uint16_t>(capabilities));
  header.u64(164, u64_from(meta["wall_epoch_ns"]));
  header.u64(172, u64_from(meta["mono_ns"]));

  const std::size_t digest_base = kHeaderFixed;
  const JsonArray& held_digests = input["digests"].items();
  for (std::size_t index = 0; index < held_digests.size(); ++index) {
    header.raw(digest_base + index * 32, bytes_from_hex(held_digests[index].string_or("")));
  }
  const std::size_t token_base = digest_base + digests * 32;
  const JsonArray& tokens = input["capabilities"].items();
  for (std::size_t index = 0; index < tokens.size(); ++index) {
    header.text(token_base + index * kTokenName, kTokenName, tokens[index].string_or(""));
  }
  const std::size_t channel_base = token_base + capabilities * kTokenName;
  const JsonArray& channels = meta["channels"].items();
  for (std::size_t index = 0; index < std::min(channels.size(), shape.channels); ++index) {
    header.text(channel_base + index * kChannelName, kChannelName, channels[index].string_or(""));
  }

  Bytes& bytes = header.bytes();
  header.u32(180, crc32_of(bytes.data(), bytes.size()));

  Json out = Json::object();
  out.set("size", Json::number(static_cast<double>(bytes.size())));
  out.set("record_size", Json::number(static_cast<double>(record_size(shape))));
  out.set("hex", Json::string(hex_of(bytes)));
  return out;
}

// ── The channel schemas (§26, §31.2) ─────────────────────────────────────────

Json text_value(std::string_view value) { return Json::string(std::string(value)); }

Json field(std::string_view type, std::string_view description) {
  Json out = Json::object();
  out.set("type", text_value(type));
  out.set("description", text_value(description));
  return out;
}

Json maybe(std::string_view type, std::string_view description) {
  Json types = Json::array();
  types.push(text_value(type));
  types.push(text_value("null"));
  Json out = Json::object();
  out.set("type", types);
  out.set("description", text_value(description));
  return out;
}

Json array_of(Json items, std::string_view description) {
  Json out = Json::object();
  out.set("type", text_value("array"));
  out.set("items", std::move(items));
  out.set("description", text_value(description));
  return out;
}

Json number_array(std::string_view description) {
  Json types = Json::array();
  types.push(text_value("number"));
  types.push(text_value("null"));
  Json items = Json::object();
  items.set("type", types);
  return array_of(items, description);
}

Json object(std::vector<std::pair<std::string, Json>> properties, std::vector<std::string> required,
            std::string_view description) {
  Json props = Json::object();
  for (auto& [name, value] : properties) {
    props.set(name, std::move(value));
  }
  Json list = Json::array();
  for (const std::string& name : required) {
    list.push(Json::string(name));
  }
  Json out = Json::object();
  out.set("type", text_value("object"));
  out.set("description", text_value(description));
  out.set("properties", props);
  out.set("required", list);
  return out;
}

Json mono() {
  return field("integer",
               "CLOCK_MONOTONIC_RAW at the instant this record was produced. The time base every control "
               "decision was made against (§35.1); MCAP's log_time is the wall-clock rendering of it.");
}

Json channel_values(std::string_view description) {
  return array_of(
      object({{"channel", field("string", "The action channel's name.")},
              {"value", maybe("number", "The value on that channel.")},
              {"hold", field("boolean",
                             "True where the mask says hold this joint at its current setpoint — the OS saying "
                             "nothing rather than saying zero (§24).")}},
             {"channel", "value", "hold"}, "One channel's value."),
      description);
}

struct Channel {
  int id;
  std::string topic;
  std::string name;
  std::string definition;
};

/// The six channels, in id order. Ids are fixed for the life of the format.
const std::vector<Channel>& wire() {
  static const std::vector<Channel> channels = [] {
    std::vector<Channel> out;
    const auto add = [&](int id, std::string topic, std::string name, const Json& document) {
      out.push_back({id, std::move(topic), std::move(name), document.canonical()});
    };

    add(1, "/sk/state", "sk_state",
        object({{"mono_ns", mono()},
                {"stamp_ns", field("integer",
                                   "rhal_state.stamp_ns: CLOCK_MONOTONIC_RAW at acquisition, never at return (§24).")},
                {"seq", field("integer", "rhal_state.seq.")},
                {"flags", field("integer", "Bitwise-or of the RHAL flags: estop, fault, degraded, braked.")},
                {"fault_code", field("integer", "Vendor-specific, surfaced verbatim (§24).")},
                {"read_result",
                 maybe("integer",
                       "What read_state returned this cycle: 0, or a negative errno (§24). Null in a recording "
                       "sealed before the field existed, which is not the same as a read that succeeded (§26.4).")},
                {"nearest_person_m",
                 maybe("number",
                       "Distance to the nearest detected person, in metres; null when nothing was measuring it. "
                       "An input to the proximity guard rather than a reading the RHAL supplies (§7.1).")},
                {"position_rad", number_array("Measured joint positions.")},
                {"velocity_rad_s", number_array("Measured velocities; null where the joint does not measure it.")},
                {"effort_nm", number_array("Measured efforts; null where unmeasured.")},
                {"base_pose", number_array("x y z qx qy qz qw; all null when the base is not localised.")},
                {"base_twist", number_array("vx vy vz wx wy wz.")}},
               {"mono_ns", "stamp_ns", "seq", "flags", "fault_code", "read_result", "nearest_person_m",
                "position_rad", "velocity_rad_s", "effort_nm", "base_pose", "base_twist"},
               "One sample of the robot's state: what the RHAL reported (§24), whether the read succeeded, "
               "and the proximity reading the guards saw."));

    add(2, "/sk/desired", "sk_desired",
        object({{"mono_ns", mono()},
                {"tick", field("integer", "The control tick.")},
                {"stamp_ns", field("integer", "When the policy produced this command, not when it was sent.")},
                {"seq", field("integer", "rhal_command.seq; must advance (§24).")},
                {"mode", field("string", "position, velocity, torque, or unknown.")},
                {"policy_digest",
                 field("string",
                       "Hex SHA-256 of the package version that produced this, or empty when the recording"
                       "'s digest table did not hold it.")},
                {"capability",
                 maybe("string",
                       "The capability token this command exercised, or null when the runtime did not say — which "
                       "is not the same as no capability, and the mode gate treats the two differently (§7.2).")},
                {"observation",
                 field("string",
                       "Hex SHA-256 of the observation frame the policy was given. A hash, never the bytes: an "
                       "hour of three-camera video is not something the robot can keep (§26).")},
                {"values", channel_values("What the policy asked for.")}},
               {"mono_ns", "tick", "stamp_ns", "seq", "mode", "policy_digest", "capability", "observation", "values"},
               "What the policy asked for, before the Safety Kernel saw it."));

    add(3, "/sk/verdict", "sk_verdict",
        object({{"mono_ns", mono()},
                {"tick", field("integer", "The control tick.")},
                {"response",
                 maybe("string",
                       "clamp, hold, stop_cat2, stop_cat1 or stop_cat0 — the strongest response any tripped guard "
                       "demanded. Null when nothing tripped, which is a fact rather than an absence and so is "
                       "present-and-null.")},
                {"tripped",
                 array_of(field("string",
                                "A guard name, in evaluation order: watchdog, envelope, rate_limit, workspace, "
                                "self_collision, balance, proximity, mode_gate (§7.1)."),
                          "Guards that tripped.")},
                {"clamped",
                 array_of(object({{"channel", field("string", "Which channel was reduced.")},
                                  {"from", maybe("number", "What the policy asked for.")},
                                  {"to", maybe("number", "What the kernel sent instead.")}},
                                 {"channel", "from", "to"}, "One clamped channel, and by how much."),
                          "Channels whose values were reduced.")},
                {"advisory",
                 field("boolean",
                       "True when the tier could only advise: the command was forwarded unchanged despite a trip. "
                       "Always false at host and certified (§42).")}},
               {"mono_ns", "tick", "response", "tripped", "clamped", "advisory"},
               "What the Safety Kernel decided this cycle."));

    add(4, "/sk/command", "sk_command",
        object({{"mono_ns", mono()},
                {"tick", field("integer", "The control tick.")},
                {"values", channel_values("What actually went to write_command.")},
                {"rhal_result",
                 field("integer",
                       "What write_command returned: 0, or a negative errno. -1 EPERM because the drive is faulted "
                       "is a different fact from -110 ETIMEDOUT (§24).")}},
               {"mono_ns", "tick", "values", "rhal_result"},
               "What was sent to the hardware, and what the hardware said about it."));

    add(5, "/sk/frame", "sk_frame",
        object({{"mono_ns", mono()},
                {"observation", field("string", "Hex SHA-256 of the assembled frame.")},
                {"target_ns", field("integer", "The instant the assembler targeted.")},
                {"worst_skew_us",
                 maybe("integer",
                       "The worst skew between any stream and that instant. Recorded so that 'the policy behaved "
                       "oddly' can be checked against 'the frame it was given was 80 ms skewed' months later "
                       "(§25.1). Null when fewer than two streams contributed: the spread of one instant is "
                       "undefined rather than zero, and zero would make a frame with three dead capture paths "
                       "read as a perfectly synchronised one.")},
                {"streams_omitted",
                 field("integer",
                       "Streams the record could not carry, counted rather than dropped. A limit of the format.")},
                {"streams_absent",
                 field("integer",
                       "Streams the policy declared that produced no sample at all. A limit of the robot, and "
                       "deliberately not the field above: an investigation reading one as the other goes looking "
                       "for a bigger record instead of a dead camera.")},
                {"streams",
                 array_of(object({{"name", field("string", "The stream's name.")},
                                  {"age_us",
                                   field("integer",
                                         "How far back this stream's sample was, relative to the instant the "
                                         "assembler targeted.")},
                                  {"stale",
                                   field("boolean",
                                         "Whether that exceeded the policy's declared staleness budget, which is a "
                                         "hold rather than a silently older frame (§25.1).")}},
                                 {"name", "age_us", "stale"}, "One stream's contribution to the frame."),
                          "Per-stream ages, for the streams that had one.")}},
               {"mono_ns", "observation", "target_ns", "worst_skew_us", "streams_omitted", "streams_absent",
                "streams"},
               "One assembled observation frame, and how well aligned it was."));

    add(6, "/sk/event", "sk_event",
        object({{"mono_ns", mono()},
                {"event",
                 field("string",
                       "lease_granted, lease_released, lease_preempted, estop, estop_cleared, package_started, "
                       "package_stopped, grant_changed, ota, clock_step, recording_started, sealed, "
                       "config_changed, admission, skill_transition, or gap.")},
                {"subject", field("string", "What it happened to: a package id, a capability token, a slot.")},
                {"detail", field("string", "A sentence about it.")},
                {"dropped",
                 field("integer",
                       "Gap events only: how many records were not written. A gap annotated as a gap is honest; a "
                       "gap that looks like a quiet period is not (§47.4).")},
                {"first_tick", field("integer", "Gap events only: the first tick that went unrecorded.")},
                {"reason",
                 field("string", "Gap events only: writer_behind, encoder_behind, not_recording or write_failed.")}},
               {"mono_ns", "event", "subject", "detail"},
               "Something that changed, and every gap in the recording."));
    return out;
  }();
  return channels;
}

Json schema_case(const Json& input) {
  const std::string topic(input["channel"].string_or(""));
  for (const Channel& channel : wire()) {
    if (channel.topic == topic) {
      Json out = Json::object();
      out.set("id", Json::number(channel.id));
      out.set("topic", Json::string(channel.topic));
      out.set("name", Json::string(channel.name));
      out.set("definition", Json::string(channel.definition));
      return out;
    }
  }
  throw std::invalid_argument("black box: no channel on topic " + topic);
}

// ── The MCAP writer ──────────────────────────────────────────────────────────

constexpr std::array<std::uint8_t, 8> kMcapMagic = {0x89, 0x4D, 0x43, 0x41, 0x50, 0x30, 0x0D, 0x0A};

enum Opcode : std::uint8_t {
  kHeader = 0x01,
  kFooter = 0x02,
  kSchema = 0x03,
  kChannel = 0x04,
  kMessage = 0x05,
  kStatistics = 0x0B,
  kMetadata = 0x0C,
  kSummaryOffset = 0x0E,
  kDataEnd = 0x0F,
};

class Builder {
 public:
  void push(std::uint8_t value) { bytes_.push_back(value); }
  void u16(std::uint16_t value) { little(value, 2); }
  void u32(std::uint32_t value) { little(value, 4); }
  void u64(std::uint64_t value) { little(value, 8); }
  void raw(const Bytes& source) { bytes_.insert(bytes_.end(), source.begin(), source.end()); }
  void raw(std::string_view source) { bytes_.insert(bytes_.end(), source.begin(), source.end()); }
  /// A length-prefixed UTF-8 string.
  void string(std::string_view value) {
    u32(static_cast<std::uint32_t>(value.size()));
    raw(value);
  }
  [[nodiscard]] const Bytes& bytes() const { return bytes_; }
  [[nodiscard]] std::size_t size() const { return bytes_.size(); }

 private:
  void little(std::uint64_t value, std::size_t width) {
    for (std::size_t index = 0; index < width; ++index) {
      bytes_.push_back(static_cast<std::uint8_t>((value >> (8 * index)) & 0xFFu));
    }
  }
  Bytes bytes_;
};

struct Message {
  std::uint16_t channel_id;
  std::uint64_t log_time;
  std::string payload;
};

struct Metadata {
  std::string name;
  std::vector<std::pair<std::string, std::string>> pairs;
};

/// One MCAP file: magic, header, schemas, channels, metadata, messages, data end, summary, summary
/// offsets, footer, magic. Unchunked and uncompressed.
Bytes write_mcap(std::string_view library, const std::vector<Metadata>& metadata,
                 const std::vector<Message>& messages) {
  Builder out;
  const auto record = [&](std::uint8_t opcode, const Builder& body) {
    out.push(opcode);
    out.u64(body.size());
    out.raw(body.bytes());
  };
  const auto schema_body = [](const Channel& channel) {
    Builder body;
    body.u16(static_cast<std::uint16_t>(channel.id));
    body.string(channel.name);
    body.string("jsonschema");
    body.u32(static_cast<std::uint32_t>(channel.definition.size()));
    body.raw(channel.definition);
    return body;
  };
  const auto channel_body = [](const Channel& channel) {
    Builder body;
    body.u16(static_cast<std::uint16_t>(channel.id));
    body.u16(static_cast<std::uint16_t>(channel.id));
    body.string(channel.topic);
    body.string("json");
    body.u32(0);
    return body;
  };
  const std::vector<Channel>& channels = wire();

  out.raw(Bytes(kMcapMagic.begin(), kMcapMagic.end()));
  Builder header;
  header.string("");
  header.string(library);
  record(kHeader, header);

  for (const Channel& channel : channels) record(kSchema, schema_body(channel));
  for (const Channel& channel : channels) record(kChannel, channel_body(channel));

  for (const Metadata& entry : metadata) {
    Builder map;
    for (const auto& [key, value] : entry.pairs) {
      map.string(key);
      map.string(value);
    }
    Builder body;
    body.string(entry.name);
    body.u32(static_cast<std::uint32_t>(map.size()));
    body.raw(map.bytes());
    record(kMetadata, body);
  }

  // Per-channel sequences and counts, in the order the channels were declared.
  std::vector<std::pair<std::uint16_t, std::uint32_t>> sequences;
  std::vector<std::pair<std::uint16_t, std::uint64_t>> counts;
  for (const Channel& channel : channels) {
    sequences.emplace_back(static_cast<std::uint16_t>(channel.id), 0);
    counts.emplace_back(static_cast<std::uint16_t>(channel.id), 0);
  }
  const auto slot_for = [](auto& table, std::uint16_t id) -> auto& {
    for (auto& entry : table) {
      if (entry.first == id) return entry.second;
    }
    table.emplace_back(id, 0);
    return table.back().second;
  };

  bool any = false;
  std::uint64_t first = 0;
  std::uint64_t last = 0;
  for (const Message& message : messages) {
    const std::uint32_t sequence = ++slot_for(sequences, message.channel_id);
    ++slot_for(counts, message.channel_id);
    // The minimum, not the first seen: a robot writing from a ring is not writing in log-time order.
    first = !any || message.log_time < first ? message.log_time : first;
    last = message.log_time > last ? message.log_time : last;
    any = true;

    Builder body;
    body.u16(message.channel_id);
    body.u32(sequence);
    body.u64(message.log_time);
    body.u64(message.log_time);
    body.raw(message.payload);
    record(kMessage, body);
  }

  Builder data_end;
  data_end.u32(crc32_of(out.bytes().data(), out.size()));
  record(kDataEnd, data_end);

  const std::uint64_t summary_start = out.size();
  const std::uint64_t schema_start = out.size();
  for (const Channel& channel : channels) record(kSchema, schema_body(channel));
  const std::uint64_t schema_length = out.size() - schema_start;

  const std::uint64_t channel_start = out.size();
  for (const Channel& channel : channels) record(kChannel, channel_body(channel));
  const std::uint64_t channel_length = out.size() - channel_start;

  const std::uint64_t statistics_start = out.size();
  Builder statistics;
  std::uint64_t total = 0;
  for (const auto& [id, count] : counts) total += count;
  statistics.u64(total);
  statistics.u16(static_cast<std::uint16_t>(channels.size()));
  statistics.u32(static_cast<std::uint32_t>(channels.size()));
  statistics.u32(0);
  statistics.u32(static_cast<std::uint32_t>(metadata.size()));
  statistics.u32(0);
  statistics.u64(first);
  statistics.u64(last);
  Builder per_channel;
  for (const auto& [id, count] : counts) {
    per_channel.u16(id);
    per_channel.u64(count);
  }
  statistics.u32(static_cast<std::uint32_t>(per_channel.size()));
  statistics.raw(per_channel.bytes());
  record(kStatistics, statistics);
  const std::uint64_t statistics_length = out.size() - statistics_start;

  const std::uint64_t summary_offset_start = out.size();
  struct Offset {
    std::uint8_t opcode;
    std::uint64_t start;
    std::uint64_t length;
  };
  for (const Offset& offset : {Offset{kSchema, schema_start, schema_length},
                               Offset{kChannel, channel_start, channel_length},
                               Offset{kStatistics, statistics_start, statistics_length}}) {
    Builder body;
    body.push(offset.opcode);
    body.u64(offset.start);
    body.u64(offset.length);
    record(kSummaryOffset, body);
  }

  Builder footer;
  footer.push(kFooter);
  footer.u64(20);
  footer.u64(summary_start);
  footer.u64(summary_offset_start);
  const std::uint32_t summary_state =
      crc32(out.bytes().data() + summary_start, out.size() - static_cast<std::size_t>(summary_start));
  footer.u32(crc32_final(crc32(footer.bytes().data(), footer.size(), summary_state)));
  out.raw(footer.bytes());

  out.raw(Bytes(kMcapMagic.begin(), kMcapMagic.end()));
  return out.bytes();
}

/// The file's record opcodes, in order, as two-digit hex — and a check that the framing walks.
std::string opcodes(const Bytes& bytes) {
  const std::size_t end = bytes.size() - 8;
  std::string found;
  std::size_t at = 8;
  static constexpr char kDigits[] = "0123456789abcdef";
  while (at < end) {
    if (at + 9 > end) {
      throw std::invalid_argument("mcap: a record header runs past the end of the file at " + std::to_string(at));
    }
    if (!found.empty()) {
      found.push_back(',');
    }
    found.push_back(kDigits[bytes[at] >> 4]);
    found.push_back(kDigits[bytes[at] & 0x0F]);
    std::uint64_t length = 0;
    for (std::size_t index = 0; index < 8; ++index) {
      length |= static_cast<std::uint64_t>(bytes[at + 1 + index]) << (8 * index);
    }
    at += 9 + static_cast<std::size_t>(length);
  }
  if (at != end) {
    throw std::invalid_argument("mcap: the framing ended at " + std::to_string(at) + ", the trailing magic at " +
                                std::to_string(end));
  }
  return found;
}

Json seal(const Json& input) {
  // One metadata record, which is what a sealed window carries (§26).
  Metadata metadata{std::string(input["metadata"]["name"].string_or("")), {}};
  for (const Json& pair : input["metadata"]["pairs"].items()) {
    metadata.pairs.emplace_back(pair.items().at(0).string_or(""), pair.items().at(1).string_or(""));
  }
  std::vector<Message> messages;
  for (const Json& message : input["messages"].items()) {
    messages.push_back({to_uint16(message["channel_id"].number_or(0)), u64_from(message["log_time"]),
                        std::string(message["payload"].string_or(""))});
  }
  const Bytes bytes = write_mcap(input["library"].string_or(""), {metadata}, messages);

  Json out = Json::object();
  out.set("size", Json::number(static_cast<double>(bytes.size())));
  out.set("sha256", Json::string(sha256_hex(std::string_view(reinterpret_cast<const char*>(bytes.data()),
                                                             bytes.size()))));
  out.set("opcodes", Json::string(opcodes(bytes)));
  return out;
}

}  // namespace

Json blackbox(const std::string& label, const Json& input) {
  if (label == "schemas") {
    return schema_case(input);
  }
  if (label == "records") {
    return encode_record(input);
  }
  if (label == "header") {
    return encode_header(input);
  }
  if (label == "seal") {
    return seal(input);
  }
  throw Unimplemented("blackbox", label);
}

}  // namespace skynet::contracts
