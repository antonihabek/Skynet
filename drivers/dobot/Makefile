# skynet-dobot — a Dobot V4 arm (CR, CRA, E6 and Nova) over the controller's TCP interface, and its suite.
#
#   make selftest          the frames, every refusal, how the arm is opened, the state, the commands, the stop, the faults, the motions and the lines, against a simulated controller on loopback ports. Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated controller in the same process
#   make check-layout      this driver's text and packets against Dobot's own client, which is run (DOBOT_PYTHON=a python with numpy installed; DOBOT_SDK=the client, `make fetch-sdk`)
#   make fetch-sdk         Dobot's client (MIT) at the commit pinned in sim/fetch-sdk.sh, checked against its SHA-256 (DOBOT_VENDOR=where it goes)
#   make chaos             the link-chaos contract: the arm's two TCP connections through a relay that delays, stalls, blackholes and resets them
#   make faultvocab        the fault-vocabulary contract: what the driver makes of the controller's own modes and alarms
#   make check-sdk-docker  the client half in a container that installs numpy at the version DOBOT_NUMPY names (2.5.3 is pinned by hash; `latest` is the newest, unchecked)
#
# DRY-COVERAGE-PLAN.md item 12.E.13. The independent counterpart is the maker's own Python client for the V4 controllers, run against a controller written in this repository: F118's rung 3. No Dobot arm and no
# Dobot controller software was run.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A Python with `pip install --require-hashes -r requirements-layout.txt` done, for `check-layout`.
DOBOT_PYTHON ?=
#: Where `fetch-sdk` puts the client, and where `check-layout` looks for it.
DOBOT_VENDOR ?= /tmp/dobot-vendor
DOBOT_SDK ?= $(DOBOT_VENDOR)/sdk
DOBOT_NUMPY ?= 2.5.3
DOBOT_IMAGE ?= skynet-dobot-sdk
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest check-layout fetch-sdk chaos faultvocab check-sdk-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_dobot \
	SKYNET_DOBOT_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

fetch-sdk:
	sh sim/fetch-sdk.sh $(DOBOT_VENDOR)

check-layout:
	@test -n "$(DOBOT_PYTHON)" || { echo "::error::DOBOT_PYTHON names no Python with numpy installed (pip install --require-hashes -r requirements-layout.txt). Nothing was compared."; exit 1; }
	DOBOT_SDK=$(DOBOT_SDK) PYTHONPATH=$(REPO)/bindings/python:. $(DOBOT_PYTHON) layout_check.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the arm's dashboard and feedback connections through a relay that delays, stalls, blackholes and resets them (`tools/faultproxy`). In a network namespace of
# its own, so the relay's loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the interface document's RobotMode values and the maker's own alarm table (341 controller alarms, from `demo/files/alarmController.py`), from
# drivers/fault-vocabulary.json, injected into the simulated controller and held to the class the table gives each.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# numpy installed in the image (sim/Dockerfile) and the client fetched in it; the driver's suite, the layout check and the sim half of SCTS in the same container.
check-sdk-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	cp requirements-layout.txt sim/requirements-layout.txt
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg DOBOT_NUMPY=$(DOBOT_NUMPY) -t $(DOBOT_IMAGE) -f sim/Dockerfile sim; status=$$?; rm -f sim/requirements-layout.txt; test $$status -eq 0
	docker rm -f skynet-dobot-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-dobot-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/dobot $(DOBOT_IMAGE) \
	  sh sim/run-sdk.sh /src/target/release/skynet

clean:
	rm -rf skynet_dobot/__pycache__ __pycache__
