# The LeRobot bridge is Python; there is nothing to compile. `make check` runs the sim conformance
# harness against the fake robot, which is the whole of what can be verified without the library.

PYTHON ?= python3

check:
	$(PYTHON) selftest.py

# The same driver, loaded by the OS through drivers/python-host, which is the thing an operator
# actually does. Needs a C compiler for the host.
host-check:
	$(MAKE) -C ../python-host check PYTHON=$(PYTHON)

# LeRobot's own SO-101 class, as shipped, on a bus of emulated Feetech servos behind a pseudo-terminal (DRY-COVERAGE-PLAN.md items 12.D
# and 12.E.5): first the emulator held to Feetech's own SDK and LeRobot's own `FeetechMotorsBus`, then the bridge over the real class.
# `PYTHON` must be an interpreter with `lerobot[feetech]` installed; `check-wire-venv` makes one.
check-wire:
	$(PYTHON) sim/test_feetech_bus.py
	$(PYTHON) selftest_wire.py

#: The LeRobot release this was measured against, or `latest` for PyPI's newest, which the nightly substrate workflow runs beside the pin (8.11).
LEROBOT_VERSION ?= 0.6.1
LEROBOT_VENV    ?= $(or $(TMPDIR),/tmp)/skynet-lerobot-$(LEROBOT_VERSION)

check-wire-venv:
	rm -rf "$(LEROBOT_VENV)"
	python3 -m venv "$(LEROBOT_VENV)"
	"$(LEROBOT_VENV)/bin/pip" install -q --extra-index-url https://download.pytorch.org/whl/cpu $(if $(filter latest,$(LEROBOT_VERSION)),"lerobot[feetech]","lerobot[feetech]==$(LEROBOT_VERSION)")
	$(MAKE) --no-print-directory check-wire PYTHON="$(LEROBOT_VENV)/bin/python"

.PHONY: check host-check check-wire check-wire-venv
