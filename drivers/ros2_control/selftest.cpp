/*
 * skynet-ros2-control's own suite, against `mock_components/GenericSystem`.
 *
 * The substrate is `ros2_control`'s own fake hardware interface, which is what keeps this from
 * being two copies of one misunderstanding agreeing with each other: the fake is theirs, the driver
 * is ours, and neither was written from the other.
 *
 * Twelve URDFs: the two-joint body every behaviour below is measured on, vendor UR5e, ABB
 * IRB 120, FANUC LR Mate, KUKA KR 6, Kinova Gen3, Doosan M1013, Techman TM7S, AUBO i5 and Elite
 * CS520H bodies nobody here wrote, a body whose joints share no command interface, and a URDF
 * with no `<ros2_control>` element at all. The two that refuse are the interesting ones, and two
 * more refuse at the plugin rather than the file: `no_such_plugin.urdf` names a plugin nobody
 * installed, and `init_fails.urdf` hands `ros2_control`'s own fake a parameter its `on_init`
 * throws on — the manager's constructor carries on past both with nothing loaded. A
 * thirteenth, `ur5e_live.urdf`, is the live UR5e behind URSim, and its block runs only when
 * SKYNET_URSIM_LIVE=1 says the simulator is up. A fourteenth, `doosan_m1013_live.urdf`, is the
 * M1013 behind DRCF — Doosan's controller emulator, driven through Doosan's own hardware plugin —
 * and its block runs only when SKYNET_DRCF_LIVE=1. A fifteenth, `abb_irb1200_egm.urdf`, is an ABB
 * IRB 1200 over EGM through the open client against a fake controller built from ABB's
 * `egm.proto`, and its block runs only when SKYNET_EGM_LIVE=1. A sixteenth,
 * `kuka_kr6r900_rsi.urdf`, is a KUKA KR 6 R900 sixx over RSI through KUKA's own plugin against
 * KUKA's own RSI simulator, and its block runs only when SKYNET_RSI_LIVE=1. A seventeenth,
 * `aubo_i5_arcs.urdf`, is an AUBO i5 through AUBO's own plugin against ARCS, AUBO's controller
 * software, and its block runs only when SKYNET_ARCS_LIVE=1 — or its other half, against the arm
 * powered off, when SKYNET_ARCS_LIVE=unready. And one more is not a body: `write_fails.urdf` hands
 * `ros2_control`'s own `test_system` the values that make it fail on purpose (F142).
 */

#include <arpa/inet.h>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <netinet/in.h>
#include <string>
#include <sys/socket.h>
#include <thread>
#include <chrono>
#include <unistd.h>

extern "C" {
#include "skynet_rhal.h"
}

extern "C" uint32_t ros2_commands_refused(const skr_ctx* ctx);
extern "C" size_t ros2_joint_count(const skr_ctx* ctx);
extern "C" size_t ros2_mode_count(const skr_ctx* ctx);

static int failures = 0;
static int checks = 0;

static void check(bool held, const char* what) {
  checks += 1;
  std::printf(held ? "  ok    %s\n" : "  FAIL  %s\n", what);
  if (!held) failures += 1;
}

static std::string config_for(const char* name) {
  return std::string("urdf=") + SOURCE_DIR "/urdf/" + name;
}

int main() {
  const skr_driver* d = skynet_rhal_entry();
  std::printf("skynet-ros2-control selftest\n\n");

  check(d->abi_version == SKYNET_RHAL_ABI_VERSION, "the driver declares this ABI version");

  // ── a body both joints can be commanded on ────────────────────────────────
  std::printf("\ntwo_joint_mock — position and velocity on both joints\n");
  skr_ctx* ctx = nullptr;
  check(d->open(config_for("two_joint_mock.urdf").c_str(), &ctx) == SKR_OK && ctx != nullptr,
        "the resource manager loads mock_components and activates it");
  if (ctx == nullptr) {
    std::printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }
  check(ros2_joint_count(ctx) == 2, "two joints came out of the <ros2_control> element");
  check(ros2_mode_count(ctx) == 2, "and two modes, which is the intersection over both joints");

  char json[8192];
  const int wrote = d->describe(ctx, json, sizeof json);
  check(wrote > 0, "describe() writes a descriptor");
  const std::string descriptor(json, static_cast<size_t>(wrote));
  check(descriptor.find("\"dof\":2") != std::string::npos, "it declares two degrees of freedom");
  check(descriptor.find("\"shoulder\"") != std::string::npos, "and names the joints the URDF does");
  // The slug is the URDF's own `<robot name>`, slugified the way `skynet descriptor from-urdf`
  // does — so this pins the agreement, underscores and all.
  check(descriptor.find("\"robot_slug\":\"two-joint-mock\"") != std::string::npos,
        "and the slug is the file's own name, slugified like from-urdf does");
  check(descriptor.find("\"position_max_rad\":3.14") != std::string::npos,
        "travel comes from the URDF's own <limit>");
  // §63.4: a URDF effort limit is a design value from a CAD model, not a safety rating. The file
  // carries `effort="80"` and the descriptor must not.
  check(descriptor.find("effort_max_nm") == std::string::npos,
        "and no torque rating — the URDF's effort is a design value, not a safety rating (§63.4)");
  check(descriptor.find("\"envelope\":null") != std::string::npos,
        "and no safety envelope, for the same reason");
  check(descriptor.find("\"capabilities\":[\"safety.estop\"]") != std::string::npos,
        "but the software stop it has — latched, later commands refused — is declared");
  check(descriptor.find("\"simulator\":true") != std::string::npos,
        "and the default declares simulated: mock_components answers like hardware (§35.3)");
  check(descriptor.find("\"velocity\"") != std::string::npos,
        "both command modes reach the control profile");
  // §5.1's roles are the operator's and are never inferred: a `ros2_control` URDF names joints and
  // says nothing about what they are.
  check(descriptor.find("\"role\":null") != std::string::npos,
        "an unconfigured joint declares no role rather than a guessed one (§63.4)");

  // A buffer too small is an error, never a truncation.
  char tiny[8];
  check(d->describe(ctx, tiny, sizeof tiny) == SKR_EINVAL,
        "a descriptor buffer too small is refused rather than truncated");

  float position[2], velocity[2], effort[2];
  skr_state state;
  std::memset(&state, 0, sizeof state);
  state.position_rad = position;
  state.velocity_rad_s = velocity;
  state.effort_nm = effort;

  check(d->read_state(ctx, &state, 0) == SKR_OK, "read_state answers");
  check(state.joint_count == 2, "with one reading per joint");
  check(std::isfinite(position[0]) && std::isfinite(position[1]), "every position is finite");
  // The URDF declares no `effort` state interface, so there is no such reading. §24's value for a
  // quantity the driver does not measure is NaN; a zero would be a joint reported as unloaded.
  check(std::isnan(effort[0]), "an undeclared effort interface reads as NaN, not zero");
  const uint32_t first = state.seq;
  d->read_state(ctx, &state, 0);
  check(state.seq != first, "and the sequence advances between reads");

  float values[2] = {0.4f, -0.2f};
  uint8_t mask[2] = {1, 1};
  skr_command command;
  std::memset(&command, 0, sizeof command);
  command.joint_count = 2;
  command.mode = SKR_MODE_POSITION;
  command.values = values;
  command.mask = mask;
  check(d->write_command(ctx, &command, 0) == SKR_OK, "a position command is accepted");
  d->read_state(ctx, &state, 0);
  check(std::fabs(position[0] - 0.4f) < 1e-3f, "and mock_components reports the commanded position");

  uint8_t held[2] = {1, 0};
  float second[2] = {0.9f, 99.0f};
  command.values = second;
  command.mask = held;
  d->write_command(ctx, &command, 0);
  d->read_state(ctx, &state, 0);
  check(std::fabs(position[0] - 0.9f) < 1e-3f, "a masked-in joint takes the new setpoint");
  check(std::fabs(position[1] + 0.2f) < 1e-3f, "and a masked-out one holds the previous");

  command.values = values;
  command.mask = mask;
  command.joint_count = 1;
  check(d->write_command(ctx, &command, 0) == SKR_EINVAL,
        "a command with the wrong joint count is refused");
  command.joint_count = 2;
  command.mode = 200;
  check(d->write_command(ctx, &command, 0) == SKR_EINVAL, "and an unknown mode is refused");
  command.mode = SKR_MODE_TORQUE;
  check(d->write_command(ctx, &command, 0) == SKR_EPERM,
        "and a mode no joint offers is refused, rather than applied to none of them");
  command.mode = SKR_MODE_POSITION;

  // §86.1's per-joint array, on a body whose joints happen to agree. Three cases and each is a
  // different refusal or the absence of one.
  uint8_t modes[2] = {SKR_JMODE_POSITION, SKR_JMODE_INHERIT};
  command.flags = SKR_CMD_PER_JOINT_MODES;
  command.modes = modes;
  check(d->write_command(ctx, &command, 0) == SKR_OK,
        "a per-joint command naming the mode each joint offers is accepted");
  // SKR_MODE_TORQUE, and **not** SKR_MODE_POSITION, which this line asked for until the suite was
  // first run on a machine with ros2_control on it. `SKR_MODE_POSITION` is 0 and so is
  // `SKR_JMODE_INHERIT`: the one base mode whose value is a valid joint mode, and therefore the one
  // value that proves nothing about the two numberings being kept apart. The driver was right to
  // accept it and this check was wrong to demand a refusal.
  modes[1] = SKR_MODE_TORQUE;
  check(d->write_command(ctx, &command, 0) == SKR_EINVAL,
        "a bare SKR_MODE_* in the per-joint array is refused, which is what its offset is for");
  modes[1] = SKR_JMODE_TORQUE;
  check(d->write_command(ctx, &command, 0) == SKR_EPERM,
        "and a well-formed mode this joint does not offer is refused as EPERM, not EINVAL");
  modes[1] = SKR_JMODE_INHERIT;
  command.flags = 0;
  command.modes = nullptr;
  float infinite[2] = {INFINITY, 0.0f};
  command.values = infinite;
  check(d->write_command(ctx, &command, 0) == SKR_EINVAL,
        "and a setpoint that is not a number is refused");
  command.values = values;

  check(d->emergency_stop(ctx) == SKR_OK, "emergency_stop returns");
  d->read_state(ctx, &state, 0);
  check((state.flags & SKR_FLAG_ESTOP) != 0, "and the next state says the stop is asserted");
  check(d->write_command(ctx, &command, 0) == SKR_EPERM, "and a command after it is refused");
  check(ros2_commands_refused(ctx) == 7, "seven refusals, which is what the cases above asked for");
  check(d->close(ctx) == SKR_OK, "close()");

  // ── and the roles, when somebody supplies them ────────────────────────────
  std::printf("\nroles — supplied by the operator, never inferred\n");
  skr_ctx* named = nullptr;
  const std::string with_roles = config_for("two_joint_mock.urdf") +
                                 " roles=shoulder:right_arm.shoulder.yaw;elbow:right_arm.elbow.flex";
  check(d->open(with_roles.c_str(), &named) == SKR_OK, "a configuration carrying roles opens");
  const int named_len = d->describe(named, json, sizeof json);
  const std::string named_descriptor(json, static_cast<size_t>(named_len > 0 ? named_len : 0));
  check(named_descriptor.find("right_arm.shoulder.yaw") != std::string::npos,
        "and the role the operator gave reaches the descriptor");
  check(named_descriptor.find("right_arm.elbow.flex") != std::string::npos, "for every joint named");
  check(named_descriptor.find("\"role\":null") == std::string::npos,
        "and no joint is left unnamed when every one was given a role");
  d->close(named);

  // ── the vendor body: a UR5e this repository did not write ─────────────────
  //
  // `urdf/ur5e_mock.urdf` is generated from Universal_Robots_ROS2_Description — the header has
  // the commit and the command — with the vendor's own mock `hardware_interface`. Six vendor
  // joints with vendor limits: F69's body for this driver.
  //
  // The numbers below are asserted as prefixes, not whole literals, because the descriptor
  // prints twelve significant digits and the file carries sixteen. Eight digits still tell 2pi
  // from pi from a guess.
  std::printf("\nur5e_mock — six vendor joints behind the vendor's mock hardware\n");
  ctx = nullptr;
  check(d->open(config_for("ur5e_mock.urdf").c_str(), &ctx) == SKR_OK,
        "the generated URDF opens, fixed joints and all");
  if (ctx == nullptr) {
    std::printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }
  check(ros2_joint_count(ctx) == 6, "six joints came out of the <ros2_control> element");
  check(ros2_mode_count(ctx) == 3, "three modes: the vendor's mock offers effort too");
  const int ur5e = d->describe(ctx, json, sizeof json);
  check(ur5e > 0, "describe() writes a descriptor");
  const std::string ur5e_descriptor(json, static_cast<size_t>(ur5e > 0 ? ur5e : 0));
  check(ur5e_descriptor.find("\"robot_slug\":\"ur5e\"") != std::string::npos,
        "and it names the body the URDF does, not the driver (F69)");
  check(ur5e_descriptor.find("\"dof\":6") != std::string::npos, "with six degrees of freedom");
  check(ur5e_descriptor.find("\"shoulder_pan_joint\"") != std::string::npos &&
            ur5e_descriptor.find("\"wrist_3_joint\"") != std::string::npos,
        "and the joints carry the vendor's names");
  check(ur5e_descriptor.find("\"position_max_rad\":6.2831853") != std::string::npos,
        "travel comes from the vendor's <limit>");
  check(ur5e_descriptor.find("\"position_max_rad\":3.14159265") != std::string::npos,
        "including the elbow's artificial half-travel, because the file says so");
  check(ur5e_descriptor.find("\"velocity_max_rad_s\":3.14159265") != std::string::npos,
        "and the vendor's 180 deg/s, in radians");
  // §63.4 again, on a file where the declined number happens to be right: the URDF's 150/28 Nm
  // equal the vendor's published max joint torques, and the descriptor still must not carry them.
  check(ur5e_descriptor.find("effort_max_nm") == std::string::npos,
        "and still no torque rating — the URDF's effort is a design value, not a safety rating");
  check(ur5e_descriptor.find("\"envelope\":null") != std::string::npos,
        "and no safety envelope, for the same reason");
  check(ur5e_descriptor.find("\"modes\":[\"position\",\"velocity\",\"torque\"]") != std::string::npos,
        "the union carries torque, because this body's joints offer effort");

  float ur5e_position[6], ur5e_velocity[6], ur5e_effort[6];
  skr_state ur5e_state;
  std::memset(&ur5e_state, 0, sizeof ur5e_state);
  ur5e_state.position_rad = ur5e_position;
  ur5e_state.velocity_rad_s = ur5e_velocity;
  ur5e_state.effort_nm = ur5e_effort;
  check(d->read_state(ctx, &ur5e_state, 0) == SKR_OK, "read_state answers on six joints");
  check(ur5e_state.joint_count == 6, "with one reading per joint");
  // The vendor declares an `effort` state interface, so there IS such a reading here — the NaN
  // two_joint_mock reads is about the interface being absent, not about effort being special.
  check(std::isfinite(ur5e_effort[0]),
        "effort reads finite here, because the vendor declares the interface");

  float ur5e_values[6] = {0.1f, -0.2f, 0.3f, -0.4f, 0.5f, -0.6f};
  uint8_t ur5e_mask[6] = {1, 1, 1, 1, 1, 1};
  skr_command ur5e_command;
  std::memset(&ur5e_command, 0, sizeof ur5e_command);
  ur5e_command.joint_count = 6;
  ur5e_command.mode = SKR_MODE_POSITION;
  ur5e_command.values = ur5e_values;
  ur5e_command.mask = ur5e_mask;
  check(d->write_command(ctx, &ur5e_command, 0) == SKR_OK,
        "a six-joint position command is accepted");
  d->read_state(ctx, &ur5e_state, 0);
  check(std::fabs(ur5e_position[5] + 0.6f) < 1e-3f, "and the wrist reports its setpoint");
  ur5e_command.mode = SKR_MODE_TORQUE;
  check(d->write_command(ctx, &ur5e_command, 0) == SKR_OK,
        "and torque is accepted, because the union above is not decorative");
  d->close(ctx);

  // ── roles on vendor joint names ────────────────────────────────────────────
  //
  // The suite as the operator, reading `drivers/ur_rtde`'s kinematics: a six-axis arm with a
  // spherical wrist, which is what every UR is.
  std::printf("\nur5e roles — the operator names six vendor joints\n");
  skr_ctx* ur5e_named = nullptr;
  const std::string ur5e_roles =
      config_for("ur5e_mock.urdf") +
      " roles=shoulder_pan_joint:right_arm.shoulder.yaw;"
      "shoulder_lift_joint:right_arm.shoulder.pitch;"
      "elbow_joint:right_arm.elbow.flex;"
      "wrist_1_joint:right_arm.wrist.pitch;"
      "wrist_2_joint:right_arm.wrist.yaw;"
      "wrist_3_joint:right_arm.wrist.roll";
  check(d->open(ur5e_roles.c_str(), &ur5e_named) == SKR_OK,
        "a configuration carrying six roles opens");
  const int ur5e_named_len = d->describe(ur5e_named, json, sizeof json);
  const std::string ur5e_named_descriptor(
      json, static_cast<size_t>(ur5e_named_len > 0 ? ur5e_named_len : 0));
  check(ur5e_named_descriptor.find("right_arm.wrist.roll") != std::string::npos,
        "and the wrist's role reaches the descriptor");
  check(ur5e_named_descriptor.find("\"role\":null") == std::string::npos,
        "with no joint left unnamed");
  d->close(ur5e_named);

  // ── the ABB body: an IRB 120 behind the house mock tags ───────────────────
  //
  // `urdf/abb_irb120_mock.urdf` is generated from ros-industrial/abb; the `<ros2_control>`
  // block is ours (the header says so), because the vendor package predates the tags. Six vendor
  // joints with vendor limits: the ABB description substrate, standing in for the live EGM row.
  std::printf("\nabb_irb120_mock — six vendor joints behind the house mock tags\n");
  ctx = nullptr;
  check(d->open(config_for("abb_irb120_mock.urdf").c_str(), &ctx) == SKR_OK,
        "the generated URDF opens");
  if (ctx == nullptr) {
    std::printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }
  check(ros2_joint_count(ctx) == 6, "six joints came out of the <ros2_control> element");
  check(ros2_mode_count(ctx) == 2, "two modes: the house tags offer position and velocity");
  const int abb = d->describe(ctx, json, sizeof json);
  check(abb > 0, "describe() writes a descriptor");
  const std::string abb_descriptor(json, static_cast<size_t>(abb > 0 ? abb : 0));
  check(abb_descriptor.find("\"robot_slug\":\"abb-irb120-3-58\"") != std::string::npos,
        "and it names the body the URDF does");
  check(abb_descriptor.find("\"dof\":6") != std::string::npos, "with six degrees of freedom");
  check(abb_descriptor.find("\"joint_1\"") != std::string::npos &&
            abb_descriptor.find("\"joint_6\"") != std::string::npos,
        "and the joints carry the vendor's names");
  check(abb_descriptor.find("\"position_max_rad\":2.87979") != std::string::npos,
        "travel comes from the vendor's <limit>");
  check(abb_descriptor.find("\"velocity_max_rad_s\":4.36332") != std::string::npos,
        "and the vendor's 250 deg/s on the base axes");
  check(abb_descriptor.find("\"velocity_max_rad_s\":7.33038") != std::string::npos,
        "with 420 deg/s on the wrist");
  // §63.4 a third time, on the file where reading it would brick the arm: ros-industrial writes
  // `effort="0"` as a placeholder, and a zero torque rating is a clamp that refuses every motion.
  check(abb_descriptor.find("effort_max_nm") == std::string::npos,
        "and no torque rating — the URDF's zero effort is a placeholder, not a rating");
  check(abb_descriptor.find("\"envelope\":null") != std::string::npos,
        "and no safety envelope, for the same reason");
  check(abb_descriptor.find("\"modes\":[\"position\",\"velocity\"]") != std::string::npos,
        "the union carries what the tags offer, no more");

  float abb_position[6], abb_velocity[6], abb_effort[6];
  skr_state abb_state;
  std::memset(&abb_state, 0, sizeof abb_state);
  abb_state.position_rad = abb_position;
  abb_state.velocity_rad_s = abb_velocity;
  abb_state.effort_nm = abb_effort;
  check(d->read_state(ctx, &abb_state, 0) == SKR_OK, "read_state answers on six joints");
  check(abb_state.joint_count == 6, "with one reading per joint");
  check(std::isnan(abb_effort[0]), "effort reads NaN here — the tags declare no interface");

  float abb_values[6] = {0.5f, -0.4f, 0.3f, -0.2f, 0.1f, -0.6f};
  uint8_t abb_mask[6] = {1, 1, 1, 1, 1, 1};
  skr_command abb_command;
  std::memset(&abb_command, 0, sizeof abb_command);
  abb_command.joint_count = 6;
  abb_command.mode = SKR_MODE_POSITION;
  abb_command.values = abb_values;
  abb_command.mask = abb_mask;
  check(d->write_command(ctx, &abb_command, 0) == SKR_OK,
        "a six-joint position command is accepted");
  d->read_state(ctx, &abb_state, 0);
  check(std::fabs(abb_position[0] - 0.5f) < 1e-3f, "and the base reports its setpoint");
  d->close(ctx);

  // ── roles on numbered joint names ──────────────────────────────────────────
  std::printf("\nabb roles — the operator names six numbered joints\n");
  skr_ctx* abb_named = nullptr;
  const std::string abb_roles =
      config_for("abb_irb120_mock.urdf") +
      " roles=joint_1:right_arm.shoulder.yaw;"
      "joint_2:right_arm.shoulder.pitch;"
      "joint_3:right_arm.elbow.flex;"
      "joint_4:right_arm.wrist.pitch;"
      "joint_5:right_arm.wrist.yaw;"
      "joint_6:right_arm.wrist.roll";
  check(d->open(abb_roles.c_str(), &abb_named) == SKR_OK,
        "a configuration carrying six roles opens");
  const int abb_named_len = d->describe(abb_named, json, sizeof json);
  const std::string abb_named_descriptor(
      json, static_cast<size_t>(abb_named_len > 0 ? abb_named_len : 0));
  check(abb_named_descriptor.find("right_arm.elbow.flex") != std::string::npos,
        "and the elbow's role reaches the descriptor");
  check(abb_named_descriptor.find("\"role\":null") == std::string::npos,
        "with no joint left unnamed");
  d->close(abb_named);

  // ── the vendor table: every description substrate behind the house tags ────
  //
  // The UR5e, the IRB 120 and the Gen3 above are worked by hand because they were first or odd.
  // Every vendor body after them runs through this table instead: the same facts (opens, the
  // joint count, the modes the tags offer, the slug the file names, vendor travel and speed, no
  // torque rating, a command round-trip), one row each. Joint counts vary — the Elite is a
  // five-axis arm — and so does effort, which reads finite exactly when the vendor declares the
  // state interface. Roles stay out of the table: the matcher is already covered on three naming
  // schemes. A body that needs more than the table says gets its own block again.
  //
  // Travel and speed are prefixes (the UR5e block's convention): the descriptor prints twelve
  // significant digits and the files carry up to sixteen.
  struct VendorBody {
    const char* file;
    const char* slug;
    const char* first_joint;
    const char* last_joint;
    const char* travel;
    const char* speed;
    const char* modes;
    int mode_count;
    int joints;
    bool effort_finite;
  };
  const VendorBody vendors[] = {
      {"fanuc_lrmate200id_mock.urdf", "fanuc-lrmate200id", "joint_1", "joint_6",
       "\"position_max_rad\":2.96705972", "\"velocity_max_rad_s\":7.85398163",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, false},
      {"kuka_kr6r900_mock.urdf", "kuka-kr6r900sixx", "joint_a1", "joint_a6",
       "\"position_max_rad\":2.96705972", "\"velocity_max_rad_s\":6.28318530",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, false},
      {"doosan_m1013_mock.urdf", "m1013", "joint_1", "joint_6",
       "\"position_max_rad\":6.2832", "\"velocity_max_rad_s\":2.0944",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, false},
      {"techman_tm7s_mock.urdf", "tm7s", "joint_1", "joint_6",
       "\"position_max_rad\":6.28318530", "\"velocity_max_rad_s\":3.66519142",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, false},
      {"aubo_i5s_mock.urdf", "aubo-i5s", "shoulder_joint", "wrist3_joint",
       "\"position_max_rad\":3.05432619", "\"velocity_max_rad_s\":3.8397",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, false},
      {"elite_cs520h_mock.urdf", "elite-cs520h", "shoulder_pan_joint", "wrist_2_joint",
       "\"position_max_rad\":6.28318530", "\"velocity_max_rad_s\":2.18166156",
       "\"modes\":[\"position\",\"velocity\"]", 2, 5, true},
      {"kawasaki_rs007l_mock.urdf", "rs007l-b001", "joint1", "joint6",
       "\"position_max_rad\":3.14159265", "\"velocity_max_rad_s\":6.0057",
       "\"modes\":[\"position\"]", 1, 6, true},
      {"denso_vs060_mock.urdf", "vs060", "joint_1", "joint_6",
       "\"position_max_rad\":2.96705972", "\"velocity_max_rad_s\":3.73064127",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, true},
      {"mitsubishi_rv4fr_mock.urdf", "rv4fr", "rv4fr_joint_1", "rv4fr_joint_6",
       "\"position_max_rad\":4.18879020", "\"velocity_max_rad_s\":7.85398163",
       "\"modes\":[\"position\"]", 1, 6, false},
      {"staubli_tx260l_mock.urdf", "tx2-60l", "joint_1", "joint_6",
       "\"position_max_rad\":3.14159265", "\"velocity_max_rad_s\":5.23598775",
       "\"modes\":[\"position\",\"velocity\"]", 2, 6, false},
  };
  for (const VendorBody& body : vendors) {
    std::printf("\n%s — the table's row for it\n", body.file);
    const std::string tag = std::string("[") + body.slug + "] ";
    ctx = nullptr;
    check(d->open(config_for(body.file).c_str(), &ctx) == SKR_OK,
          (tag + "the generated URDF opens").c_str());
    if (ctx == nullptr) {
      failures += 1;
      continue;
    }
    check(ros2_joint_count(ctx) == static_cast<size_t>(body.joints),
          (tag + "the vendor's joint count").c_str());
    check(ros2_mode_count(ctx) == static_cast<size_t>(body.mode_count),
          (tag + "the modes the tags offer").c_str());
    const int len = d->describe(ctx, json, sizeof json);
    check(len > 0, (tag + "describe() writes a descriptor").c_str());
    const std::string described(json, static_cast<size_t>(len > 0 ? len : 0));
    check(described.find(std::string("\"robot_slug\":\"") + body.slug) != std::string::npos,
          (tag + "it names the body the URDF does").c_str());
    check(described.find("\"dof\":" + std::to_string(body.joints)) != std::string::npos,
          (tag + "with matching degrees of freedom").c_str());
    check(described.find(std::string("\"") + body.first_joint + "\"") != std::string::npos &&
              described.find(std::string("\"") + body.last_joint + "\"") != std::string::npos,
          (tag + "the joints carry the vendor's names").c_str());
    check(described.find(body.travel) != std::string::npos,
          (tag + "travel comes from the vendor's <limit>").c_str());
    check(described.find(body.speed) != std::string::npos,
          (tag + "and the vendor's speed, in radians").c_str());
    check(described.find("effort_max_nm") == std::string::npos,
          (tag + "and no torque rating (§63.4)").c_str());
    check(described.find("\"envelope\":null") != std::string::npos,
          (tag + "and no safety envelope").c_str());
    check(described.find(body.modes) != std::string::npos,
          (tag + "the union carries what the tags offer").c_str());

    float position[7] = {0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f};
    float velocity[7] = {0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f};
    float effort[7] = {0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f};
    skr_state state;
    std::memset(&state, 0, sizeof state);
    state.position_rad = position;
    state.velocity_rad_s = velocity;
    state.effort_nm = effort;
    check(d->read_state(ctx, &state, 0) == SKR_OK, (tag + "read_state answers").c_str());
    check(state.joint_count == static_cast<uint32_t>(body.joints),
          (tag + "with one reading per joint").c_str());
    if (body.effort_finite) {
      check(std::isfinite(effort[0]),
            (tag + "effort reads finite — the vendor declares the interface").c_str());
    } else {
      check(std::isnan(effort[0]), (tag + "effort reads NaN — no interface declared").c_str());
    }

    float values[7] = {0.2f, -0.3f, 0.1f, -0.5f, 0.4f, -0.1f, 0.0f};
    uint8_t mask[7] = {1, 1, 1, 1, 1, 1, 1};
    skr_command command;
    std::memset(&command, 0, sizeof command);
    command.joint_count = static_cast<uint32_t>(body.joints);
    command.mode = SKR_MODE_POSITION;
    command.values = values;
    command.mask = mask;
    check(d->write_command(ctx, &command, 0) == SKR_OK,
          (tag + "a full position command is accepted").c_str());
    d->read_state(ctx, &state, 0);
    check(std::fabs(position[2] - 0.1f) < 1e-3f,
          (tag + "and the middle joint reports its setpoint").c_str());
    d->close(ctx);
  }

  // ── the seven-axis body: a Kinova Gen3, four joints of it continuous ───────
  //
  // `urdf/kinova_gen3_mock.urdf` is generated from Kinovarobotics/ros2_kortex with the vendor's
  // own `use_fake_hardware` tags. Seven joints, position only, and the first continuous joints
  // this driver has met — which is why this body is worked by hand rather than through the table.
  std::printf("\nkinova_gen3_mock — seven vendor joints, four of them continuous\n");
  ctx = nullptr;
  check(d->open(config_for("kinova_gen3_mock.urdf").c_str(), &ctx) == SKR_OK,
        "the generated URDF opens");
  if (ctx == nullptr) {
    std::printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }
  check(ros2_joint_count(ctx) == 7, "seven joints came out of the <ros2_control> element");
  check(ros2_mode_count(ctx) == 1, "one mode: the vendor's tags offer position only");
  const int kinova = d->describe(ctx, json, sizeof json);
  check(kinova > 0, "describe() writes a descriptor");
  const std::string kinova_descriptor(json, static_cast<size_t>(kinova > 0 ? kinova : 0));
  check(kinova_descriptor.find("\"robot_slug\":\"kinova-gen3\"") != std::string::npos,
        "and it names the body the URDF does");
  check(kinova_descriptor.find("\"dof\":7") != std::string::npos, "with seven degrees of freedom");
  check(kinova_descriptor.find("\"joint_1\"") != std::string::npos &&
            kinova_descriptor.find("\"joint_7\"") != std::string::npos,
        "and the joints carry the vendor's names");
  check(kinova_descriptor.find("\"position_max_rad\":2.24") != std::string::npos,
        "travel comes from the vendor's <limit>, where the joint has any");
  check(kinova_descriptor.find("\"velocity_max_rad_s\":1.3963") != std::string::npos,
        "and the vendor's 80 deg/s, in radians");
  check(kinova_descriptor.find("effort_max_nm") == std::string::npos,
        "and no torque rating — the URDF's effort is a design value, not a safety rating");
  check(kinova_descriptor.find("\"envelope\":null") != std::string::npos,
        "and no safety envelope, for the same reason");
  check(kinova_descriptor.find("\"modes\":[\"position\"]") != std::string::npos,
        "the union carries what the tags offer, no more");
  // The bridge used to say `revolute` for every joint. This file has both kinds, so it pins both.
  check(kinova_descriptor.find("\"type\":\"continuous\"") != std::string::npos,
        "a joint without travel limits is described as continuous, not revolute");
  check(kinova_descriptor.find("\"type\":\"revolute\"") != std::string::npos,
        "and a joint with them still says revolute");

  float kinova_position[7], kinova_velocity[7], kinova_effort[7];
  skr_state kinova_state;
  std::memset(&kinova_state, 0, sizeof kinova_state);
  kinova_state.position_rad = kinova_position;
  kinova_state.velocity_rad_s = kinova_velocity;
  kinova_state.effort_nm = kinova_effort;
  check(d->read_state(ctx, &kinova_state, 0) == SKR_OK, "read_state answers on seven joints");
  check(kinova_state.joint_count == 7, "with one reading per joint");
  check(std::isfinite(kinova_effort[0]),
        "effort reads finite here, because the vendor declares the interface");

  float kinova_values[7] = {0.1f, -0.2f, 0.3f, -0.4f, 0.5f, -0.6f, 0.7f};
  uint8_t kinova_mask[7] = {1, 1, 1, 1, 1, 1, 1};
  skr_command kinova_command;
  std::memset(&kinova_command, 0, sizeof kinova_command);
  kinova_command.joint_count = 7;
  kinova_command.mode = SKR_MODE_POSITION;
  kinova_command.values = kinova_values;
  kinova_command.mask = kinova_mask;
  check(d->write_command(ctx, &kinova_command, 0) == SKR_OK,
        "a seven-joint position command is accepted");
  d->read_state(ctx, &kinova_state, 0);
  check(std::fabs(kinova_position[6] - 0.7f) < 1e-3f, "and the last joint reports its setpoint");
  d->close(ctx);

  // ── roles on seven joints ──────────────────────────────────────────────────
  //
  // The suite as the operator, reading a seven-axis arm: waist yaw, shoulder pitch, upper-arm
  // roll, elbow flex, and a three-axis wrist.
  std::printf("\nkinova roles — the operator names seven joints\n");
  skr_ctx* kinova_named = nullptr;
  const std::string kinova_roles =
      config_for("kinova_gen3_mock.urdf") +
      " roles=joint_1:right_arm.shoulder.yaw;"
      "joint_2:right_arm.shoulder.pitch;"
      "joint_3:right_arm.shoulder.roll;"
      "joint_4:right_arm.elbow.flex;"
      "joint_5:right_arm.wrist.yaw;"
      "joint_6:right_arm.wrist.pitch;"
      "joint_7:right_arm.wrist.roll";
  check(d->open(kinova_roles.c_str(), &kinova_named) == SKR_OK,
        "a configuration carrying seven roles opens");
  const int kinova_named_len = d->describe(kinova_named, json, sizeof json);
  const std::string kinova_named_descriptor(
      json, static_cast<size_t>(kinova_named_len > 0 ? kinova_named_len : 0));
  check(kinova_named_descriptor.find("right_arm.shoulder.roll") != std::string::npos,
        "and the third joint's role reaches the descriptor");
  check(kinova_named_descriptor.find("\"role\":null") == std::string::npos,
        "with no joint left unnamed");
  d->close(kinova_named);

  // ── the body §86 made commandable ─────────────────────────────────────────
  //
  // This block is the amendment, inverted in place. Every assertion below used to say the opposite:
  // no modes, an empty `modes` array in the descriptor, and a refusal for the one mode a joint did
  // offer. The file it runs against has not changed a byte.
  std::printf("\nmixed_interfaces — position on one joint, velocity on the other\n");
  ctx = nullptr;
  check(d->open(config_for("mixed_interfaces.urdf").c_str(), &ctx) == SKR_OK,
        "the body comes up, because its joints are readable");
  check(ros2_mode_count(ctx) == 2,
        "and offers both modes, because §86's command carries one per joint");
  const int mixed = d->describe(ctx, json, sizeof json);
  const std::string mixed_descriptor(json, static_cast<size_t>(mixed > 0 ? mixed : 0));
  check(mixed_descriptor.find("\"modes\":[\"position\",\"velocity\"]") != std::string::npos,
        "which the descriptor says as the union rather than the intersection");
  check(mixed_descriptor.find("\"per_joint_modes\":true") != std::string::npos,
        "and it declares that this driver reads the per-joint array");

  float mixed_values[2] = {0.3f, 0.5f};
  uint8_t mixed_mask[2] = {1, 1};
  uint8_t mixed_modes[2] = {SKR_JMODE_POSITION, SKR_JMODE_VELOCITY};
  skr_command mixed_command;
  std::memset(&mixed_command, 0, sizeof mixed_command);
  mixed_command.joint_count = 2;
  mixed_command.mode = SKR_MODE_POSITION;
  mixed_command.flags = SKR_CMD_PER_JOINT_MODES;
  mixed_command.values = mixed_values;
  mixed_command.mask = mixed_mask;
  mixed_command.modes = mixed_modes;
  check(d->write_command(ctx, &mixed_command, 0) == SKR_OK,
        "a shoulder on position and an elbow on velocity, commanded in one frame");

  // And the v1 shape against the same body: one mode for everything, which is what a producer that
  // has never heard of §86 sends. It reaches a joint that does not offer it and is refused by name.
  mixed_command.flags = 0;
  mixed_command.modes = nullptr;
  check(d->write_command(ctx, &mixed_command, 0) == SKR_EPERM,
        "and a body-wide command is still refused, because one joint does not take that mode");
  d->close(ctx);

  // ── the operator's declaration, when the plugin is real ───────────────────
  std::printf("\nsimulator=false — a real integration says so deliberately\n");
  ctx = nullptr;
  const std::string real_config = config_for("two_joint_mock.urdf") + " simulator=false";
  check(d->open(real_config.c_str(), &ctx) == SKR_OK, "the flag rides beside the URDF path");
  const int real = d->describe(ctx, json, sizeof json);
  const std::string real_descriptor(json, static_cast<size_t>(real > 0 ? real : 0));
  check(real_descriptor.find("\"simulator\":false") != std::string::npos,
        "and declares real when the operator says so");
  d->close(ctx);

  // ── the modes the plugin honours, when the integrator says so ───────────────
  //
  // `modes=` narrows what the URDF's interface names offer, for a plugin that uses a name for
  // something else — Doosan's `velocity` is a feed-forward, not a setpoint (F126). Measured here
  // on the mock, which honours both, so what is asserted is the narrowing and its refusals.
  std::printf("\nmodes= — the integrator's declaration of what the plugin honours\n");
  ctx = nullptr;
  check(d->open((config_for("two_joint_mock.urdf") + " modes=position").c_str(), &ctx) == SKR_OK &&
            ctx != nullptr,
        "modes=position opens");
  if (ctx != nullptr) {
    check(ros2_mode_count(ctx) == 1, "and leaves one mode");
    const int narrowed = d->describe(ctx, json, sizeof json);
    const std::string narrowed_descriptor(json, static_cast<size_t>(narrowed > 0 ? narrowed : 0));
    check(narrowed_descriptor.find("\"modes\":[\"position\"]") != std::string::npos,
          "the control profile offers position alone");
    check(narrowed_descriptor.find("\"velocity\"") == std::string::npos,
          "and no joint offers velocity any more");
    float narrow_values[2] = {0.1f, 0.1f};
    uint8_t narrow_mask[2] = {1, 1};
    skr_command narrow;
    std::memset(&narrow, 0, sizeof narrow);
    narrow.joint_count = 2;
    narrow.values = narrow_values;
    narrow.mask = narrow_mask;
    narrow.mode = SKR_MODE_VELOCITY;
    check(d->write_command(ctx, &narrow, 0) == SKR_EPERM,
          "a velocity command is refused: the integrator said the plugin does not honour it");
    narrow.mode = SKR_MODE_POSITION;
    check(d->write_command(ctx, &narrow, 0) == SKR_OK, "a position command still goes");
    d->close(ctx);
  }
  ctx = nullptr;
  check(d->open((config_for("two_joint_mock.urdf") + " modes=position;velocity").c_str(), &ctx) ==
                SKR_OK &&
            ctx != nullptr && ros2_mode_count(ctx) == 2,
        "modes=position;velocity keeps both");
  if (ctx != nullptr) d->close(ctx);
  ctx = nullptr;
  check(d->open((config_for("two_joint_mock.urdf") + " modes=torque").c_str(), &ctx) == SKR_ENODEV,
        "a mode no joint's interfaces offer is refused: the key narrows, it cannot add");
  check(d->open((config_for("two_joint_mock.urdf") + " modes=speed").c_str(), &ctx) == SKR_ENODEV,
        "and a name that is not one of §24's modes is refused");

  // ── a URDF with nothing to command ────────────────────────────────────────
  std::printf("\nrefusals\n");
  ctx = nullptr;
  check(d->open(config_for("no_ros2_control.urdf").c_str(), &ctx) == SKR_ENODEV,
        "a URDF with no <ros2_control> element is refused");
  check(d->open("", &ctx) == SKR_ENODEV, "a configuration naming no URDF is refused");
  check(d->open("urdf=/nonexistent.urdf", &ctx) == SKR_ENODEV, "and a path that is not there");
  ctx = nullptr;
  check(d->open(config_for("no_such_plugin.urdf").c_str(), &ctx) == SKR_ENODEV && ctx == nullptr,
        "a URDF naming a plugin nobody installed is refused, although the manager reports no "
        "component to refuse");
  check(d->open(config_for("init_fails.urdf").c_str(), &ctx) == SKR_ENODEV && ctx == nullptr,
        "and so is a plugin whose on_init fails: the manager clears what it loaded and carries on");
  check(d->open(config_for("two_joint_mock.urdf").c_str(), &ctx) == SKR_OK && ctx != nullptr,
        "and a good body still opens after both");
  if (ctx != nullptr) d->close(ctx);
  ctx = nullptr;

  // ── a plugin that fails mid-session (F142) ───────────────────────────────
  //
  // ros2_control's own `test_system` fails on purpose when its first velocity command holds one
  // of `ros2_control_test_assets`' sentinels, and the manager takes it down. What the session says
  // afterwards is the question: the failure once, from the call it happened in, and then for as
  // long as it lasts. Two sentinels survive the float a command carries — `READ_FAIL_VALUE`
  // (28282828, the next read returns ERROR) and `WRITE_DEACTIVATE_VALUE` (24242424, the write
  // returns DEACTIVATE); the other two are odd numbers past 2^24 and arrive as their neighbours.
  auto failing_body = [&](float sentinel, const char* heading, const char* failure) {
    std::printf("\nwrite_fails — ros2_control's test_system, %s\n", heading);
    ctx = nullptr;
    check(d->open((config_for("write_fails.urdf") + " update_rate=100").c_str(), &ctx) == SKR_OK &&
              ctx != nullptr,
          "ros2_control's test component opens and activates");
    if (ctx == nullptr) return;
    float fpos[1] = {0};
    skr_state fstate;
    std::memset(&fstate, 0, sizeof fstate);
    fstate.position_rad = fpos;
    float speed[1] = {0.1f};
    skr_command spin;
    std::memset(&spin, 0, sizeof spin);
    spin.mode = SKR_MODE_VELOCITY;
    spin.joint_count = 1;
    spin.values = speed;
    check(d->read_state(ctx, &fstate, 0) == SKR_OK && d->write_command(ctx, &spin, 0) == SKR_OK &&
              d->read_state(ctx, &fstate, 0) == SKR_OK && (fstate.flags & SKR_FLAG_FAULT) == 0,
          "a read, an ordinary command and a read go through, and the state is healthy");
    speed[0] = sentinel;
    const int sent = d->write_command(ctx, &spin, 0);
    const int first = d->read_state(ctx, &fstate, 0);
    std::printf("  note  the sentinel command returned %d, the read after it %d\n", sent, first);
    check(sent != SKR_OK || first != SKR_OK, failure);
    bool stays = true;
    for (int i = 0; stays && i < 3; ++i) {
      stays = d->read_state(ctx, &fstate, 0) == SKR_EIO && (fstate.flags & SKR_FLAG_FAULT) != 0 &&
              fstate.fault_code == 1;
    }
    check(stays,
          "and every state after it says so — EIO, the fault flag, one component down — where "
          "the manager answers OK for a component it has taken down (F142)");
    speed[0] = 0.1f;
    check(d->write_command(ctx, &spin, 0) == SKR_EIO,
          "and an ordinary command after it is refused: nothing is behind it for this session");
    d->close(ctx);
    ctx = nullptr;
    check(d->open((config_for("write_fails.urdf") + " update_rate=100").c_str(), &ctx) == SKR_OK &&
              ctx != nullptr && d->read_state(ctx, &fstate, 0) == SKR_OK &&
              (fstate.flags & SKR_FLAG_FAULT) == 0,
          "and reopening the body is what brings it back");
    if (ctx != nullptr) d->close(ctx);
    ctx = nullptr;
  };
  failing_body(28282828.0f, "failing a read on purpose",
               "the failed read is reported, by the call it happened in");
  failing_body(24242424.0f, "deactivating itself from a write",
               "the deactivation is reported, by the call it happened in");

  // ── the live UR5e, when the operator says URSim is up ─────────────────────
  //
  // SKYNET_URSIM_LIVE=1 runs this block against the URSim the addresses baked into
  // urdf/ur5e_live.urdf point at. Anything else skips it by name: CI has no simulator,
  // and a suite that dialled a robot unasked would be a menace.
  const char* ursim_gate = std::getenv("SKYNET_URSIM_LIVE");
  if (ursim_gate == nullptr || std::strcmp(ursim_gate, "1") != 0) {
    std::printf(
        "\nur5e_live — skipped: SKYNET_URSIM_LIVE is not 1 (the URSim block, see the README)\n");
  } else {
    std::printf("\nur5e_live — the live arm behind URSim\n");
    // The dashboard address is the plugin's own robot_ip, read out of the URDF so the wake
    // and the driver can never disagree about which machine they mean.
    std::string dashboard_ip = "127.0.0.1";
    std::ifstream urdf(SOURCE_DIR "/urdf/ur5e_live.urdf");
    std::string urdf_text((std::istreambuf_iterator<char>(urdf)), std::istreambuf_iterator<char>());
    const size_t ip_at = urdf_text.find("robot_ip\">");
    if (ip_at != std::string::npos) {
      const size_t ip_end = urdf_text.find('<', ip_at);
      if (ip_end != std::string::npos) dashboard_ip = urdf_text.substr(ip_at + 10, ip_end - ip_at - 10);
    }
    // One dashboard round trip per connection; the server greets first and answers in plain
    // text. This is the headless pendant: power on, brake release, and the mode poll.
    auto dashboard = [&](const char* command, char* reply, size_t cap) -> bool {
      const int fd = ::socket(AF_INET, SOCK_STREAM, 0);
      if (fd < 0) return false;
      struct sockaddr_in addr;
      std::memset(&addr, 0, sizeof addr);
      addr.sin_family = AF_INET;
      addr.sin_port = htons(29999);
      if (::inet_pton(AF_INET, dashboard_ip.c_str(), &addr.sin_addr) != 1) {
        ::close(fd);
        return false;
      }
      const struct timeval timeout = {5, 0};
      ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
      if (::connect(fd, reinterpret_cast<struct sockaddr*>(&addr), sizeof addr) != 0) {
        ::close(fd);
        return false;
      }
      char greeting[256];
      ::recv(fd, greeting, sizeof greeting - 1, 0);
      ::send(fd, command, std::strlen(command), 0);
      ::send(fd, "\n", 1, 0);
      const ssize_t got = ::recv(fd, reply, cap - 1, 0);
      ::close(fd);
      if (got <= 0) return false;
      reply[static_cast<size_t>(got)] = '\0';
      return true;
    };
    char reply[512];
    bool awake = dashboard("close popup", reply, sizeof reply) &&
                 dashboard("close safety popup", reply, sizeof reply) &&
                 dashboard("power on", reply, sizeof reply);
    std::this_thread::sleep_for(std::chrono::seconds(4));
    awake = awake && dashboard("brake release", reply, sizeof reply);
    std::this_thread::sleep_for(std::chrono::seconds(3));
    for (int i = 0; awake && i < 10; ++i) {
      if (!dashboard("robotmode", reply, sizeof reply)) {
        awake = false;
        break;
      }
      if (std::string(reply).find("RUNNING") != std::string::npos) break;
      if (i == 9) awake = false;
      std::this_thread::sleep_for(std::chrono::seconds(1));
    }
    check(awake, "the dashboard answers and the arm wakes to RUNNING");

    // update_rate=500 is F93: the declaration's 2 kHz negotiates RTDE at a rate URSim
    // answers with silence, and the settle then reads nothing.
    ctx = nullptr;
    check(d->open((config_for("ur5e_live.urdf") + " update_rate=500").c_str(), &ctx) == SKR_OK &&
              ctx != nullptr,
          "the live body comes up at update_rate=500");
    if (ctx != nullptr) {
      check(ros2_joint_count(ctx) == 6, "six joints, the UR5e's own count");
      const int live = d->describe(ctx, json, sizeof json);
      const std::string live_descriptor(json, static_cast<size_t>(live > 0 ? live : 0));
      check(live > 0 && live_descriptor.find("\"dof\":6") != std::string::npos,
            "the descriptor declares six degrees of freedom");
      check(live_descriptor.find("\"name\":\"elbow_joint\"") != std::string::npos,
            "and names the elbow the URDF does");
      check(live_descriptor.find("\"position_max_rad\":3.14") != std::string::npos,
            "elbow travel is the URDF's own +/- pi");
      check(live_descriptor.find("\"velocity_max_rad_s\":3.14") != std::string::npos,
            "and its speed limit likewise");
      check(live_descriptor.find("\"modes\":[\"position\",\"velocity\",\"torque\"]") != std::string::npos,
            "the union offers all three modes, every joint commanding every one");

      float live_pos[6] = {0, 0, 0, 0, 0, 0};
      float live_vel[6] = {0, 0, 0, 0, 0, 0};
      float live_eff[6] = {0, 0, 0, 0, 0, 0};
      skr_state live_state;
      std::memset(&live_state, 0, sizeof live_state);
      live_state.position_rad = live_pos;
      live_state.velocity_rad_s = live_vel;
      live_state.effort_nm = live_eff;
      bool finite = d->read_state(ctx, &live_state, 0) == SKR_OK;
      for (int i = 0; finite && i < 6; ++i) finite = std::isfinite(live_pos[i]);
      check(finite, "the first live read is finite on all six");

      // The commanding contract is measured, not documented: every write is preceded by a
      // read, rounds run at 50 Hz, and the setpoints are the arm's own pose, because a stale
      // hold trips the script's speed check and a gap over 40 ms trips the plugin's receive
      // timeout. Thirty true-hold rounds prime the script; then the elbow ramps +0.08 rad in
      // 0.001 rad steps and ramps back, and the readback is the assertion.
      float setpoint[6];
      std::memcpy(setpoint, live_pos, sizeof setpoint);
      uint8_t live_mask[6] = {1, 1, 1, 1, 1, 1};
      skr_command live_command;
      std::memset(&live_command, 0, sizeof live_command);
      live_command.mode = SKR_MODE_POSITION;
      live_command.joint_count = 6;
      live_command.values = setpoint;
      live_command.mask = live_mask;
      bool primed = true;
      for (int i = 0; primed && i < 30; ++i) {
        primed = d->read_state(ctx, &live_state, 0) == SKR_OK;
        std::memcpy(setpoint, live_pos, sizeof setpoint);
        primed = primed && d->write_command(ctx, &live_command, 0) == SKR_OK;
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
      }
      check(primed, "thirty true-hold rounds prime the script without a popup");
      d->read_state(ctx, &live_state, 0);
      const float elbow_home = live_pos[2];
      float elbow_target = elbow_home + 0.08f;
      if (elbow_target > 2.5f) elbow_target = elbow_home - 0.08f;
      const float elbow_dir = elbow_target > elbow_home ? 1.0f : -1.0f;
      bool ramped = true;
      for (int i = 0; ramped && i < 80; ++i) {
        ramped = d->read_state(ctx, &live_state, 0) == SKR_OK;
        std::memcpy(setpoint, live_pos, sizeof setpoint);
        float step = elbow_home + elbow_dir * static_cast<float>(i + 1) * 0.001f;
        if ((elbow_dir > 0 && step > elbow_target) || (elbow_dir < 0 && step < elbow_target)) {
          step = elbow_target;
        }
        setpoint[2] = step;
        ramped = ramped && d->write_command(ctx, &live_command, 0) == SKR_OK;
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
      }
      d->read_state(ctx, &live_state, 0);
      const float travelled = std::fabs(live_pos[2] - elbow_home);
      check(ramped && travelled > 0.05f, "the elbow travelled out and the readback says so");
      bool restored = true;
      for (int i = 0; restored && i < 80; ++i) {
        restored = d->read_state(ctx, &live_state, 0) == SKR_OK;
        std::memcpy(setpoint, live_pos, sizeof setpoint);
        float step = elbow_target - elbow_dir * static_cast<float>(i + 1) * 0.001f;
        if ((elbow_dir > 0 && step < elbow_home) || (elbow_dir < 0 && step > elbow_home)) {
          step = elbow_home;
        }
        setpoint[2] = step;
        restored = restored && d->write_command(ctx, &live_command, 0) == SKR_OK;
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
      }
      d->read_state(ctx, &live_state, 0);
      check(restored && std::fabs(live_pos[2] - elbow_home) < 0.02f,
            "and came back to where the block found it");
      d->close(ctx);
    }
  }

  // ── the Doosan M1013 behind DRCF, when the operator says the emulator is up ─
  //
  // SKYNET_DRCF_LIVE=1 runs this block against DRCF, Doosan's controller emulator, through
  // Doosan's own `dsr_hardware2/DRHWInterface` — the first vendor plugin this bridge hosts that
  // reads the period it is handed (DRY-COVERAGE-PLAN.md Phase 8, item 8.3). `make
  // check-drcf-docker` is the place it runs; anything else skips it by name.
  const char* drcf_gate = std::getenv("SKYNET_DRCF_LIVE");
  if (drcf_gate == nullptr || std::strcmp(drcf_gate, "1") != 0) {
    std::printf(
        "\nm1013_live — skipped: SKYNET_DRCF_LIVE is not 1 (the DRCF block, see the README)\n");
  } else {
    std::printf("\nm1013_live — the M1013 behind DRCF, through Doosan's own plugin\n");
    // update_rate=10 agrees with the URDF's own `update_rate` param: the plugin skips, and
    // reports OK for, every write whose period is outside 0.3–1.5 times its configured one.
    //
    // First F126's evidence, with the URDF's modes as they stand: a velocity command on the
    // elbow, paced at the period, reports success on every write and moves nothing, because the
    // plugin commands motion only when the position command changes.
    ctx = nullptr;
    check(d->open((config_for("doosan_m1013_live.urdf") + " update_rate=10").c_str(), &ctx) ==
                  SKR_OK &&
              ctx != nullptr,
          "Doosan's plugin connects to DRCF, takes control authority and reaches STANDBY");
    if (ctx != nullptr) {
      const int offered = d->describe(ctx, json, sizeof json);
      const std::string offered_descriptor(json, static_cast<size_t>(offered > 0 ? offered : 0));
      check(offered_descriptor.find("\"modes\":[\"position\",\"velocity\"]") != std::string::npos,
            "the URDF's interfaces offer position and velocity");
      float vpos[6] = {0, 0, 0, 0, 0, 0};
      skr_state vstate;
      std::memset(&vstate, 0, sizeof vstate);
      vstate.position_rad = vpos;
      d->read_state(ctx, &vstate, 0);
      const float before = vpos[2];
      float speeds[6] = {0, 0, 0.2f, 0, 0, 0};
      uint8_t elbow_only[6] = {0, 0, 1, 0, 0, 0};
      skr_command spin;
      std::memset(&spin, 0, sizeof spin);
      spin.mode = SKR_MODE_VELOCITY;
      spin.joint_count = 6;
      spin.values = speeds;
      spin.mask = elbow_only;
      bool accepted = true;
      for (int i = 0; accepted && i < 20; ++i) {
        const auto started = std::chrono::steady_clock::now();
        accepted = d->read_state(ctx, &vstate, 0) == SKR_OK &&
                   d->write_command(ctx, &spin, 0) == SKR_OK;
        std::this_thread::sleep_until(started + std::chrono::milliseconds(100));
      }
      d->read_state(ctx, &vstate, 0);
      check(accepted, "two seconds of 0.2 rad/s on the elbow, at ten hertz: every write says OK");
      check(std::fabs(vpos[2] - before) < 0.002f,
            "and the elbow does not move — Doosan's `velocity` interface is not a velocity mode "
            "(F126), so the integration declares modes=position");
      d->close(ctx);
    }
    // The body as it is integrated: position, the one mode the plugin commands motion in.
    ctx = nullptr;
    check(d->open((config_for("doosan_m1013_live.urdf") + " update_rate=10 modes=position")
                      .c_str(),
                  &ctx) == SKR_OK &&
              ctx != nullptr,
          "a second session, modes=position, reaches STANDBY again");
    if (ctx != nullptr) {
      check(ros2_joint_count(ctx) == 6, "six joints, the M1013's own count");
      const int live = d->describe(ctx, json, sizeof json);
      const std::string live_descriptor(json, static_cast<size_t>(live > 0 ? live : 0));
      check(live > 0 && live_descriptor.find("\"robot_slug\":\"m1013\"") != std::string::npos,
            "the descriptor names the body the vendor's URDF does");
      check(live_descriptor.find("\"name\":\"joint_3\"") != std::string::npos,
            "and the vendor's joint names");
      check(live_descriptor.find("\"simulator\":true") != std::string::npos,
            "an emulator is declared simulated (§35.3)");
      check(live_descriptor.find("\"modes\":[\"position\"]") != std::string::npos,
            "and it offers position alone");

      float pos[6] = {0, 0, 0, 0, 0, 0};
      float vel[6] = {0, 0, 0, 0, 0, 0};
      float eff[6] = {0, 0, 0, 0, 0, 0};
      skr_state state;
      std::memset(&state, 0, sizeof state);
      state.position_rad = pos;
      state.velocity_rad_s = vel;
      state.effort_nm = eff;
      bool finite = d->read_state(ctx, &state, 0) == SKR_OK;
      for (int i = 0; finite && i < 6; ++i) finite = std::isfinite(pos[i]);
      check(finite, "the first read is finite on all six");

      float setpoint[6];
      uint8_t all[6] = {1, 1, 1, 1, 1, 1};
      skr_command command;
      std::memset(&command, 0, sizeof command);
      command.mode = SKR_MODE_POSITION;
      command.joint_count = 6;
      command.values = setpoint;
      command.mask = all;
      const auto period = std::chrono::milliseconds(100);  // the configured ten hertz
      // One round: read, then write the setpoint, then wait out the rest of the period. The
      // period is the plugin's contract, not ours: a write whose time since the last one is
      // outside 30–150 ms is dropped with an OK.
      auto round = [&](float elbow) -> bool {
        const auto started = std::chrono::steady_clock::now();
        bool held = d->read_state(ctx, &state, 0) == SKR_OK;
        setpoint[2] = elbow;
        held = held && d->write_command(ctx, &command, 0) == SKR_OK;
        std::this_thread::sleep_until(started + period);
        return held;
      };
      auto settle_on = [&](float elbow, int rounds) -> bool {
        bool held = true;
        for (int i = 0; held && i < rounds; ++i) {
          held = round(elbow);
          if (std::fabs(pos[2] - elbow) < 0.002f) break;
        }
        return held;
      };

      d->read_state(ctx, &state, 0);
      std::memcpy(setpoint, pos, sizeof setpoint);
      const float home = pos[2];
      // Hold rounds first: the pose itself, at the period, so the plugin's period check has a
      // previous write to measure against.
      bool primed = true;
      for (int i = 0; primed && i < 5; ++i) primed = round(home);
      check(primed, "five hold rounds at ten hertz are accepted");
      check(std::fabs(pos[2] - home) < 0.002f, "and the arm stays where it was");

      // F125, measured. The same targets, written a millisecond apart: every write returns OK,
      // and the plugin drops every one, because the period it is handed is outside its window.
      // Before the bridge passed the time actually elapsed, *every* write looked like this.
      std::this_thread::sleep_for(std::chrono::milliseconds(400));
      bool burst_ok = true;
      for (int i = 1; burst_ok && i <= 60; ++i) {
        setpoint[2] = home + 0.002f * static_cast<float>(i);
        burst_ok = d->write_command(ctx, &command, 0) == SKR_OK;
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(1500));
      d->read_state(ctx, &state, 0);
      check(burst_ok, "sixty writes a millisecond apart all report success");
      check(std::fabs(pos[2] - home) < 0.002f,
            "and move nothing: the plugin drops a write whose period is outside 0.3–1.5 of its "
            "own, and says OK (F125's failure shape, produced on purpose)");

      // The same move at the period: the elbow out 0.12 rad and back.
      const float target = home + (home > 2.0f ? -0.12f : 0.12f);
      bool out = true;
      for (int i = 0; out && i < 3; ++i) out = round(home);  // re-prime after the pause
      out = out && settle_on(target, 40);
      check(out && std::fabs(pos[2] - target) < 0.002f,
            "at ten hertz the elbow travels 0.12 rad and the readback says so");
      check(std::fabs(pos[0] - setpoint[0]) < 0.002f && std::fabs(pos[5] - setpoint[5]) < 0.002f,
            "and the joints it did not move stayed put");
      // What the plugin reports as velocity while a joint moves. In virtual mode `read` fills
      // position from `GetCurrentPose` and never writes velocity, so the declared interface holds
      // its initial zero. Named on every run rather than asserted: it is Doosan's, and a zero
      // there is §24's "joint reported as still" on a joint that is moving (F127).
      {
        bool moving = true;
        float seen_vel = 0.0f;
        const float back = home;
        for (int i = 0; moving && i < 3; ++i) {
          moving = round(back);
          seen_vel = std::fmax(seen_vel, std::fabs(vel[2]));
        }
        std::printf("  note  elbow velocity read while it moved back: %.4f rad/s (%s)\n",
                    static_cast<double>(seen_vel),
                    seen_vel == 0.0f ? "the plugin's virtual mode never writes it — F127"
                                     : "the plugin now reports it");
      }
      const bool back = settle_on(home, 40);
      check(back && std::fabs(pos[2] - home) < 0.002f, "and came back to where the block found it");

      // The software stop: the latch refuses every later command, and the state says so.
      check(d->emergency_stop(ctx) == SKR_OK, "the stop latches");
      setpoint[2] = target;
      check(d->write_command(ctx, &command, 0) == SKR_EPERM, "and refuses the next command");
      d->read_state(ctx, &state, 0);
      check((state.flags & SKR_FLAG_ESTOP) != 0, "and the state says the stop is asserted");
      d->close(ctx);
    }
  }

  // ── the ABB IRB 1200 over EGM, when the operator says the fake controller is up ─
  //
  // SKYNET_EGM_LIVE=1 runs this block against `abb/fake_egm.py` — a controller written here from
  // ABB's own `egm.proto` — through PickNik's `abb_hardware_interface` over ros-industrial's
  // `abb_libegm`, both unmodified (DRY-COVERAGE-PLAN.md Phase 8, item 8.4). `make
  // check-egm-docker` starts the fake and runs it; anything else skips it by name. The fake
  // starts at (0, 10, 20, 0, 30, 0) degrees and moves toward each reference at 90 deg/s.
  const char* egm_gate = std::getenv("SKYNET_EGM_LIVE");
  if (egm_gate == nullptr || std::strcmp(egm_gate, "1") != 0) {
    std::printf(
        "\nabb_irb1200_egm — skipped: SKYNET_EGM_LIVE is not 1 (the EGM block, see the README)\n");
  } else {
    std::printf("\nabb_irb1200_egm — an IRB 1200 over EGM, through the open client\n");
    // update_rate=250 is EGM's own 4 ms. `modes=position` because the meaning of EGM's speed
    // reference is the RAPID program's `PosCorrGain`, which only a controller has.
    ctx = nullptr;
    check(d->open((config_for("abb_irb1200_egm.urdf") + " update_rate=250 modes=position").c_str(),
                  &ctx) == SKR_OK &&
              ctx != nullptr,
          "the open client binds 6511 and hears the controller before activating");
    if (ctx != nullptr) {
      check(ros2_joint_count(ctx) == 6, "six joints, the IRB 1200's own count");
      const int live = d->describe(ctx, json, sizeof json);
      const std::string live_descriptor(json, static_cast<size_t>(live > 0 ? live : 0));
      check(live > 0 && live_descriptor.find("\"robot_slug\":\"abb-irb1200-5-90\"") !=
                            std::string::npos,
            "the descriptor names the body PickNik's description does");
      check(live_descriptor.find("\"modes\":[\"position\"]") != std::string::npos,
            "and offers position alone");
      // F131: the description's `<ros2_control>` block and its `<limit>`s disagree on every
      // joint, and the plugin enforces the block's. The declared travel is the intersection.
      check(live_descriptor.find("\"position_max_rad\":2.23,") != std::string::npos &&
                live_descriptor.find("\"position_min_rad\":-1.745,") != std::string::npos,
            "travel is what both the plugin and the arm accept: joint 6 to ±2.23 (the block), "
            "joint 2 from -1.745 (the limit)");

      float pos[6] = {0, 0, 0, 0, 0, 0};
      float vel[6] = {0, 0, 0, 0, 0, 0};
      float eff[6] = {0, 0, 0, 0, 0, 0};
      skr_state state;
      std::memset(&state, 0, sizeof state);
      state.position_rad = pos;
      state.velocity_rad_s = vel;
      state.effort_nm = eff;
      check(d->read_state(ctx, &state, 0) == SKR_OK, "the first read answers");
      const float deg = static_cast<float>(M_PI / 180.0);
      const float start[6] = {0.0f, 10.0f * deg, 20.0f * deg, 0.0f, 30.0f * deg, 0.0f};
      bool at_start = true;
      for (int i = 0; i < 6; ++i) at_start = at_start && std::fabs(pos[i] - start[i]) < 1e-4f;
      check(at_start,
            "and reads the controller's pose: degrees on the wire, radians here, joint for joint");
      check(std::isnan(eff[0]), "effort is NaN: EGM feeds back no joint torque");

      float setpoint[6];
      uint8_t all[6] = {1, 1, 1, 1, 1, 1};
      skr_command command;
      std::memset(&command, 0, sizeof command);
      command.mode = SKR_MODE_POSITION;
      command.joint_count = 6;
      command.values = setpoint;
      command.mask = all;
      const auto period = std::chrono::milliseconds(4);
      auto round = [&](float elbow) -> bool {
        const auto started = std::chrono::steady_clock::now();
        bool held = d->read_state(ctx, &state, 0) == SKR_OK;
        setpoint[2] = elbow;
        held = held && d->write_command(ctx, &command, 0) == SKR_OK;
        std::this_thread::sleep_until(started + period);
        return held;
      };
      std::memcpy(setpoint, start, sizeof setpoint);
      bool primed = true;
      for (int i = 0; primed && i < 50; ++i) primed = round(start[2]);
      check(primed && std::fabs(pos[2] - start[2]) < 1e-3f,
            "fifty hold rounds at EGM's rate, and the arm stays where it was");

      // The elbow out 0.2 rad in 0.002 rad steps, then held until the readback arrives.
      const float target = start[2] + 0.2f;
      bool out = true;
      for (int i = 1; out && i <= 100; ++i) out = round(start[2] + 0.002f * static_cast<float>(i));
      for (int i = 0; out && i < 250 && std::fabs(pos[2] - target) > 1e-3f; ++i) out = round(target);
      check(out && std::fabs(pos[2] - target) < 1e-3f,
            "the elbow travels 0.2 rad and the readback says so");
      check(std::fabs(pos[0] - start[0]) < 1e-3f && std::fabs(pos[5] - start[5]) < 1e-3f,
            "and the joints it did not move stayed put");
      bool back = true;
      for (int i = 1; back && i <= 100; ++i) back = round(target - 0.002f * static_cast<float>(i));
      for (int i = 0; back && i < 250 && std::fabs(pos[2] - start[2]) > 1e-3f; ++i) {
        back = round(start[2]);
      }
      check(back && std::fabs(pos[2] - start[2]) < 1e-3f,
            "and came back to where the block found it");

      // F131's other half. Joint 6 at 2.5 rad is inside the `<limit>` and outside the block; the
      // plugin throws on it and its component stays failed for the session. Refused here instead.
      setpoint[5] = 2.5f;
      check(d->write_command(ctx, &command, 0) == SKR_EINVAL,
            "a setpoint outside the declared travel is refused before it reaches the plugin");
      setpoint[5] = start[5];
      bool alive = true;
      for (int i = 0; alive && i < 10; ++i) alive = round(start[2]);
      check(alive, "and the session lives: the next commands go through");

      check(d->emergency_stop(ctx) == SKR_OK, "the stop latches");
      check(d->write_command(ctx, &command, 0) == SKR_EPERM, "and refuses the next command");
      d->read_state(ctx, &state, 0);
      check((state.flags & SKR_FLAG_ESTOP) != 0, "and the state says the stop is asserted");
      d->close(ctx);
    }
  }

  // ── the KUKA KR 6 R900 sixx over RSI, when the operator says KUKA's simulator is up ─
  //
  // SKYNET_RSI_LIVE=1 runs this block against `kuka_rsi_simulator`, KUKA's own, through KUKA's own
  // `KukaRSIHardwareInterface` over its external-control SDK (DRY-COVERAGE-PLAN.md Phase 8, item
  // 8.14). `make check-rsi-docker` starts the simulator and runs it; anything else skips it by name.
  // The simulator starts at (0, -90, 90, 0, 90, 0) degrees, sends a telegram every 40 ms, and moves
  // each axis to that start plus the correction the reply carries — the plugin's `AK`, in degrees,
  // relative to the first telegram it heard. The controller paces this block: the plugin's `read`
  // waits for the next telegram, and each round answers it.
  const char* rsi_gate = std::getenv("SKYNET_RSI_LIVE");
  if (rsi_gate == nullptr || std::strcmp(rsi_gate, "1") != 0) {
    std::printf(
        "\nkuka_kr6r900_rsi — skipped: SKYNET_RSI_LIVE is not 1 (the RSI block, see the README)\n");
  } else {
    std::printf("\nkuka_kr6r900_rsi — a KR 6 R900 sixx over RSI, through KUKA's own plugin\n");
    // update_rate=250 is RSI's own 4 ms. `modes=position` because position is all KUKA's plugin
    // accepts at the pinned release, and at `master`, which exports velocity and effort too, all it
    // forwards unless an RSI XML configuration enables the others — which the default does not.
    // SKYNET_RSI_URDF names another URDF than the committed one: `make check-rsi-docker
    // RSI_IMAGE_URDF=...` passes the one KUKA's descriptions built in the image, which is what a run
    // at KUKA's `master` needs (DRY-COVERAGE-PLAN.md F138).
    const char* rsi_urdf = std::getenv("SKYNET_RSI_URDF");
    const std::string rsi_config =
        (rsi_urdf != nullptr && *rsi_urdf != '\0' ? std::string("urdf=") + rsi_urdf
                                                  : config_for("kuka_kr6r900_rsi.urdf")) +
        " update_rate=250 modes=position";
    ctx = nullptr;
    check(d->open(rsi_config.c_str(), &ctx) == SKR_OK && ctx != nullptr,
          "KUKA's plugin binds 59152 and hears the controller's first telegram before activating");
    if (ctx != nullptr) {
      check(ros2_joint_count(ctx) == 6, "six joints, the KR 6's own count");
      const int live = d->describe(ctx, json, sizeof json);
      const std::string live_descriptor(json, static_cast<size_t>(live > 0 ? live : 0));
      check(live > 0 && live_descriptor.find("\"robot_slug\":\"kr6-r900-sixx\"") !=
                            std::string::npos,
            "the descriptor names the body KUKA's description does");
      check(live_descriptor.find("\"modes\":[\"position\"]") != std::string::npos,
            "and offers position alone, the one mode RSI's default telegram carries");
      // KUKA's macro: A2 from -190 to 45 degrees.
      check(live_descriptor.find("\"position_max_rad\":0.785398") != std::string::npos,
            "travel is KUKA's own <limit>: A2 up to 45 degrees");

      float pos[6] = {0, 0, 0, 0, 0, 0};
      float vel[6] = {0, 0, 0, 0, 0, 0};
      float eff[6] = {0, 0, 0, 0, 0, 0};
      skr_state state;
      std::memset(&state, 0, sizeof state);
      state.position_rad = pos;
      state.velocity_rad_s = vel;
      state.effort_nm = eff;
      check(d->read_state(ctx, &state, 0) == SKR_OK, "the first read answers");
      const float deg = static_cast<float>(M_PI / 180.0);
      const float start[6] = {0.0f, -90.0f * deg, 90.0f * deg, 0.0f, 90.0f * deg, 0.0f};
      bool at_start = true;
      for (int i = 0; i < 6; ++i) at_start = at_start && std::fabs(pos[i] - start[i]) < 1e-4f;
      check(at_start,
            "and reads the controller's pose: degrees on the wire, radians here, A1 to A6 in order");
      // The default telegram carries AIPos alone. KUKA's plugin at the pinned release exports
      // position and nothing else, so the bridge has no velocity or effort to read; at `master` it
      // exports both and leaves them at NaN, saying so at load. NaN either way — zero would be a
      // joint reported as still and unloaded (§24).
      check(std::isnan(vel[0]) && std::isnan(eff[0]),
            "velocity and effort are NaN: RSI's default telegram carries neither");

      float setpoint[6];
      uint8_t all[6] = {1, 1, 1, 1, 1, 1};
      skr_command command;
      std::memset(&command, 0, sizeof command);
      command.mode = SKR_MODE_POSITION;
      command.joint_count = 6;
      command.values = setpoint;
      command.mask = all;
      auto round = [&](float elbow) -> bool {
        bool held = d->read_state(ctx, &state, 0) == SKR_OK;
        setpoint[2] = elbow;
        return held && d->write_command(ctx, &command, 0) == SKR_OK;
      };
      std::memcpy(setpoint, start, sizeof setpoint);
      bool primed = true;
      for (int i = 0; primed && i < 10; ++i) primed = round(start[2]);
      check(primed && std::fabs(pos[2] - start[2]) < 1e-3f,
            "ten hold rounds, one reply per telegram, and the arm stays where it was");

      // A3 out 0.2 rad in 0.01 rad steps, then held until the readback arrives.
      const float target = start[2] + 0.2f;
      bool out = true;
      for (int i = 1; out && i <= 20; ++i) out = round(start[2] + 0.01f * static_cast<float>(i));
      for (int i = 0; out && i < 25 && std::fabs(pos[2] - target) > 1e-3f; ++i) out = round(target);
      check(out && std::fabs(pos[2] - target) < 1e-3f,
            "A3 travels 0.2 rad and the readback says so: the correction is absolute, in degrees");
      check(std::fabs(pos[0] - start[0]) < 1e-3f && std::fabs(pos[1] - start[1]) < 1e-3f &&
                std::fabs(pos[5] - start[5]) < 1e-3f,
            "and the axes it did not move stayed put");
      bool back = true;
      for (int i = 1; back && i <= 20; ++i) back = round(target - 0.01f * static_cast<float>(i));
      for (int i = 0; back && i < 25 && std::fabs(pos[2] - start[2]) > 1e-3f; ++i) {
        back = round(start[2]);
      }
      check(back && std::fabs(pos[2] - start[2]) < 1e-3f,
            "and came back to where the block found it");

      // A2 at 1.0 rad is past KUKA's 45 degrees. Refused before it reaches the plugin.
      setpoint[1] = 1.0f;
      check(d->write_command(ctx, &command, 0) == SKR_EINVAL,
            "a setpoint outside the declared travel is refused before it reaches the plugin");
      setpoint[1] = start[1];
      bool alive = true;
      for (int i = 0; alive && i < 5; ++i) alive = round(start[2]);
      check(alive && std::fabs(pos[1] - start[1]) < 1e-3f,
            "and the session lives, with A2 where it was");

      check(d->emergency_stop(ctx) == SKR_OK, "the stop latches");
      check(d->write_command(ctx, &command, 0) == SKR_EPERM, "and refuses the next command");
      d->read_state(ctx, &state, 0);
      check((state.flags & SKR_FLAG_ESTOP) != 0, "and the state says the stop is asserted");
      // Whether close() told the controller — RSI's stop flag — is the simulator's to say, and
      // `kuka/run-rsi.sh` reads its log for it.
      d->close(ctx);
    }
  }

  // ── an AUBO i5 against ARCS, when the operator says the controller is up and the arm readied ─
  //
  // SKYNET_ARCS_LIVE=1 runs this block against ARCS — AUBO's controller software, with AUBO's
  // simulated arm where the drives would be — through AUBO's own `AuboHardwareInterface` over AUBO's
  // SDK (DRY-COVERAGE-PLAN.md Phase 8, item 8.15). `make check-arcs-docker` starts ARCS and
  // `aubo/run-arcs.sh` powers the arm on with AUBO's own startup example before running it; anything
  // else skips it by name. ARCS's simulated i5 starts at the `qinit` AUBO's configuration gives it.
  //
  // SKYNET_ARCS_LIVE=unready runs the other half instead, against the arm as a fresh ARCS starts
  // it — powered off, brakes on — which is the controller a session meets when nobody readied it.
  const char* arcs_gate = std::getenv("SKYNET_ARCS_LIVE");
  const bool arcs_ready = arcs_gate != nullptr && std::strcmp(arcs_gate, "1") == 0;
  const bool arcs_unready = arcs_gate != nullptr && std::strcmp(arcs_gate, "unready") == 0;
  const char* arcs_urdf = std::getenv("SKYNET_ARCS_URDF");
  const std::string arcs_body = arcs_urdf != nullptr && *arcs_urdf != '\0'
                                    ? std::string("urdf=") + arcs_urdf
                                    : config_for("aubo_i5_arcs.urdf");
  const float qinit[6] = {0.0f, -0.261799f, 1.74533f, 0.436332f, 1.5708f, 0.0f};
  if (arcs_unready) {
    std::printf("\naubo_i5_arcs, unready — the arm as ARCS starts it, powered off\n");
    // AUBO's `on_activate` turns servo mode on and ignores the answer, so on an arm that is not
    // `Running` the component activates with servo mode off (F141). The first thing that can say
    // so is `write`, which returns ERROR for any robot mode but `Running`.
    ctx = nullptr;
    check(d->open((arcs_body + " update_rate=200 modes=position").c_str(), &ctx) == SKR_OK &&
              ctx != nullptr,
          "open() succeeds: AUBO's plugin activates although ARCS refused it servo mode (F141)");
    if (ctx != nullptr) {
      float pos[6] = {0, 0, 0, 0, 0, 0};
      skr_state state;
      std::memset(&state, 0, sizeof state);
      state.position_rad = pos;
      bool at_home = d->read_state(ctx, &state, 0) == SKR_OK;
      for (int i = 0; at_home && i < 6; ++i) at_home = std::fabs(pos[i] - qinit[i]) < 1e-4f;
      check(at_home, "the arm reads, at AUBO's configured home");
      float setpoint[6];
      std::memcpy(setpoint, pos, sizeof setpoint);
      uint8_t all[6] = {1, 1, 1, 1, 1, 1};
      skr_command command;
      std::memset(&command, 0, sizeof command);
      command.mode = SKR_MODE_POSITION;
      command.joint_count = 6;
      command.values = setpoint;
      command.mask = all;
      check(d->write_command(ctx, &command, 0) == SKR_EIO,
            "the first command — a hold, where the arm already is — is refused with EIO, not "
            "reported done: the plugin's write fails on an arm that is not Running");
      bool stays = true;
      for (int i = 0; stays && i < 3; ++i) {
        stays = d->read_state(ctx, &state, 0) == SKR_EIO && (state.flags & SKR_FLAG_FAULT) != 0;
      }
      check(stays,
            "and every state after it carries the fault: ros2_control takes a component whose "
            "write failed down for the session, as AUBO's own message says to restart it (F142)");
      check(d->write_command(ctx, &command, 0) == SKR_EIO, "and every command after it is refused");
      d->close(ctx);
    }
    // That nothing reached the arm — still powered off, servo mode off — is ARCS's to say, and
    // `aubo/run-arcs.sh` asks it.
  }
  if (!arcs_ready) {
    if (!arcs_unready) {
      std::printf(
          "\naubo_i5_arcs — skipped: SKYNET_ARCS_LIVE is not 1 (the ARCS block, see the README)\n");
    }
  } else {
    std::printf("\naubo_i5_arcs — an AUBO i5 against ARCS, through AUBO's own plugin\n");
    const auto period = std::chrono::milliseconds(5);  // ARCS's servo cycle, the plugin's 200 Hz

    // First F126's shape, with the URDF's modes as they stand: AUBO's plugin exports a `velocity`
    // command interface on every joint, and its `write` sends the position commands to
    // `servoJoint` and reads nothing else.
    ctx = nullptr;
    check(d->open((arcs_body + " update_rate=200").c_str(), &ctx) == SKR_OK && ctx != nullptr,
          "AUBO's plugin logs in over RPC and RTDE, turns servo mode on and activates");
    if (ctx != nullptr) {
      const int offered = d->describe(ctx, json, sizeof json);
      const std::string offered_descriptor(json, static_cast<size_t>(offered > 0 ? offered : 0));
      check(offered_descriptor.find("\"modes\":[\"position\",\"velocity\"]") != std::string::npos,
            "the URDF's interfaces offer position and velocity");
      float vpos[6] = {0, 0, 0, 0, 0, 0};
      skr_state vstate;
      std::memset(&vstate, 0, sizeof vstate);
      vstate.position_rad = vpos;
      d->read_state(ctx, &vstate, 0);
      const float before = vpos[2];
      float speeds[6] = {0, 0, 0.2f, 0, 0, 0};
      uint8_t elbow_only[6] = {0, 0, 1, 0, 0, 0};
      skr_command spin;
      std::memset(&spin, 0, sizeof spin);
      spin.mode = SKR_MODE_VELOCITY;
      spin.joint_count = 6;
      spin.values = speeds;
      spin.mask = elbow_only;
      bool accepted = true;
      for (int i = 0; accepted && i < 200; ++i) {
        const auto started = std::chrono::steady_clock::now();
        accepted = d->read_state(ctx, &vstate, 0) == SKR_OK &&
                   d->write_command(ctx, &spin, 0) == SKR_OK;
        std::this_thread::sleep_until(started + period);
      }
      d->read_state(ctx, &vstate, 0);
      check(accepted, "a second of 0.2 rad/s on the elbow, at 200 Hz: every write says OK");
      check(std::fabs(vpos[2] - before) < 1e-4f,
            "and the elbow does not move — AUBO's `velocity` interface is not a velocity mode "
            "(F126's shape), so the integration declares modes=position");
      d->close(ctx);
    }

    // The body as it is integrated: position, the one mode the plugin commands motion in.
    ctx = nullptr;
    check(d->open((arcs_body + " update_rate=200 modes=position").c_str(), &ctx) == SKR_OK &&
              ctx != nullptr,
          "a second session, modes=position, activates again");
    if (ctx != nullptr) {
      check(ros2_joint_count(ctx) == 6, "six joints, the i5's own count");
      const int live = d->describe(ctx, json, sizeof json);
      const std::string live_descriptor(json, static_cast<size_t>(live > 0 ? live : 0));
      // AUBO's xacro names every arm it describes `aubo_robot` (F140): the name is AUBO's, and
      // which arm it is was the operator's to say.
      check(live > 0 && live_descriptor.find("\"robot_slug\":\"aubo-robot\"") != std::string::npos,
            "the descriptor names the body AUBO's description does — `aubo_robot`, for any arm");
      check(live_descriptor.find("\"name\":\"foreArm_joint\"") != std::string::npos,
            "and AUBO's joint names");
      check(live_descriptor.find("\"simulator\":true") != std::string::npos,
            "a controller with a simulated arm is declared simulated (§35.3)");
      check(live_descriptor.find("\"modes\":[\"position\"]") != std::string::npos,
            "and it offers position alone");
      // F140: the block's `min`/`max` are ABB IRB 1200's on three joints and an unexpanded
      // literal on the other three, and F131's rule declares the intersection with the `<limit>`s
      // whatever their source — narrower than the arm (ARCS gives joints 2–5 ±3.107 rad), never wider.
      check(live_descriptor.find("\"position_max_rad\":2.41,") != std::string::npos &&
                live_descriptor.find("\"position_max_rad\":2.23,") != std::string::npos,
            "travel is the intersection F131 declares: the forearm to ±2.41, the last wrist to ±2.23 "
            "— ABB's numbers, in AUBO's block (F140)");

      float pos[6] = {0, 0, 0, 0, 0, 0};
      float vel[6] = {0, 0, 0, 0, 0, 0};
      float eff[6] = {0, 0, 0, 0, 0, 0};
      skr_state state;
      std::memset(&state, 0, sizeof state);
      state.position_rad = pos;
      state.velocity_rad_s = vel;
      state.effort_nm = eff;
      check(d->read_state(ctx, &state, 0) == SKR_OK, "the first read answers");
      bool at_home = true;
      for (int i = 0; i < 6; ++i) at_home = at_home && std::fabs(pos[i] - qinit[i]) < 1e-4f;
      check(at_home, "and reads ARCS's own pose for the arm: AUBO's configured home, joint for joint");
      check(std::isfinite(vel[2]), "velocity is ARCS's joint speed, streamed over RTDE");
      check(std::isnan(eff[0]), "effort is NaN: AUBO's plugin exports no effort interface");

      float setpoint[6];
      uint8_t all[6] = {1, 1, 1, 1, 1, 1};
      skr_command command;
      std::memset(&command, 0, sizeof command);
      command.mode = SKR_MODE_POSITION;
      command.joint_count = 6;
      command.values = setpoint;
      command.mask = all;
      auto round = [&](float elbow) -> bool {
        const auto started = std::chrono::steady_clock::now();
        bool held = d->read_state(ctx, &state, 0) == SKR_OK;
        setpoint[2] = elbow;
        held = held && d->write_command(ctx, &command, 0) == SKR_OK;
        std::this_thread::sleep_until(started + period);
        return held;
      };
      std::memcpy(setpoint, pos, sizeof setpoint);
      const float home = pos[2];
      bool primed = true;
      for (int i = 0; primed && i < 100; ++i) primed = round(home);
      check(primed && std::fabs(pos[2] - home) < 1e-4f,
            "a hundred hold rounds at 200 Hz, and the arm stays where it was");

      // The elbow out 0.2 rad in 0.002 rad steps, then held until the readback arrives.
      const float target = home + 0.2f;
      float fastest = 0.0f;
      const auto moved_from = std::chrono::steady_clock::now();
      bool out = true;
      for (int i = 1; out && i <= 100; ++i) {
        out = round(home + 0.002f * static_cast<float>(i));
        fastest = std::fmax(fastest, std::fabs(vel[2]));
      }
      const double stepping_s =
          std::chrono::duration<double>(std::chrono::steady_clock::now() - moved_from).count();
      for (int i = 0; out && i < 400 && std::fabs(pos[2] - target) > 1e-3f; ++i) {
        out = round(target);
        fastest = std::fmax(fastest, std::fabs(vel[2]));
      }
      std::printf("  note  a hundred 5 ms rounds took %.3f s; the readback reached the target "
                  "%s; fastest elbow speed read %.3f rad/s\n",
                  stepping_s, std::fabs(pos[2] - target) < 1e-3f ? "within 1e-3" : "NOT",
                  static_cast<double>(fastest));
      check(out && std::fabs(pos[2] - target) < 1e-3f,
            "the elbow travels 0.2 rad and ARCS's readback says so");
      check(fastest > 0.1f,
            "and the velocity read while it moved is the joint's, not a standing zero (F127's "
            "question, answered the other way here)");
      check(std::fabs(pos[0] - setpoint[0]) < 1e-4f && std::fabs(pos[5] - setpoint[5]) < 1e-4f,
            "and the joints it did not move stayed put");
      bool back = true;
      for (int i = 1; back && i <= 100; ++i) back = round(target - 0.002f * static_cast<float>(i));
      for (int i = 0; back && i < 400 && std::fabs(pos[2] - home) > 1e-3f; ++i) back = round(home);
      check(back && std::fabs(pos[2] - home) < 1e-3f, "and came back to where the block found it");

      // The forearm at 2.5 rad is inside the i5's `<limit>` (±2π) and ARCS's own (±3.107), and
      // outside the 2.41 AUBO's block carries from ABB. Refused before it reaches the plugin — the
      // narrow direction F131's rule errs in, and the price of it (F140).
      setpoint[2] = 2.5f;
      check(d->write_command(ctx, &command, 0) == SKR_EINVAL,
            "a setpoint outside the declared travel is refused before it reaches the plugin");
      bool alive = true;
      for (int i = 0; alive && i < 20; ++i) alive = round(home);
      check(alive && std::fabs(pos[2] - home) < 1e-3f,
            "and the session lives, with the forearm where it was");

      check(d->emergency_stop(ctx) == SKR_OK, "the stop latches");
      setpoint[2] = target;
      check(d->write_command(ctx, &command, 0) == SKR_EPERM, "and refuses the next command");
      d->read_state(ctx, &state, 0);
      check((state.flags & SKR_FLAG_ESTOP) != 0, "and the state says the stop is asserted");
      // Whether close() left the controller out of servo mode is ARCS's to say, and
      // `aubo/run-arcs.sh` asks it.
      d->close(ctx);
    }
  }

  std::printf("\n%d checks, %d failed\n", checks, failures);
  if (failures == 0) {
    std::printf(
        "\nThe substrate is mock_components/GenericSystem — ros2_control's own fake hardware\n"
        "interface. What that does not measure is jitter under load, which is the row the plan's\n"
        "table puts under `not dry-verifiable` for this driver, and it stays there.\n");
  }
  return failures == 0 ? 0 : 1;
}
