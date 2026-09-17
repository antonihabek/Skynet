# The C++ edition (§31.3, §42 level L2 — Contracts).
#
#   make            # build the corpus runner and the tests
#   make corpus     # build and run the conformance corpus
#   make test       # build and run the canonical-JSON round trip and the self-check
#   make check      # everything, in the order a CI step wants it
#   make clean
#
# Warnings are errors, and that is the gate rather than a preference. §6.1 of the coding standard
# ranks "compiler, warnings-as-errors" second only to making a violation unrepresentable, and an
# edition whose whole job is byte-identical output is one where an implicit conversion is a wrong
# number rather than a style question.

# `?=` cannot set CXX: make defines it before this file is read, so the assignment never happens
# and the built-in wins. Most makes default to `g++` and nobody notices; the MinGW make shipped with
# Strawberry Perl defaults to the absolute path of the machine it was built on, which exits 127 here.
# A value the user gave on the command line or in the environment is still respected.
ifeq ($(origin CXX),default)
CXX := g++
endif
CXXFLAGS ?= -std=c++20 -O2 -Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror
INCLUDES := -Iinclude

# Floating-point contraction is off, and that is a property of the format rather than a preference.
# GCC's default for C++ is -ffp-contract=fast: wherever the target has a fused multiply-add — every
# aarch64, a Jetson included, and x86-64 from v3 — `x*y + z` becomes one instruction that does not
# round `x*y`. §31.2 makes the kernel's sin, cos and sqrt a fixed sequence of correctly rounded
# operations so that every edition computes the same bits, and a fused operation is a different
# sequence: on 2026-09-11 kinematics.cpp compiled with FMA available held ninety of them, and passed
# the corpus only because baseline x86-64 has no FMA instruction to contract into. The flag lives in
# a variable of its own that every compile uses, so an override of CXXFLAGS cannot drop it.
FP_FLAGS := -ffp-contract=off
BUILD := build
OBJ := $(BUILD)/obj

# Statically linked so the binary runs wherever it is copied. An edition's runner that needs its
# compiler's runtime installed alongside it is one a vendor cannot use to check their own build.
LDFLAGS ?= -static

# The linker names its output `.exe` on Windows whatever `-o` said, and a target whose file never
# appears under its own name is rebuilt on every run.
EXE := $(if $(filter Windows_NT,$(OS)),.exe,)

# Derived from the tree, never listed. The list this replaced named six files by hand and had missed
# four — `js`, `kinematics`, `sha256` and `embodiment` — so `make` could not link, and the edition
# was being built by a command nobody had written down. A new translation unit is now in the build
# because it exists.
SOURCES := $(filter-out src/run_corpus.cpp,$(wildcard src/*.cpp)) $(wildcard src/contracts/*.cpp)
OBJECTS := $(patsubst src/%.cpp,$(OBJ)/%.o,$(SOURCES))

RUNNER := $(BUILD)/skynet-corpus-cpp$(EXE)
ROUNDTRIP := $(BUILD)/json_roundtrip$(EXE)
SELFCHECK := $(BUILD)/self_check$(EXE)

# Where the corpus is. Overridable so a vendor can point the runner at a pinned copy.
CORPUS ?= ../../corpus

# The ratchet. This is the number of cases this edition has reached; see `--min-passed` in
# `src/run_corpus.cpp` for why a floor is checked separately from the failures. Lowering it is a
# decision somebody makes in a diff.
MIN_PASSED ?= 1210

.PHONY: all corpus test fusion check clean

all: $(RUNNER) $(ROUNDTRIP) $(SELFCHECK)

# One object per translation unit, with the headers each one includes recorded by the compiler
# (`-MMD -MP`) rather than listed here. The single command this replaced rebuilt all thirty-five
# units on any change, and depended on the headers only because somebody had written them into the
# rule; a generated table included from a `.cpp` is now a dependency because the compiler read it.
$(OBJ)/%.o: src/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $(FP_FLAGS) $(INCLUDES) -MMD -MP -c $< -o $@

$(OBJ)/tests/%.o: tests/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $(FP_FLAGS) $(INCLUDES) -MMD -MP -c $< -o $@

$(RUNNER): $(OBJECTS) $(OBJ)/run_corpus.o
	$(CXX) $(CXXFLAGS) $(FP_FLAGS) -o $@ $^ $(LDFLAGS)

$(ROUNDTRIP): $(OBJECTS) $(OBJ)/tests/json_roundtrip.o
	$(CXX) $(CXXFLAGS) $(FP_FLAGS) -o $@ $^ $(LDFLAGS)

$(SELFCHECK): $(OBJECTS) $(OBJ)/tests/self_check.o
	$(CXX) $(CXXFLAGS) $(FP_FLAGS) -o $@ $^ $(LDFLAGS)

corpus: $(RUNNER)
	$(RUNNER) --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

test: $(ROUNDTRIP) $(SELFCHECK)
	$(ROUNDTRIP) $(CORPUS)
	$(SELFCHECK) .

# ── The fused multiply-add scan ───────────────────────────────────────────────
#
# FP_FLAGS says contraction is off; this is what checks the compiler agreed. Every translation unit
# is compiled to assembly with a fused multiply-add *available* — the flag that makes one exist on
# this host's architecture — and the step fails on any fused instruction in the listings. The probe
# is compiled the same way without FP_FLAGS and must fuse, so a host where the scan cannot see an
# FMA (an architecture this does not know, a compiler that ignored the flag) fails rather than
# passing by not looking.
#
# aarch64 has FMA in its base instruction set and needs no flag; x86-64 needs -mfma. Any other host
# gets neither, and its probe will say so.
FMA_TARGET := $(if $(findstring x86_64,$(shell $(CXX) -dumpmachine)),-mfma,)
FUSION := $(BUILD)/fusion
FUSION_LISTINGS := $(patsubst src/%.cpp,$(FUSION)/%.s,$(SOURCES) src/run_corpus.cpp)
# vfmadd231sd and the rest of the VEX family on x86-64; fmadd, fmsub, fnmadd, fnmsub on aarch64.
FUSED_MNEMONIC := '(^|[[:space:]])v?fn?m(add|sub)[0-9a-z]*[[:space:]]'

$(FUSION)/%.s: src/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $(FP_FLAGS) $(FMA_TARGET) $(INCLUDES) -MMD -MP -S $< -o $@

$(FUSION)/probe.s: tests/fusion_probe.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $(FMA_TARGET) -S $< -o $@

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

-include $(OBJECTS:.o=.d) $(OBJ)/run_corpus.d $(OBJ)/tests/json_roundtrip.d $(OBJ)/tests/self_check.d
-include $(FUSION_LISTINGS:.s=.d)
