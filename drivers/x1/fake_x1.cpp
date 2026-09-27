/* fake_x1 — four X1 joints over the vendor's own topics, for the suite's logic.
 *
 * The sim is the motion substrate, but physics is the wrong oracle for refusals: a clamping
 * check should not depend on MuJoCo stepping, and an estop check should not wait on a render
 * thread. So this fake speaks the same two topics the sim owns — `/joint_states` out,
 * `/joint_cmd` in, the vendor's own message types — on DDS domain 42, where the sim (domain 0)
 * can neither hear it nor answer for it.
 *
 * Four joints, real X1 names from the sim's MJCF (the driver clamps to that file, so the fake
 * must be clampable by it): lumbar yaw and the left arm's pitch chain. Positions scripted,
 * nonzero and distinct, so a driver writing zeros would not look correct; velocities and
 * efforts small and nonzero, so NaN-smuggling would not look correct either. Every command is
 * recorded — names, all six arrays — and counted: wrong names, short arrays, and targets
 * outside the MJCF ranges each have their own refusal counter, because a conformance run that
 * passed while commanding wrong would be a green tick on a driver the robot answers with
 * motion it never ordered.
 *
 * Counts print as one JSON line on SIGTERM, the unitree shape. `run-check.sh` starts this
 * before the suite and reads what it counted after.
 *
 * Usage: fake_x1 <domain> <model.xml>
 */

#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include "rclcpp/rclcpp.hpp"
#include "sensor_msgs/msg/joint_state.hpp"
#include "my_ros2_proto/msg/joint_command.hpp"

namespace {

const char *const kJoints[] = {
    "lumbar_yaw_joint",
    "left_shoulder_pitch_joint",
    "left_elbow_pitch_joint",
    "left_knee_pitch_joint",
};
const double kStarts[] = {0.15, -0.4, 0.9, -0.25};
const double kVelocities[] = {0.01, -0.02, 0.03, -0.04};
const double kEfforts[] = {0.5, -1.0, 1.5, -2.0};

std::atomic<bool> g_stop{false};

void on_term(int) { g_stop = true; }

bool mjcf_range(const std::string &xml, const std::string &joint, double *low, double *high) {
  const std::string needle = "name=\"" + joint + "\"";
  const std::size_t at = xml.find(needle);
  if (at == std::string::npos) return false;
  const std::size_t end = xml.find('>', at);
  const std::size_t range_at = xml.find("range=\"", at);
  if (range_at == std::string::npos || range_at > end) return false;
  const char *start = xml.c_str() + range_at + 7;
  char *mid = nullptr;
  *low = std::strtod(start, &mid);
  if (mid == start) return false;
  char *stop = nullptr;
  *high = std::strtod(mid, &stop);
  return stop != mid && *low < *high;
}

struct Counts {
  std::uint64_t states = 0;
  std::uint64_t commands = 0;
  std::uint64_t malformed = 0;
  std::uint64_t out_of_range = 0;
  std::uint64_t partial = 0;
  std::map<std::string, double> last_position;
  std::map<std::string, double> last_velocity;
  std::map<std::string, double> last_effort;
  std::map<std::string, double> last_stiffness;
  std::map<std::string, double> last_damping;
};

}  // namespace

int main(int argc, char **argv) {
  if (argc != 3) {
    std::fprintf(stderr, "usage: fake_x1 <domain> <model.xml>\n");
    return 2;
  }
  ::setenv("ROS_DOMAIN_ID", argv[1], 1);
  std::ifstream model(argv[2]);
  if (!model.good()) {
    std::fprintf(stderr, "fake_x1: cannot read %s\n", argv[2]);
    return 2;
  }
  std::ostringstream text;
  text << model.rdbuf();
  const std::string xml = text.str();
  std::map<std::string, std::pair<double, double>> ranges;
  for (const char *joint : kJoints) {
    double low = 0, high = 0;
    if (!mjcf_range(xml, joint, &low, &high)) {
      std::fprintf(stderr, "fake_x1: %s has no range for %s\n", argv[2], joint);
      return 2;
    }
    ranges[joint] = {low, high};
  }

  std::signal(SIGTERM, on_term);
  std::signal(SIGINT, on_term);
  rclcpp::init(argc, argv);
  auto node = std::make_shared<rclcpp::Node>("fake_x1");
  auto pub = node->create_publisher<sensor_msgs::msg::JointState>("/joint_states", 10);
  Counts counts;
  std::mutex lock;
  for (std::size_t i = 0; i < 4; ++i) {
    counts.last_position[kJoints[i]] = kStarts[i];
    counts.last_velocity[kJoints[i]] = kVelocities[i];
    counts.last_effort[kJoints[i]] = kEfforts[i];
  }
  auto sub = node->create_subscription<my_ros2_proto::msg::JointCommand>(
      "/joint_cmd", 10, [&](my_ros2_proto::msg::JointCommand::SharedPtr msg) {
        std::lock_guard<std::mutex> guard(lock);
        counts.commands += 1;
        const std::size_t n = msg->name.size();
        /* The full-vector rule, from the sim's own indexing: a command that does not name
         * every joint drives silent joints from index 0, so the fake counts it as refused. */
        if (n != 4 || msg->position.size() != n || msg->velocity.size() != n ||
            msg->effort.size() != n || msg->stiffness.size() != n || msg->damping.size() != n) {
          counts.malformed += 1;
          return;
        }
        bool known = true;
        for (const std::string &name : msg->name) {
          if (ranges.find(name) == ranges.end()) known = false;
        }
        if (!known) {
          counts.malformed += 1;
          return;
        }
        bool partial = false;
        for (const char *joint : kJoints) {
          bool found = false;
          for (const std::string &name : msg->name) {
            if (name == joint) found = true;
          }
          if (!found) partial = true;
        }
        if (partial) counts.partial += 1;
        for (std::size_t i = 0; i < n; ++i) {
          const auto range = ranges[msg->name[i]];
          if (msg->position[i] < range.first - 1e-9 || msg->position[i] > range.second + 1e-9) {
            /* Position targets outside the MJCF ranges — but only when the joint is driven
             * in position (stiffness > 0). A torque or velocity frame carries zeros in the
             * position array by construction, and the sim ignores them. */
            if (msg->stiffness[i] > 0) counts.out_of_range += 1;
          }
          counts.last_position[msg->name[i]] = msg->position[i];
          counts.last_velocity[msg->name[i]] = msg->velocity[i];
          counts.last_effort[msg->name[i]] = msg->effort[i];
          counts.last_stiffness[msg->name[i]] = msg->stiffness[i];
          counts.last_damping[msg->name[i]] = msg->damping[i];
        }
      });

  auto timer = node->create_wall_timer(std::chrono::milliseconds(10), [&] {
    sensor_msgs::msg::JointState state;
    state.header.stamp = node->now();
    {
      std::lock_guard<std::mutex> guard(lock);
      for (const char *joint : kJoints) {
        state.name.emplace_back(joint);
        state.position.push_back(counts.last_position[joint]);
        state.velocity.push_back(counts.last_velocity[joint]);
        state.effort.push_back(counts.last_effort[joint]);
      }
      counts.states += 1;
    }
    pub->publish(state);
  });

  std::fprintf(stderr, "fake-x1: serving 4 joints on domain %s\n", argv[1]);
  rclcpp::executors::SingleThreadedExecutor executor;
  executor.add_node(node);
  while (!g_stop && rclcpp::ok()) executor.spin_some(std::chrono::milliseconds(10));
  {
    std::lock_guard<std::mutex> guard(lock);
    std::printf("{\"states\":%llu,\"commands\":%llu,\"malformed\":%llu,"
                "\"out_of_range\":%llu,\"partial\":%llu}\n",
                static_cast<unsigned long long>(counts.states),
                static_cast<unsigned long long>(counts.commands),
                static_cast<unsigned long long>(counts.malformed),
                static_cast<unsigned long long>(counts.out_of_range),
                static_cast<unsigned long long>(counts.partial));
    std::fflush(stdout);
  }
  rclcpp::shutdown();
  return 0;
}
