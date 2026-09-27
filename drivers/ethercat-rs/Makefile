# skynet-ethercat-rs — CiA 402 drives on an EtherCAT segment, in Rust.
#
# A systems port of `drivers/ethercat`'s Python driver (Phase 6, item 6.6), which is the executable
# specification and has been cross-checked against SOEM.
#
#   make test    the frames against vectors from `frames.py`, and the unit tests. Needs nothing
#   make check   those, then the live test and the sim half of SCTS against the Python slave stack
#                over a veth pair in an unprivileged namespace (`check.sh`). Needs Linux, `ip` and
#                user namespaces — no root, no NIC — and fails rather than skips without them.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..

.PHONY: all test check lib clean

all: check

test:
	$(CARGO) test --manifest-path $(REPO)/Cargo.toml -p skynet-ethercat-rs

# Built outside the namespace, so nothing inside it compiles.
lib:
	$(CARGO) build --manifest-path $(REPO)/Cargo.toml --release -p skynet-ethercat-rs
	$(CARGO) build --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli
	$(CARGO) test --manifest-path $(REPO)/Cargo.toml -q -p skynet-ethercat-rs --no-run

check: lib
	PYTHON=$(PYTHON) CARGO=$(CARGO) sh ./check.sh

clean:
	@true
