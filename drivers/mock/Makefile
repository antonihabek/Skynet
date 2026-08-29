# The mock robot, and the sim half of SCTS run against it.
#
# No dependencies beyond a C11 compiler. A driver author should be able to clone this repository
# and reach a passing conformance run before deciding whether to invest in the platform.

CC      ?= cc
CFLAGS  ?= -std=c11 -O2 -Wall -Wextra -Wpedantic -I../../bindings/c
CXX     ?= c++
CXXFLAGS ?= -std=c++17 -O2 -Wall -Wextra -I../../bindings/cpp -I../../bindings/c

.PHONY: all check cpp-header-check clean

all: selftest

selftest: mock_robot.c selftest.c
	$(CC) $(CFLAGS) -o $@ mock_robot.c selftest.c -lm

check: selftest
	./selftest

# The C++ wrapper is vendor-facing and must at least compile against the same header.
cpp-header-check: header_check.cpp
	$(CXX) $(CXXFLAGS) -fsyntax-only header_check.cpp

clean:
	rm -f selftest selftest.exe
