# skynet-tello — a Ryze (DJI) Tello or Tello EDU over the SDK 2.0 User Guide's UDP text commands, and its suite.
#
#   make selftest          the frames, every refusal, how the drone is opened, the state, the motions, the stop, the keepalive and the lines, against a simulated drone on loopback ports. Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated drone in the same process
#   make check-layout      this driver's text and state string against djitellopy, a second implementation of the same guide, which is run (TELLO_PYTHON=a python with the requirements-layout.txt installed, `--no-deps`)
#   make chaos             the link-chaos contract: the drone's command and state flows through relays that delay, stall, lose, repeat, reorder, truncate and corrupt them
#   make check-sdk-docker  the layout half in a container that installs djitellopy at the version TELLO_DJITELLOPY names (2.5.0 is pinned by hash; `latest` is the newest, unchecked)
#
# DRY-COVERAGE-PLAN.md item 12.E.11. The independent counterpart is djitellopy, run against a drone written in this repository: it is another person's reading of the same document, not the maker's software, and the
# maker's simulator does not exist, so the release stage is alpha. No Tello was run. The guide has no table of error codes, so there is no fault-vocabulary subject (`drivers/fault-vocabulary.json` says why).

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A Python with `pip install --require-hashes --no-deps -r requirements-layout.txt` done, for `check-layout`.
TELLO_PYTHON ?=
TELLO_DJITELLOPY ?= 2.5.0
TELLO_IMAGE ?= skynet-tello-sdk
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest check-layout chaos check-sdk-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_tello \
	SKYNET_TELLO_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

check-layout:
	@test -n "$(TELLO_PYTHON)" || { echo "::error::TELLO_PYTHON names no Python with djitellopy installed (pip install --require-hashes --no-deps -r requirements-layout.txt). Nothing was compared."; exit 1; }
	PYTHONPATH=$(REPO)/bindings/python:. $(TELLO_PYTHON) layout_check.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the drone's command socket and the state string it pushes back, each through a relay (`tools/faultproxy`). In a network namespace of its own, so the
# relays' loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# djitellopy installed in the image (sim/Dockerfile); the layout check, the driver's suite and the sim half of SCTS in the same container.
check-sdk-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	cp requirements-layout.txt sim/requirements-layout.txt
	docker build -q $(DOCKER_BUILD_ARGS) --build-arg TELLO_DJITELLOPY=$(TELLO_DJITELLOPY) -t $(TELLO_IMAGE) -f sim/Dockerfile sim; status=$$?; rm -f sim/requirements-layout.txt; test $$status -eq 0
	docker rm -f skynet-tello-run >/dev/null 2>&1 || true
	docker run --rm --name skynet-tello-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/tello $(TELLO_IMAGE) \
	  sh sim/run-sdk.sh /src/target/release/skynet

clean:
	rm -rf skynet_tello/__pycache__ __pycache__
