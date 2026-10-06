/* skynet-x1 — an AgiBot X1 humanoid over its own ROS 2 topics.
 *
 * `DRY-COVERAGE-PLAN.md`'s Phase 7 item 3: the X1 against `genie_sim`'s stablemate, the
 * vendor's own `agibot_x1_infer` MuJoCo deployment. The whole stack is open source and it runs
 * here: AimRT modules, a MuJoCo sim, ONNX policies, ROS 2 topics. The driver speaks the two the
 * sim owns — `/joint_states` in, `/joint_cmd` out — as an ordinary rclcpp node, the way the
 * vendor's own ControlModule does. Twenty-nine joints, position/velocity/torque, discovered
 * live off the first JointState.
 *
 * Three facts shape everything below, and all three are the vendor's, read off their code:
 *
 *   1. The sim steps once per incoming command (`SimModule::CmdCallback`: write command, one
 *      `mj_step`, publish states). No commands, no states — so `open()` cannot wait for a state
 *      that will never come, and `read_state` returns the latest cached one. The suite
 *      commands and then reads, the way the physics demands.
 *   2. Every command must name every joint. `WriteMotorCmd` builds its index map from the
 *      command's own names and `operator[]` defaults a missing joint to index 0 — a partial
 *      command would drive twenty-eight joints from the first joint's setpoint. The driver
 *      always sends all twenty-nine; a masked joint is §24's hold — its last position setpoint,
 *      or the position measured when the hold began, latched once and held with the operator's
 *      gains. It used to re-read the measured position every cycle, which zeroes the stiffness
 *      term and lets a loaded joint drift (DRY-COVERAGE-PLAN.md F121, carried as 8.13).
 *   3. A command is an impedance frame: position, velocity, effort, stiffness, damping per
 *      joint, applied as torque = effort + (pos-q)*kp + (vel-dq)*kd. Position and velocity
 *      need gains the operator chose (Unitree's rule: no default stiffness, §63.4); torque
 *      needs none (kp = kd = 0, the vendor's own RL shape).
 *
 * Travel comes from the sim's own MJCF (`model`, defaulting under $X1_INFER): the sim's design
 * values, authoritative for clamping the sim and for nothing else — §63.4 keeps them out of
 * the envelope. The envelope carries a torque ceiling only, the Unitree hedge: 200 N·m is the
 * PowerFlow actuator's published peak (agibot.com/products/X1 — the knee/max, not a per-joint
 * rating), and no joint velocity is published anywhere, so `joint_ratings` stays unmet.
 *
 * The stop is zero torques plus a latch — the vendor's RL zeros — and no capability is
 * declared for it: nothing here is rated, and the second session starts clean (F26's other
 * branch). Roles wait for a human: `roles` maps joint names to §5.1, validated at open.
 */

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

#include "rclcpp/rclcpp.hpp"
#include "sensor_msgs/msg/joint_state.hpp"
#include "my_ros2_proto/msg/joint_command.hpp"

#include "skynet_rhal.h"
#include "skynet_rhal.hpp"

/* The PowerFlow actuator's published peak, N·m — the knee/max across the body, not a
 * per-joint rating (agibot.com/products/X1). Rated 60 N·m, same hedge. */
#define X1_PEAK_TORQUE_NM 200.0

/* The watchdog window, in milliseconds. The sim answers each command with a step and a state,
 * so the round trip is milliseconds; the window is driver design anyway, stated as such in the
 * envelope, because no vendor publication names a rate. */
#define X1_WATCHDOG_MS 100.0

namespace {

bool config_string(const std::string &config, const char *key, std::string *out) {
  const std::string needle = std::string("\"") + key + "\"";
  const std::size_t at = config.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle.size());
  if (colon == std::string::npos) return false;
  const std::size_t open = config.find('"', colon);
  if (open == std::string::npos) return false;
  const std::size_t close = config.find('"', open + 1);
  if (close == std::string::npos) return false;
  *out = config.substr(open + 1, close - open - 1);
  return true;
}

bool config_number(const std::string &config, const char *key, double *out) {
  const std::string needle = std::string("\"") + key + "\"";
  const std::size_t at = config.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle.size());
  if (colon == std::string::npos) return false;
  const char *start = config.c_str() + colon + 1;
  char *end = nullptr;
  double value = std::strtod(start, &end);
  if (end == start || !std::isfinite(value)) return false;
  *out = value;
  return true;
}

bool config_bool(const std::string &config, const char *key, bool *out) {
  const std::string needle = std::string("\"") + key + "\"";
  const std::size_t at = config.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t colon = config.find(':', at + needle.size());
  if (colon == std::string::npos) return false;
  const std::size_t value = config.find_first_not_of(" \t", colon + 1);
  if (config.compare(value, 4, "true") == 0) {
    *out = true;
    return true;
  }
  if (config.compare(value, 5, "false") == 0) {
    *out = false;
    return true;
  }
  return false;
}

/* `roles` is a flat JSON object of joint name to §5.1 role. Parsed by hand like everything
 * else here — the shape is fixed and a JSON library for one object would be the larger
 * dependency. */
bool config_roles(const std::string &config, std::map<std::string, std::string> *out) {
  const std::size_t at = config.find("\"roles\"");
  if (at == std::string::npos) return true;
  const std::size_t colon = config.find(':', at + 7);
  if (colon == std::string::npos) return false;
  const std::size_t open = config.find('{', colon);
  if (open == std::string::npos) return false;
  std::size_t pos = open + 1;
  bool need_pair = false;  // set after a comma: `}` there is a trailing comma
  for (;;) {
    const std::size_t peek = config.find_first_not_of(" \t\r\n", pos);
    if (peek == std::string::npos) return false;
    if (config[peek] == '}') return !need_pair;
    if (need_pair && config[peek] != '"') return false;
    const std::size_t key_open = config.find('"', pos);
    if (key_open == std::string::npos || key_open != peek) return false;
    const std::size_t key_close = config.find('"', key_open + 1);
    if (key_close == std::string::npos) return false;
    const std::string key = config.substr(key_open + 1, key_close - key_open - 1);
    const std::size_t key_colon = config.find(':', key_close);
    if (key_colon == std::string::npos) return false;
    const std::size_t val_open = config.find('"', key_colon);
    if (val_open == std::string::npos) return false;
    const std::size_t val_close = config.find('"', val_open + 1);
    if (val_close == std::string::npos) return false;
    (*out)[key] = config.substr(val_open + 1, val_close - val_open - 1);
    pos = val_close + 1;
    const std::size_t next = config.find_first_not_of(" \t\r\n", pos);
    if (next == std::string::npos) return false;
    if (config[next] == '}') return true;
    if (config[next] != ',') return false;
    pos = next + 1;
    need_pair = true;
  }
}

bool is_chain(const std::string &word) {
  static const char *const kChains[] = {"left_arm",  "right_arm", "left_leg", "right_leg",
                                        "left_hand", "right_hand", "torso",   "neck",
                                        "head",      "base",      "tail",     "aux"};
  for (const char *chain : kChains) {
    if (word == chain) return true;
  }
  return false;
}

bool is_axis(const std::string &word) {
  static const char *const kAxes[] = {"pitch", "yaw",   "roll",  "slide",
                                      "flex",  "abduct", "rotate", "grip"};
  for (const char *axis : kAxes) {
    if (word == axis) return true;
  }
  return false;
}

/* §5.1's grammar, mirrored from `skynet-contracts`' roles table including its
 * trim-and-lowercase: `chain.link[.axis][#index]`, lowercase ASCII, index a u32. The
 * normalised form is what the descriptor carries. */
bool normalise_role(const std::string &role, std::string *out) {
  std::string text = role;
  const auto trim = [](std::string *value) {
    const std::size_t first = value->find_first_not_of(" \t\r\n");
    if (first == std::string::npos) {
      value->clear();
      return;
    }
    value->erase(0, first);
    value->erase(value->find_last_not_of(" \t\r\n") + 1);
  };
  trim(&text);
  for (char &c : text) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
  if (text.empty()) return false;
  std::string head = text, index;
  const std::size_t hash = text.find('#');
  if (hash != std::string::npos) {
    head = text.substr(0, hash);
    index = text.substr(hash + 1);
    if (index.empty() || index.size() > 10) return false;
    for (char c : index) {
      if (c < '0' || c > '9') return false;
    }
    if (std::strtoul(index.c_str(), nullptr, 10) > 0xFFFFFFFFUL) return false;
    index = std::to_string(std::strtoul(index.c_str(), nullptr, 10));
  }
  std::vector<std::string> parts;
  std::size_t pos = 0;
  for (;;) {
    const std::size_t dot = head.find('.', pos);
    parts.push_back(head.substr(pos, dot == std::string::npos ? dot : dot - pos));
    if (dot == std::string::npos) break;
    pos = dot + 1;
  }
  if (parts.size() != 2 && parts.size() != 3) return false;
  const auto lower_word = [](const std::string &word) {
    if (word.empty()) return false;
    for (char c : word) {
      if ((c < 'a' || c > 'z') && c != '_') return false;
    }
    return true;
  };
  const auto lower_word_digit = [](const std::string &word) {
    if (word.empty()) return false;
    for (char c : word) {
      if ((c < 'a' || c > 'z') && (c < '0' || c > '9') && c != '_') return false;
    }
    return true;
  };
  if (!lower_word(parts[0]) || !is_chain(parts[0])) return false;
  if (!lower_word_digit(parts[1])) return false;
  if (parts.size() == 3 && (!lower_word(parts[2]) || !is_axis(parts[2]))) return false;
  *out = parts[0] + "." + parts[1];
  if (parts.size() == 3) *out += "." + parts[2];
  if (!index.empty()) *out += "#" + index;
  return true;
}

/* The sim's own joint inventory, scanned out of its MJCF: every `<joint name="...">` in
 * file order. The prime command needs names before any state has named them, and the discovery
 * below verifies the wire against this list. */
bool mjcf_names(const std::string &path, std::vector<std::string> *out) {
  std::ifstream file(path);
  if (!file.good()) return false;
  std::ostringstream text;
  text << file.rdbuf();
  const std::string xml = text.str();
  std::size_t pos = 0;
  for (;;) {
    const std::size_t joint = xml.find("<joint ", pos);
    if (joint == std::string::npos) break;
    const std::size_t end = xml.find('>', joint);
    const std::size_t name_at = xml.find("name=\"", joint);
    if (name_at == std::string::npos || name_at > end) return false;
    const std::size_t name_end = xml.find('"', name_at + 6);
    if (name_end == std::string::npos) return false;
    out->push_back(xml.substr(name_at + 6, name_end - name_at - 6));
    pos = end;
  }
  return !out->empty();
}

/* The sim's own travel, scanned out of its MJCF: `<joint name="..." ... range="a b" />`. A
 * string scan, not an XML parser — the file is machine-written, the shape is fixed, and a
 * parser would be the larger dependency for two numbers per joint. */
bool mjcf_ranges(const std::string &path, const std::vector<std::string> &joints,
                 std::map<std::string, std::pair<double, double>> *out, std::string *missing) {
  std::ifstream file(path);
  if (!file.good()) return false;
  std::ostringstream text;
  text << file.rdbuf();
  const std::string xml = text.str();
  for (const std::string &joint : joints) {
    const std::string needle = "name=\"" + joint + "\"";
    const std::size_t at = xml.find(needle);
    if (at == std::string::npos) {
      *missing = joint;
      return false;
    }
    /* The range lives on the same element — bounded, so a later joint's range cannot match. */
    const std::size_t end = xml.find('>', at);
    const std::size_t range_at = xml.find("range=\"", at);
    if (range_at == std::string::npos || range_at > end) {
      *missing = joint;
      return false;
    }
    const char *start = xml.c_str() + range_at + 7;
    char *mid = nullptr;
    double low = std::strtod(start, &mid);
    if (mid == start) {
      *missing = joint;
      return false;
    }
    char *stop = nullptr;
    double high = std::strtod(mid, &stop);
    if (stop == mid || !(low < high) || !std::isfinite(low) || !std::isfinite(high)) {
      *missing = joint;
      return false;
    }
    (*out)[joint] = {low, high};
  }
  return true;
}

}  // namespace

namespace {

class X1Bridge : public skynet::Driver {
 public:
  int open(std::string_view config_json) override {
    if (opened_) return SKR_EPERM;
    const std::string config(config_json);

    /* F47's reset, every field a previous session set. The estop latch is announced rather
     * than silently dropped — but unlike the Unitree body there is no hardware stop state to
     * read back, so a fresh session genuinely starts unstopped. */
    if (estopped_) {
      std::fprintf(stderr,
                   "skynet-x1: the previous session left an emergency stop latched. This is a "
                   "new session and the latch does not carry into it.\n");
    }
    estopped_ = false;
    seq_ = 0;
    have_state_ = false;
    have_kp_ = have_kd_ = false;
    kp_ = kd_ = 0.0;
    joints_.clear();
    ranges_.clear();
    roles_.clear();
    simulator_ = true;

    /* Which DDS domain to join. `setenv` before `rclcpp::init`: the domain is read once, at
     * context creation, so a config that arrived after init would be a lie about isolation —
     * and isolation is what keeps the fake (domain 42) from meeting the sim (domain 0). */
    double domain = 0;
    if (config_number(config, "domain", &domain)) {
      if (domain < 0 || domain > 232 || std::floor(domain) != domain) {
        std::fprintf(stderr, "skynet-x1: `domain` is a DDS domain id, 0-232.\n");
        return SKR_EINVAL;
      }
      ::setenv("ROS_DOMAIN_ID", std::to_string(static_cast<int>(domain)).c_str(), 1);
    }
    std::string states_topic = "/joint_states";
    std::string cmd_topic = "/joint_cmd";
    config_string(config, "states", &states_topic);
    config_string(config, "cmd", &cmd_topic);
    if (states_topic.empty() || cmd_topic.empty() || states_topic == cmd_topic) {
      std::fprintf(stderr, "skynet-x1: `states` and `cmd` name two distinct topics.\n");
      return SKR_EINVAL;
    }
    /* Travel's source: the sim's own MJCF, defaulting under $X1_INFER. Required, not
     * defaulted further — clamping to the wrong model's ranges would be worse than refusing. */
    std::string model;
    if (!config_string(config, "model", &model) || model.empty()) {
      const char *infer = std::getenv("X1_INFER");
      if (infer != nullptr) {
        model = std::string(infer) +
                "/build/cfg/sim_module/model/mjcf/robot/xyber_x1/xyber_x1_serial.xml";
      }
    }
    if (model.empty()) {
      std::fprintf(stderr,
                   "skynet-x1: `model` names the MJCF whose ranges clamp commands, and $X1_INFER "
                   "is unset so there is no default. Refusing rather than moving unclamped.\n");
      return SKR_EINVAL;
    }
    /* Gains, and there is deliberately no default — the Unitree rule. A position command with
     * a `kp` this driver invented is a stiffness nobody chose. */
    have_kp_ = config_number(config, "kp", &kp_);
    have_kd_ = config_number(config, "kd", &kd_);
    if ((have_kp_ && kp_ < 0) || (have_kd_ && kd_ < 0)) {
      std::fprintf(stderr, "skynet-x1: `kp` and `kd` are non-negative.\n");
      return SKR_EINVAL;
    }
    config_bool(config, "simulator", &simulator_);
    /* Whether to prime the pump: the sim steps (and publishes) once per incoming command, so
     * opening against a silent sim needs a zero-torque command first — full-vector, model
     * names, all zeros including gains (limp, not a setpoint). The fake publishes
     * continuously and must never see a prime, so this defaults off and the sim phase opts
     * in. An empty prime would crash the sim (`WriteMotorCmd` indexes position[0] of an
     * empty command past its 3s grace), which is why the model names are parsed first. */
    bool prime = false;
    config_bool(config, "prime", &prime);
    std::vector<std::string> model_joints;
    if (!mjcf_names(model, &model_joints)) {
      std::fprintf(stderr, "skynet-x1: no joint inventory in model file\n");
      return SKR_EINVAL;
    }
    std::map<std::string, std::string> roles;
    if (!config_roles(config, &roles)) {
      std::fprintf(stderr, "skynet-x1: `roles` maps joint names to §5.1 roles, as an object.\n");
      return SKR_EINVAL;
    }

    try {
      /* `rclcpp::init` once per process: a second init with live contexts is an error, and
       * this driver never shuts the context down — process-lifetime middleware, like DDS. */
      if (!rclcpp::ok()) rclcpp::init(0, nullptr);
      node_ = std::make_shared<rclcpp::Node>("skynet_x1");
      cmd_pub_ = node_->create_publisher<my_ros2_proto::msg::JointCommand>(cmd_topic, 10);
      states_sub_ = node_->create_subscription<sensor_msgs::msg::JointState>(
          states_topic, 10,
          [this](sensor_msgs::msg::JointState::SharedPtr msg) { on_states(msg); });
    } catch (const std::exception &e) {
      std::fprintf(stderr, "skynet-x1: the ROS graph did not come up: %s\n", e.what());
      return SKR_EIO;
    }
    spin_ = std::thread([this] {
      rclcpp::executors::SingleThreadedExecutor executor;
      executor.add_node(node_);
      while (rclcpp::ok() && spinning_) executor.spin_some(std::chrono::milliseconds(5));
    });

    /* Fail fast on a silent graph. The fake publishes continuously; the sim publishes
     * once per command — so with `prime`, a zero-torque full-vector command goes out every
     * 100 ms until states arrive or five seconds pass. The sim ignores everything for its
     * first three seconds, and the wait outlasts that window from any start, so a silent
     * graph after five seconds means nobody is there. */
    my_ros2_proto::msg::JointCommand zero;
    zero.name = model_joints;
    zero.position.assign(model_joints.size(), 0.0);
    zero.velocity.assign(model_joints.size(), 0.0);
    zero.effort.assign(model_joints.size(), 0.0);
    zero.stiffness.assign(model_joints.size(), 0.0);
    zero.damping.assign(model_joints.size(), 0.0);
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    for (;;) {
      {
        std::lock_guard<std::mutex> guard(lock_);
        if (have_state_) break;
      }
      if (std::chrono::steady_clock::now() > deadline) {
        std::fprintf(stderr,
                     "skynet-x1: no JointState after five seconds. The sim publishes\n"
                     "once per command and the fake continuously — either the graph is\n"
                     "empty or this driver joined the wrong domain.\n");
        stop_spin();
        return SKR_EIO;
      }
      if (prime) {
        try {
          cmd_pub_->publish(zero);
        } catch (const std::exception &e) {
          std::fprintf(stderr, "skynet-x1: the prime publish failed: %s\n", e.what());
          stop_spin();
          return SKR_EIO;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
      } else {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
      }
    }
    {
      std::lock_guard<std::mutex> guard(lock_);
      joints_ = last_.name;
    }
    /* No setpoint and no hold carries into a new session. */
    setpoint_.assign(joints_.size(), 0.0);
    have_setpoint_.assign(joints_.size(), false);
    hold_.assign(joints_.size(), 0.0);
    holding_.assign(joints_.size(), false);
    if (joints_.empty()) {
      std::fprintf(stderr, "skynet-x1: the first JointState named no joints.\n");
      stop_spin();
      return SKR_EIO;
    }
    std::string missing;
    if (!mjcf_ranges(model, joints_, &ranges_, &missing)) {
      std::fprintf(stderr,
                   "skynet-x1: `%s` has no usable range for `%s`. A joint the model does not "
                   "bound is a model mismatch, not an unclamped joint.\n",
                   model.c_str(), missing.c_str());
      stop_spin();
      return SKR_EINVAL;
    }
    for (const auto &entry : roles) {
      if (ranges_.find(entry.first) == ranges_.end()) {
        std::fprintf(stderr,
                     "skynet-x1: `roles` names `%s`, which this body did not report. Typo'd "
                     "joint names fail here, not silently.\n",
                     entry.first.c_str());
        stop_spin();
        return SKR_EINVAL;
      }
      std::string normalised;
      if (!normalise_role(entry.second, &normalised)) {
        std::fprintf(stderr,
                     "skynet-x1: `%s` is not a §5.1 canonical role (chain.link[.axis][#index]).\n",
                     entry.second.c_str());
        stop_spin();
        return SKR_EINVAL;
      }
      roles_[entry.first] = normalised;
    }
    opened_ = true;
    return SKR_OK;
  }

  int close() override {
    stop_spin();
    std::lock_guard<std::mutex> guard(lock_);
    opened_ = false;
    cmd_pub_.reset();
    states_sub_.reset();
    node_.reset();
    joints_.clear();
    ranges_.clear();
    roles_.clear();
    have_state_ = false;
    return SKR_OK;
  }

  int read_state(skynet::State &out, std::uint64_t) override {
    sensor_msgs::msg::JointState snap;
    {
      std::lock_guard<std::mutex> guard(lock_);
      if (!opened_ || !have_state_) return SKR_EPERM;
      snap = last_;
      estopped_snapshot_ = estopped_;
    }
    /* Names must match the discovery exactly — order included. A JointState from another body
     * (a crossed topic, a second sim on the same domain) would otherwise slide twenty-nine
     * positions sideways into the wrong joints. */
    if (snap.name != joints_ || snap.position.size() != joints_.size() ||
        snap.velocity.size() != joints_.size() || snap.effort.size() != joints_.size()) {
      std::fprintf(stderr,
                   "skynet-x1: a JointState arrived that is not this body's — names or array "
                   "lengths differ. Refusing to report it.\n");
      return SKR_EIO;
    }
    std::uint64_t stamp =
        static_cast<std::uint64_t>(snap.header.stamp.sec) * 1000000000ULL +
        static_cast<std::uint64_t>(snap.header.stamp.nanosec);
    if (stamp == 0) {
      const auto now = std::chrono::system_clock::now().time_since_epoch();
      stamp = static_cast<std::uint64_t>(
          std::chrono::duration_cast<std::chrono::nanoseconds>(now).count());
    }
    std::uint16_t flags = SKR_HOMING_NOT_REQUIRED;
    if (estopped_snapshot_) flags |= SKR_FLAG_ESTOP;
    out.set_stamp(stamp);
    out.set_seq(++seq_);
    out.set_flags(flags);
    out.set_fault(0);
    out.set_joint_count(joints_.size());
    auto position = out.position();
    auto velocity = out.velocity();
    auto effort = out.effort();
    for (std::size_t i = 0; i < joints_.size(); ++i) {
      position[i] = static_cast<float>(snap.position[i]);
      velocity[i] = static_cast<float>(snap.velocity[i]);
      effort[i] = static_cast<float>(snap.effort[i]);
    }
    /* Base unlocalised: the free joint's pose exists in the physics and nowhere on this wire
     * — JointState carries joints, and the IMU gives orientation. §24, the Unitree shape. */
    auto pose = out.base_pose();
    auto twist = out.base_twist();
    for (std::size_t i = 0; i < 7; ++i) pose[i] = std::nanf("");
    for (std::size_t i = 0; i < 6; ++i) twist[i] = 0.0f;
    return SKR_OK;
  }

  int write_command(const skynet::Command &cmd, std::uint64_t) override {
    if (!opened_) return SKR_EPERM;
    const std::size_t joints = joints_.size();
    if (cmd.joint_count() != joints) return SKR_EINVAL;
    const int mode = cmd.mode();
    if (mode != SKR_MODE_POSITION && mode != SKR_MODE_VELOCITY && mode != SKR_MODE_TORQUE) {
      return SKR_EINVAL;
    }
    /* §86.1, read where declared and on every entry whether the mask applies it or not:
     * a bare MODE_* in the array is the transcription error the offset exists to catch, and -1
     * is what `mode_of` says about a vocabulary violation. The mask decides which joints move;
     * it does not decide which entries are well-formed — an all-held command carrying garbage
     * modes is still malformed. */
    for (std::size_t i = 0; i < joints; ++i) {
      if (cmd.mode_of(i) < 0) {
        std::fprintf(stderr, "skynet-x1: not a per-joint mode (§86.1).\n");
        return SKR_EINVAL;
      }
    }
    /* A setpoint that is not a number is refused with the other malformed commands, before anything is built or committed (F282).
     * The clamp below does not catch one — `std::max(NaN, low)` is NaN — so a NaN position or velocity went to the wire as it was,
     * and a NaN torque was written into MuJoCo's actuator as a force. A joint the mask holds is not read, so it is not checked. */
    for (std::size_t i = 0; i < joints; ++i) {
      if (cmd.applies(i) && !std::isfinite(cmd.values()[i])) {
        std::fprintf(stderr,
                     "skynet-x1: the setpoint for joint %zu (%s) is not a finite number. A NaN or an infinity is written into the "
                     "sim's actuator as it is, so the whole command is refused; send a number (§24).\n",
                     i, joints_[i].c_str());
        return SKR_EINVAL;
      }
    }
    if (estopped_) return SKR_EPERM;
    const bool gains = have_kp_ && have_kd_;
    if ((mode == SKR_MODE_POSITION || mode == SKR_MODE_VELOCITY) && !gains) {
      std::fprintf(stderr,
                   "skynet-x1: a %s command needs `kp` and `kd` in this driver's config. An X1 "
                   "drive takes an impedance frame, not a setpoint, and this driver will not "
                   "choose a stiffness the operator did not (§63.4).\n",
                   mode == SKR_MODE_POSITION ? "position" : "velocity");
      return SKR_EPERM;
    }
    bool any_masked = false;
    for (std::size_t i = 0; i < joints; ++i) any_masked = any_masked || !cmd.applies(i);
    if (any_masked && !gains) {
      std::fprintf(stderr,
                   "skynet-x1: this command masks a joint, and a masked joint is §24's hold — "
                   "held at its current setpoint. On an impedance drive a hold is a stiffness, "
                   "and with no `kp` and `kd` in this driver's config there is nothing to hold "
                   "with. A free joint is the one reading §24 rules out, so the command is "
                   "refused; configure the gains the robot should hold with (§63.4).\n");
      return SKR_EPERM;
    }
    /* One impedance frame for all twenty-nine, always: the sim builds its index map from the
     * command's own names and defaults a missing joint to index 0, so a partial command would
     * drive silent joints from the first joint's setpoint. A masked joint is held (the header,
     * fact 2), resolved here on copies committed only once the frame is published. Only a
     * position command leaves a position setpoint; after any other a hold latches where the
     * joint was last measured. */
    std::vector<double> setpoint = setpoint_;
    std::vector<bool> have_setpoint = have_setpoint_;
    std::vector<double> hold = hold_;
    std::vector<bool> holding = holding_;
    my_ros2_proto::msg::JointCommand out;
    out.name = joints_;
    out.position.assign(joints, 0.0);
    out.velocity.assign(joints, 0.0);
    out.effort.assign(joints, 0.0);
    out.stiffness.assign(joints, 0.0);
    out.damping.assign(joints, 0.0);
    {
      std::lock_guard<std::mutex> guard(lock_);
      for (std::size_t i = 0; i < joints; ++i) {
        /* Resolved above: applies ? validated mode : position-hold. Every joint offers all
         * three modes, so there is no per-joint refusal left to make here. */
        const int joint_mode = cmd.applies(i) ? cmd.mode_of(i) : SKR_MODE_POSITION;
        double value = 0.0;
        if (cmd.applies(i)) {
          value = static_cast<double>(cmd.values()[i]);
          holding[i] = false;
          have_setpoint[i] = joint_mode == SKR_MODE_POSITION;
          if (have_setpoint[i]) setpoint[i] = value;
        } else {
          if (!holding[i]) {
            hold[i] = have_setpoint[i] ? setpoint[i] : last_.position[i];
            holding[i] = true;
          }
          value = hold[i];
        }
        if (joint_mode == SKR_MODE_POSITION) {
          const auto range = ranges_[joints_[i]];
          value = std::min(std::max(value, range.first), range.second);
          out.position[i] = value;
          out.stiffness[i] = have_kp_ ? kp_ : 0.0;
          out.damping[i] = have_kd_ ? kd_ : 0.0;
        } else if (joint_mode == SKR_MODE_VELOCITY) {
          out.velocity[i] = value;
          out.damping[i] = have_kd_ ? kd_ : 0.0;
        } else {
          out.effort[i] = value;
        }
      }
    }
    try {
      cmd_pub_->publish(out);
    } catch (const std::exception &e) {
      std::fprintf(stderr, "skynet-x1: the command publish failed: %s\n", e.what());
      return SKR_EIO;
    }
    setpoint_ = std::move(setpoint);
    have_setpoint_ = std::move(have_setpoint);
    hold_ = std::move(hold);
    holding_ = std::move(holding);
    return SKR_OK;
  }

  int emergency_stop() override {
    /* Zero torques, the vendor's RL zeros, then the latch regardless. What this does NOT claim
     * is a rated stop: nothing here persists in hardware, the descriptor says so by declaring
     * no capability, and the second session starts clean. */
    my_ros2_proto::msg::JointCommand out;
    {
      std::lock_guard<std::mutex> guard(lock_);
      out.name = joints_;
      const std::size_t joints = joints_.size();
      out.position.assign(joints, 0.0);
      out.velocity.assign(joints, 0.0);
      out.effort.assign(joints, 0.0);
      out.stiffness.assign(joints, 0.0);
      out.damping.assign(joints, 0.0);
      estopped_ = true;
    }
    if (cmd_pub_ != nullptr) {
      try {
        cmd_pub_->publish(out);
      } catch (const std::exception &e) {
        std::fprintf(stderr,
                     "skynet-x1: the stop publish failed (%s); the latch holds anyway.\n",
                     e.what());
      }
    }
    return SKR_OK;
  }

  /* Clear the session latch for §48.2's power-on self-test, under the same lock the stop took
   * it under: `read_state` snapshots the flag per read, so the next read carries no ESTOP and
   * commands are accepted again. The latch clears; nothing resumes. Idempotent. */
  int release_stop() override {
    std::lock_guard<std::mutex> guard(lock_);
    estopped_ = false;
    return SKR_OK;
  }

  std::string describe() override {
    /* Before any `open()` there is no body — and unlike the Unitree driver there is no fixed
     * joint table to describe instead, because the joints are discovered. An unopened driver
     * describes the discovery it has not done yet: zero joints, the envelope still cited. */
    std::lock_guard<std::mutex> guard(lock_);
    std::string json = "{\"schema_version\":1,\"robot_slug\":\"agibot-x1\",\"digest\":null,"
                       "\"verified\":false,\"simulator\":" +
                       std::string(simulator_ ? "true" : "false") +
                       ",\"base\":\"legged\",\"dof\":" + std::to_string(joints_.size()) +
                       ",\"joints\":[";
    for (std::size_t i = 0; i < joints_.size(); ++i) {
      if (i) json += ",";
      const auto range = ranges_.at(joints_[i]);
      char joint[1024];
      char role[256] = {0};
      const auto found = roles_.find(joints_[i]);
      if (found != roles_.end()) {
        std::snprintf(role, sizeof role, ",\"role\":\"%s\"", found->second.c_str());
      }
      /* Position travel from the sim's MJCF; velocity and effort undeclared on every joint —
       * AgiBot publishes no joint speed anywhere, and the 200 N·m peak is a body figure, not
       * a per-joint rating (§63.4 keeps both out of the joints). */
      std::snprintf(joint, sizeof joint,
                    "{\"name\":\"%s\"%s,\"type\":\"revolute\","
                    "\"limits\":{\"position_min_rad\":%.5f,\"position_max_rad\":%.5f},"
                    "\"control_modes\":[\"position\",\"velocity\",\"torque\"]}",
                    joints_[i].c_str(), role, range.first, range.second);
      json += joint;
    }
    char envelope[1024];
    std::snprintf(envelope, sizeof envelope,
                  "],\"end_effectors\":[],\"sensors\":[],"
                  "\"control\":{\"modes\":[\"position\",\"velocity\",\"torque\"],"
                  "\"per_joint_modes\":true,\"rate_hz_max\":null,\"rate_hz_min\":null,"
                  "\"command_latency_ms\":null},"
                  "\"envelope\":{\"joint_torque_max_nm\":%.1f,\"watchdog_ms\":%.1f,"
                  "\"source\":\"%s\",\"basis\":\"declared\"},"
                  "\"capabilities\":[],\"extensions\":[],\"solvers\":[]}",
                  X1_PEAK_TORQUE_NM, X1_WATCHDOG_MS,
                  "AgiBot X1 product page (agibot.com/products/X1): PowerFlow actuator peak "
                  "200 N.m, rated 60 N.m (the knee/max across the body, not a per-joint "
                  "rating); joint velocities unpublished, so joint_ratings is unmet; the "
                  "watchdog window is driver design");
    json += envelope;
    return json;
  }

 private:
  void on_states(sensor_msgs::msg::JointState::SharedPtr msg) {
    std::lock_guard<std::mutex> guard(lock_);
    last_ = *msg;
    have_state_ = true;
  }

  void stop_spin() {
    spinning_ = false;
    if (spin_.joinable()) spin_.join();
    spinning_ = true;
  }

  std::mutex lock_;
  bool opened_ = false;
  bool estopped_ = false;
  bool estopped_snapshot_ = false;
  std::uint32_t seq_ = 0;
  bool have_state_ = false;
  sensor_msgs::msg::JointState last_;
  std::vector<std::string> joints_;
  std::map<std::string, std::pair<double, double>> ranges_;
  std::map<std::string, std::string> roles_;
  bool have_kp_ = false;
  bool have_kd_ = false;
  double kp_ = 0.0;
  double kd_ = 0.0;
  /* §24's hold, per joint: the last position setpoint and whether there is one, and the latched
   * hold target and whether the joint is being held. Sized at open to the discovered joints. */
  std::vector<double> setpoint_;
  std::vector<bool> have_setpoint_;
  std::vector<double> hold_;
  std::vector<bool> holding_;
  bool simulator_ = true;
  std::shared_ptr<rclcpp::Node> node_;
  rclcpp::Publisher<my_ros2_proto::msg::JointCommand>::SharedPtr cmd_pub_;
  rclcpp::Subscription<sensor_msgs::msg::JointState>::SharedPtr states_sub_;
  std::atomic<bool> spinning_{true};
  std::thread spin_;
};

}  // namespace

SKYNET_RHAL_EXPORT(X1Bridge)
