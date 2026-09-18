# skynet-ethercat — CiA 402 drives on an EtherCAT segment, and the sim half of SCTS against them.
#
# `make selftest` is the frames, the bring-up, the profile and every refusal, against a software
# slave stack in this process. It needs python3 and nothing else.
#
# `make check` drives the whole path an operator would: `skynet driver check` dlopens
# `drivers/python-host`, which forks an interpreter, which runs this bridge against a segment of
# simulated drives.
#
# ── What is not here, and why ──────────────────────────────────────────────
#
# **SOEM is not linked.** The plan's Phase 2 table names it, and SOEM 2.x is dual-licensed GPLv3 or
# commercial — so a driver in this Apache-2.0 repository cannot link it and still be distributed
# under this repository's licence. The master here is this repository's own, and SOEM's role is the
# one a licence does not restrict: an **independent master** run against this slave stack over an
# interface, which is interoperation rather than combination — `make check-soem` below. See README.md.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: How many simulated drives the conformance run brings up.
AXES ?= 3

#: `drivers/canopen` is on the path because the CiA 402 profile is imported from it rather than
#: written twice. CoE is CANopen's application layer on EtherCAT's transport, so the state machine,
#: the statusword decoding and the increments-to-radians arithmetic are the same profile — and two
#: implementations of one state machine disagree exactly once, after the failure that matters (§80).
PYTHONPATHS := $(REPO)/bindings/python:$(REPO)/drivers/canopen:.

.PHONY: all check selftest check-nic check-soem clean

all: check

# The protocol and the profile on their own. Every refusal has a control beside it.
selftest:
	PYTHONPATH=$(PYTHONPATHS) $(PYTHON) selftest.py

# The whole path, through the real ABI.
check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_ethercat \
	PYTHONPATH=$(PYTHONPATHS) \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so \
	  --config '{"sim":$(AXES),"joints":[{"name":"J1","role":"right_arm.shoulder.pitch"},{"name":"J2","role":"right_arm.elbow.flex"},{"name":"J3","role":"right_arm.wrist.roll"}]}'

# The same driver against a real EtherCAT segment on a NIC. Needs CAP_NET_RAW and something on the
# other end of the cable; it exits non-zero where there is nothing to run against, which is the
# answer §15 asks for rather than a quiet skip.
#
#   sudo make check-nic NIC=eth1
NIC ?=
check-nic:
	@if [ -z "$(NIC)" ]; then \
	  echo "::error::NIC is not set. This target puts real EtherCAT frames on a real interface:"; \
	  echo ""; \
	  echo "  sudo make check-nic NIC=eth1 AXES=3"; \
	  echo ""; \
	  echo "There is no fallback to the loopback segment on purpose: a master that quietly ran"; \
	  echo "against a list would report a ring that is not there."; \
	  exit 1; \
	fi
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_ethercat \
	PYTHONPATH=$(PYTHONPATHS) \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so \
	  --config '{"interface":"$(NIC)","joints":[{"name":"J1","role":"right_arm.shoulder.pitch"}]}'

# SOEM, an independent master, against this slave stack over a veth pair in a network namespace of
# its own — `DRY-COVERAGE-PLAN.md` F48's owed cross-check, run for the first time in its fourteenth
# session. SOEM is GPLv3-or-commercial and is not in this repository; build it and name the build
# directory. Needs Linux, `ip` and unprivileged user namespaces — no root and no NIC.
#
#   make check-soem SOEM=$$HOME/src/SOEM/build
SOEM ?=
check-soem:
	SOEM="$(SOEM)" PYTHON=$(PYTHON) DRIVES=$(AXES) sh ./soem.sh

clean:
	rm -rf skynet_ethercat/__pycache__ __pycache__
