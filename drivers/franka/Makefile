# skynet-franka — a Franka Robotics arm over the FCI, and the sim half of SCTS against it.
#
# `make selftest` is the wire, the state machine and every refusal, against the fake Control box in
# this process. It needs python3 and nothing else and runs in a couple of seconds.
#
# `make check` drives the whole path an operator would: `skynet driver check` dlopens
# `drivers/python-host`, which forks an interpreter, which runs this bridge against a fake Control
# box the same process started. No robot, and the descriptor says `simulator: false` because the
# *driver* is not a simulator — what is simulated is on the other end of a socket, and §35.3's cap
# is not this driver's to claim on a body it did not build.
#
# `make check-layout FRANKA_COMMON=<checkout>` is the one that matters most and the one that needs
# something fetched. See README.md.

PYTHON ?= python3
CARGO  ?= cargo
CXX    ?= c++
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A checkout of https://github.com/frankarobotics/libfranka-common — the vendor's Apache-2.0 wire
#: definitions. Not vendored here, and not needed for `selftest` or `check`.
FRANKA_COMMON ?=

#: A virtualenv with franka-sim installed, for `check-franka-sim` (Phase 8, item 8.2):
#:   python3 -m venv /tmp/franka-sim && /tmp/franka-sim/bin/pip install franka-sim==1.1.7
FRANKA_SIM ?=

#: The franka-sim `check-franka-sim-venv` installs: the release this row was measured against, or
#: `latest` for PyPI's newest, which the nightly substrate workflow runs beside the pin (8.11).
FRANKA_SIM_VERSION ?= 1.1.7
FRANKA_SIM_VENV    ?= $(or $(TMPDIR),/tmp)/skynet-franka-sim-$(FRANKA_SIM_VERSION)

.PHONY: all check selftest faultvocab chaos check-layout check-franka-sim check-franka-sim-venv clean

all: check

# The protocol, the driver and every refusal, with no host and no OS.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4). The FCI's state stream goes to a UDP port the driver announces in `Connect`, so no relay with an address of its own can stand in front of it; the
# kernel's own link emulation (`tc netem`) runs on the loopback of a network namespace of its own, which the driver and the fake Control box share (`tools/faultproxy/relays.py`). Needs `tc` and `sch_netem`.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's codes in drivers/fault-vocabulary.json, injected through
# this driver's own read and write, and each held to the class the table gives it — the flags §27 reads, a command refused while the controller
# ignores it, the fault word, recovery. It is the dry proxy for layer 9: what the driver *does* with a code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# The whole path, through the real ABI.
check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_franka \
	SKYNET_FRANKA_SIM=1 \
	SKYNET_FRANKA_SIM_HZ=1000 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so

# Re-derive `layout.json` from the vendor's own headers.
#
# This is what stops the transcription being a careful reading. It refuses rather than guessing when
# the checkout is not named: a layout check that silently kept the pinned file would be reporting on
# a comparison it did not make.
check-layout:
	@if [ -z "$(FRANKA_COMMON)" ]; then \
	  echo "::error::FRANKA_COMMON is not set. This target needs Franka Robotics' own wire"; \
	  echo "definitions, which are Apache-2.0 and are not vendored here:"; \
	  echo ""; \
	  echo "  git clone --depth 1 https://github.com/frankarobotics/libfranka-common /tmp/franka-common"; \
	  echo "  make check-layout FRANKA_COMMON=/tmp/franka-common"; \
	  echo ""; \
	  echo "layout.json is committed, so `make selftest` checks the driver against it without"; \
	  echo "this. What this target does is re-derive it when a release moves a field."; \
	  exit 1; \
	fi
	@# -Wno-invalid-offsetof: `Connect::Response` has a const member and a base, so it is not a
	@# standard-layout type and `offsetof` on it is *conditionally* supported. GCC and Clang both
	@# support it and compute the right number; the warning is the standard being careful, and
	@# silencing it here is narrower than not measuring the field.
	$(CXX) -std=c++17 -O1 -Wall -Wextra -Wno-invalid-offsetof \
	  -DFRANKA_COMMON_COMMIT='"$(shell git -C $(FRANKA_COMMON) rev-parse HEAD 2>/dev/null || echo unknown)"' \
	  -DFRANKA_COMPILER='"$(shell $(CXX) --version | head -1), $(shell uname -m), -std=c++17"' \
	  -DFRANKA_MEASURED='"$(shell date -u +%Y-%m-%d)"' \
	  -I$(FRANKA_COMMON)/include -o layout_probe layout_probe.cpp
	./layout_probe > layout.json.new
	@$(PYTHON) -c "import json,sys; json.load(open('layout.json.new')); print('layout.json.new parses')"
	@if diff -q layout.json layout.json.new >/dev/null 2>&1; then \
	  echo "layout.json already matches these headers."; rm -f layout.json.new; \
	else \
	  echo "layout.json differs from these headers — review the diff before replacing it:"; \
	  diff -u layout.json layout.json.new || true; \
	  echo ""; \
	  echo "If the new one is right: mv layout.json.new layout.json && make selftest"; \
	fi

# Against franka-sim, an FCI server nobody in this repository wrote: the Python driver motion by
# motion, then the conformance suite on the Python driver and on the Rust port. The script refuses
# when FRANKA_SIM is not a franka-sim virtualenv, so this is never a green run over nothing.
check-franka-sim:
	FRANKA_SIM="$(FRANKA_SIM)" PYTHON=$(PYTHON) sh ./run-franka-sim.sh

# The same, in a virtualenv this target makes: franka-sim at FRANKA_SIM_VERSION, freshly resolved,
# so the run needs nothing installed beforehand. A fresh venv each time rather than a reused one,
# because a `latest` venv that pip found already satisfied would be last night's latest.
check-franka-sim-venv:
	rm -rf "$(FRANKA_SIM_VENV)"
	$(PYTHON) -m venv "$(FRANKA_SIM_VENV)"
	"$(FRANKA_SIM_VENV)/bin/pip" install -q \
	  $(if $(filter latest,$(FRANKA_SIM_VERSION)),franka-sim,franka-sim==$(FRANKA_SIM_VERSION))
	$(MAKE) --no-print-directory check-franka-sim FRANKA_SIM="$(FRANKA_SIM_VENV)"

clean:
	rm -f layout_probe layout.json.new
	rm -rf skynet_franka/__pycache__ __pycache__
