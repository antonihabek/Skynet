# skynet-spot — a Boston Dynamics Spot over its gRPC SDK, and its substrate.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 7 quadruped row: `bosdyn-client` 5.2.0 is public on PyPI and
# there is no vendor simulator, so the substrate is faked gRPC services with the vendor's
# client unmodified (the Pudu shape). Spot is §59.2's genuine class — twelve readable joints,
# task motions, Guest ceiling.
#
#   make check    the suite, the wire cross-check, `skynet driver check`, the fake's report
#   make suite    the driver's own suite on its own, for driving by hand
#
# Both need the project environment (the wire is gRPC over TLS and no stdlib speaks either),
# which lives untracked at the repo root and which the recipes refuse without:
#
#   python3 -m venv .venv && .venv/bin/pip install 'bosdyn-client==5.2.0'   # from the repo root
#   make check                                                            # from drivers/spot
#
# SPOT_PYTHON overrides the interpreter when it lives elsewhere.

PYTHON   ?= python3
CARGO    ?= cargo
REPO     := ../..
SPOT_PYTHON ?= $(REPO)/.venv/bin/python

.PHONY: all check suite clean

all: check

# The suite on its own needs the project environment but no other process: its fake Spot runs
# in-process, over real loopback TLS and gRPC.
suite:
	PYTHONPATH=$(REPO)/bindings/python:$(REPO)/drivers/spot $(SPOT_PYTHON) selftest.py

# The whole path, through the real ABI: the fake Spot as a process, the suite, the wire
# cross-check against the vendor's own client, then the conformance run against the driver —
# then the fake's own report of what the driver sent.
check:
	SPOT_PYTHON=$(SPOT_PYTHON) sh ./run-check.sh

clean:
	rm -rf __pycache__ skynet_spot/__pycache__
