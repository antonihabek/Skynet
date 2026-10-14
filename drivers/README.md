# Drivers

A **driver** is a small library that connects Skynet OS to one kind of robot: it reads the robot's
sensors and sends the Safety Kernel's approved commands to its motors, through one fixed interface
(the RHAL, in [`bindings/`](../bindings/)). Once a robot has a driver, everything else in Skynet OS
works on it.

## Find the driver for your robot

- **[The list on the front page](../README.md#supported-robots)** — every robot with a driver, its
  stage and what it was tested against.
- **[docs/robot-support.md](../docs/robot-support.md)** — the same list with the evidence behind each
  row and what each driver has not shown.
- **[docs/status.md](../docs/status.md)** — one paragraph per driver.

Each folder here is one driver (or one family), and each has its own `README.md` with what it speaks to
the robot, what it was tested against and how to run its tests.

## Stages

| Stage | What the driver was tested against |
|---|---|
| **beta** | Software the robot's maker wrote, or an independent implementation of the maker's protocol. Re-run every night. |
| **alpha** | Simulators written in this repository, or ROS 2's mock hardware. |
| **stable** | A physical robot. |

The stage of every driver is written once, in [`models.json`](models.json), which is also what
`skynet setup --robot` reads and prints before it installs anything. A check in CI holds the README and
the status page to it.

## The files in this folder

| File | What it is |
|---|---|
| [`models.json`](models.json) | The manifest: every driver, its stage, what it was tested against and how it is installed |
| [`robots.json`](robots.json) | Which robots each driver is for, and what its test counterpart has not shown |
| [`confidence.json`](confidence.json) | The evidence for each driver, layer by layer, and the nightly runs behind it |
| [`fault-vocabulary.json`](fault-vocabulary.json) | The faults each maker defines, and what the driver must make of them |

## Writing a new driver

```bash
skynet driver new my-robot
```

creates a driver that builds and fails honestly, in C or Rust. The steps, from there to a moving
simulated robot, are step 4 of the [developer guide](../docs/developer-guide.md). Drivers written in
Python run under `python-host`. Not sure where to start? Open a *Robot or driver support* issue.
