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

.PHONY: all lib check check-lerobot check-mujoco check-concurrent check-capture check-loop check-loop-mujoco check-loop-onnx check-offline check-extension check-frame check-stop clean

all: $(LIB)

# The name the wizard's refusals and packaging/bundle.sh use for "build the shared object".
lib: $(LIB)

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

# §89, through the host rather than through a shared object: three runs of `skynet driver check`
# against `fixtures/skynet_slow_camera.py`, whose camera is slower than a control period and whose
# descriptor says one of three things about it.
#
# This is what protocol 2 is for and the only place it is measured. A declared sensor's frames go
# down a second pipe, so `read_sensor` on one thread and `read_state` on another never share a byte
# — and with that second pipe taken away the middle run below fails, which is how it was held to a
# control while it was written.
#
# Needs nothing installed.
check-concurrent: $(LIB)
	@set -e; \
	probe() { SKYNET_PYTHON=$(PYTHON) SKYNET_PYTHON_DRIVER=skynet_slow_camera \
	  PYTHONPATH=$(REPO)/bindings/python:fixtures \
	  $(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- driver check ./$(LIB) --config "$$1" 2>&1 || true; }; \
	expect() { echo "== $$1"; probe "$$1" | grep -E '89\.4|concurrent_read' | tee /dev/stderr | grep -Eq "$$2" \
	  || { echo "::error::$$1 did not give §89.4's check the verdict /$$2/. Its output is above."; exit 1; }; }; \
	expect '{"render_ms":2}' 'n/a.*no sensor declares concurrent_read'; \
	expect '{"render_ms":2,"concurrent":true}' 'pass.*reads concurrently'; \
	expect '{"render_ms":2,"lie":true}' 'FAIL.*reads concurrently'
	@echo ""
	@echo "The middle one is the claim: a Python driver can make §89.1's promise and this check"
	@echo "exercises it. The third is the control — the same promise, and a driver that breaks it."
	@echo "Each is asserted: this target used to end in || true and could not fail (DRY-COVERAGE-PLAN.md 12.A.5)."

# The `T` verb end to end, twice: once with a child that implements `extension` (echo,
# refusals, an exploding driver, a too-small buffer — and the stream still in sync after all of
# it), and once with a child from before `T`, whose bare `R EPROTO` must read as ENOSYS rather
# than a protocol error. The check program dlopens this host, the way the OS does.
check-extension: $(LIB) fixtures/extension_check
	@echo "== a child that implements extension"
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_t_extension \
	PYTHONPATH=$(REPO)/bindings/python:fixtures \
	./fixtures/extension_check ./$(LIB)
	@echo ""
	@echo "== a child from before T"
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_v2_child \
	PYTHONPATH=$(REPO)/bindings/python:fixtures \
	./fixtures/extension_check ./$(LIB)

fixtures/extension_check: fixtures/extension_check.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) -o $@ fixtures/extension_check.c -ldl

# F234: a stop written to the pipe while an extension runs. `fixtures/skynet_t_stoppable.py` runs an extension for 1.5 s, as a streamed motion does, and `stop_check` asks the host to stop it
# 0.4 s in. A child that says its stop is safe from another thread (`stop_during_extension`) must have the extension cut short and the stream stay in sync; a child that does not say so
# gets what every child got before, which the check says out loud rather than hiding. Needs nothing installed.
check-stop: $(LIB) fixtures/stop_check
	@echo "== a child that says its stop is safe during an extension"
	SKYNET_FIXTURE_DECLARES=1 \
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_t_stoppable \
	PYTHONPATH=$(REPO)/bindings/python:fixtures \
	./fixtures/stop_check ./$(LIB) 1
	@echo ""
	@echo "== a child that has not said so"
	SKYNET_FIXTURE_DECLARES=0 \
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_t_stoppable \
	PYTHONPATH=$(REPO)/bindings/python:fixtures \
	./fixtures/stop_check ./$(LIB) 0

fixtures/stop_check: fixtures/stop_check.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) -D_POSIX_C_SOURCE=200809L -o $@ fixtures/stop_check.c -ldl -lpthread

# The `F` verb's oversize half, end to end: `fixtures/skynet_big_frame.py` answers 100,000 bytes
# for one sensor and 100 for another, and `frame_check` dlopens this host the way the OS does. The
# big read must arrive as `EINVAL` — refused before it is framed, with no payload line — and a
# small read, a `read_state` and a `describe` straight after it must still work, which is the
# stream staying in sync (DRY-COVERAGE-PLAN.md F172). Needs nothing installed.
check-frame: $(LIB) fixtures/frame_check
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_big_frame \
	PYTHONPATH=$(REPO)/bindings/python:fixtures \
	./fixtures/frame_check ./$(LIB)

fixtures/frame_check: fixtures/frame_check.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) -o $@ fixtures/frame_check.c -ldl

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

# The same gate with a physics simulator on the other side of the same ABI, rendering its own wrist
# camera offscreen rather than drawing a pattern. DRY-COVERAGE-PLAN.md's fifth session recorded the
# LeRobot fake as a departure from the gate as written and said joining MuJoCo to it was "one line in
# that Makefile on a machine with it" — it was this line plus the body block in `loop.sh`, because
# the camera's geometry is on `skynetk`'s command line and in the model, and the two are deliberately
# separate statements (F16).
#
# Needs `pip install mujoco` and an offscreen GL: MUJOCO_GL=osmesa with libosmesa6 installed, or egl.
#
# It is also §89's measurement. The render costs more than the period in software (F24), and the
# driver renders on a thread of its own and declares `concurrent_read`, so the camera is read on
# `skynetk`'s capture thread and the gate's zero-overrun assertion is the section working. Before the
# declaration this gate failed with one overrun per frame read; with §89.2 alone, three and a
# suspended stream.
check-loop-mujoco: $(LIB)
	BODY=mujoco LIB=$(LIB) PYTHON=$(PYTHON) sh ./loop.sh

# The same gate with §23.3's own runner in place of `skynet-infer`'s dense one: the model is an ONNX
# graph and `skynetp` drives it through `skynet-ort`, which dlopens ONNX Runtime. Same weights, same
# assertions, same numbers — the runner is the only thing that changes, which is what makes the two
# comparable at all.
#
# DRY-COVERAGE-PLAN.md's seventh session called this "one line in that Makefile on a Linux machine
# with ONNX Runtime", and it was: everything else was already built and nobody had a machine with
# both a Linux loop and the library on it.
#
# Needs `pip install onnx numpy` to write the graph and `onnxruntime` for the library. The script
# finds the wheel's `libonnxruntime.so.*` itself, because a pip install ships no unversioned
# `libonnxruntime.so` for the system loader to resolve; `SKYNET_ORT_LIBRARY` overrides it.
check-loop-onnx: $(LIB)
	POLICY=onnx LIB=$(LIB) PYTHON=$(PYTHON) sh ./loop.sh
# The same loop with no network at all, in a namespace holding only loopback. §11 says no control
# decision may require a network round trip, and §15 makes that a bench test because it needs a machine
# rather than a model — this is the machine. It is not a recorded bench *result*: nothing in this
# workspace produces one, which `loop.sh` says at the end of a run rather than leaving to be assumed.
check-offline: $(LIB)
	SKYNET_LOOP_OFFLINE=1 LIB=$(LIB) PYTHON=$(PYTHON) sh ./loop.sh

clean:
	rm -f $(LIB) skynet_python_host.dll libskynet_python_host.dylib libskynet_python_host.so
