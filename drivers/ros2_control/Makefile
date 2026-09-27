# skynet-ros2-control — a body behind `ros2_control`, and its suite.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 2 table asks for a *"fake `hardware_interface`"* as the substrate,
# and `ros2_control` ships one: `mock_components/GenericSystem`, the fake its own tests run against.
# So this driver is exercised against the real framework with no robot, and against a fake nobody
# in this repository wrote.
#
#   make check     the suite, against mock_components
#   make lib       libskynet_ros2_control.so
#   make check-conformance   the sim half of SCTS through the real ABI
#   make check-ursim-docker  the suite with its UR5e block, against URSim, through UR's own plugin
#   make check-drcf-docker   the suite with its Doosan block, against Doosan's DRCF emulator
#   make check-egm-docker    the suite with its ABB block, over EGM against a fake controller
#   make check-rsi-docker    the suite with its KUKA block, over RSI against KUKA's own simulator
#   make check-arcs-docker   the suite with its AUBO block, against ARCS, AUBO's controller software
#
# Needs ROS 2 with ros2_control. It refuses rather than building half of itself when that is absent,
# because a driver that compiled without its framework would be a driver nobody could load.
#
# Either ROS works: apt's (`/opt/ros/jazzy`, the default and what CI installs), or RoboStack's
# conda-forge build in user space, with that environment's own compiler —
#   make check ROS_ROOT=$CONDA_PREFIX CXX=$CONDA_PREFIX/bin/x86_64-conda-linux-gnu-c++

ROS_DISTRO ?= jazzy
ROS_ROOT   ?= /opt/ros/$(ROS_DISTRO)

CXX      ?= c++
CXXFLAGS ?= -std=c++17 -O2 -Wall -Wextra -fPIC -DSOURCE_DIR=\"$(CURDIR)\"
REPO     := ../..

# Every ROS package's include directory, because a message package's headers include another's by
# name and there is no single root that satisfies them all. *Package* directories only — ament
# installs a package's headers as `include/<pkg>/<pkg>/`, so a directory is added when it holds one
# named after itself. Adding every directory under `include/` worked on `/opt/ros`, where nothing
# else lives there, and broke on a conda-forge ROS (RoboStack), where `include/dds/features.h` then
# shadowed glibc's `<features.h>` and the standard library stopped compiling. Anything that is not an
# ament package is reached through `include/` itself, as `<fmt/format.h>` is.
ROS_INCLUDES := -I$(ROS_ROOT)/include $(shell for dir in $(ROS_ROOT)/include/*/; do \
                  [ -d "$$dir$$(basename "$$dir")" ] && echo "-I$$dir"; done 2>/dev/null)
ROS_LIBS     := -L$(ROS_ROOT)/lib -lhardware_interface -lrclcpp -lrclcpp_lifecycle -lrcutils -lrmw -lrcl -lrcl_lifecycle -ltracetools -lrealtime_tools -lrcpputils -lfmt -Wl,--allow-shlib-undefined

# The plugin loader dlopens hardware plugins and the URDF parser plugins beside them, and those pull
# in vendored libraries that are not on the default search path.
VENDOR_LIBS := $(shell ls -d $(ROS_ROOT)/opt/*/lib 2>/dev/null | paste -sd: )

#: A vendor's colcon install, searched before ROS_ROOT, for a hardware plugin the distro does not
#: ship — Doosan's `dsr_hardware2` for `check-drcf` (Phase 8, item 8.3). Empty everywhere else.
VENDOR_PREFIX ?=
RUN_ENV := AMENT_PREFIX_PATH=$(if $(VENDOR_PREFIX),$(VENDOR_PREFIX):)$(ROS_ROOT) \
           LD_LIBRARY_PATH=$(if $(VENDOR_PREFIX),$(VENDOR_PREFIX)/lib:)$(ROS_ROOT)/lib:$(ROS_ROOT)/lib/x86_64-linux-gnu:$(VENDOR_LIBS)
#: Extra `docker build` arguments for whichever vendor image a `*-docker` target builds. Empty
#: builds it at the pins in its Dockerfile; the nightly substrate workflow
#: (`.github/workflows/substrates.yml`) passes the makers' branches here, under another image name,
#: to find a vendor change the night it lands (DRY-COVERAGE-PLAN.md Phase 8, item 8.11).
DOCKER_BUILD_ARGS ?=
DRCF_IMAGE ?= skynet-doosan-drcf
#: The M1013 as integrated: Doosan's plugin at the rate its URDF was generated for, position alone
#: (the plugin's `velocity` interface is a feed-forward, F126), and the provisioned roles.
DRCF_CONFIG := urdf=$(CURDIR)/urdf/doosan_m1013_live.urdf update_rate=10 modes=position \
  roles=joint_1:right_arm.shoulder.yaw;joint_2:right_arm.shoulder.pitch;joint_3:right_arm.elbow.flex;joint_4:right_arm.wrist.pitch;joint_5:right_arm.wrist.yaw;joint_6:right_arm.wrist.roll
#: A `skynet` binary for the SCTS half of `check-drcf`. The image carries no Rust toolchain, so
#: `check-drcf-docker` builds one on the host and mounts it; empty skips that half by name.
SKYNET ?=
#: Doosan's own controller emulator, as its `install_emulator.sh` pins it.
DRCF_EMULATOR ?= doosanrobot/dsr_emulator:3.0.1

EGM_IMAGE ?= skynet-abb-egm
#: The IRB 1200 as integrated over EGM: the controller's 4 ms, position alone (what EGM's speed
#: reference means is the RAPID program's `PosCorrGain`, which only a controller has).
EGM_CONFIG := urdf=$(CURDIR)/urdf/abb_irb1200_egm.urdf update_rate=250 modes=position \
  roles=joint_1:right_arm.shoulder.yaw;joint_2:right_arm.shoulder.pitch;joint_3:right_arm.elbow.flex;joint_4:right_arm.wrist.pitch;joint_5:right_arm.wrist.yaw;joint_6:right_arm.wrist.roll

RSI_IMAGE ?= skynet-kuka-rsi
#: The URDF the RSI targets open: KUKA's own xacro output at the pinned descriptions, committed.
#: KUKA moves its plugin and its descriptions together — `master`'s plugin refuses the last released
#: description's joints (DRY-COVERAGE-PLAN.md F138) — so a run at other refs names the URDF the
#: image built from its own descriptions instead: `RSI_IMAGE_URDF=/opt/kuka_ws/kr6_r900_sixx_rsi.urdf`.
RSI_URDF ?= $(CURDIR)/urdf/kuka_kr6r900_rsi.urdf
RSI_IMAGE_URDF ?=
#: The KR 6 R900 sixx as integrated over RSI: the controller's 4 ms, position alone — the only
#: interface KUKA's plugin accepts at the pinned release, and at `master`, which exports `velocity`
#: and `effort` as well, the only one it forwards unless an RSI XML configuration enables the others
#: (F126's shape) — and the roles `kuka_kr6r900.provisioned.json` gives the same arm.
RSI_CONFIG := urdf=$(RSI_URDF) update_rate=250 modes=position \
  roles=joint_1:right_arm.shoulder.yaw;joint_2:right_arm.shoulder.pitch;joint_3:right_arm.elbow.flex;joint_4:right_arm.wrist.pitch;joint_5:right_arm.wrist.yaw;joint_6:right_arm.wrist.roll

ARCS_IMAGE ?= skynet-aubo-arcs
AUBO_IMAGE ?= skynet-aubo-ros
#: `docker build` arguments for the ARCS image alone. ARCS and AUBO's plugin are two images with
#: two sets of pins, so the nightly workflow passes each its own: `ARCS_BUILD_ARGS='--build-arg
#: ARCS_RUN=latest'` beside `DOCKER_BUILD_ARGS` for the plugin's refs.
ARCS_BUILD_ARGS ?=
#: The URDF the ARCS targets open: AUBO's own xacro output at the pinned description, committed. A
#: run at other refs names the one the image built from its own description instead, as the KUKA
#: row does (F138): `ARCS_IMAGE_URDF=/opt/aubo_ws/aubo_i5_arcs.urdf`.
ARCS_URDF ?= $(CURDIR)/urdf/aubo_i5_arcs.urdf
ARCS_IMAGE_URDF ?=
#: The i5 as integrated against ARCS: AUBO's controller rate — the 200 Hz of the plugin's own
#: `aubo_controllers.yaml`, ARCS's 5 ms servo cycle — position alone (the plugin exports a
#: `velocity` command interface and never reads it, F126's shape), and the i5's roles.
ARCS_CONFIG := urdf=$(ARCS_URDF) update_rate=200 modes=position \
  roles=shoulder_joint:right_arm.shoulder.yaw;upperArm_joint:right_arm.shoulder.pitch;foreArm_joint:right_arm.elbow.flex;wrist1_joint:right_arm.wrist.pitch;wrist2_joint:right_arm.wrist.yaw;wrist3_joint:right_arm.wrist.roll

UR_IMAGE ?= skynet-ur-ros
#: The UR5e as integrated: the rate URSim answers (F93), position alone — the vendor plugin crashes
#: on a controller mode switch (F97), so the body never offers one — and the provisioned roles.
UR_CONFIG := urdf=$(CURDIR)/urdf/ur5e_live.urdf update_rate=500 modes=position \
  roles=shoulder_pan_joint:right_arm.shoulder.yaw;shoulder_lift_joint:right_arm.shoulder.pitch;elbow_joint:right_arm.elbow.flex;wrist_1_joint:right_arm.wrist.pitch;wrist_2_joint:right_arm.wrist.yaw;wrist_3_joint:right_arm.wrist.roll

# URSim's pin, shared with the two RTDE drivers.
include ../ur_rtde/ursim.mk

LIB := libskynet_ros2_control.so

.PHONY: all lib check check-conformance check-live check-ursim-docker check-drcf check-drcf-docker \
        check-egm-suite check-egm-conformance check-egm-docker \
        check-rsi-suite check-rsi-conformance check-rsi-docker \
        check-arcs-suite check-arcs-unready check-arcs-conformance check-arcs-docker \
        require-ros clean

all: check

require-ros:
	@test -d "$(ROS_ROOT)/include/hardware_interface" || { \
	  echo "::error::no ros2_control at $(ROS_ROOT). This driver is a ros2_control host and there"; \
	  echo "is nothing to host without it. On Ubuntu: apt install ros-$(ROS_DISTRO)-ros2-control,"; \
	  echo "or set ROS_ROOT. Nothing was built and nothing was measured."; \
	  exit 1; }

lib: require-ros $(LIB)

$(LIB): ros2_control_bridge.cpp $(REPO)/bindings/c/skynet_rhal.h $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -shared -o $@ ros2_control_bridge.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(ROS_INCLUDES) $(ROS_LIBS)

selftest: require-ros ros2_control_bridge.cpp selftest.cpp $(REPO)/bindings/c/skynet_rhal.h \
          $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -o selftest ros2_control_bridge.cpp selftest.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(ROS_INCLUDES) $(ROS_LIBS)

# The suite also needs `ros2_control`'s own test components (`hardware_interface_testing`): the
# F142 fixture is theirs, failing a read and deactivating itself on command. Refused by name when
# absent, because a suite that skipped it would be a green run over the one question it asks.
check: selftest
	@test -f "$(ROS_ROOT)/share/hardware_interface_testing/test/test_components/test_components.xml" || { \
	  echo "::error::no hardware_interface_testing under $(ROS_ROOT). The suite's F142 fixture is"; \
	  echo "ros2_control's own test component: apt install ros-$(ROS_DISTRO)-hardware-interface-testing."; \
	  exit 1; }
	$(RUN_ENV) ./selftest

# The sim half of SCTS against this body, through `dlopen` and the real ABI.
check-conformance: $(LIB)
	$(RUN_ENV) cargo run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check ./$(LIB) --config 'urdf=$(CURDIR)/urdf/two_joint_mock.urdf roles=shoulder:right_arm.shoulder.yaw;elbow:right_arm.elbow.flex'

# The suite with its URSim block — power on, brake release, and the elbow out 0.08 rad and back —
# against urdf/ur5e_live.urdf, then the sim half of SCTS against the same body. Runs where URSim and
# UR's own `ur_robot_driver` both are — `check-ursim-docker` is that place — and refuses rather than
# skipping when either is absent, because this target exists only to run that block. (Until
# 2026-09-25 it probed the dashboard and *passed* when nothing answered.)
check-live: selftest
	@test -n "$(VENDOR_PREFIX)" && test -d "$(VENDOR_PREFIX)/share/ur_robot_driver" || { \
	  echo "::error::VENDOR_PREFIX does not hold UR's ur_robot_driver. Use make check-ursim-docker."; \
	  exit 1; }
	@python3 $(URSIM_WAIT) 127.0.0.1
	$(RUN_ENV) SKYNET_URSIM_LIVE=1 ./selftest
	@test -n "$(SKYNET)" || { echo "::error::SKYNET names no skynet binary."; exit 1; }
	$(MAKE) --no-print-directory lib
	$(RUN_ENV) $(SKYNET) driver check ./$(LIB) --config '$(UR_CONFIG)'

# The same inside the pinned image (ur/Dockerfile), with URSim as a sibling container whose network
# namespace the check joins: the plugin dials 127.0.0.1, and URSim's program dials the plugin's
# reverse server back on 127.0.0.1 — the topology urdf/ur5e_live.urdf was generated for. Nothing is
# published on the host. `clean` first, inside the container, for the reason `check-sim-docker` in
# drivers/unitree gives: a binary another image left in the mounted tree is not this image's.
check-ursim-docker:
	cargo build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(DOCKER_BUILD_ARGS) -t $(UR_IMAGE) ur
	docker rm -f $(URSIM_NAME) >/dev/null 2>&1 || true
	docker run -d --rm --name $(URSIM_NAME) -e ROBOT_MODEL=$(URSIM_MODEL) $(URSIM_IMAGE)
	docker run --rm --network container:$(URSIM_NAME) -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros2_control $(UR_IMAGE) \
	  sh -c 'make clean && make check-live VENDOR_PREFIX=/opt/ur_ws/install SKYNET=/src/target/release/skynet'; \
	rc=$$?; docker rm -f $(URSIM_NAME) >/dev/null 2>&1; exit $$rc

# The suite with its Doosan block (Phase 8, item 8.3): the M1013 behind DRCF, Doosan's own
# controller emulator, driven through Doosan's own `dsr_hardware2` plugin. Runs where both are —
# `check-drcf-docker` is that place — and refuses rather than skipping when the plugin is absent,
# because this target exists only to run that block.
check-drcf: selftest
	@test -n "$(VENDOR_PREFIX)" && test -d "$(VENDOR_PREFIX)/share/dsr_hardware2" || { \
	  echo "::error::VENDOR_PREFIX does not hold Doosan's dsr_hardware2. This target runs the"; \
	  echo "suite's DRCF block and nothing else would make it worth running; use"; \
	  echo "  make check-drcf-docker"; \
	  exit 1; }
	$(RUN_ENV) SKYNET_DRCF_LIVE=1 ./selftest
	@if [ -z "$(SKYNET)" ]; then \
	  echo "SKYNET is not set, so the sim half of SCTS against DRCF was not run (check-drcf-docker sets it)."; \
	else \
	  $(MAKE) --no-print-directory lib && \
	  $(RUN_ENV) $(SKYNET) driver check ./$(LIB) --config '$(DRCF_CONFIG)'; \
	fi

# The same inside the pinned image (doosan/Dockerfile), with the emulator as a sibling container
# whose network namespace the check joins: the plugin dials 127.0.0.1:12345 exactly as Doosan's own
# virtual-mode launch does, and nothing is published on the host.
#
# `-it` is not decoration. The emulator's entrypoint backgrounds DRCF and then runs `su dsr`, which
# returns at once without a terminal, and the container exits with it — Doosan's own `run_drcf.sh`
# starts it `-dit` for that reason. The wait is for DRCF's port, read from inside the container
# because the check joins its namespace rather than reaching it through the host.
check-drcf-docker:
	cargo build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(DOCKER_BUILD_ARGS) -t $(DRCF_IMAGE) doosan
	docker rm -f skynet-drcf >/dev/null 2>&1 || true
	docker run -dit --privileged --rm --name skynet-drcf --env ROBOT_MODEL=M1013 $(DRCF_EMULATOR)
	@waited=0; until docker exec skynet-drcf grep -qi ':3039 ' /proc/net/tcp 2>/dev/null; do \
	  waited=$$((waited + 1)); \
	  if [ $$waited -gt 60 ]; then \
	    echo "::error::DRCF never listened on 12345 inside skynet-drcf. Its log:"; \
	    docker logs skynet-drcf 2>&1 | tail -20; docker rm -f skynet-drcf >/dev/null 2>&1; exit 1; \
	  fi; sleep 1; done
	docker run --rm --network container:skynet-drcf -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros2_control $(DRCF_IMAGE) \
	  sh -c 'make clean && make check-drcf VENDOR_PREFIX=/opt/doosan_ws/install SKYNET=/src/target/release/skynet'; \
	rc=$$?; docker rm -f skynet-drcf >/dev/null 2>&1; exit $$rc

# The suite with its EGM block (Phase 8, item 8.4): the IRB 1200 through PickNik's
# `abb_hardware_interface` over `abb_libegm`, against `abb/fake_egm.py`, a controller built from
# ABB's own `egm.proto`. `abb/run-egm.sh` starts the controller around each of the two targets below
# and reads what it counted; `check-egm-docker` is the place all of it runs.
check-egm-suite: selftest
	@test -n "$(VENDOR_PREFIX)" && test -d "$(VENDOR_PREFIX)/share/abb_hardware_interface" || { \
	  echo "::error::VENDOR_PREFIX does not hold abb_hardware_interface. Use make check-egm-docker."; \
	  exit 1; }
	$(RUN_ENV) SKYNET_EGM_LIVE=1 ./selftest

check-egm-conformance: $(LIB)
	@test -n "$(SKYNET)" || { echo "::error::SKYNET names no skynet binary."; exit 1; }
	$(RUN_ENV) $(SKYNET) driver check ./$(LIB) --config '$(EGM_CONFIG)'

check-egm-docker:
	cargo build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(DOCKER_BUILD_ARGS) -t $(EGM_IMAGE) abb
	docker run --rm -v "$(abspath $(REPO))":/src -w /src/drivers/ros2_control $(EGM_IMAGE) \
	  sh -c 'make clean && sh abb/run-egm.sh /opt/abb_ws/install /src/target/release/skynet'

# The suite with its RSI block (Phase 8, item 8.14): a KR 6 R900 sixx through KUKA's own
# `KukaRSIHardwareInterface`, against KUKA's own `kuka_rsi_simulator`. `kuka/run-rsi.sh` starts the
# simulator around each of the two targets below and reads its log; `check-rsi-docker` is the place
# all of it runs.
check-rsi-suite: selftest
	@test -n "$(VENDOR_PREFIX)" && test -d "$(VENDOR_PREFIX)/share/kuka_rsi_driver" || { \
	  echo "::error::VENDOR_PREFIX does not hold kuka_rsi_driver. Use make check-rsi-docker."; \
	  exit 1; }
	$(RUN_ENV) SKYNET_RSI_LIVE=1 SKYNET_RSI_URDF=$(RSI_URDF) ./selftest

check-rsi-conformance: $(LIB)
	@test -n "$(SKYNET)" || { echo "::error::SKYNET names no skynet binary."; exit 1; }
	$(RUN_ENV) $(SKYNET) driver check ./$(LIB) --config '$(RSI_CONFIG)'

check-rsi-docker:
	cargo build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(DOCKER_BUILD_ARGS) -t $(RSI_IMAGE) kuka
	docker run --rm $(if $(RSI_IMAGE_URDF),-e RSI_URDF=$(RSI_IMAGE_URDF)) \
	  -v "$(abspath $(REPO))":/src -w /src/drivers/ros2_control $(RSI_IMAGE) \
	  sh -c 'make clean && sh kuka/run-rsi.sh /opt/kuka_ws/install /src/target/release/skynet'

# The suite with its ARCS block (Phase 8, item 8.15): an AUBO i5 through AUBO's own
# `AuboHardwareInterface`, against ARCS — AUBO's controller software, with AUBO's simulated arm.
# `aubo/run-arcs.sh` readies the arm and reads the controller around each of the two targets below;
# `check-arcs-docker` is the place all of it runs.
check-arcs-suite: selftest
	@test -n "$(VENDOR_PREFIX)" && test -d "$(VENDOR_PREFIX)/share/aubo_ros2_driver" || { \
	  echo "::error::VENDOR_PREFIX does not hold aubo_ros2_driver. Use make check-arcs-docker."; \
	  exit 1; }
	$(RUN_ENV) SKYNET_ARCS_LIVE=1 SKYNET_ARCS_URDF=$(ARCS_URDF) ./selftest

# The same block's other half, against the arm as a fresh ARCS starts it: powered off.
check-arcs-unready: selftest
	@test -n "$(VENDOR_PREFIX)" && test -d "$(VENDOR_PREFIX)/share/aubo_ros2_driver" || { \
	  echo "::error::VENDOR_PREFIX does not hold aubo_ros2_driver. Use make check-arcs-docker."; \
	  exit 1; }
	$(RUN_ENV) SKYNET_ARCS_LIVE=unready SKYNET_ARCS_URDF=$(ARCS_URDF) ./selftest

check-arcs-conformance: $(LIB)
	@test -n "$(SKYNET)" || { echo "::error::SKYNET names no skynet binary."; exit 1; }
	$(RUN_ENV) $(SKYNET) driver check ./$(LIB) --config '$(ARCS_CONFIG)'

# ARCS runs in its own container — its own OS, Ubuntu 22.04, as AUBO's virtual machine is — and
# the plugin's joins its network namespace, so the plugin dials 127.0.0.1 as it would a controller
# on the same host and nothing is published. The wait is for both of ARCS's ports, 30004 (RPC) and
# 30010 (RTDE), read from inside the container as `check-drcf-docker` reads DRCF's.
#
# `--ulimit rtprio=99` is not tuning. ARCS puts its servo thread on a real-time policy at start,
# and when the kernel refuses it prints one line and waits for ever, listening on its console port
# alone (DRY-COVERAGE-PLAN.md F139). A container's default forbids the policy; this allows it and
# nothing more, which is narrower than the `SYS_NICE` capability that would do the same.
check-arcs-docker:
	cargo build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(ARCS_BUILD_ARGS) -t $(ARCS_IMAGE) aubo/arcs
	docker build $(DOCKER_BUILD_ARGS) -t $(AUBO_IMAGE) aubo
	docker rm -f skynet-arcs >/dev/null 2>&1 || true
	docker run -d --rm --name skynet-arcs --ulimit rtprio=99 $(ARCS_IMAGE)
	@waited=0; until docker exec skynet-arcs sh -c \
	    'grep -qi ":7534 " /proc/net/tcp && grep -qi ":753A " /proc/net/tcp' 2>/dev/null; do \
	  waited=$$((waited + 1)); \
	  if [ $$waited -gt 90 ]; then \
	    echo "::error::ARCS never listened on 30004 and 30010 inside skynet-arcs. Its log:"; \
	    docker logs skynet-arcs 2>&1 | tail -20; docker rm -f skynet-arcs >/dev/null 2>&1; exit 1; \
	  fi; sleep 1; done
	@echo "ARCS: $$(docker exec skynet-arcs cat /opt/arcs/installer)"
	docker run --rm --network container:skynet-arcs $(if $(ARCS_IMAGE_URDF),-e ARCS_URDF=$(ARCS_IMAGE_URDF)) \
	  -v "$(abspath $(REPO))":/src -w /src/drivers/ros2_control $(AUBO_IMAGE) \
	  sh -c 'make clean && sh aubo/run-arcs.sh /opt/aubo_ws /src/target/release/skynet'; \
	rc=$$?; docker rm -f skynet-arcs >/dev/null 2>&1; exit $$rc

clean:
	rm -f selftest $(LIB)
