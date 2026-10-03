# skynet-reachy-mini — a Reachy Mini over its daemon's REST interface, and its suite.
#
#   make selftest          the shapes and every refusal, against a simulated daemon. Needs python3
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated daemon in the same process
#   make check-layout      this driver's routes, models and limits against Pollen Robotics' own daemon source (REACHY_MINI=...)
#   make chaos             the link-chaos contract: the daemon's port through a relay that delays, stalls, blackholes and resets it
#   make check-sim-docker  all of it against the maker's own daemon running its MuJoCo simulator, in a container
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md item 12.E.12: Pollen Robotics' own `reachy-mini-daemon --sim` on the other end of the REST interface, F118's rung 2 — the maker's simulator, and not a robot.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A checkout of the maker's repository, for `check-layout`: `git clone --depth 1 https://github.com/pollen-robotics/reachy_mini`.
REACHY_MINI ?=
REACHY_IMAGE ?= skynet-reachy-mini-sim
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest chaos check-layout check-sim-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_reachy_mini \
	SKYNET_REACHY_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

check-layout:
	@test -n "$(REACHY_MINI)" || { echo "::error::REACHY_MINI names no checkout of Pollen Robotics' reachy_mini. Nothing was compared."; exit 1; }
	REACHY_MINI=$(REACHY_MINI) PYTHONWARNINGS=ignore PYTHONPATH=. $(PYTHON) layout_check.py

# The maker's daemon, installed from PyPI at a pinned version in the image (sim/Dockerfile), running its MuJoCo simulator headless; the driver, the maker's own models and the live block in the same container.
check-sim-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) -t $(REACHY_IMAGE) -f sim/Dockerfile sim
	docker rm -f skynet-reachy-mini-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-reachy-mini-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/reachy_mini $(REACHY_IMAGE) \
	  sh sim/run-sim.sh /src/target/release/skynet

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the daemon's one TCP port through a relay that delays, stalls, blackholes and resets it (`tools/faultproxy`). In a network namespace of its own,
# so the relay's loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf skynet_reachy_mini/__pycache__ __pycache__
