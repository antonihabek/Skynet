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

.PHONY: all test check chaos lib clean

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

# The link-chaos contract (12.C.4) on the port: two veth pairs with `tools/faultproxy`'s raw-Ethernet relay joining them, the Python slave stack
# on one side and this crate's shared object, called through its RHAL vtable, on the other.
chaos: lib
	CHAOS_DRIVER=$(abspath $(REPO))/target/release/libskynet_ethercat_rs.so PYTHONPATH=$(REPO)/bindings/python:../ethercat:../canopen sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	@true
