/*
 * `skynet driver check`, in miniature.
 *
 * §63.3: a driver author should reach green sim conformance before touching hardware, so that the
 * bench session — which costs a day and needs a human standing next to a robot — is spent on the
 * things only hardware can prove: estop latency, watchdog behaviour under load, real joint ratings.
 *
 * This runs the sim half of SCTS against the mock driver: the checks that need no robot. Each test
 * below names the SCTS id it stands for, so a failure points at a clause rather than at a line.
 *
 * Build and run: `make check` in this directory.
 * Licence: Apache-2.0.
 */

#include <math.h>
#include <stdio.h>
#include <string.h>

#include "skynet_rhal.h"

extern uint32_t mock_commands_accepted(const skr_ctx *ctx);
extern uint32_t mock_commands_rejected(const skr_ctx *ctx);
extern float mock_applied(const skr_ctx *ctx, int joint);
extern uint8_t mock_applied_mask(const skr_ctx *ctx, int joint);
extern uint8_t mock_applied_mode(const skr_ctx *ctx, int joint);

static int failures = 0;
static int checks = 0;

static void report(const char *scts_id, const char *what, int ok) {
  checks += 1;
  if (!ok) failures += 1;
  printf("%-4s %-28s %s\n", ok ? "ok" : "FAIL", scts_id, what);
}

#define CHECK(id, what, cond) report((id), (what), (cond) ? 1 : 0)

/* A preallocated frame, reused across every cycle. The whole point of the RHAL's caller-allocated
 * arrays is that the hot path never reaches an allocator, and a harness that malloc'd per cycle
 * would not be exercising the thing it is meant to exercise. */
static float g_position[8];
static float g_velocity[8];
static float g_effort[8];

static skr_state make_frame(void) {
  skr_state state;
  memset(&state, 0, sizeof state);
  state.position_rad = g_position;
  state.velocity_rad_s = g_velocity;
  state.effort_nm = g_effort;
  return state;
}

/* ── Fault injection ──────────────────────────────────────────────────────
 *
 * Each of these opens the mock with one fault selected, provokes it, and closes. They are separate
 * functions rather than inline blocks so that this file is portable C11 — the obvious spelling uses
 * GCC statement-expressions, and a reference a vendor is meant to read should compile with whatever
 * compiler that vendor already has. */

/* A stalled producer. The sequence not advancing is the evidence the kernel's watchdog reads, and
 * it is why a repeat is treated as a stall rather than as a fresh command. */
static int check_stall(const skr_driver *driver) {
  skr_ctx *ctx = NULL;
  if (driver->open("fault=stall", &ctx) != SKR_OK) return 0;
  skr_state a = make_frame();
  skr_state b = make_frame();
  driver->read_state(ctx, &a, 0);
  driver->read_state(ctx, &b, 0);
  int ok = a.seq == b.seq;
  driver->close(ctx);
  return ok;
}

/* A dead encoder. NaN is reported as NaN, never substituted with a plausible zero — a fabricated
 * measurement reaching a guard is worse than a missing one, which at least reports unavailable. */
static int check_nan(const skr_driver *driver) {
  skr_ctx *ctx = NULL;
  if (driver->open("fault=nan", &ctx) != SKR_OK) return 0;
  skr_state s = make_frame();
  driver->read_state(ctx, &s, 0);
  int ok = isnan(s.velocity_rad_s[1]);
  driver->close(ctx);
  return ok;
}

/* An estop asserted before anything runs: the flag is visible and commands are refused. */
static int check_estop(const skr_driver *driver) {
  skr_ctx *ctx = NULL;
  if (driver->open("fault=estop", &ctx) != SKR_OK) return 0;
  skr_state s = make_frame();
  driver->read_state(ctx, &s, 0);
  int flagged = (s.flags & SKR_FLAG_ESTOP) != 0;

  /* Per-joint (§86.1), so the refusal below is the estop's and not this body's wrist refusing a
     torque it does not accept. A check that passes for the wrong reason has proved nothing. */
  float values[3] = {1.0f, 1.0f, 1.0f};
  uint8_t mask[3] = {1, 1, 1};
  uint8_t modes[3] = {SKR_JMODE_INHERIT, SKR_JMODE_INHERIT, SKR_JMODE_POSITION};
  skr_command cmd;
  memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = SKR_MODE_TORQUE;
  cmd.flags = SKR_CMD_PER_JOINT_MODES;
  cmd.joint_count = 3;
  cmd.values = values;
  cmd.mask = mask;
  cmd.modes = modes;
  int refused = driver->write_command(ctx, &cmd, 0) == SKR_EPERM;

  driver->close(ctx);
  return flagged && refused;
}

/* A bus that did not answer inside the deadline. ETIMEDOUT and EPERM are different facts and the
 * black box records which, because they lead to completely different investigations. */
static int check_timeout(const skr_driver *driver) {
  skr_ctx *ctx = NULL;
  if (driver->open("fault=timeout", &ctx) != SKR_OK) return 0;
  skr_state s = make_frame();
  int ok = driver->read_state(ctx, &s, 0) == SKR_ETIMEDOUT;
  driver->close(ctx);
  return ok;
}

/* Running, but something is wrong. The vendor fault code is surfaced verbatim. */
static int check_degraded(const skr_driver *driver) {
  skr_ctx *ctx = NULL;
  if (driver->open("fault=degraded", &ctx) != SKR_OK) return 0;
  skr_state s = make_frame();
  driver->read_state(ctx, &s, 0);
  int ok = (s.flags & SKR_FLAG_DEGRADED) != 0 && s.fault_code != 0;
  driver->close(ctx);
  return ok;
}

/* emergency_stop must work from a failing process, so it is called with the driver mid-motion and
 * must leave every joint commanding nothing — and latch, because a robot that recovers itself from
 * a safety stop is a robot that resumes moving while somebody is standing next to it working out
 * what happened. */
static int check_estop_zeroes(const skr_driver *driver) {
  skr_ctx *ctx = NULL;
  if (driver->open("", &ctx) != SKR_OK) return 0;

  /* Per-joint, so the command is actually applied and the stop below has something to zero. */
  float values[3] = {10.0f, 5.0f, 1.0f};
  uint8_t mask[3] = {1, 1, 1};
  uint8_t modes[3] = {SKR_JMODE_INHERIT, SKR_JMODE_INHERIT, SKR_JMODE_POSITION};
  skr_command cmd;
  memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = SKR_MODE_TORQUE;
  cmd.flags = SKR_CMD_PER_JOINT_MODES;
  cmd.joint_count = 3;
  cmd.values = values;
  cmd.mask = mask;
  cmd.modes = modes;
  if (driver->write_command(ctx, &cmd, 0) != SKR_OK) {
    driver->close(ctx);
    return 0;
  }

  int ok = driver->emergency_stop(ctx) == SKR_OK;
  for (int j = 0; j < 3; ++j) {
    if (mock_applied(ctx, j) != 0.0f) ok = 0;
  }
  if (driver->write_command(ctx, &cmd, 0) != SKR_EPERM) ok = 0;

  driver->close(ctx);
  return ok;
}

/* §48.2's release: the latch clears, the flag follows, and commands are accepted again — in the
 * same session, without a close. The zeroed setpoints stay zeroed: release permits commands, and
 * the next command decides what moves. Idempotent: releasing twice is SKR_OK twice. */
static int check_release(const skr_driver *driver) {
  if (driver->release_stop == NULL) return 0;
  skr_ctx *ctx = NULL;
  if (driver->open("", &ctx) != SKR_OK) return 0;
  if (driver->emergency_stop(ctx) != SKR_OK) {
    driver->close(ctx);
    return 0;
  }

  int ok = driver->release_stop(ctx) == SKR_OK;
  if (driver->release_stop(ctx) != SKR_OK) ok = 0;

  skr_state s = make_frame();
  if (driver->read_state(ctx, &s, 0) != SKR_OK || (s.flags & SKR_FLAG_ESTOP) != 0) ok = 0;

  /* A hold over every joint: accepted, and moving nothing. */
  float values[3] = {0.0f, 0.0f, 0.0f};
  uint8_t mask[3] = {0, 0, 0};
  skr_command cmd;
  memset(&cmd, 0, sizeof cmd);
  cmd.seq = 1;
  cmd.mode = SKR_MODE_POSITION;
  cmd.joint_count = 3;
  cmd.values = values;
  cmd.mask = mask;
  if (driver->write_command(ctx, &cmd, 0) != SKR_OK) ok = 0;

  driver->close(ctx);
  return ok;
}

int main(void) {
  const skr_driver *driver = skynet_rhal_entry();
  printf("driver: %s %s\n\n", driver->driver_name, driver->driver_version);

  /* ── rhal.abi.version ─────────────────────────────────────────────────── */
  CHECK("rhal.abi.version", "declares the ABI it targets",
        driver->abi_version == SKYNET_RHAL_ABI_VERSION);

  /* The five entry points with no meaningful fallback. A driver missing emergency_stop is one
   * whose stop path does not exist, which is the worst thing to discover late. */
  CHECK("rhal.abi.version", "implements every mandatory entry point",
        driver->open && driver->close && driver->read_state && driver->write_command &&
            driver->emergency_stop && driver->describe);

  skr_ctx *ctx = NULL;
  CHECK("rhal.abi.version", "opens", driver->open("", &ctx) == SKR_OK && ctx != NULL);

  /* ── rhal.state.completeness ──────────────────────────────────────────── */
  skr_state state = make_frame();
  int rc = driver->read_state(ctx, &state, 0);
  CHECK("rhal.state.completeness", "read_state succeeds", rc == SKR_OK);
  CHECK("rhal.state.completeness", "joint count matches the body", state.joint_count == 3);
  CHECK("rhal.state.completeness", "timestamps are non-zero", state.stamp_ns != 0);

  uint32_t first_seq = state.seq;
  driver->read_state(ctx, &state, 0);
  CHECK("rhal.state.completeness", "the sequence advances", state.seq == first_seq + 1);

  /* ── embodiment.descriptor.valid ──────────────────────────────────────── */
  char descriptor[4096];
  int written = driver->describe(ctx, descriptor, sizeof descriptor);
  CHECK("embodiment.descriptor.valid", "describe returns a length", written > 0);
  CHECK("embodiment.descriptor.valid", "describe names this body",
        strstr(descriptor, "\"robot_slug\":\"mock-arm\"") != NULL);
  CHECK("embodiment.descriptor.valid", "every joint carries a canonical role",
        strstr(descriptor, "right_arm.shoulder.pitch") != NULL &&
            strstr(descriptor, "right_arm.elbow.flex") != NULL &&
            strstr(descriptor, "right_arm.wrist.roll") != NULL);

  /* The line a driver author is most likely to get wrong, so it is checked explicitly: a design
   * value from a model is not a datasheet safety rating, and the descriptor must say so by
   * declaring nothing rather than by declaring a plausible number. */
  CHECK("embodiment.limits.sourced", "declares no unsourced safety envelope",
        strstr(descriptor, "\"envelope\":null") != NULL);

  /* The software stop the checks below exercise, declared — and only it: the null envelope above
   * is what keeps the declaration from being read as a rated stop (DRY-COVERAGE-PLAN.md F63). */
  CHECK("safety.estop.latency", "declares the software stop it implements",
        strstr(descriptor, "\"capabilities\":[\"safety.estop\"]") != NULL);

  /* A buffer that is too small is an error, never a truncation. */
  char tiny[8];
  CHECK("embodiment.descriptor.valid", "refuses a buffer that is too small",
        driver->describe(ctx, tiny, sizeof tiny) == SKR_EINVAL);

  /* ── rhal.command.rejection ───────────────────────────────────────────── */
  /*
   * §86.1, and the command below is the section in one object. This body's J1 and J2 accept torque
   * and its J3 accepts position only — which is a mixed-mode body, and until §86 there was no way
   * to command one. The suite used to send `mode = TORQUE` with all three joints unmasked and the
   * driver accepted it, so a position-only wrist was being handed newton-metres and nothing said
   * so (DRY-COVERAGE-PLAN.md F37). It is now refused, and the check for that is below.
   */
  float values[3] = {10.0f, 5.0f, 1.0f};
  uint8_t mask[3] = {1, 1, 1};
  uint8_t modes[3] = {SKR_JMODE_INHERIT, SKR_JMODE_INHERIT, SKR_JMODE_POSITION};
  skr_command cmd;
  memset(&cmd, 0, sizeof cmd);
  cmd.stamp_ns = state.stamp_ns;
  cmd.seq = 1;
  cmd.mode = SKR_MODE_TORQUE;
  cmd.flags = SKR_CMD_PER_JOINT_MODES;
  cmd.joint_count = 3;
  cmd.values = values;
  cmd.mask = mask;
  cmd.modes = modes;

  CHECK("rhal.command.rejection", "accepts a well-formed command",
        driver->write_command(ctx, &cmd, 0) == SKR_OK);
  /* Inheritance resolved: two joints took the command's own mode and the third took its own. */
  CHECK("rhal.command.rejection", "a joint inherits the command's mode",
        mock_applied_mode(ctx, 0) == SKR_MODE_TORQUE);
  CHECK("rhal.command.rejection", "a joint's own mode overrides",
        mock_applied_mode(ctx, 2) == SKR_MODE_POSITION);

  /* The same command with the flag cleared is the body-wide one, and this body cannot take it: its
   * wrist does not accept torque. EPERM rather than EINVAL — the request is well formed and this
   * machine will not do it. */
  cmd.flags = 0;
  CHECK("rhal.command.rejection", "a body-wide mode no joint offers is refused",
        driver->write_command(ctx, &cmd, 0) == SKR_EPERM);
  cmd.flags = SKR_CMD_PER_JOINT_MODES;

  /* A bare SKR_MODE_* written into the per-joint array. This is the transcription error the second
   * numbering exists to catch, and catching it is what makes the offset worth its cost: without it
   * `SKR_MODE_VELOCITY` (1) would be a legal-looking entry rather than a refusal. */
  modes[2] = SKR_MODE_TORQUE;
  CHECK("rhal.command.rejection", "a base mode value in the per-joint array is refused",
        driver->write_command(ctx, &cmd, 0) == SKR_EINVAL);
  modes[2] = SKR_JMODE_POSITION;

  /* A well-formed per-joint mode this joint does not offer. */
  modes[2] = SKR_JMODE_TORQUE;
  CHECK("rhal.command.rejection", "a per-joint mode the joint does not offer is refused",
        driver->write_command(ctx, &cmd, 0) == SKR_EPERM);
  modes[2] = SKR_JMODE_POSITION;

  /* And the masked half of that rule: a joint the command says nothing about is not checked, or a
   * body-wide hold could not be expressed on a mixed body at all. */
  {
    uint8_t none[3] = {0, 0, 0};
    uint8_t wrong[3] = {SKR_JMODE_INHERIT, SKR_JMODE_INHERIT, SKR_JMODE_TORQUE};
    cmd.mask = none;
    cmd.modes = wrong;
    cmd.seq = 4;
    CHECK("rhal.command.rejection", "a masked joint's mode is not checked",
          driver->write_command(ctx, &cmd, 0) == SKR_OK);
    cmd.mask = mask;
    cmd.modes = modes;
    cmd.seq = 1;
  }

  cmd.joint_count = 9;
  CHECK("rhal.command.rejection", "rejects a wrong joint count",
        driver->write_command(ctx, &cmd, 0) == SKR_EINVAL);
  cmd.joint_count = 3;

  cmd.mode = 99;
  CHECK("rhal.command.rejection", "rejects an unknown mode",
        driver->write_command(ctx, &cmd, 0) == SKR_EINVAL);
  cmd.mode = SKR_MODE_TORQUE;

  /* ── The mask ─────────────────────────────────────────────────────────── */
  /* A zero in the mask means *hold this joint*, which on a torque-controlled joint is the
   * difference between holding position and going limp. It is where a `drop` binding from a
   * transport plan meets the wire, and it must not be encoded as a zero value. */
  uint8_t held[3] = {1, 0, 1};
  float pushed[3] = {20.0f, 999.0f, 2.0f};
  cmd.mask = held;
  cmd.values = pushed;
  cmd.seq = 2;
  driver->write_command(ctx, &cmd, 0);
  CHECK("rhal.command.rejection", "a masked joint is held, not zeroed",
        mock_applied_mask(ctx, 1) == 0 && mock_applied(ctx, 1) != 999.0f);
  CHECK("rhal.command.rejection", "an unmasked joint is applied",
        mock_applied(ctx, 0) == 20.0f);

  /* The drive refuses beyond its own rating. This is hardware behaviour underneath the Safety
   * Kernel, not a substitute for it — SCTS measures the joint, not the driver's return code. */
  uint8_t all[3] = {1, 1, 1};
  float over[3] = {500.0f, 0.0f, 0.0f};
  cmd.mask = all;
  cmd.values = over;
  cmd.seq = 3;
  driver->write_command(ctx, &cmd, 0);
  CHECK("safety.envelope.clamps", "the drive refuses beyond its rating",
        mock_applied(ctx, 0) <= 80.0f);

  driver->close(ctx);

  /* ── Fault injection: the paths that cannot be provoked safely on a robot ─ */

  CHECK("safety.watchdog.fires", "a stalled bus repeats its sequence", check_stall(driver));
  CHECK("rhal.state.completeness", "a dead encoder reports NaN", check_nan(driver));
  CHECK("safety.estop.latency", "an asserted estop is observable", check_estop(driver));
  CHECK("rhal.state.rate", "a bus timeout is distinguishable", check_timeout(driver));
  CHECK("rhal.state.completeness", "degraded carries a vendor fault code", check_degraded(driver));
  CHECK("safety.estop.latency", "emergency_stop zeroes every joint", check_estop_zeroes(driver));
  CHECK("safety.estop.latency", "release_stop returns the session", check_release(driver));

  printf("\n%d checks, %d failed\n", checks, failures);
  return failures == 0 ? 0 : 1;
}
