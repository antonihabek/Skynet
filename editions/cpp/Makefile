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
MIN_PASSED ?= 1199

.PHONY: all corpus test check clean

all: $(RUNNER) $(ROUNDTRIP) $(SELFCHECK)

# One object per translation unit, with the headers each one includes recorded by the compiler
# (`-MMD -MP`) rather than listed here. The single command this replaced rebuilt all thirty-five
# units on any change, and depended on the headers only because somebody had written them into the
# rule; a generated table included from a `.cpp` is now a dependency because the compiler read it.
$(OBJ)/%.o: src/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -MMD -MP -c $< -o $@

$(OBJ)/tests/%.o: tests/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -MMD -MP -c $< -o $@

$(RUNNER): $(OBJECTS) $(OBJ)/run_corpus.o
	$(CXX) $(CXXFLAGS) -o $@ $^ $(LDFLAGS)

$(ROUNDTRIP): $(OBJECTS) $(OBJ)/tests/json_roundtrip.o
	$(CXX) $(CXXFLAGS) -o $@ $^ $(LDFLAGS)

$(SELFCHECK): $(OBJECTS) $(OBJ)/tests/self_check.o
	$(CXX) $(CXXFLAGS) -o $@ $^ $(LDFLAGS)

corpus: $(RUNNER)
	$(RUNNER) --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

test: $(ROUNDTRIP) $(SELFCHECK)
	$(ROUNDTRIP) $(CORPUS)
	$(SELFCHECK) .

check: test corpus

clean:
	rm -rf $(BUILD)

-include $(OBJECTS:.o=.d) $(OBJ)/run_corpus.d $(OBJ)/tests/json_roundtrip.d $(OBJ)/tests/self_check.d
