/* The stop path while an extension is running (DRY-COVERAGE-PLAN.md F234), through the host the way the OS drives it.
 *
 * A streamed motion — a velocity held for a few seconds, which is how `drivers/mavlink` and `drivers/ros2_base` move a vehicle or a base — is one `extension` call that does not
 * return until it is over. The host writes `E` to the child's pipe from `emergency_stop`, and the child used to read the next line only when the call returned, so a stop asked for
 * during a motion was acted on at the end of it. A child that says its stop is safe to call from another thread (`stop_during_extension`) now has the stop acted on the moment it
 * is read, and answers in the order it was asked.
 *
 * Run by `make check-stop`, twice: with `SKYNET_FIXTURE_DECLARES=1` (the child's extension must be cut short) and without (it must not: a child that has not said its stop is safe to
 * call concurrently gets what it always got, and the check says so rather than hiding it). Either way the stream must stay in sync afterwards.
 *
 * Usage: stop_check <lib> <declared 0|1>; the driver module and the run length come from the environment, the way the OS sets them.
 */
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

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

static double now_s(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (double)ts.tv_sec + (double)ts.tv_nsec * 1e-9;
}

static void sleep_s(double seconds) {
  struct timespec ts;
  ts.tv_sec = (time_t)seconds;
  ts.tv_nsec = (long)((seconds - (double)ts.tv_sec) * 1e9);
  nanosleep(&ts, NULL);
}

typedef struct {
  const skr_driver *driver;
  skr_ctx *ctx;
  int rc;
  char out[256];
  double returned_at;
} run_t;

static void *run_extension(void *arg) {
  run_t *run = (run_t *)arg;
  memset(run->out, 0, sizeof run->out);
  run->rc = run->driver->extension(run->ctx, "test.run", "x", 1, run->out, sizeof run->out);
  run->returned_at = now_s();
  return NULL;
}

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: stop_check <host-lib> <declared 0|1>\n");
    return 2;
  }
  const int declared = atoi(argv[2]);
  void *lib = dlopen(argv[1], RTLD_NOW);
  if (lib == NULL) {
    fprintf(stderr, "stop_check: cannot open %s: %s\n", argv[1], dlerror());
    return 2;
  }
  skr_entry_fn entry = (skr_entry_fn)dlsym(lib, "skynet_rhal_entry");
  if (entry == NULL) {
    fprintf(stderr, "stop_check: no skynet_rhal_entry in %s\n", argv[1]);
    return 2;
  }
  const skr_driver *driver = entry();
  skr_ctx *ctx = NULL;
  check(driver->open("{}", &ctx) == SKR_OK, "open() starts the child");
  if (ctx == NULL) {
    printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }
  const char *run_env = getenv("SKYNET_FIXTURE_RUN_S");
  const double run_s = run_env != NULL ? atof(run_env) : 1.5;

  /* An extension that is left alone runs for its whole length. */
  run_t alone = {driver, ctx, 0, {0}, 0.0};
  double began = now_s();
  run_extension(&alone);
  check(alone.rc == 3 && strcmp(alone.out, "ran") == 0, "an extension that is not stopped runs to its end and answers (the return is the bytes written)");
  check(alone.returned_at - began >= run_s * 0.9, "and takes as long as it takes");

  /* The same extension, with the host's stop asked for while it runs. */
  run_t stopped = {driver, ctx, 0, {0}, 0.0};
  pthread_t thread;
  began = now_s();
  pthread_create(&thread, NULL, run_extension, &stopped);
  sleep_s(0.4);
  const double asked_at = now_s();
  check(driver->emergency_stop(ctx) == SKR_OK, "the host's emergency_stop returns at once, as it must");
  check(now_s() - asked_at < 0.1, "without waiting for the extension");
  pthread_join(thread, NULL);
  const double reaction = stopped.returned_at - asked_at;
  if (declared) {
    check(stopped.rc == SKR_EPERM && strcmp(stopped.out, "stopped") == 0, "a child that says its stop is safe mid-extension has the extension cut short, and answers it");
    check(reaction < 0.5, "within half a second of the stop being asked for, not at the end of the run");
  } else {
    check(stopped.rc == 3 && strcmp(stopped.out, "ran") == 0, "a child that has not said so runs the extension out: its stop is read when the extension returns");
    check(reaction >= run_s - 0.4 - 0.2, "which is what every child did before, and the check says so rather than hiding it");
  }

  /* The stream is in sync afterwards: the extension's reply was the extension's, the stop's reply was drained, and a read of the state is a read of the state. */
  skr_state state;
  memset(&state, 0, sizeof state);
  check(driver->read_state(ctx, &state, 0) == SKR_OK, "the next exchange is the state read it asked for, not the stop's reply read late");
  char json[1024];
  check(driver->describe(ctx, json, sizeof json) > 0 && strstr(json, "stoppable-fixture") != NULL, "and so is the one after it");
  check(driver->release_stop(ctx) == SKR_OK, "release_stop is answered");
  run_t again = {driver, ctx, 0, {0}, 0.0};
  run_extension(&again);
  check(again.rc == 3 && strcmp(again.out, "ran") == 0, "and an extension asked for after the release runs again");
  check(driver->close(ctx) == SKR_OK, "close() stops the child");

  printf("\n%d checks, %d failed\n", checks, failures);
  dlclose(lib);
  return failures == 0 ? 0 : 1;
}
