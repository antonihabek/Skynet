/*
 * layout_probe — print the FCI wire layout as the vendor's own compiler computes it.
 *
 * This is the thing that makes `skynet_franka.protocol` more than a careful reading.
 * `DRY-COVERAGE-PLAN.md` F15: a protocol transcribed from memory and tested against a fake written
 * from the same memory proves that two copies of one misunderstanding agree. There is no Franka
 * simulator to break that symmetry with — the FCI talks to a Control box — so the symmetry is broken
 * here instead: every offset and size the driver uses comes out of Franka Robotics' own headers,
 * compiled, rather than out of anybody's reading of them.
 *
 * Build and run it against a checkout of the vendor's definitions (Apache-2.0):
 *
 *     git clone https://github.com/frankarobotics/libfranka-common
 *     make check-layout FRANKA_COMMON=../libfranka-common
 *
 * That regenerates `layout.json`, which `selftest.py` asserts the driver against on every run. The
 * file is committed so the check runs for somebody who has not cloned anything; this program is
 * what re-derives it when a release moves a field.
 *
 * Licence: Apache-2.0 (this file). The headers it includes are Franka Robotics', also Apache-2.0,
 * and are not vendored here.
 */

#include <research_interface/robot/rbk_types.h>
#include <research_interface/robot/service_types.h>

#include <cstddef>
#include <cstdio>

using namespace research_interface::robot;

#define STATE_FIELD(name) \
  printf("        \"%s\": %zu,\n", #name, offsetof(RobotState, name))

int main() {
  printf("{\n");
  printf(
      "  \"note\": \"The FCI wire layout, as the vendor's own compiler computes it. Not a design "
      "document and not this driver's opinion: every number here came from compiling "
      "research_interface/robot/{rbk_types,service_types}.h with `offsetof` and `sizeof`, and "
      "`selftest.py` asserts `skynet_franka.protocol` against it on every run. Regenerate with "
      "`make check-layout FRANKA_COMMON=<a libfranka-common checkout>`; never edit by hand, which "
      "would be switching the check off.\",\n");
  printf("  \"source\": {\n");
  printf("    \"repository\": \"https://github.com/frankarobotics/libfranka-common\",\n");
  printf("    \"commit\": \"%s\",\n", FRANKA_COMMON_COMMIT);
  printf("    \"licence\": \"Apache-2.0\",\n");
  printf("    \"compiler\": \"%s\",\n", FRANKA_COMPILER);
  printf("    \"measured\": \"%s\"\n", FRANKA_MEASURED);
  printf("  },\n");
  printf("  \"constants\": { \"version\": %u, \"command_port\": %u },\n", (unsigned)kVersion,
         (unsigned)kCommandPort);
  printf("  \"structs\": {\n");
  printf(
      "    \"CommandHeader\": { \"size\": %zu, \"fields\": { \"command\": %zu, \"command_id\": "
      "%zu, \"size\": %zu } },\n",
      sizeof(CommandHeader), offsetof(CommandHeader, command), offsetof(CommandHeader, command_id),
      offsetof(CommandHeader, size));
  printf(
      "    \"ConnectRequest\": { \"size\": %zu, \"fields\": { \"version\": %zu, \"udp_port\": %zu "
      "} },\n",
      sizeof(Connect::Request), offsetof(Connect::Request, version),
      offsetof(Connect::Request, udp_port));
  printf(
      "    \"ConnectResponse\": { \"size\": %zu, \"fields\": { \"status\": %zu, \"version\": %zu } "
      "},\n",
      sizeof(Connect::Response), offsetof(Connect::Response, status),
      offsetof(Connect::Response, version));
  printf("    \"MoveRequest\": { \"size\": %zu },\n", sizeof(Move::Request));
  printf("    \"RobotState\": {\n      \"size\": %zu,\n      \"fields\": {\n",
         sizeof(RobotState));
  STATE_FIELD(message_id);
  STATE_FIELD(O_T_EE);
  STATE_FIELD(tau_J);
  STATE_FIELD(tau_J_d);
  STATE_FIELD(dtau_J);
  STATE_FIELD(q);
  STATE_FIELD(q_d);
  STATE_FIELD(dq);
  STATE_FIELD(dq_d);
  STATE_FIELD(joint_contact);
  STATE_FIELD(cartesian_contact);
  STATE_FIELD(joint_collision);
  STATE_FIELD(tau_ext_hat_filtered);
  STATE_FIELD(theta);
  STATE_FIELD(dtheta);
  STATE_FIELD(motion_generator_mode);
  STATE_FIELD(controller_mode);
  STATE_FIELD(errors);
  STATE_FIELD(reflex_reason);
  STATE_FIELD(robot_mode);
  printf("        \"control_command_success_rate\": %zu\n",
         offsetof(RobotState, control_command_success_rate));
  printf("      }\n    },\n");
  printf(
      "    \"MotionGeneratorCommand\": {\n      \"size\": %zu,\n      \"fields\": {\n"
      "        \"q_c\": %zu,\n        \"dq_c\": %zu,\n        \"O_T_EE_c\": %zu,\n"
      "        \"O_dP_EE_c\": %zu,\n        \"elbow_c\": %zu,\n        \"valid_elbow\": %zu,\n"
      "        \"motion_generation_finished\": %zu\n      }\n    },\n",
      sizeof(MotionGeneratorCommand), offsetof(MotionGeneratorCommand, q_c),
      offsetof(MotionGeneratorCommand, dq_c), offsetof(MotionGeneratorCommand, O_T_EE_c),
      offsetof(MotionGeneratorCommand, O_dP_EE_c), offsetof(MotionGeneratorCommand, elbow_c),
      offsetof(MotionGeneratorCommand, valid_elbow),
      offsetof(MotionGeneratorCommand, motion_generation_finished));
  printf(
      "    \"ControllerCommand\": { \"size\": %zu, \"fields\": { \"tau_J_d\": %zu, "
      "\"torque_command_finished\": %zu } },\n",
      sizeof(ControllerCommand), offsetof(ControllerCommand, tau_J_d),
      offsetof(ControllerCommand, torque_command_finished));
  printf(
      "    \"RobotCommand\": { \"size\": %zu, \"fields\": { \"message_id\": %zu, \"motion\": %zu, "
      "\"control\": %zu } }\n",
      sizeof(RobotCommand), offsetof(RobotCommand, message_id), offsetof(RobotCommand, motion),
      offsetof(RobotCommand, control));
  printf("  }\n}\n");
  return 0;
}
