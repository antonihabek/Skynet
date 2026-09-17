/*
 * skynet-ros2-control's own suite, against `mock_components/GenericSystem`.
 *
 * The substrate is `ros2_control`'s own fake hardware interface, which is what keeps this from
 * being two copies of one misunderstanding agreeing with each other: the fake is theirs, the driver
 * is ours, and neither was written from the other.
 *
 * Three URDFs, and the two that refuse are the interesting ones — a body whose joints share no
 * command interface, and a URDF with no `<ros2_control>` element at all.
 */

#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>

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
  check(descriptor.find("\"position_max_rad\":3.14") != std::string::npos,
        "travel comes from the URDF's own <limit>");
  // §63.4: a URDF effort limit is a design value from a CAD model, not a safety rating. The file
  // carries `effort="80"` and the descriptor must not.
  check(descriptor.find("effort_max_nm") == std::string::npos,
        "and no torque rating — the URDF's effort is a design value, not a safety rating (§63.4)");
  check(descriptor.find("\"envelope\":null") != std::string::npos,
        "and no safety envelope, for the same reason");
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

  // ── a URDF with nothing to command ────────────────────────────────────────
  std::printf("\nrefusals\n");
  ctx = nullptr;
  check(d->open(config_for("no_ros2_control.urdf").c_str(), &ctx) == SKR_ENODEV,
        "a URDF with no <ros2_control> element is refused");
  check(d->open("", &ctx) == SKR_ENODEV, "a configuration naming no URDF is refused");
  check(d->open("urdf=/nonexistent.urdf", &ctx) == SKR_ENODEV, "and a path that is not there");

  std::printf("\n%d checks, %d failed\n", checks, failures);
  if (failures == 0) {
    std::printf(
        "\nThe substrate is mock_components/GenericSystem — ros2_control's own fake hardware\n"
        "interface. What that does not measure is jitter under load, which is the row the plan's\n"
        "table puts under `not dry-verifiable` for this driver, and it stays there.\n");
  }
  return failures == 0 ? 0 : 1;
}
