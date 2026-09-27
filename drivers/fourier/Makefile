# skynet-fourier — a Fourier GR-1 humanoid over RoCS, and its substrate.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 7 item 3: the buyable humanoids after the Unitrees. The driver
# is Python (the wire is HTTP+WebSocket and `urllib`/`socket` are stdlib, with the one framing
# they lack in `wscodec.py`) and runs under `drivers/python-host`; the fake is the RoCS
# endpoints the driver speaks, on loopback HTTP+WebSocket. Python's tier ceiling is Guest
# (§31.3) and a humanoid commanded over a socket was never holding a real-time path anyway.
#
#   make check    the suite, the wire cross-check, `skynet driver check`, the fake's report
#   make suite    the driver's own suite on its own, for driving by hand
#
# `make check` needs the vendor's client repo (fftai/rocs_client_py), which the cross-check
# imports unmodified and which is not vendored here, plus `websocket-client` (its dependency)
# importable — an unpacked checkout passed as WSCLIENT, or installed:
#
#   git clone --depth 1 https://github.com/fftai/rocs_client_py /tmp/rocs_client_py
#   make check FOURIER_SDK=/tmp/rocs_client_py WSCLIENT=/tmp/websocket-client-1.9.0

PYTHON   ?= python3
CARGO    ?= cargo
REPO     := ../..
FOURIER_SDK ?=
WSCLIENT ?=

.PHONY: all check suite clean

all: check

# The suite on its own needs nothing but this tree: its fake RoCS runs on a thread inside the
# test process, over real loopback HTTP+WebSocket.
suite:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

# The whole path, through the real ABI: the fake RoCS as a process, the suite, the wire
# cross-check against the vendor's own client, then the conformance run against the driver —
# then the fake's own report of what the driver sent.
check:
	sh ./run-check.sh

clean:
	rm -rf __pycache__ skynet_fourier/__pycache__
