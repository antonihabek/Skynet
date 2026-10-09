# skynet-leap — a LEAP Hand (sixteen Dynamixel servos on one bus) over protocol 2.0, and its suite.
#
#   make selftest          the frames, every refusal, how the hand is brought up, the exchange cycle, the stop, the faults, the motions and the line, against a simulated hand on a pseudo-terminal. Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated hand in the same process
#   make check-layout      this driver's wire and registers against ROBOTIS's own SDK, which is run (LEAP_PYTHON=a python with dynamixel-sdk and pyserial installed; LEROBOT_TABLES=the pinned table, `make fetch-vendor`)
#   make fetch-vendor      LeRobot's X-series control table (Apache-2.0) at the commit pinned in sim/fetch-vendor.sh, checked against its SHA-256 (LEAP_VENDOR=where it goes)
#   make chaos             the link-chaos contract: the hand's serial line through a relay that delays, stalls and blackholes it
#   make faultvocab        the fault-vocabulary contract: what the driver makes of the servos' own hardware error bits
#   make check-sdk-docker  the SDK half in a container that installs ROBOTIS's SDK at the version DYNAMIXEL_SDK names (4.1.0 is pinned by hash; `latest` is the newest, unchecked)
#
# DRY-COVERAGE-PLAN.md item 12.E. A LEAP Hand has no controller of its own, so the independent counterpart is ROBOTIS's own client for its servos (`dynamixel_sdk`, Apache-2.0), run against a hand written in this
# repository: F118's rung 3. The hand's authors' own client (LEAP_Hand_API) is CC BY-NC 4.0 and is run only where a person sets LEAP_API (see README.md); nothing here fetches it.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A Python with `pip install --require-hashes -r requirements-layout.txt` done, for `check-layout`.
LEAP_PYTHON ?=
#: Where `fetch-vendor` puts LeRobot's table, and where `check-layout` looks for it.
LEAP_VENDOR ?= /tmp/leap-vendor
LEROBOT_TABLES ?= $(LEAP_VENDOR)/lerobot_tables.py
#: A checkout of github.com/leap-hand/LEAP_Hand_API (CC BY-NC 4.0), if a person wants the authors' own client run too. Empty: it is not.
LEAP_API ?=
DYNAMIXEL_SDK ?= 4.1.0
LEAP_IMAGE ?= skynet-leap-sdk
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest check-layout fetch-vendor chaos faultvocab check-sdk-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_leap \
	SKYNET_LEAP_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

fetch-vendor:
	sh sim/fetch-vendor.sh $(LEAP_VENDOR)

check-layout:
	@test -n "$(LEAP_PYTHON)" || { echo "::error::LEAP_PYTHON names no Python with dynamixel-sdk installed (pip install --require-hashes -r requirements-layout.txt). Nothing was compared."; exit 1; }
	LEROBOT_TABLES=$(LEROBOT_TABLES) LEAP_API=$(LEAP_API) PYTHONPATH=$(REPO)/bindings/python:. $(LEAP_PYTHON) layout_check.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the hand's serial line through a relay that delays, stalls and blackholes it (`tools/faultproxy`). In a network namespace of its own, so the relay's
# loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the e-manual's Hardware_Error_Status bits from drivers/fault-vocabulary.json, injected into the simulated hand and held to the class the table
# gives each.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# ROBOTIS's SDK installed in the image (sim/Dockerfile) and LeRobot's table fetched in it; the driver's suite, the layout check and the sim half of SCTS in the same container.
check-sdk-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	cp requirements-layout.txt sim/requirements-layout.txt
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg DYNAMIXEL_SDK=$(DYNAMIXEL_SDK) -t $(LEAP_IMAGE) -f sim/Dockerfile sim; status=$$?; rm -f sim/requirements-layout.txt; test $$status -eq 0
	docker rm -f skynet-leap-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-leap-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/leap $(LEAP_IMAGE) \
	  sh sim/run-sdk.sh /src/target/release/skynet

clean:
	rm -rf skynet_leap/__pycache__ __pycache__
