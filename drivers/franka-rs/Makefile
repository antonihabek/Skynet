# skynet-franka-rs — a Franka arm over the FCI, in Rust.
#
# A systems port of `drivers/franka`'s Python driver (Phase 6, item 6.6), which is the executable
# specification. Three measurement layers, each naming what it holds the port to:
#
#   cargo test -p skynet-franka-rs   layout.json (the vendor's compiler), vectors.json (the
#                                    Python driver's own wire bytes), and the live fake Control
#                                    box over real sockets — plus the unit tests beside the code.
#   make check                       all of that, then `skynet driver check` (the sim half of
#                                    SCTS) against the release .so, with a fake this target starts
#                                    and stops itself.
#
# It needs python3 for the live half and says so by name when it is absent: the pure tests run
# regardless, and the live test skips loudly rather than passing quietly.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..

.PHONY: all check chaos lib clean

all: check

lib:
	$(CARGO) build --manifest-path $(REPO)/Cargo.toml --release -p skynet-franka-rs

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4) on the port: the same fake Control box as `drivers/franka`, with this crate's shared object called through its RHAL vtable, and the kernel's own
# link emulation (`tc netem`) on the loopback of a network namespace they share, because the FCI's state stream goes to a UDP port the driver announces in `Connect` and no relay can stand in front of it.
chaos: lib
	PYTHONPATH=$(REPO)/bindings/python:../franka CHAOS_DRIVER=$(abspath $(REPO))/target/release/libskynet_franka_rs.so sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

check: lib
	$(CARGO) test --manifest-path $(REPO)/Cargo.toml -p skynet-franka-rs
	@rm -f .fake-port .fake-pid; \
	PYTHONPATH=../franka:$(REPO)/bindings/python $(PYTHON) -c \
	  "from skynet_franka.sim import FakeControlBox; import threading; \
	   box = FakeControlBox(hz=500.0); \
	   open('.fake-port', 'w').write(str(box.start())); threading.Event().wait()" & \
	echo $$! > .fake-pid; \
	for i in $$(seq 1 50); do [ -s .fake-port ] && break; sleep 0.2; done; \
	if [ ! -s .fake-port ]; then echo "::error::the fake Control box did not start"; kill $$(cat .fake-pid) 2>/dev/null; rm -f .fake-port .fake-pid; exit 1; fi; \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(REPO)/target/release/libskynet_franka_rs.so \
	  --config "{\"port\": $$(cat .fake-port), \"simulator\": true}"; \
	rc=$$?; kill $$(cat .fake-pid) 2>/dev/null; rm -f .fake-port .fake-pid; exit $$rc

clean:
	rm -f .fake-port .fake-pid
