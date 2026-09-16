# skynet-python-host — build the shared object, then drive a real Python driver through it.
#
# `make check` is the whole Python path end to end and it is the only place that path is exercised
# as an operator actually runs it: `skynet driver check` dlopens this shared object, which forks an
# interpreter, which serves a driver over a pipe. Everything before that point can be tested in
# Python; this step is what holds the C.
#
# No dependencies beyond a C11 compiler and whichever interpreter you point it at.

CC      ?= cc
# `-Werror=implicit-function-declaration` because GCC 14 made it an error and GCC 13 — what CI ran
# when this was written — only warns. A function used without its declaration is called with a
# guessed signature, and a build that is only broken on the newer compiler is broken on the next CI
# image rather than today.
CFLAGS  ?= -std=c11 -O2 -Wall -Wextra -Wpedantic -Werror=implicit-function-declaration -fPIC -I../../bindings/c
PYTHON  ?= python3
CARGO   ?= cargo

REPO    := ../..

ifeq ($(OS),Windows_NT)
  LIB := skynet_python_host.dll
  LDFLAGS ?= -shared
else
  UNAME_S := $(shell uname -s)
  ifeq ($(UNAME_S),Darwin)
    LIB := libskynet_python_host.dylib
    LDFLAGS ?= -dynamiclib
  else
    LIB := libskynet_python_host.so
    LDFLAGS ?= -shared
  endif
endif

.PHONY: all check check-lerobot check-mujoco check-capture check-loop check-offline clean

all: $(LIB)

$(LIB): python_host.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ python_host.c

# The default check is the LeRobot bridge against its fake, because it needs nothing installed. The
# fake is opt-in through the environment rather than a fallback: a driver that silently substitutes
# a simulated body for a missing real one will one day report a grasp that never happened.
check: check-lerobot

check-lerobot: $(LIB)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_lerobot \
	SKYNET_LEROBOT_FAKE=1 \
	PYTHONPATH=$(REPO)/bindings/python:$(REPO)/drivers/lerobot \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- driver check ./$(LIB)

# The MuJoCo simulator driver through the same host. Needs MuJoCo installed, and needs an OpenGL
# context for the camera to be declared — without one the driver declares no sensors, which is the
# correct behaviour and is what `driver check` will then report.
check-mujoco: $(LIB)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_mujoco \
	SKYNET_MUJOCO_SCENE=$(abspath $(REPO)/drivers/mujoco/scenes/so_arm_shaped.xml) \
	PYTHONPATH=$(REPO)/bindings/python:$(REPO)/drivers/mujoco \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- driver check ./$(LIB)

# §47.2's fourth backend through the real ABI: a camera the driver renders, read with `read_sensor`
# by the control loop and published into §47.6's pool. The LeRobot fake has a 32x24 RGB wrist
# camera — 2,304 bytes a frame — so the pool is declared with slots of exactly that.
#
# Nothing connects to the socket, and nothing needs to: the pool is filled whether or not a policy
# has mapped it, which is §47.4's rule that capture is never stalled by a consumer. What this checks
# is the half before the hand-off — frames read through the C shim, over the pipe, into the region —
# and it fails unless at least one frame was published. `skynetk` exits non-zero on its own if any
# sensor read failed.
CAPTURE_OUT := $(or $(TMPDIR),/tmp)/skynetk-capture
check-capture: $(LIB)
	rm -f $(CAPTURE_OUT).sock
	SKYNET_PYTHON=$(PYTHON) 	SKYNET_PYTHON_DRIVER=skynet_lerobot 	SKYNET_LEROBOT_FAKE=1 	PYTHONPATH=$(REPO)/bindings/python:$(REPO)/drivers/lerobot 	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-kernel-host --bin skynetk -- 		run --driver ./$(LIB) --hz 50 --cycles 50 --policy-socket $(CAPTURE_OUT).sock 		--frame-pool wrist:2x2304@10 > $(CAPTURE_OUT).txt
	cat $(CAPTURE_OUT).txt
	grep -Eq '^published +[1-9][0-9]* to the pool' $(CAPTURE_OUT).txt
	rm -f $(CAPTURE_OUT).sock

# The loop, closed on that camera. `check-capture` above is the producer half with nothing reading the
# pool; this starts `skynetp` beside it running §23.3's inference backend, so a frame the driver
# rendered becomes a setpoint the kernel's guards evaluate. It is DRY-COVERAGE-PLAN.md's Phase 1 gate,
# and `loop.sh` is where the assertions and the reason for each of them are written down.
#
# A script rather than a recipe, because it starts two processes and has to wait for a socket between
# them, and `make` gives each recipe line a shell of its own.
check-loop: $(LIB)
	LIB=$(LIB) PYTHON=$(PYTHON) sh ./loop.sh
# The same loop with no network at all, in a namespace holding only loopback. §11 says no control
# decision may require a network round trip, and §15 makes that a bench test because it needs a machine
# rather than a model — this is the machine. It is not a recorded bench *result*: nothing in this
# workspace produces one, which `loop.sh` says at the end of a run rather than leaving to be assumed.
check-offline: $(LIB)
	SKYNET_LOOP_OFFLINE=1 LIB=$(LIB) PYTHON=$(PYTHON) sh ./loop.sh

clean:
	rm -f $(LIB) skynet_python_host.dll libskynet_python_host.dylib libskynet_python_host.so
