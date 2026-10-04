# skynet-robotiq — a Robotiq 2F-85 or Hand-E gripper over Modbus RTU, and its suite.
#
#   make selftest          the frames, every refusal, the exchange cycle, the stop and the line, against a simulated gripper on a pseudo-terminal. Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated gripper in the same process
#   make build-sim         build the maker's own SDK (unmodified) and four small programs against it: ROBOTIQ_REPO=a checkout of github.com/Robotiq/grippers, ROBOTIQ_SIM=where to put them
#   make check-layout      this driver's bytes against the maker's own client, which is run (needs ROBOTIQ_SIM)
#   make live              this driver against the maker's own fake gripper, with the maker's own client reading the same gripper (needs ROBOTIQ_SIM)
#   make chaos             the link-chaos contract: the gripper's TCP gateway through a relay that delays, stalls and blackholes it
#   make faultvocab        the fault-vocabulary contract: what the driver makes of the maker's own fault codes
#   make check-sim-docker  all of it, in a container that builds the maker's SDK at a pinned commit
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md item 12.E.8: Robotiq's own fake gripper on the other end of the line, F118's rung 2 — the maker's simulator, and not a gripper.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A checkout of the maker's repository, and where its harnesses are built, for `build-sim`, `check-layout` and `live`.
ROBOTIQ_REPO ?=
ROBOTIQ_SIM ?=
ROBOTIQ_IMAGE ?= skynet-robotiq-sim
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest build-sim check-layout live chaos faultvocab check-sim-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_robotiq \
	SKYNET_ROBOTIQ_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

build-sim:
	@test -n "$(ROBOTIQ_REPO)" || { echo "::error::ROBOTIQ_REPO names no checkout of Robotiq/grippers. Nothing was built."; exit 1; }
	@test -n "$(ROBOTIQ_SIM)" || { echo "::error::ROBOTIQ_SIM names nowhere to build the harnesses."; exit 1; }
	sh sim/build.sh $(ROBOTIQ_REPO) $(ROBOTIQ_SIM)

check-layout:
	@test -n "$(ROBOTIQ_SIM)" || { echo "::error::ROBOTIQ_SIM names no directory of built harnesses (make build-sim). Nothing was compared."; exit 1; }
	ROBOTIQ_SIM=$(ROBOTIQ_SIM) PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) layout_check.py

live:
	@test -n "$(ROBOTIQ_SIM)" || { echo "::error::ROBOTIQ_SIM names no directory of built harnesses (make build-sim). Nothing was run."; exit 1; }
	ROBOTIQ_SIM=$(ROBOTIQ_SIM) PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) live.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the gripper's gateway through a relay that delays, stalls and blackholes it (`tools/faultproxy`). In a network namespace of its own, so the relay's
# loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's own fault codes from drivers/fault-vocabulary.json, injected into the simulated gripper and held to the class the table gives each.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# The maker's SDK, fetched at a pinned commit and built in the image (sim/Dockerfile); the driver, the maker's own client and the live block in the same container.
check-sim-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) -t $(ROBOTIQ_IMAGE) -f sim/Dockerfile sim
	docker rm -f skynet-robotiq-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-robotiq-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/robotiq $(ROBOTIQ_IMAGE) \
	  sh sim/run-sim.sh /src/target/release/skynet

clean:
	rm -rf skynet_robotiq/__pycache__ __pycache__
