# skynet-pudu — a Pudu service robot over the vendor's OpenAPI cloud, and its substrate.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 7 item 4: one service-robot driver on the execute path. The driver
# is Python (the cloud is HTTPS and `urllib` is stdlib) and runs under `drivers/python-host`; the
# fake is the Pudu OpenAPI endpoints the driver speaks, on loopback HTTP, verifying every
# signature. Python's tier ceiling is Guest (§31.3) and the service class is Guest ceiling anyway.
#
#   make check    the suite, the signing cross-check, `skynet driver check`, the fake's report
#   make suite    the driver's own suite on its own, for driving by hand
#
# `make check` needs the vendor's skill repo (pudu-robotics/skills), which the cross-check imports
# unmodified and which is not vendored here:
#
#   git clone --depth 1 https://github.com/pudu-robotics/skills /tmp/pudu_skills
#   make check PUDU_SKILL=/tmp/pudu_skills

PYTHON   ?= python3
CARGO    ?= cargo
REPO     := ../..
PUDU_SKILL ?=

.PHONY: all check suite chaos clean

all: check

# The suite on its own needs nothing but this tree: its fake cloud runs on a thread inside the
# test process, over real loopback HTTP.
suite:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The whole path, through the real ABI: the fake cloud as a process, the suite, the signing
# cross-check against the vendor's own script, then the conformance run against the driver —
# then the fake's own report of what the driver sent.
check:
	sh ./run-check.sh

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the cloud's HTTP flow through a relay that delays, stalls and blackholes it (`tools/faultproxy`), in a network namespace of its own.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf __pycache__ skynet_pudu/__pycache__
