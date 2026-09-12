# Installing Skynet OS

Two scripts and two unit files. One puts the OS on a machine; the other turns a machine with no
robot attached into a robot, which is how any of this gets exercised before hardware exists.

```bash
# A machine with a body behind it
sudo ./packaging/install.sh --driver /path/to/libvendor.so --descriptor body.json

# A machine with no body at all
sudo ./packaging/virtual-robot.sh up
```

Both need Linux with systemd as PID 1, and they say so rather than proceeding.

> **Status, 2026-09-12: both scripts have now run on a Linux host.** Ubuntu 26.04, systemd 259 as
> PID 1, x86-64, apt's rustc 1.93 against the workspace's 1.82 floor. `virtual-robot.sh up` ends
> with ten of ten assertions passing, `install.sh --check` with twenty-nine of twenty-nine
> postconditions holding, and the control loop pinned to its isolated core at `SCHED_FIFO` 80 — read
> back from `/proc`, not from the unit file.
>
> The first run failed, which is what a first run is for. Seven defects came out of it, five in
> these four files: two `${VARIABLE}` references systemd does not expand (one fatal at every start,
> one *silent* — the cpuset that §40.2 rests on was being dropped with a log line while every
> surface said the loop was pinned), a `ReadWritePaths=` naming a directory nothing has ever
> created, an empty `--lan` argument no unit file can omit, a reinstall that wrote a blank admin
> token, and a readiness wait that tested for a socket *file* rather than for an answer. Two were
> older and underneath them: `GET /status` could not name the robot answering it, and the black box
> ring was created at its final path, so the supervisor read a partial one and ran without a
> recorder — on a factory-fresh `/data`, which is the state every robot ships in and the only state
> that reproduces it. Each has a gate now; `install.sh --check` holds systemd to what it actually
> parsed rather than to what the file says.
>
> What this run still does not prove: it is x86-64 and not the aarch64 a robot is, and the body is
> `drivers/mock`, so §35.3 caps the whole thing at guest tier and no part of it is bench evidence.

---

## What an installation is

```
/usr/bin/skynetd            the supervisor (§9, §33)
/usr/bin/skynetk            the control loop: the RHAL driver and the Safety Kernel (§25)
/usr/bin/skynet             the CLI (§18)
/usr/bin/skynetp            the policy host — the producer for the command ring (§25)
/usr/lib/skynet/<driver>    the RHAL shared object this body is behind

/etc/skynet/skynetd.env     0600 — carries the admin token
/etc/skynet/skynetk.env     0600 — names the driver, the tier and the rate
/usr/lib/systemd/system/    skynetk.service, skynetd.service

<root>/data/packages/blobs      content-addressed .sky blobs
<root>/data/packages/active     the symlinks §37.2's rollback set *is*
<root>/data/blackbox            the ring, preallocated by the daemon on first start
<root>/data/state               0700 — desired state, grants, profiles, credentials, the body
<root>/data/identity            0700 — the device key
<root>/run/skynetd.sock         the local API
```

`<root>` is `--root`, and `/` on a robot. The layout is rooted rather than absolute throughout, so
an installation under a temporary directory is the same program as one under `/`.

### Three things the installer will not do

**It will not start the robot.** `--enable` arms the units for the next boot, `--start` runs them
now, and neither is the default. An installer that begins commanding a machine the moment it
finishes copying files is one nobody can safely run while standing next to a robot. The last thing
it prints is the command to start.

**It will not invent a body.** `--driver` and `--descriptor` are both required, with no default and
no probe. §33.1 forbids the supervisor loading a driver to discover what the machine is — that would
hand it a vtable of function pointers into vendor code — so the body arrives as a file, and a robot
installed with a guessed one is a robot whose guards clamp to somebody else's joint ratings.

**It will not choose a token.** §39.2 has no default password at any point in the lifecycle, so the
admin credential is issued by `skynetd credential add`, which takes its randomness from
`/dev/urandom` and refuses rather than falling back. It is printed once. Re-running the installer on
a robot that already has credentials issues nothing and says so.

### Where the descriptor comes from

§28 pins the body at `state/embodiment.json`, and something has to write it. Three sources, in the
order you will meet them:

```bash
# From the driver itself — a bench robot, and what virtual-robot.sh uses
skynet driver check ./libvendor.so --descriptor body.json

# From a URDF — a robot whose model exists before its driver does (§63.4)
skynet descriptor from-urdf robot.urdf > body.json

# From vendor tooling, at manufacture (§50)
```

The first also runs the sim half of the conformance suite, so a body that cannot answer the ABI is
refused before it is installed rather than at the first control cycle.

### Verifying an installation

```bash
./packaging/install.sh --check
```

Every assertion is a postcondition of `install`, run against whatever is on the machine — so it
answers the same question about an installation somebody else made. It checks the binaries are
executable, both env files are `0600`, the driver named in `skynetk.env` is actually there, the body
is at the pinned path and is a JSON object, `state` and `identity` are `0700`, the robot is
provisioned, and systemd knows both units.

It does **not** fully decode the descriptor. That is the daemon's job and it keeps *absent* and
*malformed* apart by name — a provisioning step that never ran and a generator emitting something
wrong imply different work. What `--check` catches is the cheap half: an empty file, or an error
message somebody redirected into the path.

### Removing one

```bash
sudo ./packaging/install.sh --uninstall           # never touches /data
sudo ./packaging/install.sh --uninstall --purge   # asks first
```

`/data` holds the black box, the identity and the desired state. A sealed window is evidence about
whatever prompted the uninstall, so removing it is a separate flag that requires typing a word.

---

## Testing without a robot

```bash
sudo ./packaging/virtual-robot.sh up
```

Builds the four binaries and the mock body, describes the body from the driver, installs, starts
both units in §33.1's order, waits for the socket, and then asserts ten things **by asking the
robot** rather than by trusting what the installer wrote.

### This is not a simulation of the operating system

Everything is the real thing: the real binaries, the real units, the real Safety Kernel evaluating
the real eight guards, the real supervisor reconciling real desired state. The only substitution is
at the bottom of the stack, where the RHAL driver is `drivers/mock` — three revolute joints with
declared roles, no safety envelope, and fault injection selected by a substring in the config.

That substitution is a **supported configuration and not a workaround**. §35.3 caps a simulated body
at guest tier and rules that no simulated run is bench evidence. The mock declares `simulator: true`,
so the cap is applied by mechanism rather than by anybody remembering it — `skynet driver check`
prints the reason on every run:

```
sim evidence supports guest
  ↑ the body declares simulator: true, which caps the tier at guest whatever the evidence
    supports (§35.3). No simulated run is bench evidence.
```

### What it proves, and what it cannot

It proves the parts that are software: that the OS installs, that the units start in the right
order, that the loop runs, that the guards evaluate, that a package is admitted, transport-planned
and started, that the black box opens a window, and that the conformance suite runs on the robot
itself.

It proves **nothing about a physical machine**. No joint moves, no encoder reports, no torque is
applied, and the latency of a real fieldbus is absent. §15 keeps that distinction by tier and
nothing here blurs it: what comes out is guest-tier evidence, which is exactly what it is.

### On WSL

WSL2 runs systemd, which is what these units need. Put

```ini
[boot]
systemd=true
```

in `/etc/wsl.conf` and run `wsl --shutdown` from Windows. Then, inside the distribution:

```bash
sudo apt install -y build-essential rustc cargo
sudo ./packaging/virtual-robot.sh up
```

`rustc` from apt rather than rustup because it is smaller, distro-managed and removable the same
way it arrived — Ubuntu 26.04 carries 1.93 against this workspace's 1.82 floor, which was checked
rather than assumed. rustup is the right answer if you want to cross-compile for the aarch64 a
robot actually is; apt's is enough to bring one up on this machine.

Two things worth knowing before the first run. `cargo build` against `/mnt/c` goes through the
Windows filesystem and is slow; if it also trips on permissions, clone into the Linux filesystem
instead. And `sudo` here asks for a password, which is why this has not been run from an agent
session — every step of the installer needs root.

One thing will not work there and it is worth knowing in advance: **mDNS**. §82's responder binds
port 5353, Avahi usually holds it already, and `skynetd` prints that it did not advertise and
carries on — a robot that refused to start because it could not advertise would be one nobody can
reach at all. The console is still reachable at its address; it is only the browse that finds
nothing.

---

## The two units, and why neither requires the other

`skynetk.service` starts **before** `skynetd.service` and neither has `Requires=`. That is §33.1
expressed in the only place it can be enforced:

> The kernel and the RHAL driver survive a `skynetd` restart. They are separate processes with
> separate lifetimes; the kernel is started by systemd BEFORE `skynetd` and does not depend on it.

A robot with no supervisor is a robot that holds position safely. A robot with no control loop is
one that supervises packages and commands nothing. Both are states this platform is designed to be
in, and a `Requires=` in either direction would make one of them impossible.

`skynetd.service` carries an `ExecStop` that runs **while the daemon is still up** — it withdraws
the mDNS name (§82.4), seals the black box window with reason `shutdown` (§13), and exits. That is
why `/etc/skynet/skynetd.env` holds an admin token: the stop is a request over the robot's own
socket, not a signal.
