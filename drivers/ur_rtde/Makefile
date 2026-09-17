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
#   docker run -d --name ursim -p 29999:29999 -p 30001:30001 -p 30002:30002 \
#     -p 30003:30003 -p 30004:30004 universalrobots/ursim_e-series:latest
#   SKYNET_UR_HOST=127.0.0.1 make check

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

.PHONY: all selftest check check-conformance clean

all: selftest

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

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
	  --config '{"host":"$(SKYNET_UR_HOST)","simulator":true}'

clean:
	rm -rf skynet_ur/__pycache__ __pycache__
