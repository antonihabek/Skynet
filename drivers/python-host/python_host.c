/*
 * skynet-python-host — a RHAL driver that runs a Python driver out of process.
 *
 * Python's tier ceiling is Guest (§31.3, §66) and this is how a Python driver reaches the OS at
 * all: one shared object, exporting the one symbol, forwarding every RHAL call to an interpreter it
 * spawned.
 *
 * ── Why a sidecar and not embedded CPython ────────────────────────────────
 *
 * Embedding is the obvious design and it cannot honour §4's second rule:
 *
 *   > emergency_stop() must work from a signal handler and from a process that is dying.
 *
 * Calling into CPython from a signal handler is not safe. The interpreter takes locks and
 * allocates, and a stop path that can deadlock is worse than one that is slow. Writing one line to
 * a pipe is async-signal-safe, so out of process is not a workaround here — it is the only
 * arrangement in which a Python driver can implement the entry point that matters most.
 *
 * Two further properties fall out of it, both of them the right way round:
 *
 *   * A garbage-collection pause stalls the interpreter and not the host. The host owns its own
 *     timing and reports a late answer as SKR_ETIMEDOUT, which is true, rather than absorbing it.
 *   * A Python driver that crashes takes down a child process. The host notices the closed pipe,
 *     reports SKR_EIO, and the OS still has a live stop path.
 *
 * The cost is a pipe round trip per cycle — tens of microseconds. That is unaffordable on a 1 kHz
 * torque loop and irrelevant at Guest tier, which is exactly the tier this driver is capped at.
 *
 * ── Configuration ─────────────────────────────────────────────────────────
 *
 * The RHAL hands open() a JSON string, and this host has no JSON parser — adding one to a C shim
 * whose job is to forward bytes would be the tail wagging the dog. So the split is:
 *
 *   SKYNET_PYTHON         the interpreter to run          (default: python3, or python on Windows)
 *   SKYNET_PYTHON_DRIVER  the module to run with -m       (default: skynet_lerobot)
 *   PYTHONPATH            where that module lives         (the usual meaning)
 *
 * and the config JSON is forwarded to the child verbatim, which is what it is for: it configures
 * the *robot*, and the robot is the child's business.
 *
 * Build: see Makefile, or
 *   cc -std=c11 -O2 -Wall -Wextra -Wpedantic -fPIC -I../../bindings/c \
 *      -shared -o libskynet_python_host.so python_host.c
 * Licence: Apache-2.0.
 */

/* `kill`, `fork`, `pipe` and `waitpid` are POSIX rather than C11, and `-std=c11` hides their
 * declarations unless the program asks for them — before the first system header, or the request
 * arrives too late. GCC 13 declared `kill` implicitly and warned; GCC 14 made an implicit
 * declaration an error, and this file stopped building on a current distribution with nothing in
 * it changed. The Makefile now passes `-Werror=implicit-function-declaration` so an older compiler
 * refuses the same thing rather than guessing a signature. */
#if !defined(_WIN32) && !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../../bindings/c/skynet_rhal.h"

#if defined(_WIN32)
#define SKPH_WINDOWS 1
#include <windows.h>
#else
#define SKPH_POSIX 1
#include <signal.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

#define SKPH_LINE_MAX (1 << 16)
#define SKPH_MAX_JOINTS 256

/* The protocol version the child announces in its handshake. A mismatch refuses, on the same
 * grounds the ABI version does: half a driver is worse than none. */
#define SKPH_PROTOCOL 1

/* One sensor's most recent frame.
 *
 * The header says `data` is driver-owned and valid until the next read of the *same* sensor, so the
 * buffers are per sensor rather than shared — a runtime holding a wrist frame and a head frame at
 * once is doing something the ABI allows. */
typedef struct {
  char name[64];
  unsigned char *bytes;
  size_t capacity;
  size_t size;
} skph_sensor;

#define SKPH_MAX_SENSORS 8

/* A frame this size through a pipe is a design smell rather than a limit anybody should hit. A real
 * camera is read by the runtime directly (§47.2) and never crosses this boundary; what does cross
 * it is a small depth patch, a tactile array, or a test frame. The cap exists so the failure is a
 * named refusal rather than a stall. */
#define SKPH_MAX_FRAME_BYTES (4u << 20)

struct skr_ctx {
  int running;
  int estop_latched;
  /* Replies the stop path asked for and deliberately did not wait to read. Draining them before
   * the next request keeps the stream synchronised without making the stop block — and a stop path
   * that waits on an interpreter is not a stop path. */
  int pending_replies;
  skph_sensor sensors[SKPH_MAX_SENSORS];
  char ceiling[32];
  char child_name[64];
  char child_version[32];

  /* Reusable buffers. Allocated once at open() so the per-cycle path does not reach an allocator —
   * the child does plenty of allocating, and there is no reason for the host to add to it. */
  char *line;      /* SKPH_LINE_MAX */
  char *scratch;   /* SKPH_LINE_MAX, for building command lines */
  char *descriptor;
  size_t descriptor_len;

#if defined(SKPH_WINDOWS)
  HANDLE process;
  HANDLE to_child;   /* we write */
  HANDLE from_child; /* we read */
#else
  pid_t child;
  int to_child;
  int from_child;
#endif
};

static skr_ctx HOST_CONTEXT;

/* ── Platform: spawn, write, read one line, terminate ──────────────────────
 *
 * Four functions, and everything above them is portable. Each is short on purpose: this is the part
 * that can only be tested where it runs, so there is as little of it as the job allows. */

#if defined(SKPH_POSIX)

static int skph_spawn(skr_ctx *ctx, const char *interpreter, const char *module) {
  int in_pipe[2], out_pipe[2];
  if (pipe(in_pipe) != 0) return SKR_EIO;
  if (pipe(out_pipe) != 0) {
    close(in_pipe[0]);
    close(in_pipe[1]);
    return SKR_EIO;
  }

  pid_t pid = fork();
  if (pid < 0) {
    close(in_pipe[0]); close(in_pipe[1]);
    close(out_pipe[0]); close(out_pipe[1]);
    return SKR_EIO;
  }

  if (pid == 0) {
    /* Child. Only async-signal-safe calls between fork and exec. */
    dup2(in_pipe[0], STDIN_FILENO);
    dup2(out_pipe[1], STDOUT_FILENO);
    close(in_pipe[0]); close(in_pipe[1]);
    close(out_pipe[0]); close(out_pipe[1]);
    /* stderr is deliberately left alone: the child's diagnostics go to the OS's stderr, where a
     * person can see them, rather than into a pipe nobody drains. */
    execlp(interpreter, interpreter, "-u", "-m", module, (char *)NULL);
    _exit(127); /* exec failed; the parent sees the pipe close */
  }

  close(in_pipe[0]);
  close(out_pipe[1]);
  ctx->child = pid;
  ctx->to_child = in_pipe[1];
  ctx->from_child = out_pipe[0];
  return SKR_OK;
}

static int skph_write(skr_ctx *ctx, const char *bytes, size_t len) {
  size_t sent = 0;
  while (sent < len) {
    ssize_t n = write(ctx->to_child, bytes + sent, len - sent);
    if (n <= 0) {
      if (n < 0 && errno == EINTR) continue;
      return SKR_EIO;
    }
    sent += (size_t)n;
  }
  return SKR_OK;
}

static int skph_read_line(skr_ctx *ctx, char *out, size_t cap) {
  size_t used = 0;
  while (used + 1 < cap) {
    char c;
    ssize_t n = read(ctx->from_child, &c, 1);
    if (n == 0) return SKR_EIO; /* the child went away */
    if (n < 0) {
      if (errno == EINTR) continue;
      return SKR_EIO;
    }
    if (c == '\n') break;
    if (c != '\r') out[used++] = c;
  }
  out[used] = '\0';
  return SKR_OK;
}

static int skph_read_exact(skr_ctx *ctx, char *out, size_t len) {
  size_t got = 0;
  while (got < len) {
    ssize_t n = read(ctx->from_child, out + got, len - got);
    if (n == 0) return SKR_EIO;
    if (n < 0) {
      if (errno == EINTR) continue;
      return SKR_EIO;
    }
    got += (size_t)n;
  }
  return SKR_OK;
}

static void skph_terminate(skr_ctx *ctx) {
  if (ctx->to_child >= 0) close(ctx->to_child);
  if (ctx->from_child >= 0) close(ctx->from_child);
  ctx->to_child = ctx->from_child = -1;
  if (ctx->child > 0) {
    int status;
    /* The child exits when its stdin closes. Reaping it keeps a fleet service from accumulating
     * zombies over a long uptime, which is the sort of thing nobody notices for six weeks. */
    kill(ctx->child, SIGTERM);
    waitpid(ctx->child, &status, 0);
    ctx->child = -1;
  }
}

static void skph_init_handles(skr_ctx *ctx) {
  ctx->child = -1;
  ctx->to_child = -1;
  ctx->from_child = -1;
}

#else /* SKPH_WINDOWS */

static int skph_spawn(skr_ctx *ctx, const char *interpreter, const char *module) {
  SECURITY_ATTRIBUTES inherit = {sizeof(SECURITY_ATTRIBUTES), NULL, TRUE};
  HANDLE child_stdin_read = NULL, parent_write = NULL;
  HANDLE parent_read = NULL, child_stdout_write = NULL;

  if (!CreatePipe(&child_stdin_read, &parent_write, &inherit, 0)) return SKR_EIO;
  if (!CreatePipe(&parent_read, &child_stdout_write, &inherit, 0)) {
    CloseHandle(child_stdin_read);
    CloseHandle(parent_write);
    return SKR_EIO;
  }
  /* Only the child's ends are inheritable, or the pipes never report end-of-file. */
  SetHandleInformation(parent_write, HANDLE_FLAG_INHERIT, 0);
  SetHandleInformation(parent_read, HANDLE_FLAG_INHERIT, 0);

  char command[1024];
  snprintf(command, sizeof command, "\"%s\" -u -m %s", interpreter, module);

  STARTUPINFOA startup;
  memset(&startup, 0, sizeof startup);
  startup.cb = sizeof startup;
  startup.dwFlags = STARTF_USESTDHANDLES;
  startup.hStdInput = child_stdin_read;
  startup.hStdOutput = child_stdout_write;
  startup.hStdError = GetStdHandle(STD_ERROR_HANDLE);

  PROCESS_INFORMATION process;
  memset(&process, 0, sizeof process);
  BOOL ok = CreateProcessA(NULL, command, NULL, NULL, TRUE, 0, NULL, NULL, &startup, &process);

  CloseHandle(child_stdin_read);
  CloseHandle(child_stdout_write);
  if (!ok) {
    CloseHandle(parent_write);
    CloseHandle(parent_read);
    return SKR_EIO;
  }
  CloseHandle(process.hThread);

  ctx->process = process.hProcess;
  ctx->to_child = parent_write;
  ctx->from_child = parent_read;
  return SKR_OK;
}

static int skph_write(skr_ctx *ctx, const char *bytes, size_t len) {
  DWORD sent = 0;
  while (sent < len) {
    DWORD n = 0;
    if (!WriteFile(ctx->to_child, bytes + sent, (DWORD)(len - sent), &n, NULL) || n == 0) {
      return SKR_EIO;
    }
    sent += n;
  }
  return SKR_OK;
}

static int skph_read_line(skr_ctx *ctx, char *out, size_t cap) {
  size_t used = 0;
  while (used + 1 < cap) {
    char c;
    DWORD n = 0;
    if (!ReadFile(ctx->from_child, &c, 1, &n, NULL) || n == 0) return SKR_EIO;
    if (c == '\n') break;
    if (c != '\r') out[used++] = c;
  }
  out[used] = '\0';
  return SKR_OK;
}

static int skph_read_exact(skr_ctx *ctx, char *out, size_t len) {
  size_t got = 0;
  while (got < len) {
    DWORD n = 0;
    if (!ReadFile(ctx->from_child, out + got, (DWORD)(len - got), &n, NULL) || n == 0) {
      return SKR_EIO;
    }
    got += n;
  }
  return SKR_OK;
}

static void skph_terminate(skr_ctx *ctx) {
  if (ctx->to_child) CloseHandle(ctx->to_child);
  if (ctx->from_child) CloseHandle(ctx->from_child);
  ctx->to_child = ctx->from_child = NULL;
  if (ctx->process) {
    WaitForSingleObject(ctx->process, 2000);
    TerminateProcess(ctx->process, 0);
    CloseHandle(ctx->process);
    ctx->process = NULL;
  }
}

static void skph_init_handles(skr_ctx *ctx) {
  ctx->process = NULL;
  ctx->to_child = NULL;
  ctx->from_child = NULL;
}

#endif

/* ── Portable helpers ──────────────────────────────────────────────────────── */

static int skph_send_line(skr_ctx *ctx, const char *line) {
  size_t len = strlen(line);
  int rc = skph_write(ctx, line, len);
  if (rc != SKR_OK) return rc;
  return skph_write(ctx, "\n", 1);
}

/* Read a reply line, skipping nothing: the protocol has no unsolicited messages, so anything
 * unexpected here is a desynchronised stream and is reported as one. */
/* Read and discard the replies the stop path did not wait for.
 *
 * Every request/response exchange calls this first. Without it the reply to an `E` sent during a
 * cycle arrives where the answer to the *next* request should be, and the whole stream sits one
 * frame out — which presents as a protocol error on an unrelated call, some time later, and is one
 * of the least pleasant classes of bug there is. */
static void skph_drain(skr_ctx *ctx) {
  while (ctx->pending_replies > 0) {
    ctx->pending_replies -= 1;
    if (skph_read_line(ctx, ctx->line, SKPH_LINE_MAX) != SKR_OK) {
      /* The child is gone. The next real exchange reports it; there is nothing left to drain. */
      ctx->pending_replies = 0;
      return;
    }
  }
}

static int skph_expect(skr_ctx *ctx, char verb, char *line, size_t cap) {
  skph_drain(ctx);
  int rc = skph_read_line(ctx, line, cap);
  if (rc != SKR_OK) return rc;
  if (line[0] != verb) return SKR_EPROTO;
  return SKR_OK;
}

/* strtod, but treating a parse failure as NaN rather than as zero.
 *
 * The distinction is the whole reason the three state arrays are separate: NaN means "this joint
 * does not measure it" and zero is a measurement. A malformed field silently becoming 0.0 would
 * report a stationary joint. */
static float skph_number(const char **cursor) {
  char *end = NULL;
  double value = strtod(*cursor, &end);
  if (end == *cursor) {
    *cursor = *cursor + strcspn(*cursor, " ");
    return (float)(0.0 / 0.0);
  }
  *cursor = end;
  return (float)value;
}

/* ── The vtable ────────────────────────────────────────────────────────────── */

static const char *skph_env(const char *name, const char *fallback) {
  const char *value = getenv(name);
  return (value && *value) ? value : fallback;
}

static int skph_open(const char *config_json, skr_ctx **out) {
  if (out == NULL) return SKR_EINVAL;
  skr_ctx *ctx = &HOST_CONTEXT;
  if (ctx->running) return SKR_EPERM;

  memset(ctx, 0, sizeof *ctx);
  skph_init_handles(ctx);

  ctx->line = malloc(SKPH_LINE_MAX);
  ctx->scratch = malloc(SKPH_LINE_MAX);
  if (ctx->line == NULL || ctx->scratch == NULL) {
    free(ctx->line);
    free(ctx->scratch);
    ctx->line = ctx->scratch = NULL;
    return SKR_EIO;
  }

#if defined(SKPH_WINDOWS)
  const char *interpreter = skph_env("SKYNET_PYTHON", "python");
#else
  const char *interpreter = skph_env("SKYNET_PYTHON", "python3");
#endif
  const char *module = skph_env("SKYNET_PYTHON_DRIVER", "skynet_lerobot");

  int rc = skph_spawn(ctx, interpreter, module);
  if (rc != SKR_OK) {
    fprintf(stderr,
            "skynet-python-host: could not start `%s -m %s`. Set SKYNET_PYTHON to the interpreter "
            "and SKYNET_PYTHON_DRIVER to the module, and PYTHONPATH to where it lives.\n",
            interpreter, module);
    goto fail;
  }

  /* The handshake. A child that does not announce itself is not a Skynet Python driver, and
   * discovering that here is far better than discovering it inside the first cycle. */
  rc = skph_expect(ctx, 'H', ctx->line, SKPH_LINE_MAX);
  if (rc != SKR_OK) {
    fprintf(stderr,
            "skynet-python-host: `%s -m %s` did not send a handshake. Its first line on stdout must "
            "be the one skynet_rhal.serve() writes; anything else on stdout is a corrupted frame.\n",
            interpreter, module);
    goto fail;
  }

  unsigned abi = 0, protocol = 0;
  if (sscanf(ctx->line, "H %u %u %31s %63s %31s", &abi, &protocol, ctx->ceiling, ctx->child_name,
             ctx->child_version) != 5) {
    rc = SKR_EPROTO;
    goto fail;
  }
  if (abi != SKYNET_RHAL_ABI_VERSION) {
    fprintf(stderr, "skynet-python-host: the driver targets ABI %u and this host speaks %u.\n", abi,
            SKYNET_RHAL_ABI_VERSION);
    rc = SKR_EPROTO;
    goto fail;
  }
  if (protocol != SKPH_PROTOCOL) {
    fprintf(stderr, "skynet-python-host: the driver speaks protocol %u and this host speaks %u.\n",
            protocol, SKPH_PROTOCOL);
    rc = SKR_EPROTO;
    goto fail;
  }
  /* §31.3's third rule made mechanical: an edition may not raise its own ceiling. A Python driver
   * announcing anything but `guest` is making a claim its runtime cannot support, and the right
   * response is to refuse to load it rather than to quietly believe it. */
  if (strcmp(ctx->ceiling, "guest") != 0) {
    fprintf(stderr,
            "skynet-python-host: the driver announced ceiling `%s`. Python is capped at guest "
            "(§31.3) — the GIL and the garbage collector cannot hold a real-time path — and a "
            "driver claiming otherwise is refused rather than believed.\n",
            ctx->ceiling);
    rc = SKR_EPERM;
    goto fail;
  }

  /* Forward the configuration verbatim. It configures the robot, which is the child's business. */
  const char *config = config_json ? config_json : "";
  size_t config_len = strlen(config);
  snprintf(ctx->scratch, SKPH_LINE_MAX, "O %llu", (unsigned long long)config_len);
  rc = skph_send_line(ctx, ctx->scratch);
  if (rc != SKR_OK) goto fail;
  rc = skph_write(ctx, config, config_len);
  if (rc != SKR_OK) goto fail;
  rc = skph_write(ctx, "\n", 1);
  if (rc != SKR_OK) goto fail;

  rc = skph_expect(ctx, 'R', ctx->line, SKPH_LINE_MAX);
  if (rc != SKR_OK) goto fail;
  int child_rc = SKR_EPROTO;
  if (sscanf(ctx->line, "R %d", &child_rc) != 1) {
    rc = SKR_EPROTO;
    goto fail;
  }
  if (child_rc != SKR_OK) {
    rc = child_rc;
    goto fail;
  }

  ctx->running = 1;
  *out = ctx;
  return SKR_OK;

fail:
  skph_terminate(ctx);
  free(ctx->line);
  free(ctx->scratch);
  free(ctx->descriptor);
  ctx->line = ctx->scratch = ctx->descriptor = NULL;
  return rc;
}

static int skph_close(skr_ctx *ctx) {
  if (ctx == NULL) return SKR_EINVAL;
  if (ctx->running) {
    skph_send_line(ctx, "X");
    skph_read_line(ctx, ctx->line, SKPH_LINE_MAX); /* best effort; the child is exiting */
  }
  skph_terminate(ctx);
  free(ctx->line);
  free(ctx->scratch);
  free(ctx->descriptor);
  ctx->line = ctx->scratch = ctx->descriptor = NULL;
  ctx->descriptor_len = 0;
  for (int i = 0; i < SKPH_MAX_SENSORS; ++i) {
    free(ctx->sensors[i].bytes);
    ctx->sensors[i].bytes = NULL;
    ctx->sensors[i].capacity = ctx->sensors[i].size = 0;
    ctx->sensors[i].name[0] = '\0';
  }
  ctx->running = 0;
  ctx->pending_replies = 0;
  return SKR_OK;
}

static int skph_read_state(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns) {
  if (ctx == NULL || out == NULL) return SKR_EINVAL;
  if (!ctx->running) return SKR_EPERM;

  snprintf(ctx->scratch, SKPH_LINE_MAX, "S %llu", (unsigned long long)deadline_ns);
  if (skph_send_line(ctx, ctx->scratch) != SKR_OK) return SKR_EIO;
  if (skph_expect(ctx, 'S', ctx->line, SKPH_LINE_MAX) != SKR_OK) return SKR_EIO;

  int rc = SKR_EPROTO;
  unsigned long long stamp = 0;
  unsigned seq = 0, flags = 0, fault = 0, count = 0;
  int consumed = 0;
  if (sscanf(ctx->line, "S %d %llu %u %u %u %u%n", &rc, &stamp, &seq, &flags, &fault, &count,
             &consumed) != 6) {
    return SKR_EPROTO;
  }
  if (rc != SKR_OK) return rc;
  if (count > SKPH_MAX_JOINTS) return SKR_EPROTO;

  out->stamp_ns = stamp;
  out->seq = seq;
  out->joint_count = (uint16_t)count;
  out->flags = (uint16_t)(flags | (ctx->estop_latched ? SKR_FLAG_ESTOP : 0));
  out->fault_code = fault;

  const char *cursor = ctx->line + consumed;
  for (unsigned i = 0; i < count; ++i) {
    float value = skph_number(&cursor);
    if (out->position_rad) out->position_rad[i] = value;
  }
  for (unsigned i = 0; i < count; ++i) {
    float value = skph_number(&cursor);
    if (out->velocity_rad_s) out->velocity_rad_s[i] = value;
  }
  for (unsigned i = 0; i < count; ++i) {
    float value = skph_number(&cursor);
    if (out->effort_nm) out->effort_nm[i] = value;
  }

  /* This host forwards joint state and nothing else. A body whose base is localised says so through
   * its own driver; declaring NaN here is the truthful answer rather than an origin nobody
   * measured. */
  for (int i = 0; i < 7; ++i) out->base_pose[i] = (float)(0.0 / 0.0);
  for (int i = 0; i < 6; ++i) out->base_twist[i] = 0.0f;
  return SKR_OK;
}

static int skph_write_command(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns) {
  (void)deadline_ns;
  if (ctx == NULL || cmd == NULL) return SKR_EINVAL;
  if (!ctx->running) return SKR_EPERM;
  if (cmd->values == NULL || cmd->mask == NULL) return SKR_EINVAL;
  if (cmd->joint_count > SKPH_MAX_JOINTS) return SKR_EINVAL;
  /* The latch is enforced here as well as in the child, because the stop path must not depend on a
   * round trip to an interpreter that may be busy. */
  if (ctx->estop_latched) return SKR_EPERM;

  int written = snprintf(ctx->scratch, SKPH_LINE_MAX, "C %llu %u %u %u",
                         (unsigned long long)cmd->stamp_ns, cmd->seq, cmd->mode, cmd->joint_count);
  if (written < 0 || (size_t)written >= SKPH_LINE_MAX) return SKR_EINVAL;
  size_t used = (size_t)written;

  for (uint16_t i = 0; i < cmd->joint_count; ++i) {
    int n = snprintf(ctx->scratch + used, SKPH_LINE_MAX - used, " %.9g", (double)cmd->values[i]);
    if (n < 0 || (size_t)n >= SKPH_LINE_MAX - used) return SKR_EINVAL;
    used += (size_t)n;
  }
  for (uint16_t i = 0; i < cmd->joint_count; ++i) {
    int n = snprintf(ctx->scratch + used, SKPH_LINE_MAX - used, " %u", cmd->mask[i] ? 1u : 0u);
    if (n < 0 || (size_t)n >= SKPH_LINE_MAX - used) return SKR_EINVAL;
    used += (size_t)n;
  }

  if (skph_send_line(ctx, ctx->scratch) != SKR_OK) return SKR_EIO;
  if (skph_expect(ctx, 'R', ctx->line, SKPH_LINE_MAX) != SKR_OK) return SKR_EIO;
  int rc = SKR_EPROTO;
  if (sscanf(ctx->line, "R %d", &rc) != 1) return SKR_EPROTO;
  return rc;
}

/*
 * Latch first, then tell the child.
 *
 * The latch is a single store to a flag the command path already reads, so the stop takes effect
 * whether or not the interpreter ever answers. Sending the line is a best effort on top of it: it
 * is what actually disables torque, and it is allowed to fail without the stop failing.
 *
 * §4 requires this be callable from a signal handler and from a dying process. The store is; the
 * write is, on POSIX, because write(2) is async-signal-safe. Nothing here allocates, takes a lock,
 * or calls into an interpreter.
 */
static int skph_emergency_stop(skr_ctx *ctx) {
  if (ctx == NULL) return SKR_EINVAL;
  ctx->estop_latched = 1;
  if (!ctx->running) return SKR_OK;
  if (skph_write(ctx, "E\n", 2) != SKR_OK) return SKR_OK;
  /* The reply is drained by the next exchange rather than waited for here — see skph_drain.
   * Waiting is what a stop path must never do. */
  ctx->pending_replies += 1;
  return SKR_OK;
}

static int skph_describe(skr_ctx *ctx, char *json_out, size_t len) {
  if (ctx == NULL || json_out == NULL) return SKR_EINVAL;
  if (!ctx->running) return SKR_EPERM;

  if (ctx->descriptor == NULL) {
    if (skph_send_line(ctx, "D") != SKR_OK) return SKR_EIO;
    if (skph_expect(ctx, 'D', ctx->line, SKPH_LINE_MAX) != SKR_OK) return SKR_EIO;
    unsigned long long size = 0;
    if (sscanf(ctx->line, "D %llu", &size) != 1) return SKR_EPROTO;
    if (size == 0 || size > (1u << 20)) return SKR_EPROTO;

    char *buffer = malloc((size_t)size + 1);
    if (buffer == NULL) return SKR_EIO;
    if (skph_read_exact(ctx, buffer, (size_t)size) != SKR_OK) {
      free(buffer);
      return SKR_EIO;
    }
    buffer[size] = '\0';
    skph_read_line(ctx, ctx->line, SKPH_LINE_MAX); /* the newline terminating the block */
    ctx->descriptor = buffer;
    ctx->descriptor_len = (size_t)size;
  }

  /* Too small is an error, never a truncation: half a parsed body description is worse than a
   * failed one, and the OS retries with a larger buffer. */
  if (len < ctx->descriptor_len + 1) return SKR_EINVAL;
  memcpy(json_out, ctx->descriptor, ctx->descriptor_len + 1);
  return (int)ctx->descriptor_len;
}

/* Base64, because the protocol is text and mixing a binary write into a text stream is how a frame
 * ends up interleaved with the line that describes it. The 33% it costs is paid on a path that
 * should not be carrying a real camera anyway. */
static int skph_b64_value(char c) {
  if (c >= 'A' && c <= 'Z') return c - 'A';
  if (c >= 'a' && c <= 'z') return c - 'a' + 26;
  if (c >= '0' && c <= '9') return c - '0' + 52;
  if (c == '+') return 62;
  if (c == '/') return 63;
  return -1;
}

static int skph_b64_decode(const char *text, unsigned char *out, size_t cap, size_t *written) {
  size_t produced = 0;
  int quantum[4];
  int held = 0;
  for (const char *p = text; *p; ++p) {
    if (*p == '=' || *p == '\r' || *p == '\n' || *p == ' ') continue;
    int value = skph_b64_value(*p);
    if (value < 0) return SKR_EPROTO;
    quantum[held++] = value;
    if (held == 4) {
      if (produced + 3 > cap) return SKR_EPROTO;
      out[produced++] = (unsigned char)((quantum[0] << 2) | (quantum[1] >> 4));
      out[produced++] = (unsigned char)(((quantum[1] & 0xF) << 4) | (quantum[2] >> 2));
      out[produced++] = (unsigned char)(((quantum[2] & 0x3) << 6) | quantum[3]);
      held = 0;
    }
  }
  /* A trailing partial group. Two characters carry one byte, three carry two. */
  if (held == 2) {
    if (produced + 1 > cap) return SKR_EPROTO;
    out[produced++] = (unsigned char)((quantum[0] << 2) | (quantum[1] >> 4));
  } else if (held == 3) {
    if (produced + 2 > cap) return SKR_EPROTO;
    out[produced++] = (unsigned char)((quantum[0] << 2) | (quantum[1] >> 4));
    out[produced++] = (unsigned char)(((quantum[1] & 0xF) << 4) | (quantum[2] >> 2));
  } else if (held != 0) {
    return SKR_EPROTO;
  }
  *written = produced;
  return SKR_OK;
}

static skph_sensor *skph_sensor_slot(skr_ctx *ctx, const char *name) {
  for (int i = 0; i < SKPH_MAX_SENSORS; ++i) {
    if (ctx->sensors[i].name[0] != '\0' && strcmp(ctx->sensors[i].name, name) == 0) {
      return &ctx->sensors[i];
    }
  }
  for (int i = 0; i < SKPH_MAX_SENSORS; ++i) {
    if (ctx->sensors[i].name[0] == '\0') {
      snprintf(ctx->sensors[i].name, sizeof ctx->sensors[i].name, "%s", name);
      return &ctx->sensors[i];
    }
  }
  return NULL;
}

/*
 * A sensor frame, forwarded from the child.
 *
 * §67.1 routes the camera entries of `get_observation()` here, so a Python driver that declares
 * sensors must be able to deliver them: a declared stream that cannot be read binds a policy's
 * observation to nothing, and `skynet driver check` fails a driver whose descriptor and vtable
 * disagree about this — correctly, and it caught exactly that in this file's first draft.
 *
 * The honest caveat is in the size cap. A 640x480 RGB frame is 900 KB per cycle through a pipe;
 * that is not what this path is for, and the answer for a real camera is the runtime reading it
 * directly (§47.2) rather than a bigger buffer here.
 */
static int skph_read_sensor(skr_ctx *ctx, const char *name, skr_frame *out) {
  if (ctx == NULL || name == NULL || out == NULL) return SKR_EINVAL;
  if (!ctx->running) return SKR_EPERM;
  if (strlen(name) >= 64 || strpbrk(name, " \t\r\n") != NULL) return SKR_EINVAL;

  snprintf(ctx->scratch, SKPH_LINE_MAX, "F %s", name);
  if (skph_send_line(ctx, ctx->scratch) != SKR_OK) return SKR_EIO;
  if (skph_expect(ctx, 'F', ctx->line, SKPH_LINE_MAX) != SKR_OK) return SKR_EIO;

  int rc = SKR_EPROTO;
  unsigned long long stamp = 0, size = 0;
  unsigned width = 0, height = 0, stride = 0, format = 0;
  if (sscanf(ctx->line, "F %d %llu %u %u %u %u %llu", &rc, &stamp, &width, &height, &stride,
             &format, &size) != 7) {
    return SKR_EPROTO;
  }
  if (rc != SKR_OK) return rc;
  if (size > SKPH_MAX_FRAME_BYTES) {
    fprintf(stderr,
            "skynet-python-host: sensor `%s` produced a %llu-byte frame and the cap is %u. A real "
            "camera belongs on the runtime's own capture path (§47.2), not on this pipe.\n",
            name, size, SKPH_MAX_FRAME_BYTES);
    return SKR_EIO;
  }

  /* The base64 block, on its own line. */
  if (skph_read_line(ctx, ctx->line, SKPH_LINE_MAX) != SKR_OK) return SKR_EIO;

  skph_sensor *slot = skph_sensor_slot(ctx, name);
  if (slot == NULL) return SKR_ENODEV;
  if (slot->capacity < (size_t)size) {
    unsigned char *grown = realloc(slot->bytes, (size_t)size ? (size_t)size : 1);
    if (grown == NULL) return SKR_EIO;
    slot->bytes = grown;
    slot->capacity = (size_t)size ? (size_t)size : 1;
  }
  size_t decoded = 0;
  if (skph_b64_decode(ctx->line, slot->bytes, slot->capacity, &decoded) != SKR_OK) {
    return SKR_EPROTO;
  }
  if (decoded != (size_t)size) return SKR_EPROTO;
  slot->size = decoded;

  out->stamp_ns = stamp;
  out->width = width;
  out->height = height;
  out->stride = stride;
  out->format = format;
  out->data = slot->bytes;
  out->size = slot->size;
  return SKR_OK;
}

/* `extension` stays NULL rather than a stub that returns success. The OS reports the capability as
 * absent, which is true, instead of calling through to something that lies. */
static const skr_driver DRIVER = {
    SKYNET_RHAL_ABI_VERSION,
    "skynet-python-host",
    "0.1.0",
    skph_open,
    skph_close,
    skph_read_state,
    skph_write_command,
    skph_emergency_stop,
    skph_describe,
    skph_read_sensor,
    NULL,
};

const skr_driver *skynet_rhal_entry(void) { return &DRIVER; }
