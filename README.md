# Skynet OS

**One safety layer, one installer and one command-line tool for robots from different makers.**
Robot software never moves a motor by itself: a **Safety Kernel** checks every command first.

[![ci](https://github.com/antonihabek/Skynet/actions/workflows/ci.yml/badge.svg)](https://github.com/antonihabek/Skynet/actions/workflows/ci.yml)
[![package](https://github.com/antonihabek/Skynet/actions/workflows/package.yml/badge.svg)](https://github.com/antonihabek/Skynet/actions/workflows/package.yml)
[![licence: Apache-2.0](https://img.shields.io/badge/licence-Apache--2.0-blue.svg)](LICENSE)

> **Pre-release software (version 0.1).** Every driver below is labelled **alpha** or **beta**, which
> says exactly how far it has been tested ([the stages, explained](#supported-robots)). Try a robot
> in the simulator first, and put it on a bench at the Guest level before you rely on it.

[Install](#install) · [Use it](#use-it) · [Supported robots](#supported-robots) · [How it keeps a robot safe](#how-it-keeps-a-robot-safe) · [Get help](#get-help-or-get-involved)

---

## What is Skynet OS?

Robots from different companies run completely different software. A program written for one robot
arm usually cannot run on another one, and there is no common way to install robot software, update
it, or check that it is safe to run.

Skynet OS sits between robot software and the robot's hardware and gives every robot the same:

- **way to install and update software**, in signed packages — a bit like apps on a phone;
- **safety checks**, done by the **Safety Kernel**, which looks at every command before it can reach a
  motor;
- **black box**, which records what the robot was told to do and what it did, like a plane's flight
  recorder;
- **command-line tool**, `skynet`, for talking to a robot and for building drivers.

To support a new robot, someone writes a **driver**: a small library that tells Skynet OS how to read
that robot's sensors and send commands to its motors, like a printer driver. Once a robot has a
driver, everything else in Skynet OS works on it.

"Universal" does **not** mean one program that runs on every robot. What stays the same across robots
is the set of rules — the same packages, the same tools and the same tests — while each robot gets its
own driver.

**Who is it for?**

- **Robot builders and integrators** who want installs, updates, safety checks and a flight recorder
  without writing them again for every robot.
- **Robot makers** who want one driver to be enough to get all of that.
- **Researchers** who download robot software from the internet and want a robot that stays safe even
  when that software is not trustworthy.

Skynet OS is one half of a bigger project. The other half, **Skynet Hub**, is where robot software is
shared, reviewed and signed. It lives in a separate repository, and you do not need it to try Skynet OS.

---

## Install

You can turn a Linux computer into a **virtual robot** in two commands: the real Skynet OS programs and
the real Safety Kernel, with a simulated robot arm instead of hardware. Nothing physical moves.

**You need Linux with systemd** — that is most normal Linux installs, such as Ubuntu. (systemd is the
program that starts and looks after background services.) On Windows, use
[WSL2 with systemd turned on](https://learn.microsoft.com/windows/wsl/systemd). On a Mac, use a Linux
virtual machine.

### Option 1: download a release (quickest)

For 64-bit Linux on Intel/AMD (x86-64) or ARM (aarch64). Nothing to build.

```bash
curl -fsSL https://raw.githubusercontent.com/antonihabek/Skynet/main/packaging/get.sh | sh
sudo ~/.local/share/skynet-os/current/packaging/virtual-robot.sh up
```

The first line downloads Skynet OS, checks that the download is not damaged by comparing its checksum,
and unpacks it into your home folder. It never uses `sudo`, and you can
[read the script](packaging/get.sh) before running it. The second line installs and starts the virtual
robot, and needs `sudo` because it sets up system services.

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

### Remove it

```bash
sudo ./packaging/virtual-robot.sh down
```

This stops and removes the virtual robot. It keeps the `/data` folder, where the robot's black box
recordings are. If you used Option 1, the script is in `~/.local/share/skynet-os/current/packaging/`.

---

## Use it

Talk to your robot with the `skynet` command:

```bash
sudo skynet status                           # which body it has, what it runs, whether stop works
sudo skynet state --token <admin-token>      # what state the robot is in, and why
sudo skynet estop                            # the emergency stop
```

`status` and `estop` never need a password, on purpose: anyone near a robot must always be able to
check it and stop it. Almost everything else needs the **admin token**, a password the installer prints
**once** — so copy it somewhere safe. Pass it with `--token`, or set it in the `SKYNET_TOKEN`
environment variable.

The commands you will use most:

| Command | What it does |
|---|---|
| `skynet status` | Which robot this is, what it runs, whether the stop works |
| `skynet state` | The state the robot is in, and why |
| `skynet estop` | Stop the robot now |
| `skynet skills` | The tasks this robot can be asked to do |
| `skynet skill run <name>` | Ask the robot to do one |
| `skynet install <dir> --id <name> --kind <kind> --version <v> --digest <hex>` | Install a software package, after checking it |
| `skynet rollback` | Go back to the previous version |
| `skynet blackbox` | Look at what the black box recorded |
| `skynet doctor` | One bundle that says what is wrong with this robot |
| `skynet --help` | Everything else |

### Put it on a real robot

A physical robot is installed from your own PC over SSH — nothing needs to be on the robot first. You
name the robot's address, its driver (from the [list below](#supported-robots)) and where the robot's
controller is on the network:

```bash
skynet setup robot.local --robot unitree --driver-address 192.168.123.0/24,multicast
```

The wizard works out the robot's body description, downloads the release when none is here, shows every
step, and offers to start the services and run the first check. The full procedure is step 8 of the
[developer guide](docs/developer-guide.md). `--driver-address` says where the controller is: the control
loop runs with no network at all until it is told, and is then opened to that address and nothing else.

At first every robot runs at the **Guest** level (see [the three levels](#three-levels-guest-host-and-certified)),
where Skynet OS watches and records but does not block. Moving up a level is done per robot, with a bench
test — see the [bench runbook](docs/bench-runbook.md).

---

## Supported robots

These are the robots Skynet OS has drivers for. Each driver says how far it was tested:

| Stage | What it was tested against |
|---|---|
| **beta** | Software the robot's maker wrote (its controller software or its simulator), or an independent implementation of the maker's protocol. Re-run every night. |
| **alpha** | Simulators written in this repository (usually driving the maker's own client library), or ROS 2's mock hardware, because the maker's own software was not available to test against. |
| **stable** | A physical robot. Only a driver that has been through that gets this label. |

<!-- generated:robots -->

**Arms and cobots**

| Robot | Stage | Driver | Tested against |
|---|---|---|---|
| Universal Robots UR3e, UR5e, UR10e, UR16e, UR20, UR30 | **beta** | `ur_rtde`, `ur_rtde-rs`, `ros2_control` | URSim, Universal Robots' own controller software |
| Franka Robotics arms (FCI) | **beta** | `franka`, `franka-rs` | an independent Franka server with MuJoCo physics |
| UFACTORY xArm 5, xArm 6, xArm 7, Lite 6, 850 | **beta** | `xarm` | UFACTORY's own controller firmware |
| Doosan M0609, M0617, M1013, M1509, A0509, A0912, E0509, H2017, H2515 | **beta** | `ros2_control` | Doosan's own plugin and controller emulator (DRCF) |
| KUKA KR 6 R900 sixx | **beta** | `ros2_control` | KUKA's own plugin and RSI simulator |
| AUBO i5 | **beta** | `ros2_control` | AUBO's own plugin and controller software (ARCS) |
| Dobot CR, CRA, CRV, Nova and Magician E6 | **beta** | `dobot` | Dobot's own client, with a controller written from its interface document |
| Rainbow Robotics RB series | **beta** | `rainbow` | Rainbow Robotics' own client, with a control box written from its documents |
| ABB, FANUC, Yaskawa Motoman, Stäubli and other robots with a ROS-Industrial server | **beta** | `ros_industrial` | a controller server built from ROS-Industrial's own message library |
| FAIRINO FR3 | **alpha** | `fairino` | a simulator written in this repository, driven by the maker's own client |
| ABB IRB 1200 and IRB 120, AUBO i5S, DENSO VS-060, Elite CS520H, FANUC LR Mate 200iD, Kawasaki RS007L, Kinova Gen3, Mitsubishi MELFA RV-4FR, Stäubli TX2-60L, Techman TM7S | **alpha** | `ros2_control` | ROS 2's mock hardware with the maker's robot description (ABB IRB 1200: a simulator written in this repository) |
| LeRobot arms: SO-100, SO-101, Koch, OpenMANIPULATOR-X | **alpha** | `lerobot` | LeRobot's own classes on simulated servo buses written in this repository |
| AgileX Piper | **alpha** | `piper` | a simulated arm written in this repository |

**Humanoids, legged robots and service robots**

| Robot | Stage | Driver | Tested against |
|---|---|---|---|
| Unitree Go2, B2, H1, G1, H1-2, H2, R1 | **beta** | `unitree` | Unitree's own MuJoCo simulator |
| Booster K1 | **beta** | `booster-k1` | a MuJoCo simulator of the K1 by a third party (NUbots) |
| AgiBot X1 | **beta** | `x1` | AgiBot's own deployment software and simulator |
| Pollen Robotics Reachy 2 | **beta** | `lerobot-reachy2` | Pollen's own software in its mock-hardware mode |
| Pollen Robotics Reachy Mini | **beta** | `reachy_mini` | Pollen's own daemon in its simulator |
| Booster T1 | **alpha** | `booster` | a simulated robot written in this repository, with the maker's own SDK |
| Fourier GR-1 | **alpha** | `fourier` | a simulated server written in this repository, with the maker's own client |
| Deep Robotics Jueying Lite3 | **alpha** | `lite3` | a simulated robot written in this repository, with the maker's own sender and receiver |
| Boston Dynamics Spot | **alpha** | `spot` | simulated services written in this repository, with the maker's own client |
| Pudu service robots (through the maker's cloud API) | **alpha** | `pudu` | a simulated cloud written in this repository |

**Hands and grippers**

| Robot | Stage | Driver | Tested against |
|---|---|---|---|
| SCHUNK EGU 60 and EGK 25 | **beta** | `schunk` | SCHUNK's own simulator and client |
| Robotiq 2F-85 and Hand-E | **beta** | `robotiq` | Robotiq's own SDK and simulated gripper |
| OnRobot RG2, RG6, 2FG7 and 2FG14 | **beta** | `onrobot` | OnRobot's own client library, with a simulated gripper written in this repository |
| LEAP Hand (Lite and Full) | **beta** | `leap` | ROBOTIS's own servo client, with a simulated hand written in this repository |
| Inspire Robots RH56 hands | **alpha** | `inspire` | a simulated hand written in this repository |

**Drones, rovers and mobile bases**

| Robot | Stage | Driver | Tested against |
|---|---|---|---|
| ArduPilot and PX4 multirotors, fixed wings, rovers, boats and submarines | **beta** | `mavlink` | ArduPilot's own flight software, built to run on a computer |
| ROS 2 mobile bases: TurtleBot 3 and iRobot Create 3 (in their makers' own simulations) and any base on ROS 2's diff-drive controller | **beta** | `ros2_base` | ROS 2's own diff-drive controller and the makers' simulations |
| DJI Ryze Tello and Tello EDU | **alpha** | `tello` | a simulated drone written in this repository |

**Motor drives and fieldbuses**

| Robot | Stage | Driver | Tested against |
|---|---|---|---|
| Any CiA 402 servo drive on CANopen (Python and Rust) | **beta** | `canopen`, `canopen-rs` | an independent CANopen implementation on a virtual CAN bus |
| Any CiA 402 servo drive on EtherCAT (Python and Rust) | **alpha** | `ethercat`, `ethercat-rs` | simulated slaves written in this repository |
| ODrive axes (CANSimple) and mjbots moteus controllers (CAN-FD) | **alpha** | `odrive`, `moteus` | simulated devices written in this repository on a virtual CAN bus |

<!-- /generated:robots -->

Also included: a **mock robot** (no hardware at all) and a driver for the [MuJoCo](https://mujoco.org)
physics simulator, which is what the [virtual robot](#install) uses.

**Not on the list?** If your robot has a description file (a URDF), `skynet descriptor from-urdf` drafts
its Skynet OS body description, and **the fleet sweep** (`fleet/`) has put 224 public robot descriptions
of 177 robots from 79 makers, pinned by commit and bytes, 174 robots through conformance and the kernel
loop in simulation — a test of the Safety Kernel on those bodies, not of a driver. To get a robot driven,
open a [*Robot or driver support* issue](https://github.com/antonihabek/Skynet/issues/new/choose) or write
the driver: `skynet driver new <name>` creates the starting code in C or Rust (see the
[developer guide](docs/developer-guide.md)). The same list with the evidence behind every row, and what
each counterpart has not shown, is in [docs/robot-support.md](docs/robot-support.md).

`skynet setup --robot` prints a driver's stage before it installs anything.

---

## How it keeps a robot safe

The whole design rests on one rule:

> **Only the Safety Kernel is allowed to send commands to a robot's motors.**

Robot software (called a **policy**, and often an AI model) never controls a motor directly. It writes
down the command it *wants*, and the Safety Kernel decides what actually reaches the motors. Many times
a second, it runs every command through eight safety checks, called **guards**. For example, they check
that:

- the robot's software is still sending fresh commands (if it goes quiet, the robot is brought to a
  controlled stop);
- no joint moves too far, too fast or too hard for its limits;
- the arm stays inside the space it is allowed to be in, and does not hit itself;
- the robot is not moving too fast for how close the nearest person is.

Why so strict? Many AI models shared online are stored in a file format that can run hidden code the
moment it is opened. If downloaded software could move motors directly, one bad file could turn a real
machine into a dangerous one. With the Safety Kernel in the middle, even software nobody trusts can only
*ask*.

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
raises every alarm, but the command still reaches the robot unchanged, because Skynet OS is not the one
in control. That is useful, but it is **not** a safety system, and Skynet OS says so everywhere. The
virtual robot you can try above always runs at Guest level.

**No software can make a robot "certified safe" on its own.** Skynet OS keeps this sentence in its code,
and repeats it word for word wherever the Certified level is mentioned:

```
SAFETY_CLAIM_RULE:
  Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity
  assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified
  body against a specific robot in a specific application.
```

In plain words: an independent, officially approved organisation has to check each real robot doing its
real job. Skynet OS can only give that organisation evidence to look at.

---

## What is built

Skynet OS is at version 0.1. [docs/status.md](docs/status.md) has the full account, part by part.

**Built and tested**

- The Safety Kernel and all eight of its guards.
- The control loop (`skynetk`), the supervisor (`skynetd`) and the `skynet` command-line tool.
- The black box, including playing a recording back through the Safety Kernel again.
- Installing packages, checking their digital signatures, and rolling back. A robot downloads the
  packages it is told to run from its Hub by itself, and checks every file before installing it; copying
  a package over by hand still works.
- A sandbox for robot software: no network, no hardware devices, no way to change the robot's own files,
  and only the one package allowed to move the robot can reach the control loop.
- Drivers for the [robots listed above](#supported-robots), a mock robot, and a driver for the MuJoCo
  physics simulator.
- Running AI models: ONNX policies through ONNX Runtime, and a small built-in runner, with the loop
  closed from a simulated camera to a joint command.
- Tools that turn a URDF robot description into a Skynet OS body description, and that create the
  starting code for a new driver in C or Rust.
- The core rules written again in Python, C++, Go, C, Zig, Java and C#, all checked against the same
  shared tests. Some are complete and some are still in progress — see
  [editions/README.md](editions/README.md).

**Not built yet**

- Reading real cameras (V4L2, GenICam).
- The power-on self-test run against a real stop circuit. The control loop's half of it is built (the
  loop runs its five checks at start, and a robot whose checks pass leaves its start-up state), but a
  stop circuit has not yet been put through it.
- The separate safety chip the Certified level needs.
- A test of software updates on real robot hardware. The update code is written and tested.
- Bench commissioning of each robot, which is what moves a robot above the Guest level.

---

## What it does *not* promise

"Universal operating system" sounds bigger than this is, so here are the limits, stated up front:

- **No driver, no robot.** Skynet OS only works on robots someone has written a working driver for.
- **Some robots stay Guest.** A controller that only accepts whole programs — no real-time external
  motion option available or licensed, or a high-level API such as Spot's — leaves Skynet OS nothing to
  clamp: its own controller is in charge of safety, so Skynet OS can only ride along. That is a property
  of the cell and not of the brand. Current Fanuc, KUKA, ABB and Universal Robots controllers stream
  joint targets when the option is licensed, and reach Host tier through the `ros2_control` bridge
  (§59.2).
- **Running is not the same as succeeding.** Skynet OS can adapt software made for one robot to a robot
  with a different body. That makes the commands valid and keeps them inside safe limits, but it does
  not mean the task still works. Nobody knows how to guarantee that yet.
- **At the Host level, it replaces your robot's control software.** `ros2_control` and the Safety Kernel
  both want to be the one that controls the motors, and they cannot both be. If you use `ros2_control`
  today, you should know this from the start.

The full list is Part VII of [the specification](docs/skynet-os-architecture.md).

---

## What is in this repository

| Folder | What is inside |
|---|---|
| [`crates/`](crates/) | The main Rust code — the operating system itself |
| [`drivers/`](drivers/) | Robot drivers — `mock` (no hardware), `mujoco` (physics simulator), one per supported robot or interface, and `python-host` for drivers written in Python. [docs/status.md](docs/status.md) lists them with their evidence |
| [`bindings/`](bindings/) | The interface every driver implements (the RHAL), in C, C++ and Python |
| [`packaging/`](packaging/) | The installer, the virtual robot script, the system service files and the release bundle |
| [`docs/`](docs/) | The specification, the status report and the developer guide |
| [`corpus/`](corpus/) | Shared test cases that every version of the code must reproduce exactly |
| [`editions/`](editions/) | The core rules in Python, C++, Go, C, Zig, Java and C# |
| [`reference/`](reference/) | The TypeScript reference code the test cases are generated from |
| [`tools/`](tools/) | Scripts that generate and check things, run automatically on every change |
| [`fleet/`](fleet/) | The sweep of public robot descriptions through the Safety Kernel, in simulation |

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
| **Stage** | alpha, beta or stable — how far a driver has been tested |
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
- **[Supported robots, in detail](docs/robot-support.md)** — the evidence behind every row.
- **[Editions](editions/README.md)** — the same rules in other programming languages.

---

## Get help or get involved

- **Found a bug?** [Open an issue](https://github.com/antonihabek/Skynet/issues/new/choose) with the bug
  template. If a robot is involved, attach the file made by `skynet doctor --out bundle.json`.
- **Have a question or an idea?** [Start a discussion](https://github.com/antonihabek/Skynet/discussions).
- **Want Skynet OS on your robot?** Open a *Robot or driver support* issue. Drivers are the most useful
  thing anyone can contribute.
- **Want to write code?** Read [CONTRIBUTING.md](CONTRIBUTING.md) first.
- **Found a security or safety problem?** Please **do not** post it publicly — report it privately, as
  [SECURITY.md](SECURITY.md) explains.

Everyone taking part is expected to follow the [code of conduct](CODE_OF_CONDUCT.md).

---

## Licence

Skynet OS is free and open source under the [Apache 2.0 licence](LICENSE). You can use it, change it and
share it, including in commercial products, as long as you keep the licence and copyright notices. The
Skynet name and the certification programme are not covered by the licence. Third-party files that ship
in this repository keep their own licences, listed in [NOTICE](NOTICE).

All product and company names mentioned here — Universal Robots, Franka Robotics, UFACTORY, Doosan,
KUKA, Unitree, Boston Dynamics and the rest — are trademarks of their owners. Skynet OS is an
independent project. It is not affiliated with or endorsed by any of them, and names them only to say
which hardware it works with.
