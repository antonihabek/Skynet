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

.PHONY: all check selftest check-layout clean

all: check

# The protocol, the driver and every refusal, with no host and no OS.
selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

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

clean:
	rm -f layout_probe layout.json.new
	rm -rf skynet_franka/__pycache__ __pycache__
