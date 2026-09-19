# skynet-canopen-rs — CiA 402 drives on a CANopen bus, in Rust.
#
# A systems port of `drivers/canopen`'s Python driver (Phase 6, item 6.6), which is the executable
# specification. Two measurement layers and the conformance run:
#
#   cargo test -p skynet-canopen-rs   vectors.json (frames, profile, units and JSON rendering, all
#                                     generated from the Python modules), the unit tests beside the
#                                     code, and the live half — which needs a kernel CAN bus.
#   make check                        all of that on `vcan0`, with the live half *required*, then
#                                     `skynet driver check` (the sim half of SCTS) against the
#                                     release .so, with the Python drives serving on the same bus.
#
# `vcan0` comes from `drivers/canopen/vcan.sh`: the machine's own, or one made in an unprivileged
# network namespace with no root. Where neither is possible `make check` fails rather than
# reporting a pass it did not measure.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..

.PHONY: all check lib clean

all: check

# Built outside the namespace, so nothing inside it compiles.
lib:
	$(CARGO) build --manifest-path $(REPO)/Cargo.toml --release -p skynet-canopen-rs
	$(CARGO) build --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli
	$(CARGO) test --manifest-path $(REPO)/Cargo.toml -q -p skynet-canopen-rs --no-run

check: lib
	PYTHON=$(PYTHON) CARGO=$(CARGO) sh ../canopen/vcan.sh sh ./check.sh

clean:
	rm -rf .drives
