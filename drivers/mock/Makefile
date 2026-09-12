# The mock robot, and the sim half of SCTS run against it.
#
# No dependencies beyond a C11 compiler. A driver author should be able to clone this repository
# and reach a passing conformance run before deciding whether to invest in the platform.

CC      ?= cc
CFLAGS  ?= -std=c11 -O2 -Wall -Wextra -Wpedantic -fPIC -I../../bindings/c
CXX     ?= c++
CXXFLAGS ?= -std=c++17 -O2 -Wall -Wextra -I../../bindings/cpp -I../../bindings/c

# The loadable form, which is the one `skynetk` and `skynet driver check` can actually open.
#
# For a long time this Makefile built only `selftest` — a program that links `mock_robot.c` and
# calls it directly. That is enough to test the driver's own logic and it is not a driver: §4 makes
# a driver a **shared object exporting `skynet_rhal_entry`**, and nothing here produced one. So the
# one body in this repository that needs no hardware was the one body a robot could not load, and
# bringing up a bench robot meant writing a driver first.
ifeq ($(OS),Windows_NT)
  LIB := skynet_mock.dll
  LDFLAGS ?= -shared
else
  UNAME_S := $(shell uname -s)
  ifeq ($(UNAME_S),Darwin)
    LIB := libskynet_mock.dylib
    LDFLAGS ?= -dynamiclib
  else
    LIB := libskynet_mock.so
    LDFLAGS ?= -shared
  endif
endif

.PHONY: all lib check cpp-header-check clean

all: selftest $(LIB)

lib: $(LIB)

$(LIB): mock_robot.c ../../bindings/c/skynet_rhal.h
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ mock_robot.c -lm

selftest: mock_robot.c selftest.c
	$(CC) $(CFLAGS) -o $@ mock_robot.c selftest.c -lm

check: selftest
	./selftest

# The C++ wrapper is vendor-facing and must at least compile against the same header.
cpp-header-check: header_check.cpp
	$(CXX) $(CXXFLAGS) -fsyntax-only header_check.cpp

clean:
	rm -f selftest selftest.exe $(LIB) skynet_mock.dll libskynet_mock.so libskynet_mock.dylib
