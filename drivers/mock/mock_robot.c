/*
 * The mock robot — a body that answers the RHAL and has no hardware behind it.
 *
 * §63.2. A developer writes and tests a driver against this with no robot present, and — the part
 * that matters more — tests the **failure** paths that cannot be provoked safely on a real machine:
 * a stalled command stream, a joint reporting NaN, an estop asserted mid-motion, a bus timeout
 * inside a cycle. Being able to develop the dangerous cases without a robot in the room is the
 * difference between a driver that has been exercised and one that has merely been run.
 *
 * It is also the reference a driver author reads. Every entry point here is implemented the way
 * §4's three rules require, and the places where a real driver would do something different are
 * marked.
 *
 * Faults are selected by substring in the config string passed to open(), because a mock robot
 * that needed a JSON parser to be useful would be a mock robot with a dependency:
 *
 *   ""                  a well-behaved three-joint arm
 *   "fault=stall"       read_state stops advancing `seq` — a stalled producer upstream
 *   "fault=nan"         joint 1 reports NaN velocity — a dead encoder
 *   "fault=estop"       the estop flag is asserted from the first read
 *   "fault=timeout"     read_state returns SKR_ETIMEDOUT — a bus that did not answer in time
 *   "fault=degraded"    the degraded flag is set with a vendor fault code
 *
 * Build: see Makefile, or `cc -std=c11 -I../../bindings/c -c mock_robot.c`.
 * Licence: Apache-2.0.
 */

#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "skynet_rhal.h"

#define MOCK_JOINTS 3

/* ── Layout cross-check ────────────────────────────────────────────────────
 *
 * `crates/skynet-rhal` asserts these same offsets from the Rust side. §31.2 says the RHAL C ABI
 * stays single-source no matter how many editions exist, "because two definitions of a struct
 * layout is a segfault" — and the way to keep that true rather than aspirational is to assert it
 * from both sides, so that a change to either one fails a build.
 *
 * These are compile-time. A driver that includes this header on a target where the layout differs
 * does not build, which is the only acceptable time to find out. */
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(void *) == 8, "the asserted offsets below are for a 64-bit target");
_Static_assert(offsetof(skr_state, stamp_ns) == 0, "skr_state.stamp_ns moved");
_Static_assert(offsetof(skr_state, seq) == 8, "skr_state.seq moved");
_Static_assert(offsetof(skr_state, joint_count) == 12, "skr_state.joint_count moved");
_Static_assert(offsetof(skr_state, flags) == 14, "skr_state.flags moved");
_Static_assert(offsetof(skr_state, position_rad) == 16, "skr_state.position_rad moved");
_Static_assert(offsetof(skr_state, velocity_rad_s) == 24, "skr_state.velocity_rad_s moved");
_Static_assert(offsetof(skr_state, effort_nm) == 32, "skr_state.effort_nm moved");
_Static_assert(offsetof(skr_state, base_pose) == 40, "skr_state.base_pose moved");
_Static_assert(offsetof(skr_state, base_twist) == 68, "skr_state.base_twist moved");
_Static_assert(offsetof(skr_state, fault_code) == 92, "skr_state.fault_code moved");

_Static_assert(offsetof(skr_command, stamp_ns) == 0, "skr_command.stamp_ns moved");
_Static_assert(offsetof(skr_command, seq) == 8, "skr_command.seq moved");
_Static_assert(offsetof(skr_command, mode) == 12, "skr_command.mode moved");
_Static_assert(offsetof(skr_command, _pad) == 13, "skr_command._pad moved");
_Static_assert(offsetof(skr_command, joint_count) == 16, "skr_command.joint_count moved");
_Static_assert(offsetof(skr_command, values) == 24, "skr_command.values moved");
_Static_assert(offsetof(skr_command, mask) == 32, "skr_command.mask moved");

_Static_assert(offsetof(skr_frame, stamp_ns) == 0, "skr_frame.stamp_ns moved");
_Static_assert(offsetof(skr_frame, width) == 8, "skr_frame.width moved");
_Static_assert(offsetof(skr_frame, data) == 24, "skr_frame.data moved");
_Static_assert(sizeof(skr_driver) == 11 * sizeof(void *), "skr_driver gained or lost an entry point");
#endif

/* ── State ─────────────────────────────────────────────────────────────────
 *
 * Everything is statically allocated. A real driver allocates in open() and frees in close(); it
 * must not allocate in read_state() or write_command(), which is the point being demonstrated. */

enum mock_fault {
  MOCK_FAULT_NONE = 0,
  MOCK_FAULT_STALL,
  MOCK_FAULT_NAN,
  MOCK_FAULT_ESTOP,
  MOCK_FAULT_TIMEOUT,
  MOCK_FAULT_DEGRADED
};

struct skr_ctx {
  int open;
  enum mock_fault fault;
  uint32_t seq;
  uint64_t clock_ns;
  int estop_latched;
  float position[MOCK_JOINTS];
  float velocity[MOCK_JOINTS];
  float effort[MOCK_JOINTS];
  /* The last command actually applied, so a harness can check what got through. */
  float applied[MOCK_JOINTS];
  uint8_t applied_mask[MOCK_JOINTS];
  uint32_t commands_accepted;
  uint32_t commands_rejected;
};

static skr_ctx g_ctx;

/* The joint names, matching the descriptor below. A real driver reads these from its bus. */
static const char *const MOCK_JOINT_NAMES[MOCK_JOINTS] = {"J1", "J2", "J3"};

/* Per-joint torque ratings, used to make the mock behave plausibly under a command. These are
 * *design* values on a mock. Note the descriptor still declares `envelope: null` — see below. */
static const float MOCK_EFFORT_MAX[MOCK_JOINTS] = {80.0f, 60.0f, 20.0f};

static enum mock_fault parse_fault(const char *config) {
  if (config == NULL) return MOCK_FAULT_NONE;
  if (strstr(config, "fault=stall") != NULL) return MOCK_FAULT_STALL;
  if (strstr(config, "fault=nan") != NULL) return MOCK_FAULT_NAN;
  if (strstr(config, "fault=estop") != NULL) return MOCK_FAULT_ESTOP;
  if (strstr(config, "fault=timeout") != NULL) return MOCK_FAULT_TIMEOUT;
  if (strstr(config, "fault=degraded") != NULL) return MOCK_FAULT_DEGRADED;
  return MOCK_FAULT_NONE;
}

static int mock_open(const char *config_json, skr_ctx **out) {
  if (out == NULL) return SKR_EINVAL;

  memset(&g_ctx, 0, sizeof g_ctx);
  g_ctx.open = 1;
  g_ctx.fault = parse_fault(config_json);
  g_ctx.clock_ns = 1000000000ull; /* an arbitrary monotonic origin */

  for (int j = 0; j < MOCK_JOINTS; ++j) {
    g_ctx.position[j] = 0.0f;
    g_ctx.velocity[j] = 0.0f;
    g_ctx.effort[j] = 0.0f;
    g_ctx.applied_mask[j] = 0;
  }
  if (g_ctx.fault == MOCK_FAULT_ESTOP) g_ctx.estop_latched = 1;

  *out = &g_ctx;
  return SKR_OK;
}

static int mock_close(skr_ctx *ctx) {
  if (ctx == NULL) return SKR_EINVAL;
  ctx->open = 0;
  return SKR_OK;
}

/* Real-time path. Wait-free, no allocation, no locks, no syscalls beyond what the bus needs. */
static int mock_read_state(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || out == NULL) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;
  if (ctx->fault == MOCK_FAULT_TIMEOUT) return SKR_ETIMEDOUT;

  /* Advance a plausible body: each joint drifts toward whatever was last applied to it. */
  for (int j = 0; j < MOCK_JOINTS; ++j) {
    if (ctx->applied_mask[j]) {
      float delta = ctx->applied[j] * 0.001f;
      ctx->velocity[j] = delta;
      ctx->position[j] += delta;
      ctx->effort[j] = ctx->applied[j];
    } else {
      /* A masked joint holds. It does not go to zero — that is the whole point of the mask. */
      ctx->velocity[j] = 0.0f;
    }
  }

  if (ctx->fault == MOCK_FAULT_NAN) ctx->velocity[1] = NAN;

  ctx->clock_ns += 1000000ull; /* 1 kHz */
  /* A stalled bus repeats its last sample. The sequence number not advancing is the evidence, and
   * it is what the kernel's watchdog guard is looking for. */
  if (ctx->fault != MOCK_FAULT_STALL) ctx->seq += 1;

  /* The timestamp is taken at acquisition. On a mock that is here; on a real driver it is
   * whatever the bus stamped the frame with, and never the moment this function returns. */
  out->stamp_ns = ctx->clock_ns;
  out->seq = ctx->seq;
  out->joint_count = MOCK_JOINTS;
  out->flags = 0;
  if (ctx->estop_latched) out->flags |= SKR_FLAG_ESTOP;
  if (ctx->fault == MOCK_FAULT_DEGRADED) out->flags |= SKR_FLAG_DEGRADED;
  /* §24's homing field, and the answer a simulated body has to give is `not required` rather than
   * `complete`. There are no encoders here to lose their zero: `ctx->position` *is* the joint
   * position, so there is no homing routine for an operator to run and reporting `complete` would
   * claim one had been run. §27 treats the two identically; a person reading a console does not. */
  out->flags |= SKR_HOMING_NOT_REQUIRED;

  /* Caller-allocated arrays; the driver fills and never retains the pointers. */
  if (out->position_rad != NULL) {
    memcpy(out->position_rad, ctx->position, sizeof ctx->position);
  }
  if (out->velocity_rad_s != NULL) {
    memcpy(out->velocity_rad_s, ctx->velocity, sizeof ctx->velocity);
  }
  if (out->effort_nm != NULL) {
    memcpy(out->effort_nm, ctx->effort, sizeof ctx->effort);
  }
  for (int i = 0; i < 7; ++i) out->base_pose[i] = NAN; /* a bench arm is not localised */
  for (int i = 0; i < 6; ++i) out->base_twist[i] = 0.0f;
  out->fault_code = (ctx->fault == MOCK_FAULT_DEGRADED) ? 0x8042u : 0u;

  return SKR_OK;
}

static int mock_write_command(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || cmd == NULL) return SKR_EINVAL;
  if (!ctx->open) return SKR_EPERM;

  /* A command naming a joint count this body does not have is *rejected*, not ignored. SCTS
   * `rhal.command.rejection` is exactly this check, and a driver that quietly accepts it is a
   * driver that will one day drive the wrong joint. */
  if (cmd->joint_count != MOCK_JOINTS) {
    ctx->commands_rejected += 1;
    return SKR_EINVAL;
  }
  if (cmd->values == NULL) {
    ctx->commands_rejected += 1;
    return SKR_EINVAL;
  }
  if (cmd->mode > SKR_MODE_TORQUE) {
    ctx->commands_rejected += 1;
    return SKR_EINVAL;
  }
  /* A latched estop means commands are being ignored, and the driver says so rather than
   * pretending to have applied them. */
  if (ctx->estop_latched) {
    ctx->commands_rejected += 1;
    return SKR_EPERM;
  }

  for (int j = 0; j < MOCK_JOINTS; ++j) {
    uint8_t apply = (cmd->mask == NULL) ? 1u : cmd->mask[j];
    ctx->applied_mask[j] = apply;
    if (!apply) continue; /* hold: the previous setpoint stands */

    float value = cmd->values[j];
    /* The drive itself refuses beyond its rating. This is the hardware behind the Safety Kernel,
     * not a substitute for it: SCTS `safety.envelope.clamps` measures the joint, not the driver. */
    if (value > MOCK_EFFORT_MAX[j]) value = MOCK_EFFORT_MAX[j];
    if (value < -MOCK_EFFORT_MAX[j]) value = -MOCK_EFFORT_MAX[j];
    ctx->applied[j] = value;
  }
  ctx->commands_accepted += 1;
  return SKR_OK;
}

/*
 * Must be callable from a signal handler and from a process that is dying.
 *
 * So: no allocation, no locking, no I/O that can block, and no dependency on any other part of the
 * driver still being consistent. A driver that implements this as "publish a message and await an
 * ack" fails the test that matters most.
 */
static int mock_emergency_stop(skr_ctx *ctx) {
  if (ctx == NULL) return SKR_EINVAL;
  ctx->estop_latched = 1;
  for (int j = 0; j < MOCK_JOINTS; ++j) {
    ctx->applied[j] = 0.0f;
    ctx->applied_mask[j] = 0;
    ctx->effort[j] = 0.0f;
    ctx->velocity[j] = 0.0f;
  }
  return SKR_OK;
}

/*
 * The embodiment descriptor.
 *
 * Two things about it are worth a driver author's attention.
 *
 * `envelope` is **null**. This mock knows its joints' design torque limits — they are right there
 * in MOCK_EFFORT_MAX — and it still declares no safety envelope, because a design value from a CAD
 * model is not a datasheet safety rating (§63.4). Copying one into the other would auto-populate a
 * safety limit nobody verified, and the Safety Kernel would then be enforcing a rumour. The robot
 * renders "no safety envelope declared" until a person supplies a sourced figure, which is the
 * correct and slightly annoying answer.
 *
 * `roles` **are** declared here, because on a mock we know what the joints mean. A generator
 * reading a real URDF only *suggests* them and waits for a human, for the same reason at lower
 * stakes.
 *
 * `simulator` is **true**, and it is the field this descriptor went longest without. SS52 puts the
 * obligation to declare squarely on the simulator -- *"a simulator presented as a robot"* is the
 * direction that costs something, because SS35.3 caps a simulated body at guest tier and rules that
 * no simulated run is bench evidence. Absent, the flag reads as false, and this body was claiming
 * to be a real arm. It happened to be capped at guest anyway, for an unrelated reason: a driver
 * cannot sign its own descriptor, so `embodiment.descriptor.signed` is unmeasured and absence is
 * not a pass. That is a coincidence and not a mechanism -- sign this descriptor and the cap would
 * have lifted, on a robot with no joints in it.
 */
static const char MOCK_DESCRIPTOR[] =
    "{\"schema_version\":1,\"robot_slug\":\"mock-arm\",\"digest\":null,\"verified\":false,"
    "\"simulator\":true,"
    "\"base\":\"fixed\",\"dof\":3,"
    "\"joints\":["
    "{\"name\":\"J1\",\"role\":\"right_arm.shoulder.pitch\",\"type\":\"revolute\","
    "\"limits\":{\"position_min_rad\":-3.14,\"position_max_rad\":3.14,"
    "\"velocity_max_rad_s\":2,\"effort_max_nm\":80},"
    "\"control_modes\":[\"position\",\"torque\"]},"
    "{\"name\":\"J2\",\"role\":\"right_arm.elbow.flex\",\"type\":\"revolute\","
    "\"limits\":{\"position_min_rad\":-2.6,\"position_max_rad\":2.6,"
    "\"velocity_max_rad_s\":2,\"effort_max_nm\":60},"
    "\"control_modes\":[\"position\",\"torque\"]},"
    "{\"name\":\"J3\",\"role\":\"right_arm.wrist.roll\",\"type\":\"continuous\","
    "\"limits\":{\"position_min_rad\":-6.28,\"position_max_rad\":6.28,"
    "\"velocity_max_rad_s\":4,\"effort_max_nm\":20},"
    "\"control_modes\":[\"position\"]}"
    "],"
    "\"end_effectors\":[],\"sensors\":[],"
    "\"control\":{\"modes\":[\"position\",\"torque\"],\"rate_hz_max\":1000,"
    "\"rate_hz_min\":10,\"command_latency_ms\":1},"
    "\"envelope\":null,"
    "\"solvers\":[]}";

static int mock_describe(skr_ctx *ctx, char *json_out, size_t len) {
  if (ctx == NULL || json_out == NULL) return SKR_EINVAL;
  size_t needed = sizeof MOCK_DESCRIPTOR; /* includes the NUL */
  /* A buffer that is too small is an error, never a truncation. Half a parsed body description is
   * worse than a failed one, so the OS retries with a larger buffer instead. */
  if (len < needed) return SKR_EINVAL;
  memcpy(json_out, MOCK_DESCRIPTOR, needed);
  return (int)(needed - 1);
}

/* Optional entry points are NULL rather than stubs that return success. The OS reports the
 * capability as absent, which is true, instead of calling through to something that lies. */
static const skr_driver MOCK_DRIVER = {
    SKYNET_RHAL_ABI_VERSION,
    "skynet-mock-arm",
    "0.1.0",
    mock_open,
    mock_close,
    mock_read_state,
    mock_write_command,
    mock_emergency_stop,
    mock_describe,
    NULL, /* read_sensor: this body has no sensors on the RHAL */
    NULL, /* extension:   no vendor escape hatch */
};

const skr_driver *skynet_rhal_entry(void) { return &MOCK_DRIVER; }

/* Test-only accessors, so the harness can check what actually got through without reaching into
 * the context struct. A shipping driver exports nothing but `skynet_rhal_entry`. */
uint32_t mock_commands_accepted(const skr_ctx *ctx) { return ctx->commands_accepted; }
uint32_t mock_commands_rejected(const skr_ctx *ctx) { return ctx->commands_rejected; }
float mock_applied(const skr_ctx *ctx, int joint) { return ctx->applied[joint]; }
uint8_t mock_applied_mask(const skr_ctx *ctx, int joint) { return ctx->applied_mask[joint]; }
const char *mock_joint_name(int joint) { return MOCK_JOINT_NAMES[joint]; }
