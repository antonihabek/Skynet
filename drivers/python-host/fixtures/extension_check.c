/* The host's `T` verb, end to end: dlopen the host, open a fixture child, call `extension`.
 *
 * Run by `make check-extension`, twice — once with `SKYNET_PYTHON_DRIVER=skynet_t_extension`
 * (a child that implements it) and once with `skynet_v2_child` (a child from before it, whose
 * bare `R EPROTO` must read as ENOSYS with the stream still in sync).
 *
 * Usage: extension_check <lib> ; the driver module comes from the environment, the way the OS
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
    fprintf(stderr, "usage: extension_check <host-lib>\n");
    return 2;
  }
  void *lib = dlopen(argv[1], RTLD_NOW);
  if (lib == NULL) {
    fprintf(stderr, "extension_check: cannot open %s: %s\n", argv[1], dlerror());
    return 2;
  }
  skr_entry_fn entry = (skr_entry_fn)dlsym(lib, "skynet_rhal_entry");
  if (entry == NULL) {
    fprintf(stderr, "extension_check: no skynet_rhal_entry in %s\n", argv[1]);
    return 2;
  }
  const skr_driver *driver = entry();
  check(driver->extension != NULL, "the host forwards extension");

  const char *module = getenv("SKYNET_PYTHON_DRIVER");
  const int is_v2 = module != NULL && strstr(module, "v2_child") != NULL;

  skr_ctx *ctx = NULL;
  check(driver->open("{}", &ctx) == SKR_OK, "open() starts the child");
  if (ctx == NULL) {
    printf("\n%d checks, %d failed\n", checks, failures + 1);
    return 1;
  }

  char out[4096];
  memset(out, 0, sizeof out);
  if (!is_v2) {
    /* Binary-hostile bytes: a newline and a NUL, which a line protocol would choke on raw. */
    const char request[] = {'a', '\n', 'b', '\0', 'c'};
    int rc = driver->extension(ctx, "test.echo", request, sizeof request, out, sizeof out);
    check(rc == (int)(sizeof "echo:" - 1 + sizeof request),
          "echo round-trips bytes, and the return is the bytes written");
    check(rc == 10 && memcmp(out, "echo:a\nb", 8) == 0 && out[8] == '\0' && out[9] == 'c',
          "including a newline and a NUL in the middle");

    memset(out, 0, sizeof out);
    rc = driver->extension(ctx, "test.unknown", "x", 1, out, sizeof out);
    check(rc == SKR_ENOSYS, "an unknown token answers ENOSYS");
    check(strstr(out, "unknown token") != NULL, "with its words beside the code");

    memset(out, 0, sizeof out);
    rc = driver->extension(ctx, "test.echo", "boom", 4, out, sizeof out);
    check(rc == SKR_EIO, "a driver that raises arrives as in-frame EIO");

    char tiny[4];
    rc = driver->extension(ctx, "test.echo", "long-request-bytes", 18, tiny, sizeof tiny);
    check(rc == SKR_EINVAL, "a buffer too small for the answer is EINVAL, never a truncation");

    /* The stream must still be in sync after all of that: a describe works. */
    char json[4096];
    check(driver->describe(ctx, json, sizeof json) > 0,
          "and the stream is still in sync afterwards");
  } else {
    int rc = driver->extension(ctx, "test.echo", "x", 1, out, sizeof out);
    check(rc == SKR_ENOSYS, "a pre-T child reads as ENOSYS, not a protocol error");
    char json[4096];
    check(driver->describe(ctx, json, sizeof json) > 0,
          "and the stream is still in sync — no second line was read that was never sent");
  }

  /* Refusals that never reach the child. */
  check(driver->extension(ctx, "has space", "x", 1, out, sizeof out) == SKR_EINVAL,
        "a token carrying whitespace is refused rather than framed");
  check(driver->extension(ctx, "", "x", 1, out, sizeof out) == SKR_EINVAL,
        "an empty token is refused");
  check(driver->extension(NULL, "test.echo", "x", 1, out, sizeof out) == SKR_EINVAL,
        "a NULL context is refused");
  check(driver->close(ctx) == SKR_OK, "close() stops the child");
  check(driver->extension(ctx, "test.echo", "x", 1, out, sizeof out) == SKR_EPERM,
        "extension on a closed context is refused");

  printf("\n%d checks, %d failed\n", checks, failures);
  dlclose(lib);
  return failures == 0 ? 0 : 1;
}
