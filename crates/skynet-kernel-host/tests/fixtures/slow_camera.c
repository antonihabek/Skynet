/*
 * slow_camera.c — a one-joint body with a camera that is slower than the control period.
 *
 * A fixture for `tests/loop.rs`, not a driver anybody should copy: it exists to put §89 on a real
 * shared object. `DRY-COVERAGE-PLAN.md` F24 found the problem on a physics simulator whose software
 * render took 18.2 ms of a 20 ms period; this reproduces the shape with a sleep, so the two rules
 * §89 adds can be run through `dlopen`, a vtable and a second thread rather than argued.
 *
 * The config string decides the two things the tests vary:
 *
 *   "render_ms": N          how long read_sensor takes. Default 30, longer than a 20 ms period.
 *   "concurrent": true      whether describe() declares `"concurrent_read": true` on the camera.
 *   "lie": true             declares it, and is not. The control for §89.4.
 *
 * ── Why the declaration is honest when it is made ──────────────────────────
 *
 * §89.1's promise is that read_sensor for this sensor may run on a second thread while read_state
 * and write_command run, without making them wait. It holds here by construction: the camera and
 * the joint share no mutable state. read_sensor touches only `ctx->camera`, read_state and
 * write_command only `ctx->joint`, and nothing in either takes a lock the other holds. The OS
 * promises back that read_sensor never runs concurrently with itself, which is why `ctx->camera`
 * needs no lock of its own.
 *
 * ── And the control, which is the half that matters ────────────────────────
 *
 * `"lie":true` declares §89.1's promise and then breaks it, in the shape a wrapper driver breaks it
 * in: one scratch buffer for both paths, held across the render, with read_state answering SKR_EBUSY
 * rather than waiting on it. `skynet driver check` must fail that declaration (§89.4), and a check
 * that has never failed has proved nothing — so this is what makes the §89.4 check a check.
 *
 * It is deliberately the polite version of the defect. A driver that *waits* on the lock breaks
 * rule 1 and delays the control thread; one that corrupts the state buffer instead produces no
 * error at all, which is what §89.3 means by "a data race with no error anywhere". The refusal is
 * the only one of the three a conformance run can see on every attempt, and a vendor whose driver
 * does either of the other two meets this check having been told what the declaration costs.
 */

/* clock_gettime and nanosleep, which -std=c11 leaves undeclared without it. */
#define _POSIX_C_SOURCE 200809L

#include "skynet_rhal.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#if defined(_WIN32)
#include <windows.h>
#endif

#define WIDTH 8
#define HEIGHT 8
#define CHANNELS 3

struct skr_ctx {
  struct {
    float position;
    float setpoint;
    uint32_t seq;
  } joint;
  struct {
    unsigned render_ms;
    uint64_t frames;
    uint8_t pixels[WIDTH * HEIGHT * CHANNELS];
  } camera;
  int concurrent;
  /* `"lie":true`: one scratch both paths use, held for the length of a render. */
  int lie;
  volatile int scratch_held;
};

static uint64_t now_ns(void) {
#if defined(_WIN32)
  LARGE_INTEGER counter, frequency;
  QueryPerformanceCounter(&counter);
  QueryPerformanceFrequency(&frequency);
  return (uint64_t)((double)counter.QuadPart * 1e9 / (double)frequency.QuadPart);
#else
  struct timespec now;
  clock_gettime(CLOCK_MONOTONIC_RAW, &now); /* §35.1: the clock every stamp_ns is on */
  return (uint64_t)now.tv_sec * 1000000000ull + (uint64_t)now.tv_nsec;
#endif
}

static void sleep_ms(unsigned ms) {
#if defined(_WIN32)
  Sleep(ms);
#else
  struct timespec wait = {(time_t)(ms / 1000u), (long)(ms % 1000u) * 1000000L};
  while (nanosleep(&wait, &wait) != 0) {
  }
#endif
}

/* The number after `"key":` in `config`, or `fallback`. Enough JSON for a fixture's two keys. */
static long config_number(const char *config, const char *key, long fallback) {
  const char *at = config ? strstr(config, key) : NULL;
  if (at == NULL) return fallback;
  at = strchr(at, ':');
  return at ? strtol(at + 1, NULL, 10) : fallback;
}

static int slow_open(const char *config_json, skr_ctx **out) {
  if (out == NULL) return SKR_EINVAL;
  skr_ctx *ctx = calloc(1, sizeof *ctx);
  if (ctx == NULL) return SKR_EIO;
  ctx->camera.render_ms = (unsigned)config_number(config_json, "\"render_ms\"", 30);
  ctx->lie = config_json != NULL && strstr(config_json, "\"lie\":true") != NULL;
  ctx->concurrent =
      ctx->lie || (config_json != NULL && strstr(config_json, "\"concurrent\":true") != NULL);
  *out = ctx;
  return SKR_OK;
}

static int slow_close(skr_ctx *ctx) {
  free(ctx);
  return SKR_OK;
}

/* Wait-free and allocation-free, touching only the joint (rule 1). */
static int slow_read_state(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || out == NULL || out->position_rad == NULL) return SKR_EINVAL;
  if (out->joint_count < 1) return SKR_EINVAL;
  /* The lie: the camera is holding the buffer this path would fill, so there is no state to
   * return. Answering rather than waiting keeps rule 1 and makes the defect visible. The ABI has
   * no "busy", and SKR_ETIMEDOUT is the truthful one of the codes it has: the operation did not
   * complete, and the driver declined to wait for it. */
  if (ctx->scratch_held) return SKR_ETIMEDOUT;
  ctx->joint.seq += 1;
  ctx->joint.position = ctx->joint.setpoint;
  out->stamp_ns = now_ns();
  out->seq = ctx->joint.seq;
  out->joint_count = 1;
  out->flags = SKR_HOMING_NOT_REQUIRED;
  out->position_rad[0] = ctx->joint.position;
  if (out->velocity_rad_s) out->velocity_rad_s[0] = 0.0f;
  if (out->effort_nm) out->effort_nm[0] = NAN;
  for (int i = 0; i < 7; i++) out->base_pose[i] = NAN;
  for (int i = 0; i < 6; i++) out->base_twist[i] = NAN;
  out->fault_code = 0;
  return SKR_OK;
}

static int slow_write_command(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || cmd == NULL) return SKR_EINVAL;
  if (cmd->joint_count != 1 || cmd->values == NULL || cmd->mask == NULL) return SKR_EINVAL;
  if (cmd->mode != SKR_MODE_POSITION) return SKR_EPERM;
  if (cmd->mask[0]) ctx->joint.setpoint = cmd->values[0];
  return SKR_OK;
}

static int slow_emergency_stop(skr_ctx *ctx) {
  (void)ctx;
  return SKR_OK;
}

static int slow_describe(skr_ctx *ctx, char *json_out, size_t len) {
  if (ctx == NULL || json_out == NULL) return SKR_EINVAL;
  int written = snprintf(
      json_out, len,
      "{\"schema_version\":1,\"robot_slug\":\"slow-camera\",\"digest\":null,\"verified\":false,"
      "\"simulator\":true,\"base\":\"fixed\",\"dof\":1,"
      "\"joints\":[{\"name\":\"J1\",\"role\":\"right_arm.wrist.roll\",\"type\":\"continuous\","
      "\"limits\":{\"position_min_rad\":-3.14,\"position_max_rad\":3.14},"
      "\"control_modes\":[\"position\"]}],"
      "\"end_effectors\":[],"
      "\"sensors\":[{\"name\":\"wrist\",\"role\":null,\"modality\":\"rgb\",\"width\":%d,"
      "\"height\":%d%s}],"
      "\"control\":{\"modes\":[\"position\"],\"rate_hz_max\":1000,\"rate_hz_min\":10,"
      "\"command_latency_ms\":null},"
      "\"envelope\":null,\"solvers\":[]}",
      WIDTH, HEIGHT, ctx->concurrent ? ",\"concurrent_read\":true" : "");
  if (written < 0 || (size_t)written >= len) return SKR_EINVAL;
  return written;
}

/* The slow half. Touches only the camera, so it is safe beside the joint's two entry points — and
 * only ever called once at a time, which is what the OS promises in return (§89.1). */
static int slow_read_sensor(skr_ctx *ctx, const char *name, skr_frame *out) {
  if (ctx == NULL || name == NULL || out == NULL) return SKR_EINVAL;
  if (strcmp(name, "wrist") != 0) return SKR_ENODEV;
  uint64_t acquired = now_ns();
  if (ctx->lie) ctx->scratch_held = 1;
  sleep_ms(ctx->camera.render_ms);
  if (ctx->lie) ctx->scratch_held = 0;
  ctx->camera.frames += 1;
  memset(ctx->camera.pixels, (int)(ctx->camera.frames & 0xffu), sizeof ctx->camera.pixels);
  out->stamp_ns = acquired;
  out->width = WIDTH;
  out->height = HEIGHT;
  out->stride = WIDTH * CHANNELS;
  out->format = SKR_FMT_RGB8;
  out->data = ctx->camera.pixels;
  out->size = sizeof ctx->camera.pixels;
  return SKR_OK;
}

static const skr_driver SLOW_CAMERA = {
    SKYNET_RHAL_ABI_VERSION,
    "slow-camera-fixture",
    "0.1.0",
    slow_open,
    slow_close,
    slow_read_state,
    slow_write_command,
    slow_emergency_stop,
    slow_describe,
    slow_read_sensor,
    NULL,
};

const skr_driver *skynet_rhal_entry(void) { return &SLOW_CAMERA; }
