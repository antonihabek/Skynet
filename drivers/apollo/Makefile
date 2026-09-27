# skynet-apollo — an Apptronik Apollo humanoid in MuJoCo, and its substrate.
#
# Phase 7 named this row unreachable: no sales, no interface. The challenge held for the
# machine and fell for the model — DeepMind's menagerie ships an Apache-2.0 Apollo MJCF the
# vendor itself engages with (CHANGELOG: "IMU position correction based on Apptronik
# request"), so a sim driver with a pinned substrate is honest work and a hardware claim
# would not be. Guest ceiling, twice over (§31.3's Python, §35.3's simulator).
#
#   make check    fetch the pinned model, the suite, the layout cross-check, driver check
#   make suite    the driver's own suite on its own, for driving by hand
#
# Needs MuJoCo in the project environment (the repo-root .venv, untracked by design) and a
# GL backend MuJoCo can load — osmesa in CI, EGL on this machine:
#
#   make check MUJOCO_GL=egl                                    # from drivers/apollo
#
# MENAGERIE_DIR overrides the fetch when the model is already on disk (sparse checkout of
# google-deepmind/mujoco_menagerie at MENAGERIE_COMMIT, verified, never trusted).

PYTHON   ?= python3
REPO     := ../..
APOLLO_PYTHON ?= $(REPO)/.venv/bin/python
MENAGERIE_COMMIT ?= 822c2d8f877dd166c5b7d3c9f7e3c3b6589473b7

.PHONY: all check suite clean

all: check

suite:
	MENAGERIE_COMMIT=$(MENAGERIE_COMMIT) APOLLO_PYTHON=$(APOLLO_PYTHON) sh ./run-check.sh suite

check:
	MENAGERIE_COMMIT=$(MENAGERIE_COMMIT) APOLLO_PYTHON=$(APOLLO_PYTHON) sh ./run-check.sh

clean:
	rm -rf __pycache__
