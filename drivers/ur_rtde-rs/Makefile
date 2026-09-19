# skynet-ur-rtde-rs — a Universal Robots arm over RTDE, in Rust.
#
# A systems port of `drivers/ur_rtde`'s Python driver (Phase 6, item 6.6), which is the executable
# specification and was measured against URSim. Two targets, and the difference between them is the
# whole point, as it is for the Python driver:
#
#   make test    the wire, the program and the descriptor against vectors from the Python driver,
#                and the unit tests beside the code. Needs no controller
#   make check   the same, then the live test and the sim half of SCTS against URSim — UR's own
#                controller software — which SKYNET_UR_HOST names. Without it the second half is
#                **skipped by name**; with it the live test is required, never skipped.
#
#   docker run -d --name ursim -p 29999:29999 -p 30001:30001 -p 30002:30002 \
#     -p 30003:30003 -p 30004:30004 universalrobots/ursim_e-series:latest
#   SKYNET_UR_HOST=127.0.0.1 make check

CARGO ?= cargo
REPO  := ../..

.PHONY: all test check lib clean

all: test

test:
	$(CARGO) test --manifest-path $(REPO)/Cargo.toml -p skynet-ur-rtde-rs

lib:
	$(CARGO) build --manifest-path $(REPO)/Cargo.toml --release -p skynet-ur-rtde-rs

check: lib
ifndef SKYNET_UR_HOST
	$(MAKE) --no-print-directory test
	@echo ""
	@echo "SKYNET_UR_HOST is not set, so only the wire format ran. See this Makefile's header."
else
	SKYNET_UR_REQUIRE=1 $(CARGO) test --manifest-path $(REPO)/Cargo.toml -p skynet-ur-rtde-rs
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(REPO)/target/release/libskynet_ur_rtde_rs.so \
	  --config '{"host":"$(SKYNET_UR_HOST)","simulator":true}'
endif

clean:
	@true
