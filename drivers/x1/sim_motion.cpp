/*
 * sim_motion — the proof that the sim moves when this driver commands.
 *
 * The suite speaks to the fake, which echoes commands without physics; this speaks to the
 * vendor's sim and asserts the physics answered. It opens the driver the way `run-check.sh`'s
 * sim phase does (domain 0, primed, vendor-scale gains), reads the live pose, displaces the
 * left elbow by 0.3 rad while holding every other joint at its current position, and polls
 * until the elbow has travelled 0.15 toward the target or sixty seconds pass. Exit 0 on
 * motion, 1 on a timeout — a body that ignores commands must fail loudly, not pass quietly.
 *
 * The gains are the vendor's own `pd_stand` arm-pitch values (300/0.6, `rl_x1_sim.yaml`):
 * operator-chosen for this run, cited rather than invented, and applied body-wide because the
 * driver takes scalar gains. Holding the legs at their live positions with a standing-scale
 * stiffness is what keeps the body up while the arm moves.
 *
 * Usage: sim_motion <domain> <model.xml>
 */

#include "skynet_rhal.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

extern "C" const skr_driver *skynet_rhal_entry(void);

namespace {

constexpr char kJoint[] = "left_elbow_pitch_joint";
constexpr double kTravel = 0.3;
constexpr double kSettled = 0.15;
constexpr double kTimeoutSeconds = 60.0;

}  // namespace

int main(int argc, char **argv) {
  if (argc != 3) {
    std::fprintf(stderr, "usage: sim_motion <domain> <model.xml>\n");
    return 2;
  }
  const std::string domain = argv[1];
  const std::string model = argv[2];

  const skr_driver *driver = skynet_rhal_entry();
  skr_ctx *ctx = nullptr;
  const std::string config = "{\"domain\":" + domain + ",\"model\":\"" + model +
                             "\",\"kp\":300.0,\"kd\":0.6,\"prime\":true}";
  const int opened = driver->open(config.c_str(), &ctx);
  if (opened != SKR_OK) {
    std::fprintf(stderr, "sim_motion: open against the sim failed (%d).\n", opened);
    return 1;
  }

  char json[65536];
  const int written = driver->describe(ctx, json, sizeof json);
  const std::string descriptor(json, written > 0 ? written : 0);
  if (descriptor.find("\"dof\":29") == std::string::npos) {
    std::fprintf(stderr, "sim_motion: the sim did not report twenty-nine joints.\n");
    driver->close(ctx);
    return 1;
  }
  std::printf("sim_motion: twenty-nine joints discovered live off the wire.\n");

  /* Joint order is the wire's, not the MJCF's: find the elbow by name in the descriptor. */
  std::vector<std::string> names;
  {
    std::size_t pos = 0;
    for (;;) {
      const std::size_t at = descriptor.find("\"name\":\"", pos);
      if (at == std::string::npos) break;
      const std::size_t end = descriptor.find('"', at + 8);
      names.push_back(descriptor.substr(at + 8, end - at - 8));
      pos = end;
    }
  }
  std::size_t elbow = names.size();
  for (std::size_t i = 0; i < names.size(); ++i) {
    if (names[i] == kJoint) elbow = i;
  }
  if (elbow >= names.size() || names.size() != 29) {
    std::fprintf(stderr, "sim_motion: %s is not among the discovered joints.\n", kJoint);
    driver->close(ctx);
    return 1;
  }

  std::vector<float> position(29), velocity(29), effort(29);
  auto read = [&] {
    skr_state state;
    std::memset(&state, 0, sizeof state);
    state.joint_count = 29;
    state.position_rad = position.data();
    state.velocity_rad_s = velocity.data();
    state.effort_nm = effort.data();
    return driver->read_state(ctx, &state, 0);
  };
  if (read() != SKR_OK) {
    std::fprintf(stderr, "sim_motion: the first read failed.\n");
    driver->close(ctx);
    return 1;
  }
  const double start = position[elbow];
  if (!std::isfinite(start)) {
    std::fprintf(stderr, "sim_motion: the elbow reports no finite position.\n");
    driver->close(ctx);
    return 1;
  }
  /* 0.3 toward mid-travel, so the target is well inside the MJCF whatever the start pose is —
   * the proof is about motion, not about the clamp, which the suite owns. */
  double target = start + (start < 0.85 ? kTravel : -kTravel);
  target = std::min(std::max(target, 0.05), 1.65);

  std::vector<float> values(position.begin(), position.end());
  values[elbow] = static_cast<float>(target);
  std::vector<std::uint8_t> mask(29, 1);
  std::printf("sim_motion: %s from %.3f to %.3f, the rest held.\n", kJoint, start, target);

  /* The sim steps once per incoming command, so motion needs a stream, not a shot: command at
   * 20 Hz and read back after every tenth, until the elbow has covered half the displacement
   * or the minute is up. */
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::duration<double>(kTimeoutSeconds);
  double now = start;
  bool moved = false;
  for (int cycle = 0;; ++cycle) {
    skr_command cmd;
    std::memset(&cmd, 0, sizeof cmd);
    cmd.seq = static_cast<std::uint32_t>(cycle + 1);
    cmd.mode = SKR_MODE_POSITION;
    cmd.joint_count = 29;
    cmd.values = values.data();
    cmd.mask = mask.data();
    if (driver->write_command(ctx, &cmd, 0) != SKR_OK) {
      std::fprintf(stderr, "sim_motion: a command was refused mid-proof.\n");
      driver->close(ctx);
      return 1;
    }
    if (cycle % 10 == 0) {
      if (read() != SKR_OK) {
        std::fprintf(stderr, "sim_motion: a read failed mid-proof.\n");
        driver->close(ctx);
        return 1;
      }
      now = position[elbow];
      if (std::fabs(now - start) >= kSettled) {
        moved = true;
        break;
      }
    }
    if (std::chrono::steady_clock::now() > deadline) break;
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }

  driver->close(ctx);
  if (!moved) {
    std::fprintf(stderr, "sim_motion: after sixty seconds %s sits at %.3f, having started at ",
                 kJoint, now);
    std::fprintf(stderr, "%.3f — the sim did not move.\n", start);
    return 1;
  }
  std::printf("sim_motion: %s reached %.3f from %.3f — the sim moved.\n", kJoint, now, start);
  return 0;
}
