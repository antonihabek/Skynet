# The pendant robot: a body behind a program-level controller (§59.2), and its own suite.
#
# `drivers/mock` is the teaching driver for the machines this design was written for. This is the
# teaching driver for the other kind — the Fanuc, KUKA, ABB and older Universal Robots controllers
# that own their own servo loop and take a *program* rather than a setpoint. Everything here is a
# refusal or the control for one, because for this class of machine the refusals are the interface.
#
# No dependencies beyond a C11 compiler, as everywhere else under `drivers/`.

CC      ?= cc
CFLAGS  ?= -std=c11 -O2 -Wall -Wextra -Wpedantic -fPIC -I../../bindings/c

ifeq ($(OS),Windows_NT)
  LIB := skynet_pendant.dll
  LDFLAGS ?= -shared
else
  UNAME_S := $(shell uname -s)
  ifeq ($(UNAME_S),Darwin)
    LIB := libskynet_pendant.dylib
    LDFLAGS ?= -dynamiclib
  else
    LIB := libskynet_pendant.so
    LDFLAGS ?= -shared
  endif
endif

.PHONY: all lib check check-conformance clean

all: selftest $(LIB)

lib: $(LIB)

$(LIB): pendant_robot.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ pendant_robot.c -lm

selftest: pendant_robot.c selftest.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) -o $@ pendant_robot.c selftest.c -lm

check: selftest
	./selftest

# The sim half of SCTS against this body, which is the point of having it: the report is what the
# design says today about a robot it cannot command, and it is not a pass.
check-conformance: $(LIB)
	cargo run --manifest-path ../../Cargo.toml -q -p skynet-cli -- driver check ./$(LIB) || true

clean:
	rm -f selftest selftest.exe $(LIB) skynet_pendant.dll libskynet_pendant.so libskynet_pendant.dylib
