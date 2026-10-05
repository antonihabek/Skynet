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
 * the seven bodies differ in counts and figures but not in mapping: every body gets the descriptor
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

#include <signal.h>
#include <sys/types.h>
#include <time.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
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

/* The robot's process, which `run-check.sh` passes in `FAKE_PID`: the suite stops it (SIGSTOP) to give
 * the driver a robot that has gone quiet while its process, and the discovery that matched the two,
 * stay exactly where they were, and starts it again (SIGCONT). Zero when it was not given, as when
 * the suite is run by hand. */
pid_t robot_pid() {
  const char *text = std::getenv("FAKE_PID");
  return text == nullptr ? 0 : static_cast<pid_t>(std::atol(text));
}

/* A robot that is stopped is started again however the section that stopped it ends: a process left
 * in SIGSTOP holds a SIGTERM pending, and `run-check.sh` waits for it. */
struct Stopped {
  pid_t pid;
  explicit Stopped(pid_t robot) : pid(robot) { ::kill(pid, SIGSTOP); }
  ~Stopped() { ::kill(pid, SIGCONT); }
};

/* CLOCK_MONOTONIC_RAW, the clock a state's stamp is taken from (§24). */
std::uint64_t monotonic_raw_ns() {
  timespec now;
  ::clock_gettime(CLOCK_MONOTONIC_RAW, &now);
  return static_cast<std::uint64_t>(now.tv_sec) * 1000000000ull + static_cast<std::uint64_t>(now.tv_nsec);
}

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
  /* The maker's own words, where the envelope's source quotes them, as they must read in the
   * descriptor: JSON-escaped. Null where the source quotes nothing. */
  const char *source_quote;
  /* The joint whose own limits block must carry `travel`, by the driver's name for it; null pins the
   * figure anywhere in the descriptor. A bare substring is satisfied by any joint that shares the
   * number — the H2's right hip pitch has the left's travel — so a row says which joint it means. */
  const char *travel_joint;
  /* A joint whose travel the maker did not publish, which must carry no `position_min_rad` at all
   * (the R1's head); null where every joint has one. */
  const char *travel_omitted;
};

const Expect EXPECTS[7] = {
    {
        "go2", "unitree-go2", "\"dof\":12", "\"joint_torque_max_nm\":45.0", "\"rate_hz_max\":500",
        "\"position_min_rad\":-2.72270", {"\"effort_max_nm\":45.0", nullptr, nullptr}, 1,
        {nullptr, nullptr}, 0, nullptr, "FR_calf", nullptr,
    },
    {
        /* The Go2's wire and a different machine: the calf's travel is -2.82 where the Go2's is
         * -2.7227, the rating is the B2 page's 360 and not the Go2's 45, and no rate is published. */
        "b2", "unitree-b2", "\"dof\":12", "\"joint_torque_max_nm\":360.0", "\"rate_hz_max\":null",
        "\"position_min_rad\":-2.82000", {"\"effort_max_nm\":360.0", nullptr, nullptr}, 1,
        {nullptr, nullptr}, 0, "\\\"About 360N.m\\\"", "FR_calf", nullptr,
    },
    {
        "h1", "unitree-h1", "\"dof\":19", "\"joint_torque_max_nm\":360.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-3.14000",
        {"\"effort_max_nm\":360.0", "\"effort_max_nm\":220.0", "\"effort_max_nm\":75.0"}, 3,
        {"waist_yaw", nullptr}, 1, nullptr, "L_hip_pitch", nullptr,
    },
    {
        "g1", "unitree-g1", "\"dof\":23", "\"joint_torque_max_nm\":120.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-2.53070",
        {"\"effort_max_nm\":120.0", nullptr, nullptr}, 1,
        {nullptr, nullptr}, 0, nullptr, "L_hip_pitch", nullptr,
    },
    {
        "h1_2", "unitree-h1-2", "\"dof\":27", "\"joint_torque_max_nm\":360.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-3.14000",
        {"\"effort_max_nm\":360.0", "\"effort_max_nm\":220.0", "\"effort_max_nm\":75.0"}, 3,
        {"waist_yaw", "L_wrist_pitch"}, 2, nullptr, "L_hip_pitch", nullptr,
    },
    {
        /* Thirty-one joints on the humanoid message. The figures are the page's two group figures, 360
         * on the hips and knees and 120 on the shoulders and elbows, and the ankles and the head
         * carry none; the travel pin is the left hip's pitch, which no other body's table has. */
        "h2", "unitree-h2", "\"dof\":31", "\"joint_torque_max_nm\":360.0",
        "\"rate_hz_max\":null", "\"position_min_rad\":-2.45260",
        {"\"effort_max_nm\":360.0", "\"effort_max_nm\":120.0", nullptr}, 2,
        {"L_ankle_roll", "head_yaw"}, 2,
        "\\\"Maximum Torque of Leg Joint [1]: 360N.m\\\"", "L_hip_pitch", nullptr,
    },
    {
        /* The standard R1: twenty-six motors in sparse slots, and three things the maker did not
         * publish and the descriptor must not invent — a peak torque (the page's arm line is a load,
         * "About 2kg"), any rating on any joint, and the head's travel. */
        "r1", "unitree-r1", "\"dof\":26", "\"joint_torque_max_nm\":null",
        "\"rate_hz_max\":null", "\"position_min_rad\":-2.93215",
        {nullptr, nullptr, nullptr}, 0,
        {"L_hip_pitch", "head_yaw"}, 2,
        "\\\"About 2kg\\\"", "L_hip_pitch", "head_pitch",
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
  /* Never destroyed, for the driver's own reason (`Link` in `unitree_bridge.cpp`, F122): a spy
   * torn down while the fake still publishes can take CycloneDDS's self-deadlock abort. */
  Spy<Cmd> &spy = *new Spy<Cmd>(interface, "rt/lowcmd");
  Spy<State> &state_spy = *new Spy<State>(interface, "rt/lowstate");

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
        "the envelope's peak torque is what the maker published, or null when it published none, "
        "with the source beside it");
  check(descriptor.find("\"watchdog_ms\":20.0") != std::string::npos,
        "the watchdog is ten DDS periods");
  check(descriptor.find("\"basis\":\"declared\"") != std::string::npos,
        "and states the kind beside the citation (§5.3)");
  check(descriptor.find(expect->rate) != std::string::npos,
        std::string(expect->rate).find("null") != std::string::npos
            ? "no control rate: unpublished for the humanoids and the B2, and the community's "
              "500 Hz convention is not a vendor publication (§63.4)"
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
  if (expect->travel_joint == nullptr) {
    check(descriptor.find(expect->travel) != std::string::npos,
          "travel comes from the vendor's model, pinned on one joint");
  } else {
    /* The joint's own `limits` block and nothing past it, as the omitted-rating check below reads it. */
    const std::string name = std::string("\"name\":\"") + expect->travel_joint + "\"";
    const std::size_t at = descriptor.find(name);
    const std::size_t limits =
        at == std::string::npos ? std::string::npos : descriptor.find("\"limits\":{", at);
    const std::size_t end =
        limits == std::string::npos ? std::string::npos : descriptor.find('}', limits);
    check(end != std::string::npos &&
              descriptor.substr(limits, end - limits).find(expect->travel) != std::string::npos,
          std::string("travel comes from the vendor's model, pinned inside ") +
              expect->travel_joint + "'s own limits");
  }
  if (expect->travel_omitted != nullptr) {
    const std::string name = std::string("\"name\":\"") + expect->travel_omitted + "\"";
    const std::size_t at = descriptor.find(name);
    const std::size_t limits =
        at == std::string::npos ? std::string::npos : descriptor.find("\"limits\":{", at);
    const std::size_t end =
        limits == std::string::npos ? std::string::npos : descriptor.find('}', limits);
    check(end != std::string::npos &&
              descriptor.substr(limits, end - limits).find("position_m") == std::string::npos,
          std::string(expect->travel_omitted) +
              " has no published travel, so its limits carry no position bound at all");
  }
  if (expect->source_quote != nullptr) {
    check(descriptor.find(expect->source_quote) != std::string::npos,
          "the maker's words in the envelope's source are quoted and escaped, so the descriptor "
          "stays JSON (the first B2 descriptor was not)");
  }
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
  check(state.fault_code == 0 && (state.flags & SKR_FLAG_DEGRADED) == 0,
        "a robot that reports nothing wrong reads healthy: a zero fault word and no DEGRADED");
  check(state.joint_count == static_cast<std::uint32_t>(joints),
        "and reports the joints written, not the capacity it was handed");
  const std::uint32_t first_seq = state.seq;
  bool finite = true;
  for (int i = 0; i < joints; ++i) {
    if (!std::isfinite(state.position_rad[i])) finite = false;
  }
  check(finite, "every joint reports a finite position");
  {
    /* The fake's own published numbers are the oracle: what the robot said, slot by slot, held
     * against what the driver reported for each joint. The fake publishes a different non-zero angle
     * for every joint and a fixed offset on its velocity and torque estimates (`fake_robot.cpp`), so
     * a sign, a neighbour's slot or the wrong field would each change a number that is neither zero
     * nor shared with another joint. */
    check(state_spy.wait_for(1), "the fake's state is on the wire to compare the reading with");
    const auto published = state_spy.last();
    bool nonzero = true;
    bool positions = true;
    bool velocities = true;
    bool efforts = true;
    for (int i = 0; i < joints; ++i) {
      const auto &motor = published.motor_state()[wire->slots[i]];
      if (std::fabs(motor.q()) < 1e-3f || std::fabs(motor.dq()) < 1e-3f ||
          std::fabs(motor.tau_est()) < 1e-3f) {
        nonzero = false;
      }
      if (std::fabs(state.position_rad[i] - motor.q()) > 5e-3f) positions = false;
      if (std::fabs(state.velocity_rad_s[i] - motor.dq()) > 5e-3f) velocities = false;
      if (std::fabs(state.effort_nm[i] - motor.tau_est()) > 5e-3f) efforts = false;
    }
    check(nonzero, "(and every number it publishes is non-zero, so a sign has something to flip)");
    check(positions, "each joint's position is the one the robot published in that joint's slot");
    check(velocities, "each joint's velocity is its own, with the sign the robot reports");
    check(efforts, "and each joint's torque estimate, likewise");
  }
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

  /* DRY-COVERAGE-PLAN.md F277. The robot publishes on a clock of its own, so the state a read returns is a
   * sample with an arrival time, and what the read says about it has to be the sample's: the stamp is when it
   * arrived (§24: at acquisition, never at return), and the sequence counts samples and not calls, so that a
   * robot that has gone quiet is a sequence that stands still (§24: a repeat is a stalled producer). The link
   * chaos contract found the driver stamping every read with the time of the read and advancing the sequence
   * with every call, so a body whose link had been cut read as fresh for ever. */
  section("the robot's own clock: the stamp is the sample's and the sequence counts samples");
  {
    state = buffers.state();
    driver->read_state(ctx, &state, 0);
    const std::uint32_t seq_start = state.seq;
    const std::uint64_t seen_start = state_spy.seen();
    const auto loop_began = std::chrono::steady_clock::now();
    for (int i = 0; i < 25; ++i) {
      state = buffers.state();
      driver->read_state(ctx, &state, 0);
    }
    const auto loop_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                             std::chrono::steady_clock::now() - loop_began).count();
    const std::uint32_t seq_gain = state.seq - seq_start;
    const std::uint64_t seen_gain = state_spy.seen() - seen_start;
    check(loop_ms < 300,
          "a read waits for the robot's next state and wakes when it arrives, two milliseconds away, and not for "
          "the watchdog window: twenty-five reads took " + std::to_string(loop_ms) + " ms (a read that slept out "
          "its whole window would have taken half a second)");
    check(seq_gain >= 10 && seq_gain <= seen_gain + 3,
          "while the robot publishes, the sequence counts its states and not the driver's calls: it went up by " +
              std::to_string(seq_gain) + " while " + std::to_string(seen_gain) +
              " states went by (twenty-five calls that each advanced it would have made twenty-five in microseconds)");
    check(monotonic_raw_ns() - state.stamp_ns < 100000000ull,
          "and the stamp of a state that has just arrived is recent");

    const pid_t robot = robot_pid();
    if (robot <= 0) {
      std::printf("  SKIPPED  the robot's pid is not in FAKE_PID, so it cannot be made to go quiet (run-check.sh gives it)\n");
    } else {
      std::uint32_t last_seq = 0;
      std::uint64_t last_stamp = 0;
      {
        Stopped quiet(robot);
        /* What was in flight arrives, and then nothing does. */
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
        state = buffers.state();
        driver->read_state(ctx, &state, 0);
        last_seq = state.seq;
        last_stamp = state.stamp_ns;

        state = buffers.state();
        const auto began = std::chrono::steady_clock::now();
        const int rc_first = driver->read_state(ctx, &state, 0);
        const auto took_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                                 std::chrono::steady_clock::now() - began).count();
        const std::uint32_t first_seq_quiet = state.seq;
        const std::uint64_t first_stamp_quiet = state.stamp_ns;
        std::this_thread::sleep_for(std::chrono::milliseconds(60));
        state = buffers.state();
        const int rc_second = driver->read_state(ctx, &state, 0);
        const std::uint64_t age_ns = monotonic_raw_ns() - state.stamp_ns;

        check(rc_first == SKR_OK && rc_second == SKR_OK,
              "a robot that has gone quiet is still read — the last state it sent — and says so by the sequence, "
              "not by refusing");
        check(first_seq_quiet == last_seq && state.seq == last_seq,
              "the sequence stands still while it is quiet: that is §24's stalled producer, and the one thing "
              "a state stream that stopped can say");
        check(first_stamp_quiet == last_stamp && state.stamp_ns == last_stamp,
              "and so does the stamp, which is the state's arrival");
        check(age_ns >= 150000000ull,
              "so a robot quiet for the better part of two hundred milliseconds hands back a state that old, and "
              "not one stamped with the time of the read (it was " + std::to_string(age_ns / 1000000ull) + " ms)");
        check(took_ms >= 15 && took_ms <= 150,
              "a read that found nothing new waited for the watchdog window, and for no more than a tenth of a "
              "second, before it called the robot quiet (" + std::to_string(took_ms) + " ms)");
      }
      bool advanced = false;
      for (int i = 0; i < 100 && !advanced; ++i) {
        state = buffers.state();
        driver->read_state(ctx, &state, 0);
        advanced = state.seq > last_seq;
      }
      check(advanced, "and when the robot publishes again the sequence advances again");
      check(state.stamp_ns > last_stamp && monotonic_raw_ns() - state.stamp_ns < 100000000ull,
            "with a stamp that is recent again");
    }
  }

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
  bool servo_mode = true;
  for (std::size_t i = 0; i < slots; ++i) {
    if (sent.motor_cmd()[i].mode() != 1) servo_mode = false;
  }
  check(servo_mode,
        "and the PMSM servo mode on every slot: mode 0 is idle, and a drive told to be idle is "
        "unpowered whatever else the command says");

  before = spy.seen();
  cmd = command(SKR_MODE_VELOCITY, values, mask, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a velocity command is sent");
  check(spy.wait_for(before + 1), "and it reached the wire");
  sent = spy.last();
  bool velocity_placed = true;
  for (int i = 0; i < joints; ++i) {
    const auto &motor = sent.motor_cmd()[wire->slots[i]];
    if (std::fabs(motor.dq() - values[i]) >= 1e-6f || motor.q() != POS_STOP_F ||
        motor.kp() != 0.0f || motor.kd() != 5.0f || motor.tau() != 0.0f) {
      velocity_placed = false;
    }
  }
  check(velocity_placed,
        "every joint's rate lands in its own slot's `dq`, with its sign, the operator's `kd` "
        "behind it, no stiffness, and `PosStopF` in `q` — nothing about position, which with a "
        "gain would be a pull toward the sentinel");

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

  /* §24: "0 = hold this joint at its current setpoint". The last command above was torque, which
   * leaves no position setpoint, so the first hold latches where the joint is measured to be. Until
   * 2026-09-24 this section asserted the opposite — sentinels and no gain, a free joint — and
   * unitree_mujoco measured what that does to a standing Go2 (DRY-COVERAGE-PLAN.md F121). */
  section("§24's mask, on an impedance drive: a masked joint is held");
  std::uint8_t partial[skynet_unitree::MAX_JOINTS];
  for (int i = 0; i < joints; ++i) partial[i] = (i % 2 == 0) ? 1 : 0;
  state = buffers.state();
  driver->read_state(ctx, &state, 0);
  const float measured1 = state.position_rad[1];
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, values, partial, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a partly masked position command is sent");
  spy.wait_for(before + 1);
  sent = spy.last();
  check(std::fabs(sent.motor_cmd()[slot0].q() - values[0]) < 1e-6f,
        "an unmasked joint carries the setpoint");
  const std::size_t slot1 = wire->slots[1];
  const float latched = sent.motor_cmd()[slot1].q();
  check(latched != POS_STOP_F && std::fabs(latched - measured1) < 0.1f,
        "a masked joint with no position setpoint is held where it was measured — not the "
        "sentinel, which with zero gains is a free joint and on a standing body a collapse");
  check(sent.motor_cmd()[slot1].kp() == 60.0f && sent.motor_cmd()[slot1].kd() == 5.0f,
        "with the operator's `kp` and `kd`, because on this drive a hold is a stiffness");
  check(sent.motor_cmd()[slot1].dq() == 0.0f && sent.motor_cmd()[slot1].tau() == 0.0f,
        "a zero velocity setpoint and no feed-forward torque");

  before = spy.seen();
  driver->write_command(ctx, &cmd, 0);
  spy.wait_for(before + 1);
  check(spy.last().motor_cmd()[slot1].q() == latched,
        "the hold target is latched, not re-read: a target that followed the measurement every "
        "cycle would set the stiffness term to zero and hold nothing");

  for (int i = 0; i < joints; ++i) values[i] = 0.3f + static_cast<float>(i) * 0.01f;
  cmd = command(SKR_MODE_POSITION, values, mask, static_cast<std::uint16_t>(joints));
  before = spy.seen();
  driver->write_command(ctx, &cmd, 0);
  /* Waited for, so the hold frame below is the one the spy reads next — and so nothing of this
   * section is still in flight when the emergency stop section counts its one message. */
  spy.wait_for(before + 1);
  std::uint8_t none[skynet_unitree::MAX_JOINTS] = {0};
  float zeros[skynet_unitree::MAX_JOINTS] = {0.0f};
  before = spy.seen();
  cmd = command(SKR_MODE_POSITION, zeros, none, static_cast<std::uint16_t>(joints));
  check(driver->write_command(ctx, &cmd, 0) == SKR_OK,
        "the kernel's hold frame — every joint masked, values irrelevant — is accepted");
  spy.wait_for(before + 1);
  sent = spy.last();
  bool at_setpoint = true;
  for (int i = 0; i < joints; ++i) {
    if (sent.motor_cmd()[wire->slots[i]].q() != values[i]) at_setpoint = false;
  }
  check(at_setpoint,
        "and after a position command every joint is held at that setpoint — §24's 'current "
        "setpoint' — rather than at the zeros the hold frame carries");

  /* The hold above is latched, and the check that says so is only worth anything if the joint it
   * holds is moving: a target re-read from a measurement that never changes looks latched. So a
   * velocity command sets the second joint going, the first hold after it latches where that joint
   * is, and a second hold a moment later — with the joint still settling — must carry the same
   * target. */
  section("a hold is a target, not a reading: the joint moves and the target does not");
  {
    float rates[skynet_unitree::MAX_JOINTS];
    /* Non-zero on every joint: the fake counts a velocity of exactly zero with a damping and no
     * position as a zero where a sentinel belongs, and a driver that wrote one by mistake is the
     * defect it exists to catch. */
    for (int i = 0; i < skynet_unitree::MAX_JOINTS; ++i) rates[i] = 0.01f;
    rates[1] = 0.8f;
    cmd = command(SKR_MODE_VELOCITY, rates, mask, static_cast<std::uint16_t>(joints));
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "a velocity command sets joint two going");
    std::this_thread::sleep_for(std::chrono::milliseconds(150));
    std::uint8_t second_only[skynet_unitree::MAX_JOINTS];
    for (int i = 0; i < skynet_unitree::MAX_JOINTS; ++i) second_only[i] = i == 1 ? 0 : 1;
    before = spy.seen();
    cmd = command(SKR_MODE_POSITION, values, second_only, static_cast<std::uint16_t>(joints));
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "the first hold after it is sent");
    spy.wait_for(before + 1);
    const float first_target = spy.last().motor_cmd()[slot1].q();
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    state = buffers.state();
    driver->read_state(ctx, &state, 0);
    const float measured_now = state.position_rad[1];
    before = spy.seen();
    check(driver->write_command(ctx, &cmd, 0) == SKR_OK, "and a second a moment later");
    spy.wait_for(before + 1);
    check(std::fabs(measured_now - first_target) > 1e-4f,
          "with the held joint no longer where the first hold found it");
    check(spy.last().motor_cmd()[slot1].q() == first_target,
          "the second hold carries the first one's target, not the position the joint has "
          "reached: a target that followed the measurement would set the stiffness term to zero "
          "and hold nothing, and the joint would creep");
  }

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

  /* One driver object lives for the process, so everything a session set is still there when the
   * next one opens unless `open()` clears it. A hold latched in a session, or a position setpoint
   * remembered, would send the next session's first masked joint to a pose from before. The gain
   * is zero here so that nothing moves: the joint is measured where it was, and the target is the
   * only number that can tell the sessions apart. */
  section("what a session leaves behind");
  {
    const std::string inert = "{\"interface\":\"" + interface + "\",\"model\":\"" + expect->model +
                              "\",\"kp\":0.0,\"kd\":5.0}";
    skr_ctx *first_session = nullptr;
    check(driver->open(inert.c_str(), &first_session) == SKR_OK, "a session opens");
    float far_away[skynet_unitree::MAX_JOINTS];
    for (int i = 0; i < skynet_unitree::MAX_JOINTS; ++i) far_away[i] = 3.0f;
    cmd = command(SKR_MODE_POSITION, far_away, mask, static_cast<std::uint16_t>(joints));
    check(driver->write_command(first_session, &cmd, 0) == SKR_OK,
          "it takes a position command, which leaves a setpoint of 3 rad on every joint");
    std::uint8_t second_masked[skynet_unitree::MAX_JOINTS];
    for (int i = 0; i < skynet_unitree::MAX_JOINTS; ++i) second_masked[i] = i == 1 ? 0 : 1;
    cmd = command(SKR_MODE_POSITION, far_away, second_masked, static_cast<std::uint16_t>(joints));
    check(driver->write_command(first_session, &cmd, 0) == SKR_OK,
          "and one that masks the second joint, which latches a hold at that setpoint");
    /* Twenty-five samples returned, so the count a new session must not inherit is one its first
     * reads could not outrun: at 500 Hz that many samples take fifty milliseconds to arrive, and
     * the watchdog window is twenty (`uni-reported-count-kept-across-sessions`). */
    for (int i = 0; i < 25; ++i) {
      state = buffers.state();
      driver->read_state(first_session, &state, 0);
    }
    check(driver->close(first_session) == SKR_OK, "the session closes");

    skr_ctx *next_session = nullptr;
    check(driver->open(inert.c_str(), &next_session) == SKR_OK, "a new session opens");
    state = buffers.state();
    const auto reopened_began = std::chrono::steady_clock::now();
    driver->read_state(next_session, &state, 0);
    const auto reopened_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                                 std::chrono::steady_clock::now() - reopened_began).count();
    check(reopened_ms < 15,
          "and the new session's first read returns at once, without waiting out the watchdog "
          "window: it took " + std::to_string(reopened_ms) +
          " ms (a count of what the last session had returned would read the first samples of "
          "this one as old, and the read would sleep its whole twenty-millisecond window)");
    const float where_it_is = state.position_rad[1];
    {
      /* The count of samples this session has already returned is the session's own: one carried over would
       * make the new session's first samples look like ones it had read, and every read would wait out its
       * whole window. */
      const std::uint32_t reopened_seq = state.seq;
      state = buffers.state();
      driver->read_state(next_session, &state, 0);
      check(state.seq > reopened_seq,
            "and its sequence counts its own samples, so the second read of a new session advances it: a count "
            "of what the last session had returned would read the first samples of this one as old");
    }
    float nearby[skynet_unitree::MAX_JOINTS];
    for (int i = 0; i < skynet_unitree::MAX_JOINTS; ++i) nearby[i] = 0.1f;
    before = spy.seen();
    cmd = command(SKR_MODE_POSITION, nearby, second_masked, static_cast<std::uint16_t>(joints));
    check(driver->write_command(next_session, &cmd, 0) == SKR_OK,
          "its first command masks the second joint");
    spy.wait_for(before + 1);
    check(std::fabs(spy.last().motor_cmd()[slot1].q() - where_it_is) < 0.05f,
          "and that joint is held where the robot is measured, not at the setpoint or the hold "
          "the last session left");
    driver->close(next_session);
  }

  section("releasing the latch without closing the session (§48.2)");
  {
    skr_ctx *session = nullptr;
    check(driver->open(with_gains.c_str(), &session) == SKR_OK, "a session opens");
    cmd = command(SKR_MODE_POSITION, values, mask, static_cast<std::uint16_t>(joints));
    check(driver->write_command(session, &cmd, 0) == SKR_OK, "and takes a command");
    check(driver->emergency_stop(session) == SKR_OK, "the driver's own stop is sent");
    check(driver->write_command(session, &cmd, 0) == SKR_EPERM, "and the latch refuses what follows");
    check(driver->release_stop(session) == SKR_OK,
          "a power-on self-test releases the latch without closing the session");
    state = buffers.state();
    driver->read_state(session, &state, 0);
    check((state.flags & SKR_FLAG_ESTOP) == 0, "the state no longer carries the stop flag");
    check(driver->write_command(session, &cmd, 0) == SKR_OK, "and a command is accepted again");
    driver->close(session);
  }

  section("a second open, and another interface");
  {
    skr_ctx *session = nullptr;
    check(driver->open(with_gains.c_str(), &session) == SKR_OK, "a session opens");
    skr_ctx *second = nullptr;
    check(driver->open(with_gains.c_str(), &second) == SKR_EPERM,
          "a second open on the same driver is refused while one session is open");
    driver->close(session);
    const std::string elsewhere = "{\"interface\":\"not-the-one-joined\",\"model\":\"" +
                                  std::string(expect->model) + "\",\"kp\":60.0,\"kd\":5.0}";
    skr_ctx *other = nullptr;
    check(driver->open(elsewhere.c_str(), &other) == SKR_EINVAL,
          "a session naming another interface than the one this process joined DDS on is refused: "
          "unitree_sdk2 joins one interface per process, and the session would be silently "
          "given the first");
  }

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
    cmd = command(SKR_MODE_TORQUE, values, partial, static_cast<std::uint16_t>(joints));
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "but a torque command that masks a joint is refused: that joint is to be held, a hold "
          "is a stiffness, and going limp is the reading §24 rules out");
    std::uint8_t hold_all[skynet_unitree::MAX_JOINTS] = {0};
    cmd = command(SKR_MODE_POSITION, values, hold_all, static_cast<std::uint16_t>(joints));
    check(driver->write_command(bare, &cmd, 0) == SKR_EPERM,
          "and so is the kernel's hold frame, loudly, rather than answered with a free body");
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

/* DRY-COVERAGE-PLAN.md F205. A robot that reports trouble on one joint, as the maker's own field
 * carries it — `lost` on the go2 message, `motorstate` on the hg one — against a fake started with
 * `<joint>=<value>` (`run-check.sh` does it). The driver's word used to be the joint's ordinal alone,
 * under a comment that called it the vendor's word verbatim; the number the maker defined never
 * reached the black box. The expectation is written out here by hand and not taken from the driver:
 * the joint, 1-based, in the high half, and the vendor's own number in the low sixteen bits. */
int run_fault(const Expect *expect, const std::string &interface, int joint, std::uint32_t value) {
  const skr_driver *driver = skynet_rhal_entry();
  section("a joint the robot reports trouble on");
  skr_ctx *ctx = nullptr;
  const std::string with_gains = "{\"interface\":\"" + interface + "\",\"model\":\"" +
                                 expect->model + "\",\"kp\":60.0,\"kd\":5.0}";
  check(driver->open(with_gains.c_str(), &ctx) == SKR_OK,
        "open() joins DDS and waits for the robot's first state");
  if (ctx == nullptr) {
    std::printf("\n  the fake robot is not publishing; nothing below can run.\n");
    return 1;
  }
  Buffers buffers;
  skr_state state = buffers.state();
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "read_state succeeds");
  check((state.flags & SKR_FLAG_DEGRADED) != 0,
        "a joint the robot reports trouble on reads DEGRADED: §27 reads that bit");
  check((state.flags & SKR_FLAG_ESTOP) == 0,
        "and is not a stop: the maker says a joint is in trouble, not that the stop chain is open");
  const std::uint32_t wanted =
      (static_cast<std::uint32_t>(joint + 1) << 16) | (value & 0xFFFFu);
  check(state.fault_code == wanted,
        "the word carries the joint (1-based, high half) and the vendor's own number (low sixteen "
        "bits), not the ordinal alone: " + std::to_string(joint + 1) + " and " +
            std::to_string(value & 0xFFFFu));
  driver->close(ctx);
  return 0;
}

}  // namespace

int main(int argc, char **argv) {
  const std::string interface = argc > 1 ? argv[1] : "lo";
  const std::string model = argc > 2 ? argv[2] : "go2";

  const skynet_unitree::WireModel *wire = skynet_unitree::find_wire_model(model);
  const Expect *expect = find_expect(model);
  if (wire == nullptr || expect == nullptr) {
    std::fprintf(stderr, "selftest: unknown model \"%s\" (go2, b2, h1, g1, h1_2, h2, r1)\n", model.c_str());
    return 2;
  }

  /* `<joint>=<value>`: run only the fault section, against a fake started with the same argument. */
  if (argc > 3) {
    char *rest = nullptr;
    const long joint = std::strtol(argv[3], &rest, 10);
    if (rest == nullptr || *rest != '=' || joint < 0 || joint >= wire->joints) {
      std::fprintf(stderr, "selftest: the third argument is <joint>=<value> with a joint below %d (got \"%s\")\n",
                   wire->joints, argv[3]);
      return 2;
    }
    const std::uint32_t value = static_cast<std::uint32_t>(std::strtoul(rest + 1, nullptr, 10));
    std::printf("\n== model %s: joint %ld reports %u ==\n", model.c_str(), joint, value);
    const int result = run_fault(expect, interface, static_cast<int>(joint), value);
    if (result != 0) return result;
    std::printf("\n%d checks, %zu failed\n", CHECKS, FAILURES.size());
    for (const auto &failure : FAILURES) std::printf("  - %s\n", failure.c_str());
    return FAILURES.empty() ? 0 : 1;
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
  if (wire->family == skynet_unitree::Family::GO2) {
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
