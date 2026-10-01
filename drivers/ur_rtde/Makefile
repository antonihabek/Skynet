# skynet-ur — a Universal Robots arm over RTDE, and the sim half of SCTS against it.
#
# Two targets and the difference between them is the whole point:
#
#   make selftest   the wire format and the control program, with no controller. Runs anywhere
#   make check      the same, plus everything above it against URSim — UR's own controller software
#
# `check` needs SKYNET_UR_HOST. Without it the second half is **skipped by name**, because a suite
# that reported green having talked to nothing is §15's own worst shape.
#
#   make check-ursim-docker    # URSim at the pin in ursim.mk, `check` against it, URSim removed
#
# or, against a URSim that is already up:
#
#   SKYNET_UR_HOST=127.0.0.1 make check

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

.PHONY: all selftest faultvocab check check-conformance check-ursim-docker check-ursim-models clean

comma := ,
#: URSim's `ROBOT_MODEL` and the e-Series name the driver's ratings table knows it by. UR20 and
#: UR30 have no transcribed ratings, so they run with no envelope and the descriptor says so.
URSIM_MODELS := UR3:UR3e UR5:UR5e UR10:UR10e UR16:UR16e UR20:UR20 UR30:UR30

include ursim.mk

all: selftest

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's codes in drivers/fault-vocabulary.json, injected through
# this driver's own read and write, and each held to the class the table gives it — the flags §27 reads, a command refused while the controller
# ignores it, the fault word, recovery. It is the dry proxy for layer 9: what the driver *does* with a code, never what the code means.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

check: selftest
ifndef SKYNET_UR_HOST
	@echo ""
	@echo "SKYNET_UR_HOST is not set, so only the wire format ran. See this Makefile's header."
else
	$(MAKE) --no-print-directory check-conformance
endif

# The whole path through the real ABI, against a real controller.
check-conformance:
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_ur \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so \
	  --config '{"host":"$(SKYNET_UR_HOST)","simulator":true$(if $(SKYNET_UR_MODEL),$(comma)"model":"$(SKYNET_UR_MODEL)")}'

# `check` against a URSim this target starts, at the pin in ursim.mk, and removes afterwards whatever
# the check said. It assumes no controller is up and leaves none running, which is what the nightly
# substrate workflow needs (DRY-COVERAGE-PLAN.md Phase 8, item 8.11).
check-ursim-docker:
	docker rm -f $(URSIM_NAME) >/dev/null 2>&1 || true
	docker run -d --rm --name $(URSIM_NAME) -e ROBOT_MODEL=$(URSIM_MODEL) $(URSIM_PORTS) $(URSIM_IMAGE)
	@$(PYTHON) $(URSIM_WAIT) 127.0.0.1 || { docker logs $(URSIM_NAME) 2>&1 | tail -20; \
	  docker rm -f $(URSIM_NAME) >/dev/null 2>&1; exit 1; }
	$(MAKE) --no-print-directory check SKYNET_UR_HOST=127.0.0.1; \
	rc=$$?; docker rm -f $(URSIM_NAME) >/dev/null 2>&1; exit $$rc

# Every arm one URSim image emulates, one after another, each with the driver told which it is.
# Phase 10 item 10.6: the row was beta on the UR5e alone, and the controller never says which model
# it is (RTDE and the dashboard publish none), so each model is a separate run, not a flag read back.
check-ursim-models:
	@set -e; for pair in $(URSIM_MODELS); do \
	  model=$${pair%%:*}; name=$${pair##*:}; \
	  echo ""; echo "== URSim as $$model, the driver told $$name"; \
	  $(MAKE) --no-print-directory check-ursim-docker URSIM_MODEL=$$model SKYNET_UR_MODEL=$$name; \
	done

clean:
	rm -rf skynet_ur/__pycache__ __pycache__
