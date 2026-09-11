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

CXX ?= g++
CXXFLAGS ?= -std=c++20 -O2 -Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror
INCLUDES := -Iinclude
BUILD := build

# Statically linked so the binary runs wherever it is copied. An edition's runner that needs its
# compiler's runtime installed alongside it is one a vendor cannot use to check their own build.
LDFLAGS ?= -static

SOURCES := src/json.cpp src/text.cpp src/vocabulary.cpp src/corpus_files.cpp src/corpus.cpp \
           src/dispatch.cpp $(wildcard src/contracts/*.cpp)

RUNNER := $(BUILD)/skynet-corpus-cpp
ROUNDTRIP := $(BUILD)/json_roundtrip
SELFCHECK := $(BUILD)/self_check

# Where the corpus is. Overridable so a vendor can point the runner at a pinned copy.
CORPUS ?= ../../corpus

# The ratchet. This is the number of cases this edition has reached; see `--min-passed` in
# `src/run_corpus.cpp` for why a floor is checked separately from the failures. Lowering it is a
# decision somebody makes in a diff.
MIN_PASSED ?= 840

.PHONY: all corpus test check clean

all: $(RUNNER) $(ROUNDTRIP) $(SELFCHECK)

$(RUNNER): $(SOURCES) src/run_corpus.cpp | $(BUILD)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -o $@ $(SOURCES) src/run_corpus.cpp $(LDFLAGS)

$(ROUNDTRIP): $(SOURCES) tests/json_roundtrip.cpp | $(BUILD)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -o $@ $(SOURCES) tests/json_roundtrip.cpp $(LDFLAGS)

$(SELFCHECK): $(SOURCES) tests/self_check.cpp | $(BUILD)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -o $@ $(SOURCES) tests/self_check.cpp $(LDFLAGS)

$(BUILD):
	mkdir -p $(BUILD)

corpus: $(RUNNER)
	$(RUNNER) --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

test: $(ROUNDTRIP) $(SELFCHECK)
	$(ROUNDTRIP) $(CORPUS)
	$(SELFCHECK)

check: test corpus

clean:
	rm -rf $(BUILD)
