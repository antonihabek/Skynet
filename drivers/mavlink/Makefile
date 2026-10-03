# skynet-mavlink — an ArduPilot or PX4 vehicle over MAVLink, and its suite.
#
#   make selftest          the wire and every refusal, against a simulated vehicle. Needs python3
#   make check             the whole path: skynet driver check -> python-host -> this bridge -> a simulated
#                          vehicle in the same process, ArduPilot's and PX4's
#   make check-layout      this driver's MAVLink against the reference implementation's (pymavlink)
#   make check-sim-docker  all of that against ArduPilot's own flight code, built for x86, one vehicle
#                          of each family it drives, and the reference implementation as a second opinion
#   make check-px4-docker  the same against PX4's own firmware (its published SIH image), a multirotor, a fixed wing
#                          and a rover
#   make chaos             the link-chaos contract: the driver's UDP link to a simulated vehicle through a relay that delays,
#                          drops, duplicates, reorders, truncates, corrupts and goes silent (tools/faultproxy, item 12.C.4)
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md Phase 11, item 11.1: the maker's flight code on the other end
# of the wire, which is F118's rung 1, for a class of robot this repository had no driver for. `check-px4-docker`
# is item 12.E.1: the second autopilot, on the same footing.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: ArduPilot's SITL, pinned by commit inside sim/Dockerfile (ARDUPILOT_REF). The nightly workflow builds
#: `master` too, by passing ARDUPILOT_REF=master here.
MAVLINK_SIM_IMAGE ?= skynet-mavlink-sim
DOCKER_BUILD_ARGS ?=
#: A virtualenv with pymavlink==2.4.49, for `check-layout` outside the image.
PYMAVLINK_PYTHON  ?= $(PYTHON)
#: The image the PX4 live block's client runs in (sim/px4-client.Dockerfile); PX4's own image is pinned by digest in sim/run-px4.sh.
PX4_CLIENT_IMAGE  ?= skynet-px4-client

.PHONY: all check selftest faultvocab check-layout check-sim-docker check-px4-docker chaos clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): MAV_STATE, injected into the simulated vehicle's heartbeat and held to the class
# drivers/fault-vocabulary.json gives each value — the flags §27 reads, the motion that takes the vehicle somewhere new refused in a failsafe, the
# way out (`hold`, `rtl`, `land`, `disarm`) still answered, a state in no table never read as healthy. A proxy for layer 9: what the driver does
# with a state, never what a firmware reports.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_mavlink \
	SKYNET_MAVLINK_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_mavlink \
	SKYNET_MAVLINK_SIM=px4 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

check-layout:
	PYTHONWARNINGS=ignore $(PYMAVLINK_PYTHON) layout_check.py

# ArduPilot's firmware as a container, four vehicles in it, the client (this tree, mounted) in the same
# network namespace so the driver dials 127.0.0.1 and nothing is published. `run-sim.sh` starts the vehicles,
# waits for their ports and runs the suites against them.
check-sim-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(DOCKER_BUILD_ARGS) -t $(MAVLINK_SIM_IMAGE) sim
	docker rm -f skynet-mavlink-sim-run >/dev/null 2>&1 || true
	docker run --rm --cap-add NET_ADMIN --name skynet-mavlink-sim-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/mavlink $(MAVLINK_SIM_IMAGE) \
	  sh sim/run-sim.sh /src/target/release/skynet

# PX4's published SIH image (pinned by digest in sim/run-px4.sh), one vehicle at a time, and the client in its network namespace.
check-px4-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q -t $(PX4_CLIENT_IMAGE) -f sim/px4-client.Dockerfile sim
	PX4_CLIENT_IMAGE=$(PX4_CLIENT_IMAGE) sh sim/run-px4.sh "$(abspath $(REPO))" /src/target/release/skynet

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the one UDP flow between the driver and a simulated vehicle, through a relay that delays, stalls, blackholes,
# loses, duplicates, reorders, truncates and corrupts it (`tools/faultproxy`). In a network namespace of its own, so the relay's loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf skynet_mavlink/__pycache__ __pycache__
