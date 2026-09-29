# skynet-cia402-rs — CiA 402's drive profile, shared by canopen-rs and ethercat-rs.
#
# A library, so there is no shared object to build and no substrate to run: `make check` is the crate's
# own tests. The comparison with the Python profile that this crate is a port of lives in
# `drivers/canopen-rs/tests/vectors.rs`, and runs under `make -C drivers/canopen-rs check`.

CARGO ?= cargo
REPO  := ../..

.PHONY: all check clean

all: check

check:
	$(CARGO) test --manifest-path $(REPO)/Cargo.toml -p skynet-cia402-rs

clean:
	@true
