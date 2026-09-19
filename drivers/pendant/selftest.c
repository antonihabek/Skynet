/*
 * The pendant robot's own suite.
 *
 * `drivers/mock/selftest.c` exercises a driver that takes commands. This one exercises a driver
 * that refuses them, and the difference is the point: for this class of machine the refusals *are*
 * the interface, so a suite that only checked the happy path would be checking the one thing that
 * cannot go wrong.
 *
 * Every case here is a refusal or its control. `CONTRIBUTING.md`'s rule is that a check which has
 * never failed has proved nothing, so each refusal is asserted beside the nearly-identical request
 * that succeeds — a missing `speed` against a supplied one, a pose inside the range against a pose
 * one step outside it.
 *
 * No test framework: a C11 compiler and nothing else, as the rest of `drivers/` does.
 */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "skynet_rhal.h"

uint32_t pendant_motions_accepted(const skr_ctx *ctx);
uint32_t pendant_motions_refused(const skr_ctx *ctx);
uint32_t pendant_commands_refused(const skr_ctx *ctx);
int pendant_program_running(const skr_ctx *ctx);
float pendant_position(const skr_ctx *ctx, int joint);
const char *pendant_joint_name(int joint);
int pendant_motion_count(void);
const char *pendant_motion_name(int index);
const char *pendant_motion_canonical(int index);

static int failures = 0;
static int checks = 0;

static void check(int held, const char *what) {
  checks += 1;
  if (!held) {
    failures += 1;
    printf("  FAIL  %s\n", what);
  } else {
    printf("  ok    %s\n", what);
  }
}

/* One motion request, with the reply the caller would read. */
static int motion(const skr_driver *d, skr_ctx *ctx, const char *request, char *reply,
                  size_t reply_len) {
  memset(reply, 0, reply_len);
  return d->extension(ctx, "vendor.pendant.motion", request, strlen(request), reply, reply_len);
}

int main(void) {
  const skr_driver *d = skynet_rhal_entry();
  skr_ctx *ctx = NULL;
  char reply[256];

  printf("skynet-pendant selftest\n\n");

  check(d->abi_version == SKYNET_RHAL_ABI_VERSION, "the driver declares this ABI version");
  check(d->open(NULL, &ctx) == SKR_OK && ctx != NULL, "open() with no configuration");

  /* ── read_state: the half this class of machine really offers ─────────── */
  float position[6], velocity[6], effort[6];
  skr_state state;
  memset(&state, 0, sizeof state);
  state.position_rad = position;
  state.velocity_rad_s = velocity;
  state.effort_nm = effort;

  check(d->read_state(ctx, &state, 0) == SKR_OK, "read_state() answers");
  check(state.joint_count == 6, "six joints are reported");
  check((state.flags & SKR_FLAG_ESTOP) == 0, "the stop is not asserted at rest");
  /* §24: a quantity the driver does not measure is NaN, never zero. A pendant robot does not
   * publish joint torque, and a zero would read as "under no load" on every cycle. */
  check(isnan((double)effort[0]), "unmeasured torque is NaN rather than zero");

  /* ── write_command: the refusal this driver exists to make ─────────────── */
  float values[6] = {0.1f, 0, 0, 0, 0, 0};
  uint8_t mask[6] = {1, 0, 0, 0, 0, 0};
  skr_command command;
  memset(&command, 0, sizeof command);
  command.joint_count = 6;
  command.mode = SKR_MODE_POSITION;
  command.values = values;
  command.mask = mask;

  int wrote = d->write_command(ctx, &command, 0);
  check(wrote == SKR_EPERM, "a well-formed joint command is refused with EPERM");
  check(wrote != SKR_ENOSYS,
        "and not with ENOSYS — the entry point exists, the machine refuses the command");
  check(pendant_commands_refused(ctx) == 1, "the refusal is counted rather than inferred");

  /* The control: a joint command changes nothing, on the joint it named. */
  float before = pendant_position(ctx, 0);
  for (int i = 0; i < 50; ++i) d->read_state(ctx, &state, 0);
  check(pendant_position(ctx, 0) == before, "and nothing moved, fifty cycles later");

  /* ── the motion interface ──────────────────────────────────────────────── */
  check(d->extension(ctx, "vendor.somebody.else", "x", 1, reply, sizeof reply) == SKR_ENOSYS,
        "another vendor's token is refused");

  check(motion(d, ctx, "move_l pose=0.4 speed=0.1", reply, sizeof reply) > 0,
        "a complete move_l is accepted");
  check(strcmp(reply, "accepted") == 0, "and says so");
  check(pendant_program_running(ctx) == 1, "the controller's program is running");

  /* The joints reach the target the *controller* interpolated to. Nothing outside the driver chose
   * the steps, which is §59.2's sentence made concrete. */
  for (int i = 0; i < 400; ++i) d->read_state(ctx, &state, 0);
  check(position[0] > 0.39f && position[0] < 0.41f, "the controller drove the joints to the pose");
  check(pendant_program_running(ctx) == 0, "and the program finished");

  /* An optional parameter really is optional. The control for the refusal below it. */
  check(motion(d, ctx, "move_l pose=0.2", reply, sizeof reply) > 0,
        "an optional parameter may be omitted");

  /* And a required one may not. This is the failure a controller would raise at run time. */
  check(motion(d, ctx, "move_l speed=0.1", reply, sizeof reply) == SKR_EINVAL,
        "a missing required parameter is refused");
  check(strstr(reply, "requires `pose`") != NULL, "and the refusal names the parameter");
  check(strstr(reply, "run time") != NULL, "and says where it would otherwise have failed");

  check(motion(d, ctx, "move_l pose=0.2 speed=9", reply, sizeof reply) == SKR_EINVAL,
        "a parameter outside its declared range is refused");
  check(strstr(reply, "0.25") != NULL, "and the refusal quotes the range");
  check(motion(d, ctx, "move_l pose=0.2 speed=0.25", reply, sizeof reply) > 0,
        "and the bound itself is accepted — the range is inclusive");

  check(motion(d, ctx, "move_x pose=0.2", reply, sizeof reply) == SKR_ENODEV,
        "an unknown motion is refused");
  check(strstr(reply, "move_l") != NULL, "and the refusal lists the ones that exist");

  check(motion(d, ctx, "move_l pose=banana", reply, sizeof reply) == SKR_EINVAL,
        "a parameter that is not a number is refused");

  check(motion(d, ctx, "close_gripper", reply, sizeof reply) > 0,
        "a motion with no parameters needs none");

  /* A reply buffer too small is an error, never a truncation — `describe`'s rule. */
  char tiny[4];
  check(motion(d, ctx, "close_gripper", tiny, sizeof tiny) == SKR_EINVAL,
        "a reply buffer too small is refused rather than truncated");

  uint32_t accepted = pendant_motions_accepted(ctx);
  uint32_t refused = pendant_motions_refused(ctx);
  check(accepted == 5 && refused == 4,
        "the counts are what the cases above did — five accepted, four refused");

  /* ── the stop ──────────────────────────────────────────────────────────── */
  check(d->emergency_stop(ctx) == SKR_OK, "emergency_stop() returns");
  d->read_state(ctx, &state, 0);
  check((state.flags & SKR_FLAG_ESTOP) != 0, "and the next state says the stop is asserted");
  check(motion(d, ctx, "home", reply, sizeof reply) == SKR_EPERM,
        "a motion is refused while the stop is latched");
  check(strstr(reply, "at the controller") != NULL,
        "and the refusal says where to clear it, because nothing here can");

  /* ── the table, and the descriptor that carries it (§84.1) ─────────────── */
  check(pendant_motion_count() == 4, "the controller declares four motions");
  check(strcmp(pendant_motion_canonical(0), "move_linear") == 0,
        "and each names the canonical motion it is, where one exists");
  check(strcmp(pendant_joint_name(5), "A6") == 0, "the joint names are the controller's");

  char described[4096];
  int written = d->describe(ctx, described, sizeof described);
  check(written > 0 && (size_t)written == strlen(described), "describe() returns what it wrote");
  check(strstr(described, "\"envelope\":null,\"capabilities\":[\"safety.estop\"]") != NULL,
        "the software stop above is declared, with no envelope to read it as a rated one");
  check(strstr(described, "\"motions\":[{\"name\":\"move_l\",\"motion\":\"move_linear\"") != NULL,
        "the descriptor's motions are the table, in the table's order");
  check(strstr(described, "{\"name\":\"speed\",\"unit\":\"m/s\",\"required\":false,\"min\":0.01") !=
            NULL,
        "an optional parameter says so, with the bounds the interpreter enforces");
  check(strstr(described, "{\"name\":\"close_gripper\",\"motion\":\"grasp\",\"parameters\":[]}") !=
            NULL,
        "a motion with no parameters declares an empty list, not an absent one");
  /* The control for the writer: one byte short of the document is a refusal, never a document
   * that parses with its last motion missing. */
  char *short_buffer = malloc((size_t)written);
  check(short_buffer != NULL && d->describe(ctx, short_buffer, (size_t)written) == SKR_EINVAL,
        "a buffer one byte short is refused rather than truncated");
  free(short_buffer);

  check(d->close(ctx) == SKR_OK, "close()");

  printf("\n%d checks, %d failed\n", checks, failures);
  if (failures == 0) {
    printf(
        "\nWhat this suite does not show: a transport plan. §84's motions are in the descriptor,\n"
        "and `skynet transport` and `skynet driver check` are what read them. What no suite here\n"
        "shows is a controller: the interpolator above is this file's, not a vendor's.\n");
  }
  return failures == 0 ? 0 : 1;
}
