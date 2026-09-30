# skynet-ur-rtde-rs — a Universal Robots arm over RTDE, in Rust.
#
# A systems port of `drivers/ur_rtde`'s Python driver (Phase 6, item 6.6), which is the executable
# specification and was measured against URSim. Two targets, and the difference between them is the
# whole point, as it is for the Python driver:
#
#   make test    the wire, the program and the descriptor against vectors from the Python driver,
#                the unit tests beside the code, and the driver's own decisions (what a sample
#                becomes, what the stop writes, what close leaves) against a scripted controller on
#                127.0.0.1:29999, :30002 and :30004. Needs no URSim — and needs those three ports free
#   make check   the same, then the live test and the sim half of SCTS against URSim — UR's own
#                controller software — which SKYNET_UR_HOST names. Without it the second half is
#                **skipped by name**; with it the live test is required, never skipped.
#
#   make check-ursim-docker    # URSim at the pin in ../ur_rtde/ursim.mk, `check`, URSim removed
#
# or, against a URSim that is already up:
#
#   SKYNET_UR_HOST=127.0.0.1 make check

CARGO  ?= cargo
PYTHON ?= python3
REPO   := ../..

.PHONY: all test check check-ursim-docker lib clean

include ../ur_rtde/ursim.mk

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
	@# Not `scripted`: URSim is listening on the ports the scripted controller needs. It runs in `make check` with no host, on every push.
	SKYNET_UR_REQUIRE=1 $(CARGO) test --manifest-path $(REPO)/Cargo.toml -p skynet-ur-rtde-rs --lib --test vectors --test live_ursim
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(REPO)/target/release/libskynet_ur_rtde_rs.so \
	  --config '{"host":"$(SKYNET_UR_HOST)","simulator":true}'
endif

# `check` against a URSim this target starts and removes, as `drivers/ur_rtde`'s target of the same
# name does and for the same reason: the nightly substrate workflow (8.11) assumes no controller.
check-ursim-docker:
	docker rm -f $(URSIM_NAME) >/dev/null 2>&1 || true
	docker run -d --rm --name $(URSIM_NAME) -e ROBOT_MODEL=$(URSIM_MODEL) $(URSIM_PORTS) $(URSIM_IMAGE)
	@$(PYTHON) $(URSIM_WAIT) 127.0.0.1 || { docker logs $(URSIM_NAME) 2>&1 | tail -20; \
	  docker rm -f $(URSIM_NAME) >/dev/null 2>&1; exit 1; }
	$(MAKE) --no-print-directory check SKYNET_UR_HOST=127.0.0.1; \
	rc=$$?; docker rm -f $(URSIM_NAME) >/dev/null 2>&1; exit $$rc

clean:
	@true
