# skynet-mavlink — an ArduPilot vehicle over MAVLink, and its suite.
#
#   make selftest          the wire and every refusal, against a simulated vehicle. Needs python3
#   make check             the whole path: skynet driver check -> python-host -> this bridge -> a simulated
#                          vehicle in the same process
#   make check-layout      this driver's MAVLink against the reference implementation's (pymavlink)
#   make check-sim-docker  all of that against ArduPilot's own flight code, built for x86, one vehicle
#                          of each family it drives, and the reference implementation as a second opinion
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md Phase 11, item 11.1: the maker's flight code on the other end
# of the wire, which is F118's rung 1, for a class of robot this repository had no driver for.

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

.PHONY: all check selftest check-layout check-sim-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_mavlink \
	SKYNET_MAVLINK_SIM=1 \
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

clean:
	rm -rf skynet_mavlink/__pycache__ __pycache__
