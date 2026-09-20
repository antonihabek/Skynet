/*
 * skynet-unitree's own suite: the mode mapping, the sentinels, the CRC and every refusal.
 *
 * Linked against `unitree_bridge.cpp` rather than dlopening it, so a refusal can be asserted
 * directly through the vtable. It runs against `fake_robot` on DDS — the vendor's SDK on both
 * sides — which the caller starts; `run-check.sh` is what starts it, once per body.
 *
 * What makes this worth more than a list of return codes is the **substrate's own report**. The
 * driver's job here is not only to return SKR_OK: it is to write Unitree's sentinels where they
 * belong, a CRC the robot will accept, and nothing at all into the motor slots the body does not
 * have. Those are things only the thing on the other end can see, so the fake counts them and this
 * suite reads the count.
 *
 * The suite is templated on the message family and driven by one expectation row per body, because
 * the four bodies differ in counts and figures but not in mapping: every body gets the descriptor
 * pins, the mode mapping read off the wire, the joint-to-slot placement against the shared header,
 * the mask, the refusals and the estop. The placement check is the one that earns the shared
 * header: the suite commands distinct values per joint and asserts each landed in the slot
 * `unitree_models.hpp` names, so a driver map disagreeing with the wire truth fails here rather
 * than silently driving the wrong joint.
 */

#include "skynet_rhal.h"
#include "unitree_models.hpp"

#include <unitree/idl/go2/LowCmd_.hpp>
#include <unitree/idl/go2/LowState_.hpp>
#include <unitree/idl/hg/LowCmd_.hpp>
#include <unitree/idl/hg/LowState_.hpp>
#include <unitree/robot/channel/channel_subscriber.hpp>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

constexpr float POS_STOP_F = 2.146e9f;
constexpr float VEL_STOP_F = 16000.0f;

int CHECKS = 0;
std::vector<std::string> FAILURES;

void check(bool held, const std::string &what) {
  CHECKS += 1;
  if (held) {
    std::printf("  ok    %s\n", what.c_str());
  } else {
    FAILURES.push_back(what);
    std::printf("  FAIL  %s\n", what.c_str());
  }
}

void section(const char *title) { std::printf("\n%s\n", title); }

/* A listener on a topic, so the suite can read the exact message the driver published — or the
 * exact state the fake publishes — rather than infer it from a return code. This is the other half
 * of what `fake_robot` counts: the fake says *whether* something was wrong, and this says *what
 * was in the message*. */
template <typename Msg>
class Spy {
 public:
  Spy(const std::string &interface, const std::string &topic) {
    unitree::robot::ChannelFactory::Instance()->Init(0, interface);
    subscriber_.reset(new unitree::robot::ChannelSubscriber<Msg>(topic));
    subscriber_->InitChannel(
        [this](const void *message) {
          std::lock_guard<std::mutex> guard(lock_);
          last_ = *static_cast<const Msg *>(message);
          seen_ += 1;
        },
        1);
  }

  bool wait_for(std::uint64_t count, double seconds = 2.0) {
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::duration<double>(seconds);
    while (std::chrono::steady_clock::now() < deadline) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (seen_ >= count) return true;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    return false;
  }

  Msg last() {
    std::lock_guard<std::mutex> guard(lock_);
    return last_;
  }

  std::uint64_t seen() {
    std::lock_guard<std::mutex> guard(lock_);
    return seen_;
  }

 private:
  std::mutex lock_;
  std::uint64_t seen_ = 0;
  Msg last_{};
  unitree::robot::ChannelSubscriberPtr<Msg> subscriber_;
};

std::uint32_t crc32_core(const std::uint32_t *ptr, std::uint32_t len) {
  std::uint32_t crc = 0xFFFFFFFFu;
  const std::uint32_t polynomial = 0x04C11DB7u;
  for (std::uint32_t i = 0; i < len; i++) {
    std::uint32_t xbit = 1u << 31;
    const std::uint32_t data = ptr[i];
    for (std::uint32_t bits = 0; bits < 32; bits++) {
      if (crc & 0x80000000u) {
        crc <<= 1;
        crc ^= polynomial;
      } else {
        crc <<= 1;
      }
      if (data & xbit) crc ^= polynomial;
      xbit >>= 1;
    }
  }
  return crc;
}

struct Buffers {
  float position[skynet_unitree::MAX_JOINTS * 4];
  float velocity[skynet_unitree::MAX_JOINTS * 4];
  float effort[skynet_unitree::MAX_JOINTS * 4];

  skr_state state() {
    skr_state out;
    std::memset(&out, 0, sizeof out);
    // The caller's capacity, exactly as the runtime passes it (F45).
    out.joint_count = static_cast<std::uint16_t>(skynet_unitree::MAX_JOINTS * 4);
    out.position_rad = position;
    out.velocity_rad_s = velocity;
    out.effort_nm = effort;
    return out;
  }
};

skr_command command(std::uint8_t mode, const float *values, const std::uint8_t *mask,
                    std::uint16_t joints) {
  skr_command cmd;
  std::memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = mode;
  cmd.joint_count = joints;
  cmd.values = values;
  cmd.mask = mask;
  return cmd;
}

/* One row per body: the figures the suite pins, and only those. Everything else the descriptor
 * carries — names, roles, the per-joint travel table — is the driver's transcription to state, and
 * pinning all of it here would be pinning the transcription against itself. */
struct Expect {
  const char *model;
  const char *slug;
  const char *dof;
  const char *torque;
  const char *rate;
  const char *travel;
  const char *effort_pins[3];
  int effort_pin_count;
  const char *effort_omitted[2];
  int effort_omitted_count;
};

const Expect EXPECTS[4] = {
    {
        "go2", "unitree-go2", "\"dof\":12", "\"joint_torque_max_nm\":45.0", "\"rate_hz_max\":500",
        "\"position_min_rad\":-2.72270", {"\"effort_max_nm\":45.0", nullptr, nullptr}, 1,
        {nullptr, nullptr}, 0,
    },
    {
        "h1", "unitree-h1", "\"dof\":19", "\"joint_torque_max_nm\":360.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-3.14000",
        {"\"effort_max_nm\":360.0", "\"effort_max_nm\":220.0", "\"effort_max_nm\":75.0"}, 3,
        {"waist_yaw", nullptr}, 1,
    },
    {
        "g1", "unitree-g1", "\"dof\":23", "\"joint_torque_max_nm\":120.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-2.53070",
        {"\"effort_max_nm\":120.0", nullptr, nullptr}, 1,
        {nullptr, nullptr}, 0,
    },
    {
        "h1_2", "unitree-h1-2", "\"dof\":27", "\"joint_torque_max_nm\":360.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-3.14000",
        {"\"effort_max_nm\":360.0", "\"effort_max_nm\":220.0", "\"effort_max_nm\":75.0"}, 3,
        {"waist_yaw", "L_wrist_pitch"}, 2,
    },
};

const Expect *find_expect(const std::string &model) {
  for (const Expect &row : EXPECTS) {
    if (model == row.model) return &row;
  }
  return nullptr;
}

/* The family header: the `go2` message carries Unitree's header and low-level flag, the `hg`
 * message carries the series-control mode instead. Overloads, because one template covers both
 * families and the members do not exist on both. */
bool header_ok(const unitree_go::msg::dds_::LowCmd_ &sent) {
  return sent.head()[0] == 0xFE && sent.head()[1] == 0xEF && sent.level_flag() == 0xFF;
}

bool header_ok(const unitree_hg::msg::dds_::LowCmd_ &sent) { return sent.mode_pr() == 0x00; }

const char *header_words(bool is_hg) {
  return is_hg ? "series joint control, the vendor example's default"
               : "the message carries Unitree's header and the low-level flag";
}

template <typename Cmd, typename State, bool IsHg>
int run_model(const Expect *expect, const skynet_unitree::WireModel *wire,
              const std::string &interface) {
  const skr_driver *driver = skynet_rhal_entry();
  const int joints = wire->joints;
  const std::size_t slots = skynet_unitree::family_slots(wire->family);
  Spy<Cmd> spy(interface, "rt/lowcmd");
  Spy<State> state_spy(interface, "rt/lowstate");

  section("the body, and what it refuses to declare");
  skr_ctx *ctx = nullptr;
  const std::string with_gains =
      "{\"interface\":\"" + interface + "\",\"model\":\"" + expect->model + "\",\"kp\":60.0,\"kd\":5.0}";
  check(driver->abi_version == SKYNET_RHAL_ABI_VERSION, "the driver targets this ABI");
  check(driver->open(with_gains.c_str(), &ctx) == SKR_OK,
        "open() joins DDS and waits for the robot's first state");
  if (ctx == nullptr) {
    std::printf("\n  the fake robot is not publishing; nothing below can run.\n");
    return 1;
  }

  char json[16384];
  const int written = driver->describe(ctx, json, sizeof json);
  check(written > 0, "describe() writes the descriptor");
  const std::string descriptor(json, written > 0 ? written : 0);
  check(descriptor.find(std::string("\"robot_slug\":\"") + expect->slug) != std::string::npos,
        std::string("it names the body: ") + expect->slug);
  check(descriptor.find(expect->dof) != std::string::npos,
        "the joint count — not the slots the message carries");
  check(descriptor.find(expect->torque) != std::string::npos,
        "the envelope carries the largest motor's peak, with the footnote beside it");
  check(descriptor.find("\"watchdog_ms\":20.0") != std::string::npos,
        "the watchdog is ten DDS periods");
  check(descriptor.find("\"basis\":\"declared\"") != std::string::npos,
        "and states the kind beside the citation (§5.3)");
  check(descriptor.find(expect->rate) != std::string::npos,
        IsHg || std::string(expect->model) == "h1"
            ? "no control rate: unpublished for the humanoids, and the community's 500 Hz "
              "convention is not a vendor publication (§63.4)"
            : "the documented 500 Hz low-level loop");
  check(descriptor.find("joint_velocity_max_rad_s") == std::string::npos,
        "but no velocity ceiling: Unitree publishes no joint speed, and the sim configs that "
        "carry one are CAD values (§63.4)");
  for (int i = 0; i < expect->effort_pin_count; ++i) {
    check(descriptor.find(expect->effort_pins[i]) != std::string::npos,
          std::string("a published rating reaches the joints: ") + expect->effort_pins[i]);
  }
  for (int i = 0; i < expect->effort_omitted_count; ++i) {
    /* The joint's own `limits` block and nothing past it: the window ends at the block's closing
     * brace, because the next joint's rating sitting inside a wider window would fail a joint
     * that correctly declares nothing. */
    const std::string name = std::string("\"name\":\"") + expect->effort_omitted[i] + "\"";
    const std::size_t at = descriptor.find(name);
    const std::size_t limits =
        at == std::string::npos ? std::string::npos : descriptor.find("\"limits\":{", at);
    const std::size_t end =
        limits == std::string::npos ? std::string::npos : descriptor.find('}', limits);
    bool omitted = end != std::string::npos &&
                   descriptor.substr(limits, end - limits).find("effort_max_nm") ==
                       std::string::npos;
    check(omitted, std::string("and the unpublished joint stays undeclared: ") +
                       expect->effort_omitted[i]);
  }
  check(descriptor.find(expect->travel) != std::string::npos,
        "travel comes from the vendor's model, pinned on one joint");
  check(descriptor.find("\"capabilities\":[\"safety.estop\",\"platform.onboard_compute\"]") !=
            std::string::npos,
        "the zero-torque stop the driver has is a capability it declares, beside the cited "
        "onboard CPU");
  check(descriptor.find("\"simulator\":true") != std::string::npos,
        "declared simulated: the default is the safe direction (§35.3)");
  check(descriptor.find("\"base\":\"legged\"") != std::string::npos,
        "the base is legged, so nothing treats it as a wheeled one");

  section("read_state");
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check(state.joint_count == static_cast<std::uint32_t>(joints),
        "and reports the joints written, not the capacity it was handed");
  const std::uint32_t first_seq = state.seq;
  bool finite = true;
  for (int i = 0; i < joints; ++i) {
    if (!std::isfinite(state.position_rad[i])) finite = false;
  }
  check(finite, "every joint reports a finite position");
  bool pose_nan = true;
  for (std::size_t i = 0; i < 7; ++i) {
    if (!std::isnan(state.base_pose[i])) pose_nan = false;
  }
  check(pose_nan,
        "the base pose is all NaN — nothing in LowState_ localises the body, and a driver that "
        "wrote the IMU quaternion into a pose would be claiming a position it does not have");
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check(state.seq > first_seq, "the sequence advances between reads");

  section("the mode mapping, read off the wire");
  float values[skynet_unitree::MAX_JOINTS];
  std::uint8_t mask[skynet_unitree::MAX_JOINTS];
  for (int i = 0; i < joints; ++i) {
    /* Distinct per joint, so the placement check below can tell joint 3's setpoint from joint
     * 4's: identical values would pass while driving the wrong joints. */
    values[i] = 0.2f + static_cast<float>(i) * 0.01f;
    mask[i] = 1;
  }
  std::uint64_t before = spy.seen();
  skr_command cmd = command(SKR_MODE_POSITION, values, mask, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a well-formed position command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  auto sent = spy.last();
  check(header_ok(sent), header_words(IsHg));
  const std::size_t slot0 = wire->slots[0];
  check(std::fabs(sent.motor_cmd()[slot0].q() - values[0]) < 1e-6f, "the setpoint is in `q`");
  check(sent.motor_cmd()[slot0].kp() > 0.0f, "with the operator's `kp`, not a default");
  check(sent.motor_cmd()[slot0].kd() > 0.0f, "and the operator's `kd`");
  check(sent.crc() == crc32_core(reinterpret_cast<const std::uint32_t *>(&sent),
                                 (sizeof sent >> 2) - 1),
        "the CRC is the one Unitree computes — a wrong one is dropped silently by the robot");
  if constexpr (IsHg) {
    check(state_spy.wait_for(1), "the fake's state is on the wire to echo from");
    check(sent.mode_machine() == state_spy.last().mode_machine(),
          "and the machine code is echoed from the last state, not a constant");
  }
  bool placed = true;
  for (int i = 0; i < joints; ++i) {
    if (std::fabs(sent.motor_cmd()[wire->slots[i]].q() - values[i]) >= 1e-6f) placed = false;
  }
  check(placed,
        "every joint's setpoint landed in the slot the shared header names — a map disagreeing "
        "with the wire truth fails here");
  bool slots_idle = true;
  for (std::size_t i = 0; i < slots; ++i) {
    bool used = false;
    for (int j = 0; j < joints; ++j) {
      if (wire->slots[j] == i) used = true;
    }
    if (used) continue;
    if (sent.motor_cmd()[i].q() != POS_STOP_F || sent.motor_cmd()[i].kp() != 0.0f ||
        sent.motor_cmd()[i].tau() != 0.0f) {
      slots_idle = false;
    }
  }
  check(slots_idle,
        "the motor slots the body does not have carry the sentinels and no gain — a zero there "
        "is a command to a joint that is not present, and on a bigger body it is a motion");

  before = spy.seen();
  cmd = command(SKR_MODE_TORQUE, values, mask, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a torque command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[slot0].tau() - values[0]) < 1e-6f, "the setpoint is in `tau`");
  check(sent.motor_cmd()[slot0].q() == POS_STOP_F,
        "`q` carries PosStopF and not 0.0 — this is the defect the row exists to catch: a zero "
        "there is a command to go to zero radians");
  check(sent.motor_cmd()[slot0].dq() == VEL_STOP_F, "`dq` carries VelStopF for the same reason");
  check(sent.motor_cmd()[slot0].kp() == 0.0f && sent.motor_cmd()[slot0].kd() == 0.0f,
        "and both gains are zero, so the drive follows the torque and nothing else");

  section("§24's mask, on an impedance drive");
  std::uint8_t partial[skynet_unitree::MAX_JOINTS];
  for (int i = 0; i < joints; ++i) partial[i] = (i % 2 == 0) ? 1 : 0;
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, partial, static_cast<std::uint16_t>(joints));
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[slot0].q() - values[0]) < 1e-6f,
        "an unmasked joint carries the setpoint");
  const std::size_t slot1 = wire->slots[1];
  check(sent.motor_cmd()[slot1].q() == POS_STOP_F && sent.motor_cmd()[slot1].kp() == 0.0f,
        "a masked joint carries the sentinel and no gain — §24's 'say nothing', which on an "
        "impedance drive is the only way to say it");

  section("refusals");
  cmd = command(SKR_MODE_POSITION, values, mask, 7);
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a seven-joint command to a body with more joints is refused");
  cmd = command(200, values, mask, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_EINVAL,
        "a mode this ABI does not define is malformed, not merely refused");

  section("emergency_stop (§24's second rule)");
  before = spy.seen();
  check(driver->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
  check(spy.wait_for(before + 1), "and publishes one message");
  sent = spy.last();
  bool everything_idle = true;
  for (std::size_t i = 0; i < slots; ++i) {
    if (sent.motor_cmd()[i].kp() != 0.0f || sent.motor_cmd()[i].kd() != 0.0f ||
        sent.motor_cmd()[i].tau() != 0.0f || sent.motor_cmd()[i].q() != POS_STOP_F) {
      everything_idle = false;
    }
  }
  check(everything_idle,
        "every gain, every torque and every setpoint is idle — which on an impedance drive is the "
        "definition of commanding nothing");
  cmd = command(SKR_MODE_POSITION, values, mask, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_EPERM,
        "commands are refused while the latch is set");
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  check((state.flags & SKR_FLAG_ESTOP) != 0, "and the stop is visible in the next state");

  driver->close(ctx);

  section("§63.4 through a gain: a driver with no gains configured");
  const std::string no_gains =
      "{\"interface\":\"" + interface + "\",\"model\":\"" + expect->model + "\"}";
  skr_ctx *bare = nullptr;
  check(driver->open(no_gains.c_str(), &bare) == SKR_OK, "opens without `kp` and `kd`");
  if (bare != nullptr) {
    cmd = command(SKR_MODE_POSITION, values, mask, static_cast<std::uint16_t>(joints));
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and refuses a position command, because a stiffness is the operator's to choose and "
          "not this driver's to invent");
    cmd = command(SKR_MODE_TORQUE, values, mask, static_cast<std::uint16_t>(joints));
    check(driver->write_command(bare, &cmd, 0) == SKR_OK,
          "…while a torque command needs no gain and is accepted");
    driver->close(bare);
  }

  section("the flag follows the substrate, declared by the operator");
  const std::string declared_real =
      "{\"interface\":\"" + interface + "\",\"model\":\"" + expect->model + "\",\"simulator\":false}";
  skr_ctx *real = nullptr;
  check(driver->open(declared_real.c_str(), &real) == SKR_OK, "a declared-real session opens");
  if (real != nullptr) {
    char real_json[16384];
    const int real_written = driver->describe(real, real_json, sizeof real_json);
    const std::string real_descriptor(real_json, real_written > 0 ? real_written : 0);
    check(real_descriptor.find("\"simulator\":false") != std::string::npos,
          "and declares real when the operator says so");
    driver->close(real);
  }
  return 0;
}

}  // namespace

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
  const std::string model = argc > 2 ? argv[2] : "go2";

  const skynet_unitree::WireModel *wire = skynet_unitree::find_wire_model(model);
  const Expect *expect = find_expect(model);
  if (wire == nullptr || expect == nullptr) {
    std::fprintf(stderr, "selftest: unknown model \"%s\" (go2, h1, g1, h1_2)\n", model.c_str());
    return 2;
  }

  /* No DDS needed: an unknown model is refused before discovery is touched. */
  {
    const skr_driver *driver = skynet_rhal_entry();
    skr_ctx *ctx = nullptr;
    check(driver->open("{\"model\":\"nope\"}", &ctx) == SKR_EINVAL,
          "an unknown model is refused before DDS is touched");
  }

  std::printf("\n== model %s: %d joints on %s ==\n", model.c_str(), wire->joints,
              wire->family == skynet_unitree::Family::GO2 ? "the go2 message" : "the hg message");
  int result = 0;
  if (model == "go2" || model == "h1") {
    result = run_model<unitree_go::msg::dds_::LowCmd_, unitree_go::msg::dds_::LowState_, false>(
        expect, wire, interface);
  } else {
    result = run_model<unitree_hg::msg::dds_::LowCmd_, unitree_hg::msg::dds_::LowState_, true>(
        expect, wire, interface);
  }
  if (result != 0) return result;

  std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
  for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
  return FAILURES.empty() ? 0 : 1;
}
