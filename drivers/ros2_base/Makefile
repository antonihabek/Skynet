# skynet-ros2-base — a ROS 2 mobile base over its velocity-command and odometry topics, and its suite.
#
#   make selftest                the configuration, the graph it will and will not open on, the state, the motions and the stop,
#                                against a simulated base in the same process. Needs python3 and nothing else
#   make check                   the whole path: skynet driver check -> python-host -> this driver -> the simulated base
#   make check-ros-docker        all of that against ros2_control's own diff_drive_controller (ROS 2 Jazzy, mock hardware), in one
#                                container, with a second reader of the same base and ROS's own command-line tools for the graph
#   make check-ros-humble-docker the same live block against Humble's controller, which takes a plain Twist
#   make check-tb3-docker        the driver against ROBOTIS's own TurtleBot3 simulation (Gazebo's server, the burger, its bridge), with Gazebo's own pose of the
#                                model as the second opinion: a robot's simulator, which the controller above is not
#   make check-create3-docker    the driver against iRobot's own Create 3 simulation, with iRobot's `motion_control` between the command topic and the controller
#                                and Gazebo's own pose of the model as the second opinion
#   make chaos-docker            the link-chaos contract (item 12.C.4) against the same controller: DDS has no address to put a relay at, so the kernel's own link
#                                emulation goes on the container's loopback, the driver's DDS is made to use it, and the truth is read by a path the faults do not touch
#
# `check-ros-docker` is DRY-COVERAGE-PLAN.md item 12.E.3: ROS 2's own controller on the other end of the two topics, which is F118's rung 3 (an independent
# implementation of the interface, not any base maker's software) for a class of robot this repository had one driver for, and that one over a cloud.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: ROS 2 Jazzy on Ubuntu 24.04 with `ros2_control` and its diff_drive_controller from the ROS apt archive (sim/Dockerfile). `ROS_DISTRO=humble` is the older
#: distribution whose controller still takes a plain Twist (`check-ros-humble-docker`); the nightly workflow runs `rolling` too.
ROS_DISTRO        ?= jazzy
ROS_IMAGE         ?= skynet-ros2-base-sim
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest check-ros-docker check-ros-humble-docker check-tb3-docker check-create3-docker chaos-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_ros2_base \
	SKYNET_ROS2_BASE_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{"max_linear":0.5,"max_angular":1.0}'
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_ros2_base \
	SKYNET_ROS2_BASE_SIM=twist \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{"max_linear":0.5,"max_angular":1.0}'

# ros2_control's controller, its mock hardware, the driver, ROS's own CLI and the live block, all in one container (no network is published
# and nothing leaves it). `sim/run-ros.sh` starts the controller, waits for it to be active and runs the suites against it. Jazzy's takes
# TwistStamped and the skynet binary built here runs in its image (Ubuntu 24.04), so that pass adds the sim half of SCTS through the real ABI;
# Humble's takes Twist, and its pass is the live block alone.
check-ros-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg ROS_DISTRO=$(ROS_DISTRO) -t $(ROS_IMAGE):$(ROS_DISTRO) -f sim/Dockerfile sim
	docker rm -f skynet-ros2-base-run >/dev/null 2>&1 || true
	docker run --rm --cap-add NET_ADMIN --name skynet-ros2-base-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros2_base $(ROS_IMAGE):$(ROS_DISTRO) \
	  sh sim/run-ros.sh /src/target/release/skynet

check-ros-humble-docker:
	$(MAKE) --no-print-directory check-ros-docker ROS_DISTRO=humble

# The link-chaos contract (item 12.C.4) in the same image: `sim/run-chaos.sh` starts a controller for each profile, sets the kernel's link emulation on the container's loopback, and
# asks the driver the contract's questions. `--cap-add NET_ADMIN` is what lets `tc` change that loopback; nothing of the host's network is touched and nothing is published.
chaos-docker:
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg ROS_DISTRO=$(ROS_DISTRO) -t $(ROS_IMAGE):$(ROS_DISTRO) -f sim/Dockerfile sim
	docker rm -f skynet-ros2-base-chaos >/dev/null 2>&1 || true
	docker run --rm --cap-add NET_ADMIN --name skynet-ros2-base-chaos -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros2_base $(ROS_IMAGE):$(ROS_DISTRO) \
	  sh sim/run-chaos.sh

# ROBOTIS's own simulation of the TurtleBot3 (sim/Dockerfile.turtlebot3): F118's rung 2. Gazebo's physics, the burger's differential-drive plugin and its bridge, run the way the
# maker's launch files run them without a window. The skynet binary built here runs in the image (Ubuntu 24.04), so this pass adds the sim half of SCTS through the real ABI.
TB3_IMAGE ?= skynet-ros2-base-tb3

check-tb3-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg ROS_DISTRO=$(ROS_DISTRO) -t $(TB3_IMAGE):$(ROS_DISTRO) -f sim/Dockerfile.turtlebot3 sim
	docker rm -f skynet-ros2-tb3-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-ros2-tb3-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros2_base $(TB3_IMAGE):$(ROS_DISTRO) \
	  sh sim/run-tb3.sh /src/target/release/skynet

CREATE3_IMAGE ?= skynet-ros2-base-create3

check-create3-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg ROS_DISTRO=$(ROS_DISTRO) -t $(CREATE3_IMAGE):$(ROS_DISTRO) -f sim/Dockerfile.create3 sim
	docker rm -f skynet-ros2-create3-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-ros2-create3-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros2_base $(CREATE3_IMAGE):$(ROS_DISTRO) \
	  sh sim/run-create3.sh /src/target/release/skynet

clean:
	rm -rf skynet_ros2_base/__pycache__ __pycache__
