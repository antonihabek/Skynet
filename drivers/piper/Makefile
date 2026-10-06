# skynet-piper — an AgileX Piper arm over CAN, and the sim half of SCTS against it.
#
# `DRY-COVERAGE-PLAN.md` item 12.E.10's second driver, and the first arm in the vcan CAN family. The driver is Python (the protocol is
# arithmetic over eight-byte frames and `socket` is stdlib) and runs under `drivers/python-host`; the bus underneath is the CANopen
# driver's (`skynet_canopen.bus`, imported rather than rewritten), and the arm is a simulated one written here — first on an in-process
# bus, then on a kernel `vcan`.
#
#   make check         the suite, then `skynet driver check` through the real ABI
#   make selftest      the protocol, the simulated arm and every refusal, on loopback
#   make check-vcan    the same driver on a kernel CAN bus, against an arm in another process
#   make check-layout  this driver's identifiers and frame layouts against AgileX's own SDK, in both directions
#   make chaos         the link-chaos contract: the bridge and the arm on two vcans with a CAN relay across the gap
#   make faultvocab    the fault-vocabulary contract: the maker's arm statuses, error bits and driver bits through the bridge
#
# `check-layout` needs a checkout of AgileX's SDK (`agilexrobotics/piper_sdk`), which it reads unmodified and which is not vendored here,
# and a Python with `python-can` (the SDK's one dependency):
#
#   git clone https://github.com/agilexrobotics/piper_sdk /tmp/piper_sdk   # then check out the pin in layout_check.py
#   python3 -m venv /tmp/piper-venv && /tmp/piper-venv/bin/pip install python-can typing_extensions
#   make check-layout PIPER_SDK=/tmp/piper_sdk PIPER_PYTHON=/tmp/piper-venv/bin/python

PYTHON       ?= python3
CARGO        ?= cargo
REPO         := ../..
HOST         := $(REPO)/drivers/python-host
PIPER_SDK    ?=
PIPER_PYTHON ?= $(PYTHON)

#: The configuration the conformance run opens the body with: the operator's speed and grip force (neither has a default), the hand
#: declared, and the roles the operator names (§5.1: roles are never inferred).
CONFIG := {"speed_percent":30,"gripper":true,"gripper_torque_nm":1.0,"roles":{"joint1":"right_arm.shoulder.yaw","joint2":"right_arm.shoulder.pitch","joint3":"right_arm.elbow.flex","joint4":"right_arm.forearm.roll","joint5":"right_arm.wrist.pitch","joint6":"right_arm.wrist.roll","gripper":"right_hand.gripper.grip"}}

.PHONY: all check selftest check-vcan check-layout chaos faultvocab clean

all: check

# The protocol, with no host and no OS. Every refusal has a control beside it. The CANopen package rides on the path beside this one:
# the bus underneath is its `skynet_canopen.bus`, and `models.json` stages both packages onto the robot for the same reason.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) selftest.py

# The whole path, through the real ABI.
check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_piper \
	SKYNET_PIPER_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '$(CONFIG)'

# The same driver on a kernel CAN bus, against a simulated arm in a process of its own. `../canopen/vcan.sh` uses the machine's `vcan0`, or
# creates one in an unprivileged network namespace — no root — and fails rather than skips where it can do neither.
check-vcan:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. sh ../canopen/vcan.sh $(PYTHON) selftest.py --vcan

# This driver's identifiers and layouts against AgileX's own SDK, read unmodified from a checkout at the pinned commit. What it refuses to run
# without, and why, is in `layout_check.py`'s header.
check-layout:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PIPER_PYTHON) layout_check.py "$(PIPER_SDK)"

# The link-chaos contract (12.C.4): the bridge on `vcan0`, the simulated arm on `vcan1`, and `tools/faultproxy`'s CAN relay across the gap with
# delay, stall, blackhole, loss, bursts, duplicates and reordering. `chaos.py` makes both interfaces in the namespace it runs in.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's arm statuses, error bits and driver status bits in
# drivers/fault-vocabulary.json, injected through this driver's own read and write, and each held to the class the table gives it. It is the
# dry proxy for layer 9: what the driver *does* with a code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

clean:
	rm -rf skynet_piper/__pycache__ __pycache__
