# The MuJoCo bridge is Python; there is nothing to compile. `make check` runs the sim conformance
# harness against the bundled scene, which needs MuJoCo installed and nothing else.

PYTHON ?= python3

check:
	$(PYTHON) selftest.py

# The same driver, loaded by the OS through drivers/python-host, which is the thing an operator
# actually does. Needs a C compiler for the host.
host-check:
	$(MAKE) -C ../python-host check PYTHON=$(PYTHON)

.PHONY: check host-check
