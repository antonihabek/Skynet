/* The host's `F` verb, end to end: dlopen the host, open a fixture child, read a frame
 * the pipe cannot carry.
 *
 * Run by `make check-frame` with `SKYNET_PYTHON_DRIVER=skynet_big_frame`, whose `big` sensor
 * answers 100,000 bytes — past the 64 KiB line both sides honour — and whose `small` answers
 * 100. The big read must arrive as `EINVAL`, refused before it is framed, with the stream still
 * in sync: a small read and a `read_state` straight after it still work
 * (DRY-COVERAGE-PLAN.md F172).
 *
 * Usage: frame_check <lib> ; the driver module comes from the environment, the way the OS
 * sets it.
 */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "skynet_rhal.h"

static int checks = 0;
static int failures = 0;

static void check(int held, const char *what) {
  checks += 1;
  if (held) {
    printf("  ok    %s\n", what);
  } else {
    failures += 1;
    printf("  FAIL  %s\n", what);
  }
}

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: frame_check <host-lib>\n");
    return 2;
  }
  void *lib = dlopen(argv[1], RTLD_NOW);
  if (lib == NULL) {
    fprintf(stderr, "frame_check: cannot open %s: %s\n", argv[1], dlerror());
    return 2;
  }
  skr_entry_fn entry = (skr_entry_fn)dlsym(lib, "skynet_rhal_entry");
  if (entry == NULL) {
    fprintf(stderr, "frame_check: no skynet_rhal_entry in %s\n", argv[1]);
    return 2;
  }
  const skr_driver *driver = entry();
  check(driver->read_sensor != NULL, "the host forwards read_sensor");

  skr_ctx *ctx = NULL;
  check(driver->open("{}", &ctx) == SKR_OK, "open() starts the child");
  if (ctx == NULL) {
    printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }

  /* 100,000 bytes encode past the 64 KiB line: refused, not half-read. */
  skr_frame big;
  memset(&big, 0, sizeof big);
  check(driver->read_sensor(ctx, "big", &big) == SKR_EINVAL,
        "a frame the pipe cannot carry arrives as EINVAL");

  /* The refusal sent no payload line, so the next exchanges start clean. */
  skr_frame small;
  memset(&small, 0, sizeof small);
  check(driver->read_sensor(ctx, "small", &small) == SKR_OK,
        "a small read straight after it still works");
  check(small.size == 100, "with all of its 100 bytes");

  skr_state state;
  float position[1] = {0.0f}, velocity[1] = {0.0f}, effort[1] = {0.0f};
  memset(&state, 0, sizeof state);
  state.position_rad = position;
  state.velocity_rad_s = velocity;
  state.effort_nm = effort;
  check(driver->read_state(ctx, &state, 0) == SKR_OK,
        "and the body is still served after the refusal");

  char json[4096];
  check(driver->describe(ctx, json, sizeof json) > 0,
        "and describe works, so the stream is still in sync");

  check(driver->close(ctx) == SKR_OK, "close() stops the child");

  printf("\n%d checks, %d failed\n", checks, failures);
  dlclose(lib);
  return failures == 0 ? 0 : 1;
}
