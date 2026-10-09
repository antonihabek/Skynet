# skynet-onrobot — an OnRobot RG2, RG6, 2FG7 or 2FG14 gripper over Modbus TCP or RTU, and its suite.
#
#   make selftest          the register map, the Modbus frames, every refusal, the exchange cycle, the stop, the faults, the motions and the line, against a simulated gripper (TCP, and RTU on a pseudo-terminal). Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated gripper in the same process
#   make fetch-sdk         fetch OnRobot's own client packages from the maker's GitHub release (ONROBOT_SDK=where to unpack; ONROBOT_RELEASE=1.2.0 is pinned by SHA-256, `latest` is not)
#   make build-oracle      build the maker's own client as a process (sim/oracle.cpp) against those packages: ONROBOT_ORACLE=where the binary goes (needs g++ and libmodbus-dev)
#   make check-layout      this driver's registers against the maker's own client, which is run (needs ONROBOT_ORACLE)
#   make chaos             the link-chaos contract, on both links: the Compute Box's Modbus TCP, and the tool bus's Modbus RTU on a serial line, each through a relay that delays, stalls and blackholes it (the serial one loses, truncates and corrupts it too)
#   make faultvocab        the fault-vocabulary contract: what the driver makes of the status bits the maker's library names, and of codes outside them
#   make check-oracle-docker  all of the maker's-client half in a container that fetches the packages and builds the oracle at the release ONROBOT_RELEASE names
#
# DRY-COVERAGE-PLAN.md item 12.E.8. OnRobot publish a closed client library and no register table and no simulator: the registers were read off the library by running it, and the gripper on the other end of the line
# is written in this repository, so `check-layout` is F118's rung 3 (the maker's own client, run, against a counterpart of ours), not a gripper and not the maker's simulator.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: Where the maker's packages are unpacked, and where the oracle is built, for `fetch-sdk`, `build-oracle` and `check-layout`.
ONROBOT_SDK ?= /tmp/onrobot-sdk
ONROBOT_ORACLE ?= /tmp/onrobot-oracle/onrobot-oracle
ONROBOT_RELEASE ?= 1.2.0
ONROBOT_IMAGE ?= skynet-onrobot-oracle
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest fetch-sdk build-oracle check-layout chaos faultvocab check-oracle-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_onrobot \
	SKYNET_ONROBOT_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

fetch-sdk:
	ONROBOT_RELEASE=$(ONROBOT_RELEASE) sh sim/fetch.sh $(ONROBOT_SDK)

build-oracle:
	@test -d "$(ONROBOT_SDK)/sdk" || { echo "::error::$(ONROBOT_SDK)/sdk is not there (make fetch-sdk). Nothing was built."; exit 1; }
	sh sim/build.sh $(ONROBOT_SDK)/sdk $(dir $(ONROBOT_ORACLE))

check-layout:
	@test -x "$(ONROBOT_ORACLE)" || { echo "::error::ONROBOT_ORACLE names no built oracle (make fetch-sdk build-oracle). Nothing was compared."; exit 1; }
	ONROBOT_ORACLE=$(ONROBOT_ORACLE) PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) layout_check.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the gripper's Modbus TCP link through a relay that delays, stalls and blackholes it (`tools/faultproxy`). In a network namespace of its own, so the
# relay's loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py
	ONROBOT_CHAOS_LINK=serial PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the status bits the maker's library names, from drivers/fault-vocabulary.json, injected into the simulated gripper and held to the class the
# table gives each.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# The maker's packages, fetched and the oracle built in the image (sim/Dockerfile); the driver's suite, the layout check and the sim half of SCTS in the same container.
check-oracle-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg ONROBOT_RELEASE=$(ONROBOT_RELEASE) -t $(ONROBOT_IMAGE) -f sim/Dockerfile sim
	docker rm -f skynet-onrobot-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-onrobot-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/onrobot $(ONROBOT_IMAGE) \
	  sh sim/run-oracle.sh /src/target/release/skynet

clean:
	rm -rf skynet_onrobot/__pycache__ __pycache__
