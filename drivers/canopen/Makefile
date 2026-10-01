# skynet-canopen — CiA 402 drives on a CANopen bus, and the sim half of SCTS against them.
#
# `make check` drives the whole path an operator would: `skynet driver check` dlopens
# `drivers/python-host`, which forks an interpreter, which serves this bridge over a pipe to a
# simulated drive on an in-process bus. No CAN hardware, and no CAN *kernel module* — which is the
# difference that matters, because `vcan` needs `modprobe` and most containers cannot.
#
# `make selftest` is the protocol on its own: frames, the state machine, SDO, the refusals. It needs
# nothing but python3 and runs in well under a second.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: The image `check-lely-docker` runs in, and the `docker build` arguments for it. Empty builds it at
#: the pin in `lely/Dockerfile`; the nightly substrate workflow passes `--build-arg
#: CANOPEN_REF=master` under another image name to find a change in `ros2_canopen` the night it
#: lands (DRY-COVERAGE-PLAN.md Phase 8, item 8.11).
LELY_IMAGE        ?= skynet-canopen-lely
DOCKER_BUILD_ARGS ?=

#: How many simulated drives the conformance run brings up.
AXES ?= 3

.PHONY: all check selftest faultvocab check-vcan chaos check-lely-docker clean

all: check

# The protocol, with no host and no OS. Every refusal has a control beside it.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's codes in drivers/fault-vocabulary.json, injected through
# this driver's own read and write, and each held to the class the table gives it — the flags §27 reads, a command refused while the controller
# ignores it, the fault word, recovery. It is the dry proxy for layer 9: what the driver *does* with a code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# The whole path, through the real ABI.
check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_canopen \
	SKYNET_CANOPEN_SIM=$(AXES) \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so \
	  --config '{"joints":[{"node":1,"name":"J1","role":"right_arm.shoulder.pitch"},{"node":2,"name":"J2","role":"right_arm.elbow.flex"},{"node":3,"name":"J3","role":"right_arm.wrist.roll"}]}'

# The same driver on a kernel CAN bus, against simulated drives in a process of their own. `vcan.sh`
# uses the machine's `vcan0`, or creates one in an unprivileged network namespace — no root — and
# fails rather than skips where it can do neither.
#
# Until DRY-COVERAGE-PLAN.md F71 this target checked that the kernel had CAN support and then ran
# the loopback suite again, so it had never put a frame on a kernel bus; the first run that did found
# `read_state` reporting the previous SYNC's positions.
check-vcan:
	PYTHONPATH=$(REPO)/bindings/python:. sh ./vcan.sh $(PYTHON) selftest.py --vcan

# Both CANopen drivers against Lely's CiA 402 slave, which `ros2_canopen` ships and nobody here wrote
# (Phase 10, item 10.7): a CANopen stack and a CiA 402 state machine on a kernel bus, in the pinned
# image (`lely/Dockerfile`). The image needs `vcan` in the *host* kernel — `modprobe vcan` — and
# creates the interface itself inside the container's own network namespace, which
# `--cap-add NET_ADMIN` allows and nothing wider. The Rust port's shared object and the CLI are
# built here rather than in the image, which carries no Rust toolchain, and mounted.
#
# The module is loaded here if it is not, with `sudo -n`, which never asks: where that cannot work
# the run script says so and fails, and a machine that has the module loaded already needs neither.
check-lely-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli -p skynet-canopen-rs
	@lsmod 2>/dev/null | grep -q '^vcan' || sudo -n modprobe vcan 2>/dev/null || true
	docker build $(DOCKER_BUILD_ARGS) -t $(LELY_IMAGE) lely
	docker run --rm --cap-add NET_ADMIN -v "$(abspath $(REPO))":/src -w /src/drivers/canopen \
	  $(LELY_IMAGE) sh lely/run-lely.sh /src/target/release/skynet /src/target/release/libskynet_canopen_rs.so

# The link-chaos contract (12.C.4): the bridge on `vcan0`, the simulated drives on `vcan1`, and `tools/faultproxy`'s CAN relay across the gap
# with delay, stall, blackhole, loss, bursts, duplicates and reordering. `chaos.py` makes both interfaces in the namespace it runs in.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf skynet_canopen/__pycache__ __pycache__
