# Contributing to Skynet OS

Thanks for looking. This is an operating system that sits between software and machines that can
hurt people, so the bar for a change is less "does it work" and more "does it say exactly what it
does, and fail loudly when it doesn't". Everything below follows from that.

## Setting up

You need **Rust stable** (1.82 or newer), **a C compiler** and **make**. That is all the reference
edition needs — the Rust workspace has no third-party dependencies, so the first build needs no
network and there is nothing to audit.

```bash
git clone https://github.com/antonihabek/Skynet.git
cd Skynet
cargo test --workspace          # the whole reference edition
make -C drivers/mock check      # the mock robot's own fault-injection suite
```

To see the whole system run — installer, systemd units, Safety Kernel, supervisor — on a Linux
machine with no robot attached:

```bash
sudo ./packaging/virtual-robot.sh up
```

That needs Linux with systemd as PID 1 (a normal distribution, WSL2 with systemd enabled, or a VM).
It says so and stops if it isn't.

Other toolchains are only needed for the part you touch: Node 24 for `tools/` and the corpus
generator, and Python 3, Go, a JDK 21, Zig or .NET 8 for their editions under
[`editions/`](editions/).

## Before you open a pull request

CI runs these, and all of them block. Running them first saves a round trip:

```bash
cargo fmt --all --check
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets
cargo test --workspace
node tools/check-mirrors.mjs
```

If you changed an edition, run its line from [`editions/README.md`](editions/README.md).

**Those four are not everything CI runs.** The `invariants` job is about fifty more checks — claims
this repository makes about itself, each mechanical — and none of them is in the list above. Twice
now a tree has passed that list and failed CI, once on a lint released after the local toolchain and
once on four checks that had been red for up to thirteen days behind another red one. So there is a
runner for the rest:

```bash
node tools/run-ci-job.mjs invariants          # every step, reported one line each
node tools/run-ci-job.mjs invariants --wsl    # on Windows, shell steps through WSL
node tools/run-ci-job.mjs invariants --list   # what it would run
```

It reads the steps out of `.github/workflows/ci.yml`, so there is nothing to keep in sync, and it
runs every step whatever the one before it did. What it cannot reproduce is the runner — a toolchain,
a `systemd` or a locale your machine does not have is exactly how the two disagreements above
happened, so a green run here is the checks passing and not a promise that CI is green.

## What a good change looks like here

**A bug fix comes with the test that would have caught it.** Where you can, show the test failing
without the fix before it passes with it — a check that has never failed has proved nothing.

**Errors say what happened and what to do next.** Every refusal in this codebase names the cause
and the next action, often with the section of the specification behind it. `--lan  is not an
address` is a bug report waiting to happen; *"--lan was given an empty value — omit it for no
console"* is not. Match that.

**Fail loudly, not quietly.** A required argument beats an optional one with a permissive default.
An error beats a log line. A log line beats nothing. The worst class of bug in a robot is the one
that looks like success.

**Cite the specification.** Code, comments and messages reference
[`docs/skynet-os-architecture.md`](docs/skynet-os-architecture.md) by section (`§33.1`). If your
change implements or depends on a rule, say which.

**One change, one concern.** A refactor and a behaviour change in the same pull request hide each
other from review.

## Rules that are enforced, not requested

These are CI checks. They exist because each one was broken once.

* **No third-party Rust dependencies.** The kernel is `no_std` without `alloc`, and a vendor
  building a driver years from now should not need a package registry to have survived. This is why
  §23.3's ONNX Runtime session loads the library at run time (`crates/skynet-ort`) instead of
  binding to it: the rule stays a check rather than being narrowed until it admits a transitive
  tree. See `DRY-COVERAGE-PLAN.md` *Decisions* 12 for what that trade cost.
* **`unsafe` is forbidden** everywhere except the ABI, the two loaders and the syscall crates. Each
  allow-listed crate states its own argument in its header, and CI checks the argument is still
  there — a crate on that list whose header stops mentioning `unsafe` fails.
* **The Safety Kernel links no allocator.**
* **`SAFETY_CLAIM_RULE` is never shortened or paraphrased.** It is a constant in
  `crates/skynet-kernel`, and every surface that mentions the certified tier renders it in full.
* **The specification matches its pinned digest.** It is shared with the Hub's repository and is
  edited in both or neither (see [`docs/README.md`](docs/README.md)). If you think the
  specification is wrong, open an issue rather than a pull request against it — a maintainer
  carries the change to both copies.
* **Every edition reproduces the corpus byte-for-byte.** Changing behaviour means regenerating the
  corpus from the oracle, and the diff in `corpus/` is what gets reviewed.

## Writing a driver

Drivers are the most useful contribution there is: Skynet OS runs on exactly the robots somebody
has written a conformant driver for. Start from the scaffold rather than a blank file:

```bash
cargo install --path crates/skynet-cli --locked    # puts `skynet` on your PATH
skynet driver new my-arm --lang c                  # anywhere — it does not need this repository
cd my-arm && make
skynet driver check ./libmy_arm.so
```

`driver check` runs the simulated half of the conformance suite and tells you the tier your
evidence supports. The golden path in [`docs/developer-guide.md`](docs/developer-guide.md) walks
the rest.

## Reporting a problem

* **A security or safety issue** — something that lets a command bypass the Safety Kernel, a
  credential or signature check that can be skipped, a sandbox escape: **do not open a public
  issue.** See [`SECURITY.md`](SECURITY.md).
* **A bug** — use the bug template, and attach the output of `skynet doctor` if a robot is
  involved. It collects everything the question "what is wrong with this robot" needs into one
  file.

## Licence

Skynet OS is Apache-2.0. By submitting a contribution you agree it is licensed under the same terms
(section 5 of the [licence](LICENSE)). The Skynet name and the certification programme are not
covered by the licence — implement the interface freely.
