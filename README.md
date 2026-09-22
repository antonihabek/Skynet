# Skynet OS

**An open-source operating system for robots, built around one safety rule: robot software never
moves a motor by itself. A safety layer checks every command first.**

[![ci](https://github.com/antonihabek/Skynet/actions/workflows/ci.yml/badge.svg)](https://github.com/antonihabek/Skynet/actions/workflows/ci.yml)
[![package](https://github.com/antonihabek/Skynet/actions/workflows/package.yml/badge.svg)](https://github.com/antonihabek/Skynet/actions/workflows/package.yml)
[![licence: Apache-2.0](https://img.shields.io/badge/licence-Apache--2.0-blue.svg)](LICENSE)

> **This is an early project (version 0.1).** You can run a simulated robot on your own computer
> today. It is **not** ready to control a real robot yet — see [What works today](#what-works-today).

---

## What is it?

Robots from different companies run completely different software. A program written for one robot
arm usually cannot run on another one, and there is no common way to install robot software, update
it, or check that it is safe to run.

Skynet OS tries to fix that. It sits between robot software and the robot's hardware, and gives
every robot the same:

- **way to install and update software**, in packages — a bit like apps on a phone;
- **safety checks**, done by the **Safety Kernel**, which looks at every command before it can reach
  a motor;
- **black box**, which records what the robot was told to do and what it did, like a plane's flight
  recorder;
- **command-line tool**, `skynet`, for talking to a robot and for building drivers.

To support a new robot, someone writes a **driver**: a small library that tells Skynet OS how to
read that robot's sensors and send commands to its motors. It works like a printer driver, which
lets any program print on that printer. Once a robot has a driver, everything else in Skynet OS
works on it.

"Universal" does **not** mean one program that runs on every robot. What stays the same across
robots is the set of rules — the same packages, the same tools and the same tests — while each
robot gets its own driver.

Skynet OS is one half of a bigger project. The other half, **Skynet Hub**, is where robot software is
shared, reviewed and signed. The Hub lives in a separate repository, and you do not need it to try
Skynet OS.

---

## How it keeps a robot safe

The whole design rests on one rule:

> **Only the Safety Kernel is allowed to send commands to a robot's motors.**

Robot software (called a **policy**, and often an AI model) never controls a motor directly. It
writes down the command it *wants*, and the Safety Kernel decides what actually reaches the motors.
Many times a second, it runs every command through eight safety checks, called **guards**. For
example, they check that:

- the robot's software is still sending fresh commands (if it goes quiet, the robot is brought to a
  controlled stop);
- no joint moves too far, too fast or too hard for its limits;
- the arm stays inside the space it is allowed to be in, and does not hit itself;
- the robot is not moving too fast for how close the nearest person is.

Why so strict? Many AI models shared online are stored in a file format that can run hidden code
the moment it is opened. If downloaded software could move motors directly, one bad file could turn
a real machine into a dangerous one. With the Safety Kernel in the middle, even software nobody
trusts can only *ask*.

```
   Robot software  (downloaded, not trusted)
          │   "move the elbow to 45°"
          ▼
  ┌──────────────────────────────────┐
  │          SAFETY KERNEL           │   checks every command, many times a second
  │     8 safety checks · no AI      │   the only part allowed to move a motor
  └──────────────────────────────────┘
          │   allowed, limited, or stopped
          ▼
   Driver  ──►  Robot hardware
```

---

## Three levels: Guest, Host and Certified

Not every robot lets outside software control its motors. So Skynet OS connects to a robot at one of
three levels, called **tiers**, and always tells you honestly which one it is at.

| | **Guest** | **Host** | **Certified** |
|---|---|---|---|
| Where it runs | In a container, next to the robot maker's own software | On the robot's computer, on real-time Linux | Like Host, plus a separate safety chip |
| Controls the motors | No | Yes | Yes |
| Safety Kernel | **Only watches** — spots and records problems, but cannot block them | **Enforces** — blocks unsafe commands | **Enforces, with a backup** on independent hardware |
| Can stop the robot even if the main computer fails | No | No | Yes |
| Can be used as evidence in an official safety review | No | No | Yes |
| Effort to set up | An afternoon | A porting project for the robot maker | That, plus extra hardware |

At the **Guest** level, Skynet OS is a passenger. It checks every command, records every problem and
raises every alarm, but the command still reaches the robot unchanged, because Skynet OS is not the
one in control. That is useful, but it is **not** a safety system, and Skynet OS says so everywhere.
The simulated robot you can try below always runs at Guest level.

**No software can make a robot "certified safe" on its own.** Skynet OS keeps this sentence in its
code, and repeats it word for word wherever the Certified level is mentioned:

```
SAFETY_CLAIM_RULE:
  Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity
  assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified
  body against a specific robot in a specific application.
```

In plain words: an independent, officially approved organisation has to check each real robot doing
its real job. Skynet OS can only give that organisation evidence to look at.

---

## Try it without a robot

You can turn a Linux computer into a **virtual robot**: the real Skynet OS programs and the real
Safety Kernel, with a simulated robot arm instead of hardware. Nothing physical moves.

**You need Linux with systemd** — that is most normal Linux installs, such as Ubuntu. (systemd is
the program that starts and looks after background services.) On Windows, use
[WSL2 with systemd turned on](https://learn.microsoft.com/windows/wsl/systemd). On a Mac, use a
Linux virtual machine.

### Option 1: download a release (quickest)

For 64-bit Linux on Intel/AMD (x86-64) or ARM (aarch64). Nothing to build.

```bash
curl -fsSL https://raw.githubusercontent.com/antonihabek/Skynet/main/packaging/get.sh | sh
sudo ~/.local/share/skynet-os/current/packaging/virtual-robot.sh up
```

The first line downloads Skynet OS, checks that the download is not damaged by comparing its
checksum, and unpacks it into your home folder. It never uses `sudo`, and you can
[read the script](packaging/get.sh) before running it. The second line installs and starts the
virtual robot, and needs `sudo` because it sets up system services.

### Option 2: build it from the source code

You need [Rust](https://rustup.rs) (1.82 or newer), a C compiler and `make`. Nothing else gets
downloaded, because the Rust code does not use any outside libraries.

```bash
git clone https://github.com/antonihabek/Skynet.git && cd Skynet
sudo ./packaging/virtual-robot.sh up
```

It builds as your normal user and installs as root.

### What you will see

When it finishes, the script asks the running robot questions about itself and checks every answer:

```
  What the robot says about itself
  ✓ the local socket answers (§33.7)
  ✓ status reports a running supervisor
  ✓ the control loop is up
  ...
  ✓ the black box has a window

  The tier this evidence supports
  ✓ guest — §35.3's cap, because the body declares simulator: true

  This machine is now a robot
```

If a line says `FAIL` instead, the script tells you what to read next.

### Talk to your robot

```bash
sudo skynet status                           # which body it has, what it runs, whether stop works
sudo skynet state --token <admin-token>      # what state the robot is in, and why
```

`status` and the emergency stop, `skynet estop`, never need a password, on purpose: anyone near a
robot must always be able to check it and stop it. Almost everything else needs the **admin token**, a password the installer
prints **once** — so copy it somewhere safe. Pass it with `--token`, or set it in the `SKYNET_TOKEN`
environment variable.

### Remove it

```bash
sudo ./packaging/virtual-robot.sh down
```

This stops and removes the virtual robot. It keeps the `/data` folder, where the robot's black box
recordings are. If you used Option 1, the script is in `~/.local/share/skynet-os/current/packaging/`.

### On a real robot

A physical robot is installed from this PC over SSH — nothing needs to be on the robot
first. Host and driver are the whole of the input:

```bash
skynet setup robot.local --robot unitree
```

The wizard derives the body, fetches the release when none is here, streams every step,
and offers to start the units and run the first check. The full procedure is step 8 of
the [developer guide](docs/developer-guide.md).

---

## What works today

Skynet OS is at version 0.1. Here is the honest short version; [docs/status.md](docs/status.md) has
the full one, part by part.

**Built and tested**

- The Safety Kernel and all eight of its guards.
- The control loop (`skynetk`), the supervisor (`skynetd`) and the `skynet` command-line tool.
- The black box, including playing a recording back through the Safety Kernel again.
- Installing packages, checking their digital signatures, and rolling back.
- A mock robot, a driver for the [MuJoCo](https://mujoco.org) physics simulator, and a bridge to
  robots supported by [LeRobot](https://github.com/huggingface/lerobot) (so far tested only against
  a simulated stand-in).
- Tools that turn a URDF robot description into a Skynet OS body description, and that create the
  starting code for a new driver in C or Rust.
- The core rules written again in Python, C++, Go, C, Zig, Java and C#, all checked against the same
  shared tests. Some are complete and some are still in progress — see
  [editions/README.md](editions/README.md).

**Not built yet**

- The part that runs AI models on the robot. For now there is only a test program (`skynetp`) that
  sends simple, scripted commands.
- Drivers for EtherCAT and for robot makers' own software kits. Drivers for `ros2_control`, CANopen
  and Universal Robots *are* written — see [docs/status.md](docs/status.md) — but none of them has
  driven a physical machine: they are tested against a fake, a simulated bus and UR's own controller
  software, which shows the protocol is right and says nothing about a robot.
- The separate safety chip the Certified level needs.
- A test of software updates on real robot hardware. The update code is written and tested, but has
  never run on a real device.

The specification itself estimates **several engineer-years** of work before a real robot moves
under Skynet OS.

---

## What it does *not* promise

"Universal operating system" sounds bigger than this is, so here are the limits, stated up front:

- **No driver, no robot.** Skynet OS only works on robots someone has written a working driver for.
- **Some robots will always be Guest.** Many industrial arms — for example Fanuc, KUKA, ABB and older
  Universal Robots controllers — only accept whole programs, not motor-by-motor commands. Their own
  controller is in charge of safety, so Skynet OS can only ride along.
- **Running is not the same as succeeding.** Skynet OS can adapt software made for one robot to a
  robot with a different body. That makes the commands valid and keeps them inside safe limits, but
  it does not mean the task still works. Nobody knows how to guarantee that yet.
- **At the Host level, it replaces your robot's control software.** `ros2_control` and the Safety
  Kernel both want to be the one that controls the motors, and they cannot both be. If you use
  `ros2_control` today, you should know this from the start.

The full list is Part VII of [the specification](docs/skynet-os-architecture.md).

---

## What is in this repository

| Folder | What is inside |
|---|---|
| [`crates/`](crates/) | The main Rust code — the operating system itself |
| [`drivers/`](drivers/) | Robot drivers: `mock` (no hardware), `mujoco` (physics simulator), `lerobot` and `python-host` |
| [`bindings/`](bindings/) | The interface every driver implements (the RHAL), in C, C++ and Python |
| [`packaging/`](packaging/) | The installer, the virtual robot script, the system service files and the release bundle |
| [`docs/`](docs/) | The specification, the status report and the developer guide |
| [`corpus/`](corpus/) | Shared test cases that every version of the code must reproduce exactly |
| [`editions/`](editions/) | The core rules in Python, C++, Go, C, Zig, Java and C# |
| [`reference/`](reference/) | The TypeScript reference code the test cases are generated from |
| [`tools/`](tools/) | Scripts that generate and check things, run automatically on every change |

---

## Words you will see

| Word | What it means |
|---|---|
| **Driver** | A small library that connects Skynet OS to one kind of robot |
| **RHAL** | Robot Hardware Abstraction Layer — the fixed C interface every driver implements |
| **Safety Kernel** | The part that checks every command, and the only part allowed to move a motor |
| **Guard** | One of the Safety Kernel's eight safety checks |
| **Policy** | Software that decides how a robot should move, often a trained AI model |
| **Skill** | A named task a robot can be asked to do, such as "pick up the cup" |
| **Package** | Robot software bundled so it can be installed on a robot |
| **Descriptor** | A file describing a robot's body: its joints, their limits and its shape |
| **Tier** | Guest, Host or Certified — how deeply Skynet OS is connected to a robot |
| **Black box** | The recorder that saves what happened in every control cycle |
| **Corpus** | The shared set of test cases every edition must pass |
| **Edition** | The core rules written in one programming language |
| **`skynetd` / `skynetk`** | The two background services: the supervisor, and the control loop |

---

## Learn more

- **[Developer guide](docs/developer-guide.md)** — build it, write a driver, describe a robot's body,
  see what the Safety Kernel will enforce, run the services by hand, and fix problems.
- **[Specification](docs/skynet-os-architecture.md)** — the complete design. It is long: start with
  Part I, and read Part VII (what does not work) early.
- **[Status](docs/status.md)** — what is built, part by part.
- **[Editions](editions/README.md)** — the same rules in other programming languages.

---

## Get help or get involved

- **Found a bug?** [Open an issue](https://github.com/antonihabek/Skynet/issues/new/choose) with
  the bug template. If a robot is involved, attach the file made by `skynet doctor --out bundle.json`.
- **Have a question or an idea?** [Start a discussion](https://github.com/antonihabek/Skynet/discussions).
- **Want Skynet OS on your robot?** Open a *Robot or driver support* issue. Drivers are the most
  useful thing anyone can contribute.
- **Want to write code?** Read [CONTRIBUTING.md](CONTRIBUTING.md) first.
- **Found a security or safety problem?** Please **do not** post it publicly — report it privately,
  as [SECURITY.md](SECURITY.md) explains.

Everyone taking part is expected to follow the [code of conduct](CODE_OF_CONDUCT.md).

---

## Licence

Skynet OS is free and open source under the [Apache 2.0 licence](LICENSE). You can use it, change it
and share it, including in commercial products, as long as you keep the licence and copyright
notices. The Skynet name and the certification programme are not covered by the licence.
