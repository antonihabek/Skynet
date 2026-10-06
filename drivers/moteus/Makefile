# skynet-moteus — mjbots moteus controllers over CAN-FD, and the sim half of SCTS against them.
#
# `DRY-COVERAGE-PLAN.md` item 12.E.10's third driver, and the first in the vcan CAN family that needs a CAN-FD bus. The driver is Python (the register
# protocol is arithmetic over byte strings and `socket` is stdlib) and runs under `drivers/python-host`; the bus underneath is the CANopen driver's
# (`skynet_canopen.bus`, imported rather than rewritten: its loopback carries any frame, and a socket that speaks FD is `skynet_moteus.bus`), and the
# controllers are simulated ones written here — first on an in-process bus, then on a kernel `vcan` with the MTU an FD frame needs.
#
#   make check         the suite, then `skynet driver check` through the real ABI
#   make selftest      the protocol, the simulated controllers and every refusal, on loopback
#   make check-vcan    the same driver on a kernel CAN-FD bus, against controllers in another process
#   make check-layout  this driver's registers, modes, faults, scales and frames against mjbots's own client and firmware
#   make chaos         the link-chaos contract: the bridge and the controllers on two vcans with a CAN relay across the gap
#   make faultvocab    the fault-vocabulary contract: the maker's mode and fault registers through the bridge
#
# `check-layout` needs a checkout of mjbots's repository (`mjbots/moteus`, Apache-2.0), which it reads unmodified and which is not vendored here. It
# needs no third-party Python package: the maker's client is imported with its transports' dependencies stubbed, and `layout_check.py` says how.
#
#   git clone https://github.com/mjbots/moteus /tmp/moteus   # then check out the pin in layout_check.py
#   make check-layout MOTEUS=/tmp/moteus

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host
MOTEUS ?=

#: How many simulated controllers the conformance run brings up (`SKYNET_MOTEUS_SIM`), and the configuration it opens the body with: the operator's stop
#: mode (which has no default), a watchdog of its own, and the roles the operator names (§5.1: roles are never inferred).
CONTROLLERS ?= 2
CONFIG := {"joints":[{"id":1,"name":"J1","role":"right_arm.shoulder.pitch"},{"id":2,"name":"J2","role":"right_arm.elbow.flex"}],"stop_mode":"zero_velocity","watchdog_ms":200}

.PHONY: all check selftest check-vcan check-layout chaos faultvocab clean

all: check

# The protocol, with no host and no OS. Every refusal has a control beside it. The CANopen package rides on the path beside this one: the bus underneath is
# its `skynet_canopen.bus`, and `models.json` stages both packages onto the robot for the same reason.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) selftest.py

# The whole path, through the real ABI.
check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_moteus \
	SKYNET_MOTEUS_SIM=$(CONTROLLERS) \
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '$(CONFIG)'

# The same driver on a kernel CAN-FD bus, against simulated controllers in a process of their own. `vcan-fd.sh` uses the machine's `vcan0` when it is
# already FD-capable, or creates one with `mtu 72` in an unprivileged network namespace — no root — and fails rather than skips where it can do neither.
check-vcan:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. sh vcan-fd.sh $(PYTHON) selftest.py --vcan

# This driver's registers, modes, fault codes, scales and frames against mjbots's own Python client (run, not read) and firmware headers, from a checkout at
# the pinned commit. What it refuses to run without, and why, is in `layout_check.py`'s header.
check-layout:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) layout_check.py "$(MOTEUS)"

# The link-chaos contract (12.C.4): the bridge on `vcan0`, the simulated controllers on `vcan1`, and `tools/faultproxy`'s CAN relay across the gap with
# delay, stall, blackhole, loss, bursts, duplicates and reordering. `chaos.py` makes both interfaces in the namespace it runs in, with the FD MTU.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's mode register and fault codes in drivers/fault-vocabulary.json, injected
# through this driver's own read and write, and each held to the class the table gives it. It is the dry proxy for layer 9: what the driver *does* with a
# code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:../canopen:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

clean:
	rm -rf skynet_moteus/__pycache__ __pycache__
