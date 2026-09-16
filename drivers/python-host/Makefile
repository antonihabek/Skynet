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

.PHONY: all check check-lerobot check-mujoco clean

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

clean:
	rm -f $(LIB) skynet_python_host.dll libskynet_python_host.dylib libskynet_python_host.so
