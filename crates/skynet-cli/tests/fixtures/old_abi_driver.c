/*
 * A driver built against ABI 1, which is what a `.so` left over from before `release_stop` was
 * appended to the vtable looks like to this OS (bindings/c/skynet_rhal.h, v2).
 *
 * The table below is one word shorter than the v2 `skr_driver`, exactly as v1's was, and every
 * entry point in it leaves a marker file at the path named by SKYNET_TEST_MARKER. The test that
 * loads it asserts the marker was never written: `skynet driver check` must refuse a table whose
 * layout it does not know before it calls anything through it (DRY-COVERAGE-PLAN.md F173).
 *
 * Deliberately not built from the header's `skr_driver`, which is v2's.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "skynet_rhal.h"

static void touch(void) {
  const char *path = getenv("SKYNET_TEST_MARKER");
  if (path) {
    FILE *marker = fopen(path, "w");
    if (marker) {
      fputs("called\n", marker);
      fclose(marker);
    }
  }
}

static int v1_open(const char *config, skr_ctx **out) {
  (void)config;
  touch();
  *out = NULL;
  return SKR_EINVAL;
}
static int v1_close(skr_ctx *ctx) { (void)ctx; touch(); return SKR_OK; }
static int v1_read_state(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns) {
  (void)ctx; (void)out; (void)deadline_ns; touch(); return SKR_EINVAL;
}
static int v1_write_command(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns) {
  (void)ctx; (void)cmd; (void)deadline_ns; touch(); return SKR_EINVAL;
}
static int v1_emergency_stop(skr_ctx *ctx) { (void)ctx; touch(); return SKR_OK; }
static int v1_describe(skr_ctx *ctx, char *json_out, size_t len) {
  (void)ctx; (void)json_out; (void)len; touch(); return SKR_EINVAL;
}

typedef struct {
  uint32_t abi_version;
  const char *driver_name;
  const char *driver_version;
  int (*open)(const char *config_json, skr_ctx **out);
  int (*close)(skr_ctx *ctx);
  int (*read_state)(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns);
  int (*write_command)(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns);
  int (*emergency_stop)(skr_ctx *ctx);
  int (*describe)(skr_ctx *ctx, char *json_out, size_t len);
  int (*read_sensor)(skr_ctx *ctx, const char *name, skr_frame *out);
  int (*extension)(skr_ctx *ctx, const char *token, const void *in, size_t in_len, void *out,
                   size_t out_len);
} skr_driver_v1;

static const skr_driver_v1 OLD = {
    .abi_version = 1,
    .driver_name = "old-abi",
    .driver_version = "0.0.1",
    .open = v1_open,
    .close = v1_close,
    .read_state = v1_read_state,
    .write_command = v1_write_command,
    .emergency_stop = v1_emergency_stop,
    .describe = v1_describe,
};

const skr_driver *skynet_rhal_entry(void) { return (const skr_driver *)&OLD; }
