# The RobotStudio runbook

The ABB-live substrate for the `ros2_control` bridge (Phase 7 item 1, F89): a virtual
IRB 120 speaking EGM to the bridge. F89's three gates — not installed, account-walled
download, EGM option unconfirmed — are down to one: EGM on a virtual controller is
confirmed by PickNik's `abb_ros2` documentation (option `689-1`, UDPUC, RAPID module),
and only the install itself is still a human action.

**Status: installed 2026-09-22** — RobotStudio 2025.2 (`25.2.11220.0`) via
`winget install ABB.RobotStudio`, at `C:\Program Files (x86)\ABB\RobotStudio 2025`.
What remains is GUI work this runbook walks through: the RobotWare catalogue fetch
(section 1.3), the virtual controller with the EGM option (section 2), and the WSL
networking (section 3). No headless virtual-controller tooling ships with it
(`Bin/` holds the GUI, the PC SDK and `RSSystemInfo.exe`, nothing that creates a
system), so those three sections are owner steps and everything after them is work.

## 1. Install (Windows, admin)

1. Download RobotStudio from `https://new.abb.com/products/robotics/robotstudio/downloads`
   (myABB account required). The trial carries full functionality for 30 days.
2. Run the installer as administrator. It pulls its own prerequisites (.NET); allow it.
3. Open RobotStudio once and let it fetch the RobotWare catalogue: the virtual
   controller downloads its RobotWare (OmniCore 7.x for an IRB 120) on creation.

## 2. Virtual controller with EGM (Windows)

PickNik's recipe (`picknikrobotics/abb_ros2`, `docs/RobotStudioSetup.md`), for an IRB 120:

1. `File` → `New` → `Solution with Station and Virtual Controller`, with `Customize
   options` selected.
2. Under `Engineering Tools`, add **`689-1 Externally Guided Motion (EGM)**`.
3. `Controller` → `Configuration` → `Communication`, add a `Transmission Protocol`:
   name `ROB_1`, type `UDPUC` (if UDPUC is absent, EGM did not land — go back to 2),
   Remote Address = the WSL machine's IP (step 3 below), Remote port `6511`.
4. Load the EGM RAPID module: `T_ROB1` → `Load Module...` →
   `TRob1Main.mod` from `abb_ros2` (`robot_studio_resources`, or the Pack and Go
   example). Start the controller.

## 3. Network to WSL (both sides)

EGM is controller → ROS over UDP, so the Windows side must reach the WSL side:

1. In WSL: `ip addr show eth0` — the `172.x` address is the Remote Address for step 2.3.
   (With `.wslconfig` `networkingMode=mirrored`, Windows and WSL share localhost and
   `127.0.0.1` works instead.)
2. Allow UDP inbound on the WSL side if a firewall runs there (default Ubuntu WSL has
   none). No Windows firewall change is normally needed for host → WSL on the virtual
   switch; if EGM packets never arrive, that rule is the first suspect.
3. The bridge listens on the port `robotstudio_port` names in the robot description
   (`6511` in the sample). One side's typo here presents as a silent robot.

## 4. Bridge side (Linux)

The open-source half already exists: `abb_hardware_interface` (the `ros2_control`
plugin) and `robot_studio_resources` (Pack and Go) in `picknikrobotics/abb_ros2`, over
the same RoboStack Jazzy user-space the live-UR row used. The IRB 120 description and
provisioned envelope are in `drivers/ros2_control/urdf/` (`abb_irb120_mock.urdf`,
`abb_irb120.provisioned.json`). The work is the live-UR shape: point the bridge's
substrate at the VC's EGM endpoint, run the suite, then SCTS.

## 5. What a green run proves, and does not

Proves: the bridge commands an ABB controller through EGM at rate, against controller
software rather than a mock. Does not prove: servo timing (a virtual controller keeps
simulated time — the URSim row carries the same caveat), the envelope's torque figures
(the IRB 120 provision is velocity-only), or anything a bench test asks (§15 stays
`missing` until a person stands next to the machine).

## 6. The route that needs no RobotStudio

`egm.proto` ships openly under BSD-3-Clause in `ros-industrial/abb_libegm` (`proto/`),
which unblocks the Spot shape: a fake EGM controller speaking the documented protobuf
over UDP loopback, with the vendor/open client unmodified against it. That proves the
bridge's protocol half with no account, no download and no Windows. It does not replace
this runbook — a fake corroborates the bytes, only controller software corroborates the
controller — but it means no driver work waits on section 1.

**Built 2026-09-24** (Phase 8, item 8.4): `make -C drivers/ros2_control check-egm-docker`. The
client is PickNik's `abb_hardware_interface` over `abb_libegm`, unmodified, with
`configure_via_rws:=false`; the body is `abb_ros2`'s own IRB 1200 5/90 — the arm its RobotStudio
Pack and Go uses, so section 2's virtual controller and this fake describe the same machine; the
controller is `drivers/ros2_control/abb/fake_egm.py`. What it found is F131 in the plan, and what it
cannot show is everything section 5 lists plus the controller's own EGM behaviour.
