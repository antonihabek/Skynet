/*
 * skynet_rhal.h — the Skynet Robot Hardware Abstraction Layer.
 *
 * This header is the entire integration surface between Skynet OS and a robot. A vendor
 * implements it once, in whatever language they already use, and every policy in the Skynet
 * catalogue becomes runnable on their machine.
 *
 * ── Why this is C, when Skynet is written in Rust ──────────────────────────
 *
 * Because a stable ABI is a compatibility requirement, not a language preference. Rust's ABI is
 * unstable by design and C++'s is compiler- and flag-dependent; a driver compiled in 2026 must
 * link against an OS built in 2036. C is the only ABI every language can both produce and
 * consume, so it is the boundary — and only the boundary. Nothing above it is written in C.
 *
 * Wrappers ship for C++ (bindings/cpp/skynet_rhal.hpp), Rust (the `skynet-rhal` crate) and
 * Python (ctypes, for simulator and bench drivers where the RT path does not matter). They are
 * conveniences over these symbols; none of them is a second implementation.
 *
 * ── Three rules a conformant driver obeys ─────────────────────────────────
 *
 * 1. read_state() and write_command() are wait-free and allocate nothing. They run inside the
 *    real-time path. A driver taking a mutex held by a non-RT thread causes a priority inversion
 *    at 1 kHz; SCTS `rhal.state.rate` measures the worst-case gap precisely to catch it.
 * 2. emergency_stop() must work from a signal handler and from a process that is dying. A driver
 *    implementing it as "publish a message and await an ack" fails the test that matters most.
 * 3. describe() is the source of truth for the body. The embodiment descriptor comes out of the
 *    driver, so it cannot drift from the hardware it describes without the driver being wrong.
 *
 * Licence: Apache-2.0. Implement it freely; the trademark and the certification programme are
 * what SCTS gates, not the interface.
 */

#ifndef SKYNET_RHAL_H
#define SKYNET_RHAL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Bumped only for a breaking change. The OS refuses a driver whose major does not match, rather
 * than attempting a partial load — half a hardware driver is worse than none. */
#define SKYNET_RHAL_ABI_VERSION 1

/* ── Return codes ──────────────────────────────────────────────────────────
 *
 * Negative errno-style. The distinction between them is recorded in the black box, because
 * "the drive is faulted" and "the bus timed out" lead to completely different investigations. */
#define SKR_OK             0
#define SKR_EPERM         -1   /* drive disabled, or the command is not permitted in this mode */
#define SKR_EIO           -5   /* bus or transport failure */
#define SKR_ENODEV        -19  /* the named joint, sensor or device does not exist */
#define SKR_EINVAL        -22  /* malformed request — a wrong joint_count, an unknown mode */
#define SKR_ENOSYS        -38  /* the driver does not implement this optional entry point */
#define SKR_ETIMEDOUT    -110  /* the deadline passed before the operation completed */
#define SKR_EPROTO       -71   /* the device answered, and the answer was not valid */

/* ── State flags ─────────────────────────────────────────────────────────── */
#define SKR_FLAG_ESTOP     (1u << 0)  /* an emergency stop is asserted */
#define SKR_FLAG_FAULT     (1u << 1)  /* the drive has faulted; commands are being ignored */
#define SKR_FLAG_DEGRADED  (1u << 2)  /* running, but something is wrong — thermal, a dead encoder */
#define SKR_FLAG_BRAKED    (1u << 3)  /* holding brakes are engaged */

/* ── Control modes ───────────────────────────────────────────────────────── */
#define SKR_MODE_POSITION  0   /* values are radians (or metres, for a prismatic joint) */
#define SKR_MODE_VELOCITY  1   /* values are rad/s (or m/s) */
#define SKR_MODE_TORQUE    2   /* values are Nm (or N) */

typedef struct skr_ctx skr_ctx;

/*
 * One sample of the robot's state.
 *
 * Every array is caller-allocated and reused across cycles, so the hot path never reaches an
 * allocator. The driver fills; it never frees, resizes or retains a pointer past the call.
 */
typedef struct {
  /* CLOCK_MONOTONIC_RAW at ACQUISITION, never at return. A timestamp applied on return folds the
   * driver's own latency into the measurement and silently corrupts every downstream budget. */
  uint64_t stamp_ns;
  uint32_t seq;
  uint16_t joint_count;
  uint16_t flags;             /* SKR_FLAG_* */
  float   *position_rad;      /* [joint_count] */
  float   *velocity_rad_s;    /* [joint_count]; NaN where the joint does not measure it */
  float   *effort_nm;         /* [joint_count]; NaN where unmeasured */
  float    base_pose[7];      /* x y z qx qy qz qw; all NaN when the base is not localised */
  float    base_twist[6];     /* vx vy vz wx wy wz */
  uint32_t fault_code;        /* vendor-specific; surfaced verbatim to the black box */
} skr_state;

/*
 * One command.
 *
 * `mask` is the field worth reading twice. A zero means *hold this joint*, and it is how the OS
 * says nothing rather than saying zero — on a torque-controlled joint that is the difference
 * between holding position and going limp. It is also where a `drop` binding from the transport
 * plan meets the wire: a channel this body has no equivalent for becomes a masked joint rather
 * than a fabricated value.
 */
typedef struct {
  uint64_t       stamp_ns;    /* when the command was produced, not when it was sent */
  uint32_t       seq;         /* MUST advance; a repeat is treated as a stalled producer */
  uint8_t        mode;        /* SKR_MODE_* */
  uint8_t        _pad[3];
  uint16_t       joint_count;
  const float   *values;      /* [joint_count]; units follow `mode` */
  const uint8_t *mask;        /* [joint_count]; 0 = hold, non-zero = apply */
} skr_command;

/* One sensor frame. `data` is driver-owned and valid until the next read of the same sensor. */
typedef struct {
  uint64_t    stamp_ns;       /* acquisition, as above */
  uint32_t    width;
  uint32_t    height;
  uint32_t    stride;
  uint32_t    format;         /* SKR_FMT_* */
  const void *data;
  size_t      size;
} skr_frame;

#define SKR_FMT_RGB8      0
#define SKR_FMT_BGR8      1
#define SKR_FMT_DEPTH16   2
#define SKR_FMT_DEPTH32F  3
#define SKR_FMT_JPEG      4
#define SKR_FMT_POINTS32F 5

/*
 * The driver vtable.
 *
 * A driver exports one symbol, `skynet_rhal_entry`, returning a pointer to a statically allocated
 * instance of this struct. Optional entry points may be NULL; the OS reports the capability as
 * absent rather than calling through a null pointer.
 */
typedef struct {
  uint32_t abi_version;       /* must equal SKYNET_RHAL_ABI_VERSION */
  const char *driver_name;
  const char *driver_version;

  int (*open)(const char *config_json, skr_ctx **out);
  int (*close)(skr_ctx *ctx);

  /* Real-time path. Wait-free, no allocation, must respect `deadline_ns`. */
  int (*read_state)(skr_ctx *ctx, skr_state *out, uint64_t deadline_ns);
  int (*write_command)(skr_ctx *ctx, const skr_command *cmd, uint64_t deadline_ns);

  /* Must be callable from a signal handler and from a failing process. Never blocks. */
  int (*emergency_stop)(skr_ctx *ctx);

  /* Writes the embodiment descriptor as JSON. Returns bytes written, or SKR_EINVAL if `len`
   * is too small — the OS then retries with a larger buffer rather than truncating a body
   * description, because a half-parsed descriptor is worse than a failed one. */
  int (*describe)(skr_ctx *ctx, char *json_out, size_t len);

  /* Optional. NULL when the robot exposes no sensors through the RHAL — common when cameras are
   * read directly by the runtime from V4L2 or GenICam. */
  int (*read_sensor)(skr_ctx *ctx, const char *name, skr_frame *out);

  /* Optional. The vendor escape hatch, guarded by a capability token so a policy using a
   * manufacturer feature declares the dependency rather than reaching for it. */
  int (*extension)(skr_ctx *ctx, const char *token,
                   const void *in, size_t in_len, void *out, size_t out_len);
} skr_driver;

/* The one symbol a driver shared object must export. */
typedef const skr_driver *(*skr_entry_fn)(void);
const skr_driver *skynet_rhal_entry(void);

#ifdef __cplusplus
}  /* extern "C" */
#endif

#endif /* SKYNET_RHAL_H */
