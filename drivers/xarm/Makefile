# skynet-xarm — a UFACTORY xArm over the controller's own wire, and its suite.
#
#   make selftest          the wire and every refusal, against a simulated controller. Needs python3
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated
#                          controller in the same process
#   make check-layout      this driver's constants against the vendor's own SDK source (XARM_SDK=...)
#   make check-sim-docker  all of that against UFACTORY's controller firmware, in their simulator image
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md Phase 10, item 10.5: the maker's own controller software
# on the other end of the wire, which is F118's rung 1.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: UFACTORY's simulator, pinned by digest (its `latest` tag moves): the image their own support article
#: names, running the controller's own firmware. The nightly workflow passes `latest` here.
XARM_SIM_IMAGE    ?= danielwang123321/uf-ubuntu-docker@sha256:ad56ecec2ba7818b7221650b7f9d26d5cac3f75af7aeb545854a860bd35c125a
XARM_CLIENT_IMAGE ?= skynet-xarm-client
DOCKER_BUILD_ARGS ?=
#: The arm the simulator starts: `xarm_start.sh <axis> <type>` — an xArm 6, type 6.
XARM_SIM_START    ?= 6 6
#: A checkout of UFACTORY's SDK, for `check-layout` outside the image.
XARM_SDK          ?=

#: What `check` opens: an xArm 6, with the roles a fixture asserts (§63.4: a role is a human's assertion).
JOINTS := {"joints":[{"name":"joint1","role":"right_arm.shoulder.yaw"},{"name":"joint2","role":"right_arm.shoulder.pitch"},{"name":"joint3","role":"right_arm.elbow.flex"},{"name":"joint4","role":"right_arm.wrist.pitch"},{"name":"joint5","role":"right_arm.wrist.yaw"},{"name":"joint6","role":"right_arm.wrist.roll"}]}

.PHONY: all check selftest faultvocab chaos check-layout check-sim-docker check-sim-models check-sim-run build-sim-client clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's codes in drivers/fault-vocabulary.json, injected through
# this driver's own read and write, and each held to the class the table gives it — the flags §27 reads, a command refused while the controller
# ignores it, the fault word, recovery. It is the dry proxy for layer 9: what the driver *does* with a code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_xarm \
	SKYNET_XARM_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '$(JOINTS)'

check-layout:
	@test -n "$(XARM_SDK)" || { echo "::error::XARM_SDK names no checkout of UFACTORY's xArm-Python-SDK. Nothing was compared."; exit 1; }
	XARM_SDK=$(XARM_SDK) PYTHONWARNINGS=ignore PYTHONPATH=. $(PYTHON) layout_check.py

# UFACTORY's simulator as a container, its controller started the way their own script starts it, the
# client image in its network namespace so the driver dials 127.0.0.1 and nothing is published. The wait
# is for the three ports the driver uses — 502, 30002 and 30003 — read from inside the simulator as
# `check-drcf-docker` reads DRCF's. The controller takes some ten seconds to listen.
check-sim-docker: build-sim-client
	$(MAKE) --no-print-directory check-sim-run XARM_SIM_START="$(XARM_SIM_START)"

#: The five controllers UFACTORY's image carries, each started the way its own script starts it (`AXIS TYPE`):
#: the xArm 5, the xArm 6, the Lite 6 (type 9), the 850 (type 12) and the xArm 7. The image has all five
#: firmwares, so the 5- and 7-axis paths are no longer held only against a controller written here.
XARM_MODELS ?= "5 5" "6 6" "6 9" "6 12" "7 7"

check-sim-models: build-sim-client
	@failed=""; for m in $(XARM_MODELS); do \
	  echo "===== xarm_start.sh $$m"; \
	  $(MAKE) --no-print-directory check-sim-run XARM_SIM_START="$$m" || failed="$$failed [$$m]"; \
	done; \
	if [ -n "$$failed" ]; then echo "::error::failed for:$$failed"; exit 1; fi

build-sim-client:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build $(DOCKER_BUILD_ARGS) -t $(XARM_CLIENT_IMAGE) sim

check-sim-run:
	docker rm -f skynet-xarm-sim >/dev/null 2>&1 || true
	docker run -dit --name skynet-xarm-sim $(XARM_SIM_IMAGE) >/dev/null
	docker exec skynet-xarm-sim sh -c '/xarm_scripts/xarm_start.sh $(XARM_SIM_START) >/dev/null 2>&1'
	@waited=0; until docker exec skynet-xarm-sim sh -c \
	    'grep -qi ":01F6 " /proc/net/tcp && grep -qi ":7532 " /proc/net/tcp && grep -qi ":7533 " /proc/net/tcp' 2>/dev/null; do \
	  waited=$$((waited + 1)); \
	  if [ $$waited -gt 90 ]; then \
	    echo "::error::the xArm controller never listened on 502, 30002 and 30003 inside skynet-xarm-sim."; \
	    docker exec skynet-xarm-sim sh -c 'ps aux | grep -i xarm | head' 2>&1 | head; \
	    docker rm -f skynet-xarm-sim >/dev/null 2>&1; exit 1; \
	  fi; sleep 1; done
	@echo "controller: $$(docker exec skynet-xarm-sim cat /home/uf/xArm/version.txt)"
	docker run --rm --network container:skynet-xarm-sim -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/xarm $(XARM_CLIENT_IMAGE) \
	  sh sim/run-sim.sh /src/target/release/skynet $(XARM_SIM_START); \
	rc=$$?; docker rm -f skynet-xarm-sim >/dev/null 2>&1; exit $$rc

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the bridge's three sockets, each through a relay that delays, stalls, blackholes and
# resets them (`tools/faultproxy`). In a network namespace of its own, so the relays' loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf skynet_xarm/__pycache__ __pycache__
