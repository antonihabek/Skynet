// skynet_rhal.hpp — header-only C++ wrapper over the Skynet RHAL C ABI.
//
// C++ is the largest population in robotics: ROS 2 is C++, and every vendor SDK worth
// integrating is C++. Making them write C against a struct of function pointers would be a
// pointless tax, so this wrapper exists — and it is emphatically a *wrapper*, not a port. It
// compiles down to the same `skr_driver` vtable, and there is exactly one implementation of
// every rule underneath it.
//
// Requires C++17. Header-only, zero dependencies, no allocation on the hot path.
//
//   class MyArm : public skynet::Driver {
//     int read_state(skynet::State& s, uint64_t deadline_ns) override { ... }
//     int write_command(const skynet::Command& c, uint64_t deadline_ns) override { ... }
//     int emergency_stop() override { return cut_drive_power(); }
//     std::string describe() override { return embodiment_json_; }
//   };
//   SKYNET_RHAL_EXPORT(MyArm)

#ifndef SKYNET_RHAL_HPP
#define SKYNET_RHAL_HPP

#include "../c/skynet_rhal.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <string_view>

namespace skynet {

// Non-owning views over the caller's buffers. Deliberately not std::vector: the real-time path
// must not allocate, and a type that could would invite exactly that.
template <typename T>
class Span {
 public:
  Span() = default;
  Span(T* data, std::size_t size) : data_(data), size_(size) {}
  T* data() const { return data_; }
  std::size_t size() const { return size_; }
  bool empty() const { return size_ == 0; }
  T& operator[](std::size_t i) const { return data_[i]; }
  T* begin() const { return data_; }
  T* end() const { return data_ + size_; }

 private:
  T* data_ = nullptr;
  std::size_t size_ = 0;
};

class State {
 public:
  explicit State(skr_state* raw) : raw_(raw) {}

  void set_stamp(std::uint64_t ns) { raw_->stamp_ns = ns; }
  void set_seq(std::uint32_t seq) { raw_->seq = seq; }
  void set_flags(std::uint16_t flags) { raw_->flags = flags; }
  void set_fault(std::uint32_t code) { raw_->fault_code = code; }

  std::size_t joint_count() const { return raw_->joint_count; }
  Span<float> position() const { return {raw_->position_rad, raw_->joint_count}; }
  Span<float> velocity() const { return {raw_->velocity_rad_s, raw_->joint_count}; }
  Span<float> effort() const { return {raw_->effort_nm, raw_->joint_count}; }
  Span<float> base_pose() const { return {raw_->base_pose, 7}; }
  Span<float> base_twist() const { return {raw_->base_twist, 6}; }

 private:
  skr_state* raw_;
};

class Command {
 public:
  explicit Command(const skr_command* raw) : raw_(raw) {}

  std::uint64_t stamp_ns() const { return raw_->stamp_ns; }
  std::uint32_t seq() const { return raw_->seq; }
  // The mode of every joint that inherits (§86.1). `mode_of` is what a driver wants.
  std::uint8_t mode() const { return raw_->mode; }
  std::size_t joint_count() const { return raw_->joint_count; }
  Span<const float> values() const { return {raw_->values, raw_->joint_count}; }

  // True when this joint should be held rather than driven. Check it: applying a masked value is
  // the difference between holding position and going limp under torque control.
  bool applies(std::size_t joint) const {
    return raw_->mask == nullptr || raw_->mask[joint] != 0;
  }

  // Whether this producer wrote the per-joint mode array (§86.1). A command from a producer that
  // predates §86 does not have the field, so nothing below reads it unless this is true.
  bool has_joint_modes() const {
    return (raw_->flags & SKR_CMD_PER_JOINT_MODES) != 0 && raw_->modes != nullptr;
  }

  // The mode this joint is commanded in, inheritance resolved (§86.1).
  //
  // Returns -1 for an entry this ABI does not define, which a driver answers with SKR_EINVAL: the
  // value it is most likely to be is a bare SKR_MODE_* written into the wrong array, and guessing
  // would move a joint in the wrong units. Never silently defaults.
  int mode_of(std::size_t joint) const {
    if (!has_joint_modes()) return raw_->mode;
    const std::uint8_t entry = raw_->modes[joint];
    if (entry == SKR_JMODE_INHERIT) return raw_->mode;
    if (!SKR_JMODE_VALID(entry)) return -1;
    return SKR_MODE_OF_JMODE(entry);
  }

 private:
  const skr_command* raw_;
};

class Frame {
 public:
  explicit Frame(skr_frame* raw) : raw_(raw) {}
  void set(std::uint64_t stamp_ns, std::uint32_t w, std::uint32_t h, std::uint32_t stride,
           std::uint32_t format, const void* data, std::size_t size) {
    raw_->stamp_ns = stamp_ns; raw_->width = w; raw_->height = h;
    raw_->stride = stride; raw_->format = format; raw_->data = data; raw_->size = size;
  }

 private:
  skr_frame* raw_;
};

// Subclass this. Everything except the four pure virtuals has a default that reports the
// capability as absent, so a minimal driver is genuinely minimal.
class Driver {
 public:
  virtual ~Driver() = default;

  virtual int open(std::string_view config_json) { (void)config_json; return SKR_OK; }
  virtual int close() { return SKR_OK; }

  // Real-time path: wait-free, no allocation, respect the deadline.
  virtual int read_state(State& out, std::uint64_t deadline_ns) = 0;
  virtual int write_command(const Command& cmd, std::uint64_t deadline_ns) = 0;

  // Must be safe from a signal handler and from a dying process. Never block.
  virtual int emergency_stop() = 0;

  // The embodiment descriptor. Build it once at open() and return the cached string — this is
  // called rarely, but building JSON inside a driver that also serves the RT path invites
  // somebody to reuse the allocator on the wrong thread.
  virtual std::string describe() = 0;

  virtual int read_sensor(std::string_view name, Frame& out) {
    (void)name; (void)out; return SKR_ENOSYS;
  }
  virtual int extension(std::string_view token, const void* in, std::size_t in_len,
                        void* out, std::size_t out_len) {
    (void)token; (void)in; (void)in_len; (void)out; (void)out_len; return SKR_ENOSYS;
  }
};

namespace detail {

template <typename T>
struct Bridge {
  static T& instance() { static T driver; return driver; }

  static int open(const char* cfg, skr_ctx** out) {
    *out = reinterpret_cast<skr_ctx*>(&instance());
    return instance().open(cfg ? std::string_view{cfg} : std::string_view{});
  }
  static int close(skr_ctx*) { return instance().close(); }

  static int read_state(skr_ctx*, skr_state* out, std::uint64_t deadline) {
    State view{out};
    return instance().read_state(view, deadline);
  }
  static int write_command(skr_ctx*, const skr_command* cmd, std::uint64_t deadline) {
    const Command view{cmd};
    return instance().write_command(view, deadline);
  }
  static int emergency_stop(skr_ctx*) { return instance().emergency_stop(); }

  static int describe(skr_ctx*, char* out, std::size_t len) {
    const std::string json = instance().describe();
    // Refuse rather than truncate. A half-parsed body description is worse than a failed one.
    if (json.size() + 1 > len) return SKR_EINVAL;
    std::memcpy(out, json.data(), json.size());
    out[json.size()] = '\0';
    return static_cast<int>(json.size());
  }

  static int read_sensor(skr_ctx*, const char* name, skr_frame* out) {
    Frame view{out};
    return instance().read_sensor(name ? std::string_view{name} : std::string_view{}, view);
  }
  static int extension(skr_ctx*, const char* token, const void* in, std::size_t in_len,
                       void* out, std::size_t out_len) {
    return instance().extension(token ? std::string_view{token} : std::string_view{},
                                in, in_len, out, out_len);
  }
};

}  // namespace detail
}  // namespace skynet

// Emits the one symbol the OS looks for. Place it once, at file scope, in the driver's
// translation unit.
#define SKYNET_RHAL_EXPORT(DriverType)                                            \
  extern "C" const skr_driver* skynet_rhal_entry(void) {                          \
    using B = ::skynet::detail::Bridge<DriverType>;                               \
    static const skr_driver table = {                                             \
      SKYNET_RHAL_ABI_VERSION,                                                    \
      #DriverType,                                                                \
      "0.1.0",                                                                    \
      &B::open, &B::close,                                                        \
      &B::read_state, &B::write_command,                                          \
      &B::emergency_stop, &B::describe,                                           \
      &B::read_sensor, &B::extension,                                             \
    };                                                                            \
    return &table;                                                                \
  }

#endif  // SKYNET_RHAL_HPP
