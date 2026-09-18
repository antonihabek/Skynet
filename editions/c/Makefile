# The C edition (§31.3, §42 level L2 — Contracts).
#
#   make            # build the corpus runner, the round trip and the self-check
#   make corpus     # build and run the conformance corpus
#   make test       # build and run the canonical-JSON round trip and the self-check
#   make fusion     # the fused multiply-add scan
#   make check      # everything, in the order a CI step wants it
#   make clean
#
# Warnings are errors, and that is the gate rather than a preference. §6.1 of the coding standard
# ranks the compiler second only to making a violation unrepresentable, and in an edition whose whole
# job is byte-identical output an implicit conversion is a wrong number rather than a style question.
# C needs three warnings C++ does not: a function declared without a prototype, defined without one,
# or defined in the old style is how a C translation unit calls something with the wrong arguments
# and links anyway.

# `?=` cannot set CC: make defines it before this file is read, so the assignment never happens and
# the built-in wins. A value the user gave on the command line or in the environment is respected.
ifeq ($(origin CC),default)
CC := gcc
endif
CFLAGS ?= -std=c17 -O2 -Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror \
          -Wstrict-prototypes -Wmissing-prototypes -Wold-style-definition -Wvla
# POSIX, for `dirent.h` and `getcwd`: `-std=c17` is strict ISO C, under which those declarations are
# hidden and the calls would compile against implicit ones. Requested by name rather than by relaxing
# the standard to `gnu17`, so the only extension this edition uses is the one it asked for.
CPPFLAGS ?= -D_POSIX_C_SOURCE=200809L
INCLUDES := -Iinclude

# Floating-point contraction is off, and that is a property of the format rather than a preference.
# Wherever the target has a fused multiply-add — every aarch64, a Jetson included, and x86-64 from v3
# — `x*y + z` can become one instruction that does not round `x*y`. §31.2 makes the kernel's sin, cos
# and sqrt a fixed sequence of correctly rounded operations so that every edition computes the same
# bits, and a fused operation is a different sequence. GCC's C default permits contraction (`fast`
# outside ISO mode, `on` inside it, and both fuse), so the flag is explicit, and it lives in a
# variable of its own that every compile uses so an override of CFLAGS cannot drop it. The `fusion`
# target is what makes it a gate rather than a line somebody could remove.
FP_FLAGS := -ffp-contract=off
BUILD := build
OBJ := $(BUILD)/obj

# Statically linked so the binary runs wherever it is copied. An edition's runner that needs its
# compiler's runtime installed alongside it is one a vendor cannot use to check their own build.
LDFLAGS ?= -static
LDLIBS ?= -lm

EXE := $(if $(filter Windows_NT,$(OS)),.exe,)

# Derived from the tree, never listed: a new translation unit is in the build because it exists. The
# three programs each have their own `main`, so they are filtered out of the shared object list and
# linked in one at a time.
PROGRAM_SOURCES := src/main.c src/roundtrip.c src/selfcheck.c
SOURCES := $(filter-out $(PROGRAM_SOURCES),$(wildcard src/*.c)) $(wildcard src/contracts/*.c)
OBJECTS := $(patsubst src/%.c,$(OBJ)/%.o,$(SOURCES))

RUNNER := $(BUILD)/skynet-corpus-c$(EXE)
ROUNDTRIP := $(BUILD)/skynet-roundtrip-c$(EXE)
SELFCHECK := $(BUILD)/skynet-selfcheck-c$(EXE)

# Where the corpus is. Overridable so a vendor can point the runner at a pinned copy.
CORPUS ?= ../../corpus

# The ratchet. This is the number of cases this edition has reached; see `--min-passed` in
# `src/main.c` for why a floor is checked separately from the failures. Lowering it is a decision
# somebody makes in a diff.
MIN_PASSED ?= 383

.PHONY: all corpus test fusion check clean

all: $(RUNNER) $(ROUNDTRIP) $(SELFCHECK)

# One object per translation unit, with the headers each one includes recorded by the compiler
# (`-MMD -MP`) rather than listed here, so a generated table included from a `.c` is a dependency
# because the compiler read it.
$(OBJ)/%.o: src/%.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(FP_FLAGS) $(INCLUDES) -MMD -MP -c $< -o $@

$(RUNNER): $(OBJECTS) $(OBJ)/main.o
	$(CC) $(CFLAGS) $(FP_FLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

$(ROUNDTRIP): $(OBJECTS) $(OBJ)/roundtrip.o
	$(CC) $(CFLAGS) $(FP_FLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

$(SELFCHECK): $(OBJECTS) $(OBJ)/selfcheck.o
	$(CC) $(CFLAGS) $(FP_FLAGS) -o $@ $^ $(LDFLAGS) $(LDLIBS)

corpus: $(RUNNER)
	$(RUNNER) --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

test: $(ROUNDTRIP) $(SELFCHECK)
	$(ROUNDTRIP) --corpus-root $(CORPUS)
	$(SELFCHECK) --corpus-root $(CORPUS)

# ── The fused multiply-add scan ───────────────────────────────────────────────
#
# FP_FLAGS says contraction is off; this is what checks the compiler agreed. Every translation unit is
# compiled to assembly with a fused multiply-add *available* — the flag that makes one exist on this
# host's architecture — and the step fails on any fused instruction in the listings. The probe is
# compiled the same way without FP_FLAGS and must fuse, so a host where the scan cannot see an FMA (an
# architecture this does not know, a compiler that ignored the flag) fails rather than passing by not
# looking. That positive control is finding 171, and it is why an empty result here is evidence.
#
# aarch64 has FMA in its base instruction set and needs no flag; x86-64 needs -mfma.
FMA_TARGET := $(if $(findstring x86_64,$(shell $(CC) -dumpmachine)),-mfma,)
FUSION := $(BUILD)/fusion
FUSION_LISTINGS := $(patsubst src/%.c,$(FUSION)/%.s,$(SOURCES) $(PROGRAM_SOURCES))
# vfmadd231sd and the rest of the VEX family on x86-64; fmadd, fmsub, fnmadd, fnmsub on aarch64.
FUSED_MNEMONIC := '(^|[[:space:]])v?fn?m(add|sub)[0-9a-z]*[[:space:]]'

$(FUSION)/%.s: src/%.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) $(CPPFLAGS) $(FP_FLAGS) $(FMA_TARGET) $(INCLUDES) -MMD -MP -S $< -o $@

# The probe is compiled with contraction explicitly *on*, which is the one place in this Makefile
# where it is. Leaving it at the default would not have proved anything on this host: GCC's default
# under `-std=c17` is `-ffp-contract=on`, and GCC 13 implements `on` as `off`, so the probe emitted no
# fused instruction and the scan could not have seen one in the edition either. GCC 14 implements `on`
# as contraction within an expression and does fuse. So a C edition built with `-std=c17` on GCC 13
# does not contract whatever this file says, and the same source on GCC 14 does — which is exactly
# why FP_FLAGS is stated rather than inherited, and why the control is `fast` rather than the default.
$(FUSION)/probe.s: tests/fusion_probe.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) $(CPPFLAGS) -ffp-contract=fast $(FMA_TARGET) -S $< -o $@

fusion: $(FUSION_LISTINGS) $(FUSION)/probe.s
	@echo "== fused multiply-add scan ($(or $(FMA_TARGET),no flag): $(words $(FUSION_LISTINGS)) listings)"
	@if ! grep -qiE $(FUSED_MNEMONIC) $(FUSION)/probe.s; then \
		echo "the probe did not fuse, so this scan cannot see a fused instruction on this host"; exit 1; fi
	@if grep -ciE $(FUSED_MNEMONIC) $(FUSION_LISTINGS) | grep -v ':0$$'; then \
		echo "fused multiply-adds above: a product fused into an add is a different sequence of"; \
		echo "roundings from the reference's, and section 31.2 pins the bits (see FP_FLAGS)"; exit 1; fi
	@echo "none, and the probe fused"

check: test fusion corpus

clean:
	rm -rf $(BUILD)

-include $(OBJECTS:.o=.d) $(patsubst src/%.c,$(OBJ)/%.d,$(PROGRAM_SOURCES))
-include $(FUSION_LISTINGS:.s=.d)
