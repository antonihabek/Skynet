# skynet-rainbow — a Rainbow Robotics RB arm (RB3, RB5, RB10, RB16 and the rest of the series) over the control box's command and data ports, and its suite.
#
#   make selftest          the frames, the links, how the arm is opened, the state, the commands, the stop, the motions and the session, against a simulated control box on loopback ports. Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated control box in the same process
#   make check-layout      this driver's text and packets against Rainbow's own client, `rbpodo`, which is run (RBPODO_PYTHON=a python with rbpodo and numpy installed, `make fetch-sdk`)
#   make fetch-sdk         rbpodo (Apache-2.0) at the commit pinned in sim/fetch-sdk.sh, checked against its SHA-256 and built into a virtual environment of its own (RBPODO_VENDOR=where it goes)
#   make chaos             the link-chaos contract: the control box's two TCP connections through a relay that delays, stalls, blackholes and resets them
#   make faultvocab        the fault-vocabulary contract: what the driver makes of the controller's 606 message codes and of the safety fields of its state packet
#   make check-vm          the bridge run once through against the maker's own Virtual Control Box (a virtual machine the maker supplies; VCB_ADDRESS=host:port of its command port, `sim/run-vm.sh` boots it)
#   make check-sdk-docker  the client half in a container that builds rbpodo at the commit RBPODO_COMMIT names (the pinned one is checked by hash; `main` is the tip, unchecked)
#
# DRY-COVERAGE-PLAN.md item 12.E.14. The independent counterpart is the maker's own client, `rbpodo`, run against a control box written in this repository: F118's rung 3. The maker's Virtual Control Box (rung 1) was run
# by hand, once, on the machine that wrote this driver, and has no download address that stays still, so no workflow runs it. No Rainbow arm and no Rainbow control box was run.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A Python with `rbpodo` and `numpy` installed, for `check-layout` (`make fetch-sdk` prints one).
RBPODO_PYTHON ?=
#: Where `fetch-sdk` puts the client and its virtual environment.
RBPODO_VENDOR ?= /tmp/rbpodo-vendor
RBPODO_COMMIT ?= f6ef41adf629dd27c96b0d3e7ebf3d3fc0cc53c4
RBPODO_IMAGE ?= skynet-rainbow-sdk
VCB_ADDRESS ?= 127.0.0.1:15000
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest check-layout fetch-sdk chaos faultvocab check-vm check-sdk-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_rainbow \
	SKYNET_RAINBOW_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

fetch-sdk:
	RBPODO_COMMIT=$(RBPODO_COMMIT) sh sim/fetch-sdk.sh $(RBPODO_VENDOR)

check-layout:
	@test -n "$(RBPODO_PYTHON)" || { echo "::error::RBPODO_PYTHON names no Python with rbpodo and numpy installed (make fetch-sdk builds one and prints it). Nothing was compared."; exit 1; }
	PYTHONPATH=$(REPO)/bindings/python:. $(RBPODO_PYTHON) layout_check.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the control box's command and data connections through a relay that delays, stalls, blackholes and resets them (`tools/faultproxy`). In a network namespace of
# its own, so the relay's loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's own message table (606 codes, from `error_code.hpp` in `rbpodo`) as the controller's errors and warnings, and the packet document's device error,
# software stop, collision, joint-word, SOS-state and mode fields, from drivers/fault-vocabulary.json, injected into the simulated control box and held to the class the table gives each.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# The bridge once through against the maker's Virtual Control Box: it opens, reads, streams, stops and is released, moves, and is told what the box says. `sh sim/run-vm.sh` boots the box (see its comment).
check-vm:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) vm_check.py $(VCB_ADDRESS)

# rbpodo built in the container (sim/Dockerfile) and the driver's suite, the layout check and the sim half of SCTS in the same container.
check-sdk-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	cp requirements-layout.txt sim/requirements-layout.txt
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg RBPODO_COMMIT=$(RBPODO_COMMIT) -t $(RBPODO_IMAGE) -f sim/Dockerfile sim; status=$$?; rm -f sim/requirements-layout.txt; test $$status -eq 0
	docker rm -f skynet-rainbow-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-rainbow-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/rainbow $(RBPODO_IMAGE) \
	  sh sim/run-sdk.sh /src/target/release/skynet

clean:
	rm -rf skynet_rainbow/__pycache__ __pycache__
