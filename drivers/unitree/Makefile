# skynet-unitree — Unitree bodies over unitree_sdk2's low-level channels, and their substrate.
#
# The plan's Phase 2 table asks for an *"SDK sim"*, and this is the honest reading of that: the
# vendor's SDK on **both** sides of the wire. `fake_robot` publishes `rt/lowstate` and consumes
# `rt/lowcmd` using unitree_sdk2's own channel classes, so the CDR encoding, the topic types and
# the DDS discovery are Unitree's code rather than a transcription of it. Phase 7 item 3 runs four
# bodies through it: the Go2, the H1, the G1 and the H1-2, selected by `model` (`run-check.sh` runs
# all four in sequence).
#
#   make check    the whole path per body: fake robot, the suite, `skynet driver check`
#   make lib      libskynet_unitree.so
#   make fake     the fake robot on its own, for driving by hand (`./fake_robot lo 500 0 g1`)
#
# It needs a checkout of the SDK, which is 27 MB of static library and is not vendored here:
#
#   git clone --depth 1 https://github.com/unitreerobotics/unitree_sdk2 /tmp/unitree_sdk2
#   make check UNITREE_SDK2=/tmp/unitree_sdk2
#
# It refuses rather than building half of itself when that is absent, for the reason
# `drivers/ros2_control` gives about its own framework: a driver that compiled without its SDK
# would be a driver nobody could load.

UNITREE_SDK2 ?=
ARCH         ?= $(shell uname -m)
CXX          ?= c++
CARGO        ?= cargo
CXXFLAGS     ?= -std=c++17 -O2 -Wall -Wextra -fPIC
REPO         := ../..

# `-isystem` rather than `-I`: the SDK's own headers are not this driver's code and `-Wall
# -Wextra` over CycloneDDS's templates buries a real warning under a thousand lines of somebody
# else's. Warnings from `unitree_bridge.cpp` still arrive.
SDK_INCLUDES := -isystem $(UNITREE_SDK2)/include -isystem $(UNITREE_SDK2)/thirdparty/include \
                -isystem $(UNITREE_SDK2)/thirdparty/include/ddscxx \
                -isystem $(UNITREE_SDK2)/thirdparty/include/ddsc
SDK_LIBS     := $(UNITREE_SDK2)/lib/$(ARCH)/libunitree_sdk2.a \
                -L$(UNITREE_SDK2)/thirdparty/lib/$(ARCH) -lddsc -lddscxx -lpthread
SDK_RPATH    := -Wl,-rpath,$(UNITREE_SDK2)/thirdparty/lib/$(ARCH)

LIB  := libskynet_unitree.so
FAKE := fake_robot
SUITE := selftest

#: Which interface DDS runs on. `lo` keeps the whole exchange inside this machine, which is what a
#: substrate should do — a test that discovered a robot on the office network would be a test that
#: commanded one.
IFACE ?= lo
HZ    ?= 500

.PHONY: all check lib fake suite clean require-sdk

all: check

require-sdk:
	@if [ -z "$(UNITREE_SDK2)" ]; then \
	  echo "::error::UNITREE_SDK2 is not set. This driver links Unitree's own SDK (BSD-3-Clause),"; \
	  echo "which is 27 MB of static library and is not vendored here:"; \
	  echo ""; \
	  echo "  git clone --depth 1 https://github.com/unitreerobotics/unitree_sdk2 /tmp/unitree_sdk2"; \
	  echo "  make check UNITREE_SDK2=/tmp/unitree_sdk2"; \
	  echo ""; \
	  echo "Refusing rather than building half of itself: a driver that compiled without its SDK"; \
	  echo "would be a driver nobody could load."; \
	  exit 1; \
	fi
	@if [ ! -f "$(UNITREE_SDK2)/lib/$(ARCH)/libunitree_sdk2.a" ]; then \
	  echo "::error::$(UNITREE_SDK2)/lib/$(ARCH)/libunitree_sdk2.a does not exist."; \
	  echo "The SDK ships prebuilt libraries for x86_64 and aarch64 only, and this machine is"; \
	  echo "$(ARCH). There is nothing to link against here."; \
	  exit 1; \
	fi

lib: require-sdk $(LIB)

$(LIB): unitree_bridge.cpp $(REPO)/bindings/cpp/skynet_rhal.hpp $(REPO)/bindings/c/skynet_rhal.h
	$(CXX) $(CXXFLAGS) -shared -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c $(SDK_INCLUDES) \
	  -o $@ unitree_bridge.cpp $(SDK_LIBS) $(SDK_RPATH)

fake: require-sdk $(FAKE)

$(FAKE): fake_robot.cpp
	$(CXX) $(CXXFLAGS) $(SDK_INCLUDES) -o $@ fake_robot.cpp $(SDK_LIBS) $(SDK_RPATH)

suite: require-sdk $(SUITE)

$(SUITE): selftest.cpp unitree_bridge.cpp
	$(CXX) $(CXXFLAGS) -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c $(SDK_INCLUDES) \
	  -o $@ selftest.cpp unitree_bridge.cpp $(SDK_LIBS) $(SDK_RPATH)

# The whole path, through the real ABI: the fake robot on DDS, then the conformance run against it,
# then the fake's own report of what the driver sent. The last part is what makes this more than a
# tier: a green run that had written zeros where the sentinels belong would be reported here.
check: lib fake suite
	sh ./run-check.sh "$(IFACE)" "$(HZ)"

clean:
	rm -f $(LIB) $(FAKE) $(SUITE)
