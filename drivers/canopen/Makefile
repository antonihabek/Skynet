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

#: How many simulated drives the conformance run brings up.
AXES ?= 3

.PHONY: all check selftest check-vcan clean

all: check

# The protocol, with no host and no OS. Every refusal has a control beside it.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

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

clean:
	rm -rf skynet_canopen/__pycache__ __pycache__
