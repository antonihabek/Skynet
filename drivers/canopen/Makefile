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

# The same driver against a real SocketCAN interface. Needs `vcan0` up, which needs a kernel with
# CAN and the privilege to load a module:
#
#   modprobe vcan && ip link add dev vcan0 type vcan && ip link set up vcan0
#
# It is **not** part of `check` and it is not run in CI, because a target that silently does nothing
# where the module is absent is a target that reports a pass it did not earn. This one says so and
# exits non-zero instead.
check-vcan:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) -c "\
	from skynet_canopen.bus import can_available, SocketCanBus; \
	import sys; \
	sys.exit(0) if can_available() else (print('no CAN support in this kernel: `modprobe vcan` first. Nothing was measured.') or sys.exit(1))"
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py --vcan

clean:
	rm -rf skynet_canopen/__pycache__ __pycache__
