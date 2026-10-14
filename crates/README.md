# Crates

The operating system itself, in Rust. Nothing in here uses an outside library, and the code that must
never allocate memory or fail (the Safety Kernel) is `no_std`.

| Crate | What it is |
|---|---|
| `skynet-kernel` | **The Safety Kernel.** The only writer to a motor. `no_std`, no allocation. |
| `skynet-kernel-host` | The process that runs the control loop (`skynetk`): reads the driver, runs the kernel |
| `skynet-policy-host` | The process that runs robot software and fills the command ring |
| `skynet-ring`, `skynet-ring-shm` | The one channel from robot software to the Safety Kernel |
| `skynet-obs`, `skynet-frames`, `skynet-capture` | What the robot senses, handed to robot software |
| `skynet-blackbox`, `skynet-claim`, `skynet-lock` | The black box: the recording ring, sealing and replay |
| `skynetd`, `skynetd-io` | The supervisor: installs and updates packages, keeps the robot in its desired state |
| `skynet-cli` | The `skynet` command-line tool |
| `skynet-rhal`, `skynet-dl` | The driver interface (RHAL) and the loader that opens a driver |
| `skynet-contracts` | Roles, limits, package admission and readiness: the rules both sides agree on |
| `skynet-urdf` | Turns a URDF robot description into a Skynet OS body description |
| `skynet-infer`, `skynet-ort` | Running AI models (ONNX Runtime, and a small built-in runner) |
| `skynet-json`, `skynet-sha256`, `skynet-ed25519` | Canonical JSON, SHA-256 and signature checking, with no dependencies |
| `skynet-corpus` | Runs the shared test cases that every edition must reproduce |
| `skynet-mdns`, `skynet-rt`, `skynet-statfs`, `skynet-crash` | Finding a robot on the network, real-time checks, free disk space, crash reports |

To build everything: `cargo build --release` from the repository root (Rust 1.82 or newer). The
[developer guide](../docs/developer-guide.md) shows how to run the pieces by hand, and the
[specification](../docs/skynet-os-architecture.md) says what each part must do (the `§` numbers in the
code point into it).
