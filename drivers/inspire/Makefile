# skynet-inspire — an Inspire RH56 dexterous hand over its private serial protocol, and its suite.
#
#   make selftest          the frames, every refusal, the exchange cycle, the stop, the faults and the line, against a simulated hand on a pseudo-terminal. Needs python3 on Linux
#   make check             the whole path: skynet driver check → python-host → this bridge → a simulated hand in the same process
#   make check-layout      this driver's bytes against two independent clients of the same protocol, which are run (INSPIRE_PYTHON=a python with pyrh56, inspire-rh56dftp and pyserial installed)
#   make chaos             the link-chaos contract: the hand's serial line through a relay that delays, stalls and blackholes it
#   make faultvocab        the fault-vocabulary contract: what the driver makes of the maker's own error bits and status codes
#   make vocab             regenerate the maker's error bits and status codes in drivers/fault-vocabulary.json from the two manuals (INSPIRE_SERIES_PDF, INSPIRE_DFTP_PDF)
#
# DRY-COVERAGE-PLAN.md item 12.E.8. There is no maker's simulator or maker's client for this hand: the two clients `check-layout` runs are open-source transcriptions of the manual by other people, and the hand on
# the other end of the line is written in this repository, so the release stage is alpha.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: A Python with `pip install --require-hashes -r requirements-layout.txt` done, for `check-layout`.
INSPIRE_PYTHON ?=
#: The two manuals, for `vocab` (the script checks their SHA-256).
INSPIRE_SERIES_PDF ?=
INSPIRE_DFTP_PDF ?=

.PHONY: all check selftest check-layout chaos faultvocab vocab clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_inspire \
	SKYNET_INSPIRE_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

check-layout:
	@test -n "$(INSPIRE_PYTHON)" || { echo "::error::INSPIRE_PYTHON names no Python with pyrh56 and inspire-rh56dftp installed (pip install --require-hashes -r requirements-layout.txt). Nothing was compared."; exit 1; }
	PYTHONPATH=$(REPO)/bindings/python:. $(INSPIRE_PYTHON) layout_check.py

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the hand's serial line through a relay that delays, stalls and blackholes it (`tools/faultproxy`). In a network namespace of its own, so the relay's
# loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

# The fault-vocabulary contract (DRY-COVERAGE-PLAN.md item 12.C.6): the maker's own error bits and status codes from drivers/fault-vocabulary.json, injected into the simulated hand and held to the class the table gives each.
faultvocab:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) $(REPO)/tools/fault-vocab/contract.py faultvocab.py

# Reads the manuals' own tables (pdftotext, from poppler-utils) and writes the `inspire` entry. Run to check, with no --write, that the committed entry is what the manuals say.
vocab:
	@test -n "$(INSPIRE_SERIES_PDF)" && test -n "$(INSPIRE_DFTP_PDF)" || { echo "::error::INSPIRE_SERIES_PDF and INSPIRE_DFTP_PDF name no manuals. Nothing was read."; exit 1; }
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) vocab_gen.py "$(INSPIRE_SERIES_PDF)" "$(INSPIRE_DFTP_PDF)"

clean:
	rm -rf skynet_inspire/__pycache__ __pycache__
