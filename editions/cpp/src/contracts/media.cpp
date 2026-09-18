// Frame handles for the sensor and media pipeline (§47.6).
//
// §47.4 gives the policy a latest-wins slot and the black box a bounded queue, and says nothing
// about how a frame reaches a policy **in another process**. §80 rules out the obvious answer:
// §25's ring has 4 KiB slots sized for joint vectors, a frame does not fit in one, and widening the
// ring would put a megabyte of copy in the path a control loop runs. So frames stay where capture
// put them and what crosses is a *handle* — `(stream, slot, generation)`, enough to map exactly one
// frame read-only and not enough to name a different one.
//
// ── What the transcription had to be careful about ──────────────────────────
//
// `sequence` is compared, never arithmetic. The reference orders slots by `identity.sequence`, and
// nothing here adds or subtracts one, which is where two number models would start to differ.
//
// `Number.POSITIVE_INFINITY` as the initial "oldest so far" becomes an empty optional, tested
// explicitly. An infinity would work and would invite a later reader to compare it against a
// sequence, which is exactly the mixed comparison the rest of this edition avoids.
//
// The refusal **messages** are transcribed, not paraphrased. §64 makes an error a design surface
// and the corpus compares the sentence byte for byte, so a message reworded here is a divergence in
// the one field an operator actually reads.
//
// ── The bookkeeping half ────────────────────────────────────────────────────
//
// Everything here is arithmetic over slot state. The **mapping** half — a read-only mapping of the
// named slot in the policy's address space — is per-OS code that cannot be expressed in a contract.
// What it must honour is the property this contract exists to guarantee: a handle grants read
// access to exactly one generation of one slot, and the pool may reuse that slot the moment it is
// released.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"

#include <optional>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

struct Identity {
  std::string stream;
  double stamp_ns = 0;
  double sequence = 0;
};

struct Slot {
  double generation = 0;
  std::optional<Identity> identity;
  bool acquired = false;
};

struct Handle {
  std::string stream;
  double slot = 0;
  double generation = 0;
};

struct Outcome {
  bool refused = false;
  std::string code;
  std::string message;
  std::optional<Handle> handle;
  std::optional<Identity> identity;
};

Outcome refusal(std::string code, std::string message) {
  Outcome out;
  out.refused = true;
  out.code = std::move(code);
  out.message = std::move(message);
  return out;
}

/// One stream's fixed frame pool. Pure bookkeeping: no clocks, no I/O, no mapping.
class FramePool {
 public:
  FramePool(std::string stream, std::size_t capacity)
      : stream_(std::move(stream)), slots_(capacity) {}

  Outcome publish(const Identity& frame) {
    if (frame.stream != stream_) {
      return refusal("unknown_stream", "This pool holds " + stream_ +
                                           " and was handed a frame from " + frame.stream + ".");
    }
    const int index = oldest_released();
    if (index < 0) {
      ++dropped_;
      return refusal("pool_exhausted",
                     "Every slot is acquired, so frame " + js::number_to_string(frame.sequence) +
                         " was dropped and counted rather than stalling capture.");
    }
    Slot& slot = slots_[static_cast<std::size_t>(index)];
    slot.generation += 1;
    slot.identity = frame;
    slot.acquired = false;
    ++published_;

    Outcome out;
    out.handle = Handle{stream_, static_cast<double>(index), slot.generation};
    return out;
  }

  /// The newest published frame's handle, or nothing when nothing has been published.
  ///
  /// Never a frame of zeros: a zero frame is a measurement nobody took, and a policy handed one
  /// cannot tell it from a dark room.
  [[nodiscard]] std::optional<Handle> latest() const {
    std::optional<Handle> best;
    std::optional<double> best_sequence;
    for (std::size_t index = 0; index < slots_.size(); ++index) {
      const Slot& slot = slots_[index];
      if (slot.identity.has_value() &&
          (!best_sequence.has_value() || slot.identity->sequence > *best_sequence)) {
        best_sequence = slot.identity->sequence;
        best = Handle{stream_, static_cast<double>(index), slot.generation};
      }
    }
    return best;
  }

  /// Take a read loan on the named frame. The slot is pinned until `release`: publishing reuses an
  /// older released slot first, and drops rather than reusing this one.
  Outcome acquire(const Handle& handle) {
    const int index = locate(handle);
    if (index < 0) {
      return unknown_slot();
    }
    Slot& slot = slots_[static_cast<std::size_t>(index)];
    if (!slot.identity.has_value() || handle.generation != slot.generation) {
      return refusal("stale_handle",
                     "The slot has been reused since this handle was issued, so the handle names a "
                     "frame that is no longer there.");
    }
    if (slot.acquired) {
      return refusal("already_acquired",
                     "This handle is already checked out; a handle is one loan, not a share.");
    }
    slot.acquired = true;
    ++acquired_;

    Outcome out;
    out.identity = slot.identity;
    return out;
  }

  /// Return a loan. Refuses unknown, stale and already-released handles by name.
  Outcome release(const Handle& handle) {
    const int index = locate(handle);
    if (index < 0) {
      return unknown_slot();
    }
    Slot& slot = slots_[static_cast<std::size_t>(index)];
    if (!slot.identity.has_value() || handle.generation != slot.generation) {
      return refusal("stale_handle",
                     "The slot has been reused since this handle was issued; there is nothing of "
                     "it left to release.");
    }
    if (!slot.acquired) {
      return refusal("double_release",
                     "This handle was already released; releasing it twice would unpin a later "
                     "loan.");
    }
    slot.acquired = false;
    ++released_;
    return {};
  }

  /// What the pool has done, which is the only place a drop is visible.
  [[nodiscard]] Json stats() const {
    Json out = Json::object();
    out.set("published", Json::number(published_));
    out.set("dropped", Json::number(dropped_));
    out.set("acquired", Json::number(acquired_));
    out.set("released", Json::number(released_));
    return out;
  }

 private:
  std::string stream_;
  std::vector<Slot> slots_;
  double published_ = 0;
  double dropped_ = 0;
  double acquired_ = 0;
  double released_ = 0;

  static Outcome unknown_slot() {
    return refusal("unknown_slot", "The handle names a slot this pool does not hold.");
  }

  /// Which slot the next publish takes: the released one holding the oldest frame.
  ///
  /// The obvious implementation — the first slot that is not currently acquired — is wrong in a way
  /// no single-publish test can see. With nothing acquired it returns slot 0 every time, so a pool
  /// of any declared capacity uses one slot, and §47.4 already has a one-slot latest-wins channel.
  /// The cost lands on the *reader*: a policy calls `latest` and then `acquire`, capture publishes
  /// in between, and taking the first free slot means that publish lands on the slot `latest` just
  /// named.
  ///
  /// Empty slots go first because there is nothing in them to destroy. After that, oldest is by the
  /// frame's `sequence` and never by the slot's index — the two stop agreeing the first time a
  /// reader holds a frame and lets it go. Ties break to the lower index.
  [[nodiscard]] int oldest_released() const {
    int chosen = -1;
    std::optional<double> chosen_sequence;
    for (std::size_t index = 0; index < slots_.size(); ++index) {
      const Slot& slot = slots_[index];
      if (slot.acquired) {
        continue;
      }
      if (!slot.identity.has_value()) {
        return static_cast<int>(index);
      }
      if (!chosen_sequence.has_value() || slot.identity->sequence < *chosen_sequence) {
        chosen_sequence = slot.identity->sequence;
        chosen = static_cast<int>(index);
      }
    }
    return chosen;
  }

  /// The stream and slot checks both `acquire` and `release` open with.
  [[nodiscard]] int locate(const Handle& handle) const {
    if (handle.stream != stream_ || handle.slot < 0 ||
        handle.slot >= static_cast<double>(slots_.size()) ||
        handle.slot != static_cast<double>(static_cast<long long>(handle.slot))) {
      return -1;
    }
    return static_cast<int>(handle.slot);
  }
};

Json handle_json(const std::optional<Handle>& handle) {
  if (!handle.has_value()) {
    return Json::null();
  }
  Json out = Json::object();
  out.set("stream", Json::string(handle->stream));
  out.set("slot", Json::number(handle->slot));
  out.set("generation", Json::number(handle->generation));
  return out;
}

Json identity_json(const Identity& identity) {
  Json out = Json::object();
  out.set("stream", Json::string(identity.stream));
  out.set("stamp_ns", Json::number(identity.stamp_ns));
  out.set("sequence", Json::number(identity.sequence));
  return out;
}

Json refusal_json(const Outcome& outcome) {
  Json out = Json::object();
  out.set("code", Json::string(outcome.code));
  out.set("message", Json::string(outcome.message));
  return out;
}

Handle handle_from(const Json& value) {
  Handle out;
  out.stream = std::string(value["stream"].string_or(""));
  out.slot = value["slot"].number_or(-1);
  out.generation = value["generation"].number_or(-1);
  return out;
}

// ── §89 Capture placement ──────────────────────────────────────────────────
//
// Transcribed from `reference/typescript/media.ts`. `is_true()` is the reference's `=== true`,
// and a value that is not a JSON number is not a measurement — `number()` answers nothing for a
// string, a boolean or a null, which is the reference's `typeof value === 'number'`.

constexpr std::size_t kCaptureSuspendAfter = 3;
constexpr std::size_t kCaptureSuspendWindow = 8;

bool measured(const std::optional<double>& value) { return value.has_value() && *value >= 0; }

bool read_caused_miss(const Json& read, const std::optional<double>& period_ns) {
  if (!period_ns.has_value() || !(*period_ns > 0)) {
    return true;
  }
  const std::optional<double> read_ns = read["read_ns"].number();
  const std::optional<double> rest_ns = read["rest_ns"].number();
  if (!measured(read_ns) || !measured(rest_ns)) {
    return true;
  }
  return *read_ns + *rest_ns > *period_ns && *rest_ns <= *period_ns;
}

Json placement_json(const char* placement, std::size_t window, std::size_t misses,
                    const std::optional<std::size_t>& suspended_at) {
  Json out = Json::object();
  out.set("placement", Json::string(placement));
  out.set("window", Json::number(static_cast<double>(window)));
  out.set("misses", Json::number(static_cast<double>(misses)));
  out.set("suspended_at", suspended_at.has_value()
                              ? Json::number(static_cast<double>(*suspended_at))
                              : Json::null());
  return out;
}

Json capture_placement(const Json& input) {
  if (input["sensor"]["concurrent_read"].is_true()) {
    return placement_json("own_thread", 0, 0, std::nullopt);
  }
  const std::optional<double> period_ns = input["period_ns"].number();
  std::vector<bool> missed;
  for (const Json& read : input["reads"].items()) {
    missed.push_back(read_caused_miss(read, period_ns));
  }
  const auto count = [&missed](std::size_t start, std::size_t end) {
    std::size_t misses = 0;
    for (std::size_t index = start; index < end; ++index) {
      misses += missed[index] ? 1 : 0;
    }
    return misses;
  };
  for (std::size_t end = 1; end <= missed.size(); ++end) {
    const std::size_t start = end > kCaptureSuspendWindow ? end - kCaptureSuspendWindow : 0;
    const std::size_t misses = count(start, end);
    if (misses >= kCaptureSuspendAfter) {
      return placement_json("suspended", end - start, misses, end);
    }
  }
  const std::size_t start =
      missed.size() > kCaptureSuspendWindow ? missed.size() - kCaptureSuspendWindow : 0;
  return placement_json("control_thread", missed.size() - start, count(start, missed.size()),
                        std::nullopt);
}

}  // namespace

Json media(const std::string& label, const Json& input) {
  if (label.find("placement") != std::string::npos) {
    return capture_placement(input);
  }
  // A case is a script rather than one input because a pool's whole subject is what the fourth
  // publish does given what the first three did. `from` names an earlier step's handle by its index
  // in `ops`, which is how a reader actually holds one: `latest`, then `acquire` on what it named.
  // Steps that produce no handle still occupy an index.
  const std::string stream(input["stream"].string_or(""));
  if (stream.empty()) {
    throw JsonError("a frame pool is for one named stream");
  }
  const double capacity = input["capacity"].number_or(0);
  if (capacity < 1) {
    throw JsonError("a frame pool holds at least one slot");
  }
  FramePool pool(stream, static_cast<std::size_t>(capacity));

  std::vector<std::optional<Handle>> handles;
  Json steps = Json::array();

  for (const Json& op : input["ops"].items()) {
    const std::string name(op["op"].string_or(""));
    std::optional<Handle> produced;
    Json step = Json::object();
    step.set("op", Json::string(name));

    if (name == "publish") {
      Identity frame;
      frame.stream = op.has("stream") && !op["stream"].is_null()
                         ? std::string(op["stream"].string_or(""))
                         : stream;
      frame.stamp_ns = op["stamp_ns"].number_or(0);
      frame.sequence = op["sequence"].number_or(0);
      const Outcome result = pool.publish(frame);
      if (result.refused) {
        step.set("refusal", refusal_json(result));
      } else {
        step.set("handle", handle_json(result.handle));
        produced = result.handle;
      }
    } else if (name == "latest") {
      produced = pool.latest();
      step.set("handle", handle_json(produced));
    } else if (name == "acquire" || name == "release") {
      std::optional<Handle> named;
      if (op.has("handle") && !op["handle"].is_null()) {
        named = handle_from(op["handle"]);
      } else {
        const auto index = static_cast<std::size_t>(op["from"].number_or(-1));
        if (index < handles.size()) {
          named = handles[index];
        }
      }
      if (!named.has_value()) {
        throw JsonError(name + " names no handle");
      }
      const Outcome result = name == "acquire" ? pool.acquire(*named) : pool.release(*named);
      if (result.refused) {
        step.set("refusal", refusal_json(result));
      } else if (name == "acquire") {
        step.set("identity", identity_json(*result.identity));
      } else {
        step.set("released", Json::boolean(true));
      }
    } else if (name == "stats") {
      step.set("stats", pool.stats());
    } else {
      throw JsonError("unknown pool op `" + name + "`");
    }

    handles.push_back(produced);
    steps.push(step);
  }

  Json final_state = Json::object();
  final_state.set("latest", handle_json(pool.latest()));
  final_state.set("stats", pool.stats());

  Json out = Json::object();
  out.set("steps", steps);
  out.set("final", final_state);
  return out;
}

}  // namespace skynet::contracts
