# skynet-schunk — a SCHUNK EGU, EGK or EZU gripper over its module's HTTP interface, and its suite.
#
#   make selftest          the frames and every refusal, against a simulated module. Needs python3
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated module in the same process
#   make check-layout      this driver's bytes against SCHUNK's own client, which is run (SCHUNK_PYTHON=a python with schunk_gripper_library, SCHUNK_REPO=a checkout of the maker's repository)
#   make chaos             the link-chaos contract: the module's port through a relay that delays, stalls and blackholes it
#   make check-sim-docker  all of it against the maker's own protocol simulator, in a container, with the maker's own client as the second opinion
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md item 12.E.9: SCHUNK's own `schunk_gripper_dummy` on the other end of the web interface, F118's rung 2 — the maker's simulator, and not a gripper.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A checkout of the maker's repository, and a Python with its client installed (`pip install ./schunk_gripper_library ./schunk_gripper_dummy pytest` inside the checkout), for `check-layout`.
SCHUNK_REPO ?=
SCHUNK_PYTHON ?=
SCHUNK_IMAGE ?= skynet-schunk-sim
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest chaos check-layout check-sim-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_schunk \
	SKYNET_SCHUNK_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

check-layout:
	@test -n "$(SCHUNK_PYTHON)" || { echo "::error::SCHUNK_PYTHON names no Python with SCHUNK's client installed. Nothing was compared."; exit 1; }
	@test -n "$(SCHUNK_REPO)" || { echo "::error::SCHUNK_REPO names no checkout of SCHUNK-SE-Co-KG/schunk_mechatronic_gripper. Nothing was compared."; exit 1; }
	SCHUNK_REPO=$(SCHUNK_REPO) PYTHONPATH=$(REPO)/bindings/python:. $(SCHUNK_PYTHON) layout_check.py

# The maker's simulator and client, installed from the maker's repository at a pinned commit in the image (sim/Dockerfile); the driver, the maker's own client and the live block in the same container.
check-sim-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) -t $(SCHUNK_IMAGE) -f sim/Dockerfile sim
	docker rm -f skynet-schunk-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-schunk-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/schunk $(SCHUNK_IMAGE) \
	  sh sim/run-sim.sh /src/target/release/skynet

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the module's one TCP port through a relay that delays, stalls and blackholes it (`tools/faultproxy`). In a network namespace of its own,
# so the relay's loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf skynet_schunk/__pycache__ __pycache__
