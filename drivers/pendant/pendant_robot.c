/*
 * The pendant robot — a body behind a program-level controller, which is most of them.
 *
 * §59.2 is the section this driver exists for, and it is the bluntest paragraph in the
 * specification:
 *
 *   "Joint" appears forty-three times in this document. `URScript`, `KRL`, `RAPID`, `Fanuc`,
 *   `KUKA` and "teach pendant" appear zero times. The whole design assumes it can send joint
 *   positions, velocities or torques at 50-1000 Hz. **Most of the world's installed industrial
 *   robots do not offer that.**
 *
 * `drivers/mock` is the teaching driver for the machines this design was written for: three joints,
 * a torque command every millisecond, a Safety Kernel between the policy and the drive. This is the
 * teaching driver for the other kind. Behind a Fanuc, KUKA, ABB or older Universal Robots
 * controller you upload a *program*; the controller interpolates, owns the servo loop, and enforces
 * its own safety — often with a certification Skynet does not have and is not trying to replace.
 *
 * So every entry point here answers the way such a machine answers, and the two that matter are the
 * two that refuse:
 *
 *   read_state      works, and is the whole of what this design gets for free. A pendant robot
 *                   reports where its joints are; it simply will not take a setpoint for them.
 *   write_command   **SKR_EPERM, always.** Not ENOSYS: the entry point exists and the command is
 *                   refused, which is a different sentence and the true one. §24's own gloss on
 *                   EPERM is "the command is not permitted in this mode", and there is no mode in
 *                   which it is.
 *   extension       the only route that commands anything. §4 calls it "the vendor escape hatch,
 *                   guarded by a capability token so a policy using a manufacturer feature declares
 *                   the dependency rather than reaching for it" — and for this class of robot it is
 *                   not an escape hatch, it is the interface.
 *
 * # What this driver is evidence of
 *
 * Run `skynet driver check` against it and read the transport line. The design's answer to a
 * program-level controller today is a body whose every joint is uncommandable, and a plan in which
 * every channel of every policy is `blocked`. That is arithmetically correct and it tells an
 * operator nothing, because *"this policy cannot drive this robot"* and *"this robot is not driven
 * that way at all"* are different sentences and only the second has a remedy.
 *
 * `docs/issues/motion-action-space.md` is the amendment that would let the second be written down,
 * and this driver is what it would be tested against. Until then the motions below are reachable
 * only through `extension()`, under a vendor token, which is exactly where an un-standardised thing
 * belongs (§5.1's argument, one level up).
 *
 * # The motion interface
 *
 * Text rather than JSON, because a mock that needed a parser to be useful would be a mock with a
 * dependency — `drivers/mock`'s rule, and its reason. One motion per call:
 *
 *   "move_l pose=0.42 speed=0.10"    a linear move, speed optional within the declared range
 *   "move_j pose=0.42"               a joint-interpolated move
 *   "close_gripper"                  no parameters
 *   "home"                           no parameters
 *
 * The reply is a short line, never a truncation: a buffer too small is SKR_EINVAL, for `describe`'s
 * reason. Every refusal names the cause and the next action, which is what `CONTRIBUTING.md` asks
 * of an error and what a controller's own rejection almost never gives you.
 *
 * Build: see Makefile, or `cc -std=c11 -I../../bindings/c -c pendant_robot.c`.
 * Licence: Apache-2.0.
 */

#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "skynet_rhal.h"

#define PENDANT_JOINTS 6
#define PENDANT_MOTIONS 4

/* The vendor token a caller must present. §4: a policy using a manufacturer feature declares the
 * dependency rather than reaching for it, and the descriptor lists this under `extensions` — not
 * under `capabilities`, because nobody standardised it. */
#define PENDANT_TOKEN "vendor.pendant.motion"

/* ── Layout cross-check ────────────────────────────────────────────────────
 * The same assertions `drivers/mock` makes, for the same reason: two definitions of a struct
 * layout is a segfault, and a build is the only acceptable time to find out (§31.2). */
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(void *) == 8, "the asserted offsets below are for a 64-bit target");
_Static_assert(offsetof(skr_state, position_rad) == 16, "skr_state.position_rad moved");
_Static_assert(offsetof(skr_command, values) == 24, "skr_command.values moved");
_Static_assert(sizeof(skr_driver) == 11 * sizeof(void *), "skr_driver gained or lost an entry point");
#endif

/* ── What the controller accepts ───────────────────────────────────────────
 *
 * One row per motion, and the row *is* the contract: a name, the canonical motion it is where one
 * exists, and its parameters with their bounds. It is the table `docs/issues/motion-action-space.md`
 * proposes putting in the descriptor, written here first because a driver can declare it today
 * through `extension()` and cannot declare it in a descriptor until that amendment lands.
 *
 * `required` is the field that earns the table. A motion whose name a caller gets right and whose
 * required parameter it omits is a command the controller rejects at run time — and moving that
 * failure to install time is the whole of what transport is for. */
struct pendant_param {
  const char *name;
  const char *unit;
  int required;
  /* Declared bounds, and `has_range == 0` means **nobody declared one**, never "unbounded". §5.2's
   * rule, and the reason it is a flag rather than a sentinel: a driver that wrote 0.0/0.0 for
   * "unknown" would be declaring a parameter that may only be zero. */
  int has_range;
  double min;
  double max;
};

struct pendant_motion {
  const char *name;
  /* The canonical motion this is, from the closed vocabulary the amendment proposes, or NULL where
   * the vendor's motion is not one of them. */
  const char *canonical;
  int param_count;
  struct pendant_param params[3];
};

static const struct pendant_motion PENDANT_PROGRAM[PENDANT_MOTIONS] = {
    {"move_l",
     "move_linear",
     2,
     {{"pose", "m", 1, 1, -1.5, 1.5}, {"speed", "m/s", 0, 1, 0.01, 0.25}, {0}}},
    {"move_j",
     "move_joint",
     2,
     {{"pose", "rad", 1, 1, -3.14, 3.14}, {"speed", "rad/s", 0, 1, 0.01, 1.0}, {0}}},
    {"close_gripper", "grasp", 0, {{0}, {0}, {0}}},
    {"home", "home", 0, {{0}, {0}, {0}}},
};

/* ── State ─────────────────────────────────────────────────────────────────
 * Statically allocated, for `drivers/mock`'s reason: a real driver allocates in open() and frees in
 * close(), and must not allocate on the real-time path. */
struct skr_ctx {
  int open;
  uint32_t seq;
  uint64_t clock_ns;
  int estop_latched;
  /* Where the joints are, and where the running program is taking them. The controller owns the
   * interpolation — that is the entire point of this class of machine — so the target moves and
   * this driver watches, exactly as it would over a vendor's status interface. */
  float position[PENDANT_JOINTS];
  float velocity[PENDANT_JOINTS];
  float target[PENDANT_JOINTS];
  int program_running;
  uint32_t motions_accepted;
  uint32_t motions_refused;
  uint32_t commands_refused;
};

static skr_ctx g_ctx;

static const char *const PENDANT_JOINT_NAMES[PENDANT_JOINTS] = {"A1", "A2", "A3",
                                                                "A4", "A5", "A6"};

static int pendant_open(const char *config_json, skr_ctx **out) {
  (void)config_json;
  if (out == NULL) return SKR_EINVAL;
  memset(&g_ctx, 0, sizeof g_ctx);
  g_ctx.open = 1;
  g_ctx.clock_ns = 1000000000ull;
  *out = &g_ctx;
  return SKR_OK;
}

static int pendant_close(skr_ctx *ctx) {
  if (ctx == NULL) return SKR_EINVAL;
  ctx->open = 0;
  return SKR_OK;
}

/* Real-time path, and the half of the RHAL this class of robot genuinely offers. Wait-free, no
 * allocation. */
static int pendant_read_state(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || out == NULL) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;

  /* The controller's own interpolator, standing in for the one behind a real pendant. Nothing
   * outside this driver decides the step: a policy asked for a motion, and what happens between
   * here and the target belongs to the vendor. */
  int moving = 0;
  for (int j = 0; j < PENDANT_JOINTS; ++j) {
    float gap = ctx->target[j] - ctx->position[j];
    if (gap > 0.002f || gap < -0.002f) {
      float step = gap * 0.05f;
      ctx->position[j] += step;
      ctx->velocity[j] = step * 1000.0f;
      moving = 1;
    } else {
      ctx->position[j] = ctx->target[j];
      ctx->velocity[j] = 0.0f;
    }
  }
  if (!moving) ctx->program_running = 0;

  ctx->clock_ns += 1000000ull;
  ctx->seq += 1;

  out->stamp_ns = ctx->clock_ns;
  out->seq = ctx->seq;
  out->joint_count = PENDANT_JOINTS;
  out->flags = 0;
  if (ctx->estop_latched) out->flags |= SKR_FLAG_ESTOP;
  /* An industrial controller with absolute encoders and a mastered zero. `not_required` rather
   * than `complete`: there is no homing routine for an operator to run, and reporting `complete`
   * would claim one had been. §24's two-bit field exists for exactly this distinction. */
  out->flags |= SKR_HOMING_NOT_REQUIRED;

  if (out->position_rad != NULL) memcpy(out->position_rad, ctx->position, sizeof ctx->position);
  if (out->velocity_rad_s != NULL) memcpy(out->velocity_rad_s, ctx->velocity, sizeof ctx->velocity);
  /* **NaN, not zero.** §24's rule for a quantity the driver does not measure, and a pendant robot
   * does not publish joint torque over its status interface. A zero here would be a policy reading
   * "this joint is under no load" every cycle, which is a measurement that was never taken. */
  if (out->effort_nm != NULL) {
    for (int j = 0; j < PENDANT_JOINTS; ++j) out->effort_nm[j] = NAN;
  }
  for (int i = 0; i < 7; ++i) out->base_pose[i] = NAN; /* a cell robot is not localised */
  for (int i = 0; i < 6; ++i) out->base_twist[i] = 0.0f;
  out->fault_code = 0u;
  return SKR_OK;
}

/*
 * The refusal this driver exists to make.
 *
 * SKR_EPERM on every well-formed call, and counted so a harness can show that the refusal happened
 * rather than infer it from a robot that did not move. The entry point is present because §4
 * requires it; returning SKR_ENOSYS would say "this driver does not implement joint commands", and
 * what is true is that the *machine* does not accept them — the driver implements the refusal.
 *
 * **The shape is checked first, and that ordering is the whole of the subtlety.** A command
 * declaring five joints against a six-joint body is malformed whether or not this machine takes
 * commands at all, and SCTS `rhal.command.rejection` asks for SKR_EINVAL on exactly that. A driver
 * that answered EPERM to everything would be telling a caller "you may not" about a command that is
 * also wrong — the caller fixes its permissions, ships, and finds the array overrun on a machine
 * that does take commands. Malformed is malformed: EINVAL, then EPERM for what is merely refused.
 */
static int pendant_write_command(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || cmd == NULL) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;
  if (cmd->joint_count != PENDANT_JOINTS) return SKR_EINVAL;
  if (cmd->values == NULL) return SKR_EINVAL;
  if (cmd->mode > SKR_MODE_TORQUE) return SKR_EINVAL;
  ctx->commands_refused += 1;
  return SKR_EPERM;
}

/*
 * Callable from a signal handler and from a process that is dying: no allocation, no locking, no
 * I/O that can block. On a real pendant robot this drops the safety relay, which is the one command
 * path that does not go through the controller's program interpreter — and that is why §59.2 says
 * these machines are safe without Skynet rather than unsafe with it.
 */
static int pendant_emergency_stop(skr_ctx *ctx) {
  if (ctx == NULL) return SKR_EINVAL;
  ctx->estop_latched = 1;
  ctx->program_running = 0;
  for (int j = 0; j < PENDANT_JOINTS; ++j) {
    ctx->target[j] = ctx->position[j];
    ctx->velocity[j] = 0.0f;
  }
  return SKR_OK;
}

/*
 * The descriptor.
 *
 * Three things a reader should not skim past.
 *
 * **Every joint declares `control_modes: []`**, and the profile declares `modes: []` beside it.
 * That is the honest shape of this machine in the schema as it stands today: the joints exist, they
 * are reported, and there is no mode in which they are commanded. It is not an omission and a
 * future version of this driver must not "fix" it by naming `position` — the controller would
 * refuse the command, and the descriptor would have lied to admission rather than to a person.
 *
 * **`envelope` is null**, for `drivers/mock`'s reason and one more. The controller enforces its own
 * limits and they are genuinely certified; what it does not do is publish them as the numbers §5.2
 * asks for, and copying a value off a data sheet into a field the Safety Kernel enforces would be
 * enforcing a rumour (§63.4).
 *
 * **`simulator` is true.** There is no machine here. §35.3 caps it at guest, and §59.2 caps a real
 * one at guest too — two independent reasons for the same ceiling, and a reader should not take the
 * first as evidence for the second. A real pendant robot's driver declares `simulator: false` and
 * is still permanently Guest tier.
 */
static const char PENDANT_DESCRIPTOR[] =
    "{\"schema_version\":1,\"robot_slug\":\"pendant-cell-arm\",\"digest\":null,"
    "\"verified\":false,\"simulator\":true,"
    "\"base\":\"fixed\",\"dof\":6,"
    "\"joints\":["
    "{\"name\":\"A1\",\"role\":\"right_arm.shoulder.yaw\",\"type\":\"revolute\","
    "\"limits\":{\"position_min_rad\":-3.14,\"position_max_rad\":3.14,"
    "\"velocity_max_rad_s\":2},\"control_modes\":[]},"
    "{\"name\":\"A2\",\"role\":\"right_arm.shoulder.pitch\",\"type\":\"revolute\","
    "\"limits\":{\"position_min_rad\":-2.4,\"position_max_rad\":2.4,"
    "\"velocity_max_rad_s\":2},\"control_modes\":[]},"
    "{\"name\":\"A3\",\"role\":\"right_arm.elbow.flex\",\"type\":\"revolute\","
    "\"limits\":{\"position_min_rad\":-2.6,\"position_max_rad\":2.6,"
    "\"velocity_max_rad_s\":2},\"control_modes\":[]},"
    "{\"name\":\"A4\",\"role\":\"right_arm.wrist.roll\",\"type\":\"continuous\","
    "\"limits\":{\"position_min_rad\":-6.28,\"position_max_rad\":6.28,"
    "\"velocity_max_rad_s\":4},\"control_modes\":[]},"
    "{\"name\":\"A5\",\"role\":\"right_arm.wrist.pitch\",\"type\":\"revolute\","
    "\"limits\":{\"position_min_rad\":-2.0,\"position_max_rad\":2.0,"
    "\"velocity_max_rad_s\":4},\"control_modes\":[]},"
    "{\"name\":\"A6\",\"role\":\"right_arm.wrist.yaw\",\"type\":\"continuous\","
    "\"limits\":{\"position_min_rad\":-6.28,\"position_max_rad\":6.28,"
    "\"velocity_max_rad_s\":4},\"control_modes\":[]}"
    "],"
    "\"end_effectors\":[],\"sensors\":[],"
    "\"control\":{\"modes\":[],\"rate_hz_max\":125,\"rate_hz_min\":1},"
    "\"envelope\":null,"
    "\"capabilities\":[],"
    "\"extensions\":[\"" PENDANT_TOKEN "\"],"
    "\"solvers\":[]}";

static int pendant_describe(skr_ctx *ctx, char *json_out, size_t len) {
  if (ctx == NULL || json_out == NULL) return SKR_EINVAL;
  size_t needed = sizeof PENDANT_DESCRIPTOR;
  if (len < needed) return SKR_EINVAL;
  memcpy(json_out, PENDANT_DESCRIPTOR, needed);
  return (int)(needed - 1);
}

/* ── The motion interface ──────────────────────────────────────────────────*/

/* Write a reply, refusing rather than truncating. A caller that gets half a sentence acts on half a
 * sentence, and `describe`'s rule is the right one here too. */
static int reply(char *out, size_t out_len, const char *text) {
  size_t needed = strlen(text) + 1;
  if (out == NULL || out_len < needed) return SKR_EINVAL;
  memcpy(out, text, needed);
  return (int)(needed - 1);
}

/* `name=value` out of the request, if it is there. Returns 1 and writes `value`. */
static int argument_of(const char *request, const char *name, double *value) {
  const char *at = request;
  size_t name_len = strlen(name);
  while ((at = strstr(at, name)) != NULL) {
    /* A match must start a word and be followed by `=`, so `speed` does not match `xspeed` and
     * `pose` does not match `pose_frame`. */
    int starts_word = (at == request) || at[-1] == ' ';
    if (starts_word && at[name_len] == '=') {
      char *end = NULL;
      double parsed = strtod(at + name_len + 1, &end);
      if (end == at + name_len + 1) return -1; /* present and not a number */
      *value = parsed;
      return 1;
    }
    at += name_len;
  }
  return 0;
}

static const struct pendant_motion *motion_named(const char *name, size_t len) {
  for (int i = 0; i < PENDANT_MOTIONS; ++i) {
    if (strlen(PENDANT_PROGRAM[i].name) == len &&
        strncmp(PENDANT_PROGRAM[i].name, name, len) == 0) {
      return &PENDANT_PROGRAM[i];
    }
  }
  return NULL;
}

/*
 * The only route that moves this robot.
 *
 * Every refusal names the cause and the next action. That is `CONTRIBUTING.md`'s rule, and it is
 * worth more here than almost anywhere else in this repository: a real controller's rejection is
 * typically a numeric code in a status word, and an integrator's afternoon is spent turning it back
 * into a sentence.
 */
static int pendant_extension(skr_ctx *ctx, const char *token, const void *in, size_t in_len,
                             void *out, size_t out_len) {
  char *text = (char *)out;
  if (ctx == NULL || token == NULL) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;
  if (strcmp(token, PENDANT_TOKEN) != 0) {
    return reply(text, out_len,
                 "unknown token. This controller offers `" PENDANT_TOKEN "` and nothing else; it "
                 "is listed under `extensions` in the descriptor.") == SKR_EINVAL
               ? SKR_EINVAL
               : SKR_ENOSYS;
  }
  if (in == NULL || in_len == 0) {
    return reply(text, out_len,
                 "empty request. One motion per call: `move_l pose=0.4 speed=0.1`.") == SKR_EINVAL
               ? SKR_EINVAL
               : SKR_EINVAL;
  }
  /* The request is a caller's buffer of `in_len` bytes and is not promised to be NUL-terminated. */
  char request[128];
  if (in_len >= sizeof request) {
    return reply(text, out_len, "request longer than this controller accepts (127 bytes).") ==
                   SKR_EINVAL
               ? SKR_EINVAL
               : SKR_EINVAL;
  }
  memcpy(request, in, in_len);
  request[in_len] = '\0';

  if (ctx->estop_latched) {
    reply(text, out_len,
          "the emergency stop is latched. Clear it at the controller before sending a motion; "
          "nothing on this interface can.");
    return SKR_EPERM;
  }

  size_t name_len = strcspn(request, " ");
  const struct pendant_motion *motion = motion_named(request, name_len);
  if (motion == NULL) {
    char message[224];
    snprintf(message, sizeof message,
             "this controller has no motion by that name. It accepts: %s, %s, %s, %s.",
             PENDANT_PROGRAM[0].name, PENDANT_PROGRAM[1].name, PENDANT_PROGRAM[2].name,
             PENDANT_PROGRAM[3].name);
    ctx->motions_refused += 1;
    reply(text, out_len, message);
    return SKR_ENODEV;
  }

  double pose = 0.0;
  double speed = 0.0;
  int have_pose = 0;
  for (int p = 0; p < motion->param_count; ++p) {
    const struct pendant_param *param = &motion->params[p];
    double value = 0.0;
    int found = argument_of(request, param->name, &value);
    if (found < 0) {
      char message[224];
      snprintf(message, sizeof message, "`%s` was given and is not a number.", param->name);
      ctx->motions_refused += 1;
      reply(text, out_len, message);
      return SKR_EINVAL;
    }
    if (found == 0) {
      if (!param->required) continue;
      char message[224];
      snprintf(message, sizeof message,
               "`%s` requires `%s` (%s) and it was not given. The controller would reject this at "
               "run time; it is refused here instead.",
               motion->name, param->name, param->unit);
      ctx->motions_refused += 1;
      reply(text, out_len, message);
      return SKR_EINVAL;
    }
    if (param->has_range && (value < param->min || value > param->max)) {
      char message[224];
      snprintf(message, sizeof message,
               "`%s` = %g is outside the range this controller declares for it (%g to %g %s).",
               param->name, value, param->min, param->max, param->unit);
      ctx->motions_refused += 1;
      reply(text, out_len, message);
      return SKR_EINVAL;
    }
    if (strcmp(param->name, "pose") == 0) {
      pose = value;
      have_pose = 1;
    } else if (strcmp(param->name, "speed") == 0) {
      speed = value;
    }
  }
  (void)speed; /* declared, bounded and accepted; this mock interpolates at one rate */

  /* Accepted. The program runs and the controller owns what happens next — which is the fact this
   * whole driver is here to make concrete. */
  for (int j = 0; j < PENDANT_JOINTS; ++j) {
    if (strcmp(motion->name, "home") == 0) {
      ctx->target[j] = 0.0f;
    } else if (strcmp(motion->name, "close_gripper") == 0) {
      /* No axis moves: the gripper is on the tool flange and is not one of the six. */
    } else if (have_pose) {
      ctx->target[j] = (float)pose;
    }
  }
  ctx->program_running = 1;
  ctx->motions_accepted += 1;
  return reply(text, out_len, "accepted");
}

static const skr_driver PENDANT_DRIVER = {
    SKYNET_RHAL_ABI_VERSION,
    "skynet-pendant-cell-arm",
    "0.1.0",
    pendant_open,
    pendant_close,
    pendant_read_state,
    pendant_write_command,
    pendant_emergency_stop,
    pendant_describe,
    NULL, /* read_sensor: the cell's cameras are not on this controller's interface */
    pendant_extension,
};

const skr_driver *skynet_rhal_entry(void) { return &PENDANT_DRIVER; }

/* Test-only accessors, as `drivers/mock` has. A shipping driver exports `skynet_rhal_entry` alone. */
uint32_t pendant_motions_accepted(const skr_ctx *ctx) { return ctx->motions_accepted; }
uint32_t pendant_motions_refused(const skr_ctx *ctx) { return ctx->motions_refused; }
uint32_t pendant_commands_refused(const skr_ctx *ctx) { return ctx->commands_refused; }
int pendant_program_running(const skr_ctx *ctx) { return ctx->program_running; }
float pendant_position(const skr_ctx *ctx, int joint) { return ctx->position[joint]; }
const char *pendant_joint_name(int joint) { return PENDANT_JOINT_NAMES[joint]; }
int pendant_motion_count(void) { return PENDANT_MOTIONS; }
const char *pendant_motion_name(int index) { return PENDANT_PROGRAM[index].name; }
const char *pendant_motion_canonical(int index) { return PENDANT_PROGRAM[index].canonical; }
