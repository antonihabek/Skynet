# The LeRobot bridge is Python; there is nothing to compile. `make check` runs the sim conformance
# harness against the fake robot, which is the whole of what can be verified without the library.

PYTHON ?= python3

check:
	$(PYTHON) selftest.py

# The same driver, loaded by the OS through drivers/python-host, which is the thing an operator
# actually does. Needs a C compiler for the host.
host-check:
	$(MAKE) -C ../python-host check PYTHON=$(PYTHON)

# LeRobot's own robot classes, as shipped, on buses of emulated servos behind pseudo-terminals (DRY-COVERAGE-PLAN.md items 12.D and 12.E.5): the SO-101 on
# Feetech STS3215s, and the Koch and the OpenMANIPULATOR-X on Dynamixel X-series. For each family first the emulator held to the maker's own SDK and LeRobot's own
# motors bus, then the bridge over the real class. `PYTHON` must be an interpreter with `lerobot[feetech,dynamixel]` installed; `check-wire-venv` makes one.
check-wire:
	$(PYTHON) sim/test_feetech_bus.py
	$(PYTHON) selftest_wire.py
	$(PYTHON) sim/test_dynamixel_bus.py
	$(PYTHON) selftest_wire_dynamixel.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): LeRobot's own SO-101 on an emulated Feetech bus, with its serial line through a relay that delays, stalls, loses and corrupts bytes (`tools/faultproxy`). `PYTHON`
# must be an interpreter with `lerobot[feetech]`. In a network namespace of its own, so the relay's loopback is nobody else's.
chaos:
	PYTHONPATH=../../bindings/python:. sh ../../tools/seed-defects/in-netns.sh $(PYTHON) ../../tools/faultproxy/chaos.py chaos.py

#: The LeRobot release this was measured against, or `latest` for PyPI's newest, which the nightly substrate workflow runs beside the pin (8.11).
LEROBOT_VERSION ?= 0.6.1
LEROBOT_VENV    ?= $(or $(TMPDIR),/tmp)/skynet-lerobot-$(LEROBOT_VERSION)

check-wire-venv:
	rm -rf "$(LEROBOT_VENV)"
	python3 -m venv "$(LEROBOT_VENV)"
	"$(LEROBOT_VENV)/bin/pip" install -q --extra-index-url https://download.pytorch.org/whl/cpu $(if $(filter latest,$(LEROBOT_VERSION)),"lerobot[feetech,dynamixel]","lerobot[feetech,dynamixel]==$(LEROBOT_VERSION)")
	$(MAKE) --no-print-directory check-wire PYTHON="$(LEROBOT_VENV)/bin/python"

# The link-chaos contract in an environment of its own, as the nightly runs it (`faults.yml`): the Feetech extra is all it needs.
chaos-venv:
	rm -rf "$(LEROBOT_VENV)"
	python3 -m venv "$(LEROBOT_VENV)"
	"$(LEROBOT_VENV)/bin/pip" install -q --extra-index-url https://download.pytorch.org/whl/cpu $(if $(filter latest,$(LEROBOT_VERSION)),"lerobot[feetech]","lerobot[feetech]==$(LEROBOT_VERSION)")
	$(MAKE) --no-print-directory chaos PYTHON="$(LEROBOT_VENV)/bin/python"

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): what the bridge makes of a servo's error bits, on the
# emulated Feetech bus behind LeRobot's own SO-101 and the emulated Dynamixel bus behind its Koch. `PYTHON` must be an
# interpreter with `lerobot[feetech,dynamixel]`, as `check-wire` needs.
faultvocab:
	PYTHONPATH=../../bindings/python:. $(PYTHON) ../../tools/fault-vocab/contract.py faultvocab.py

# The contract in an environment of its own, as the nightly runs it (`substrates.yml`): both servo families' extras,
# because the subject drives the SO-101 and the Koch in one run.
faultvocab-venv:
	rm -rf "$(LEROBOT_VENV)"
	python3 -m venv "$(LEROBOT_VENV)"
	"$(LEROBOT_VENV)/bin/pip" install -q --extra-index-url https://download.pytorch.org/whl/cpu $(if $(filter latest,$(LEROBOT_VERSION)),"lerobot[feetech,dynamixel]","lerobot[feetech,dynamixel]==$(LEROBOT_VERSION)")
	$(MAKE) --no-print-directory faultvocab PYTHON="$(LEROBOT_VENV)/bin/python"

.PHONY: check host-check check-wire check-wire-venv chaos chaos-venv faultvocab faultvocab-venv
