# skynet-odrive — ODrive servo axes over CANSimple, and the sim half of SCTS against them.
#
# `DRY-COVERAGE-PLAN.md` item 12.E.10's first driver: the vcan CAN family starts here. The
# driver is Python (CANSimple is arithmetic over eight-byte frames and `socket` is stdlib)
# and runs under `drivers/python-host`; the bus underneath is the CANopen driver's
# (`skynet_canopen.bus`, imported rather than rewritten), and the axes are simulated ones
# written here — first on an in-process bus, then on a kernel `vcan`.
#
#   make check         the suite, then `skynet driver check` through the real ABI
#   make selftest      the protocol, the simulated axis and every refusal, on loopback
#   make check-vcan    the same driver on a kernel CAN bus, against axes in another process
#   make check-layout  this driver's constants and frame layouts against ODrive's own sources
#
# `check-layout` needs a checkout of ODrive's firmware (`odriverobotics/ODrive`), which it
# reads unmodified and which is not vendored here:
#
#   git clone https://github.com/odriverobotics/ODrive /tmp/odrive-fw   # then check out the pin
#   make check-layout ODRIVE_FW=/tmp/odrive-fw

PYTHON   ?= python3
CARGO    ?= cargo
REPO     := ../..
HOST     := $(REPO)/drivers/python-host
ODRIVE_FW ?=

#: How many simulated axes the conformance run brings up.
AXES ?= 2

.PHONY: all check selftest check-vcan check-layout chaos faultvocab clean

all: check

# The protocol, with no host and no OS. Every refusal has a control beside it. The CANopen
# package rides on the path beside this one: the bus underneath is its `skynet_canopen.bus`,
# and `models.json` stages both packages onto the robot for the same reason.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) selftest.py

# The whole path, through the real ABI.
check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_odrive \
	SKYNET_ODRIVE_SIM=$(AXES) \
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so \
	  --config '{"joints":[{"node":0,"name":"J1","role":"right_arm.shoulder.pitch"},{"node":1,"name":"J2","role":"right_arm.elbow.flex"}]}'

# The same driver on a kernel CAN bus, against simulated axes in a process of their own.
# `../canopen/vcan.sh` uses the machine's `vcan0`, or creates one in an unprivileged network
# namespace — no root — and fails rather than skips where it can do neither.
check-vcan:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. sh ../canopen/vcan.sh $(PYTHON) selftest.py --vcan

# This driver's constants and frame layouts against ODrive's own firmware and CAN database,
# read unmodified from a checkout at the pinned commit. What it refuses to run without, and
# why, is in `layout_check.py`'s header.
check-layout:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) layout_check.py "$(ODRIVE_FW)"

# The link-chaos contract (12.C.4): the bridge on `vcan0`, the simulated axes on `vcan1`, and `tools/faultproxy`'s CAN relay across the gap
# with delay, stall, blackhole, loss, bursts, duplicates and reordering. `chaos.py` makes both interfaces in the namespace it runs in.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's axis error word in drivers/fault-vocabulary.json, injected through
# this driver's own read and write, and each held to the class the table gives it — the flags §27 reads, a command refused while the axis holds
# it, the fault word, recovery. It is the dry proxy for layer 9: what the driver *does* with a code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

clean:
	rm -rf skynet_odrive/__pycache__ __pycache__
