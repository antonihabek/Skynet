# skynet-booster — a Booster T1 over the vendor SDK's low-level channels, and its substrate.
#
# The plan's Phase 7 item 3 names the T1 after the Unitrees, and this is the same honest reading of
# *"SDK sim"*: the vendor's SDK on **both** sides of the wire. `fake_robot` publishes `rt/low_state`
# and consumes `rt/joint_ctrl` using booster_robotics_sdk's own channel classes, so the CDR encoding,
# the topic types and the DDS discovery are Booster's code rather than a transcription of it.
#
#   make check    the whole path: fake robot, the suite, `skynet driver check` against the driver
#   make lib      libskynet_booster.so
#   make fake     the fake robot on its own, for driving by hand
#   make chaos    the link-chaos contract: the driver's DDS traffic on a loopback whose queue the kernel makes delay, lose, repeat, reorder, corrupt and blackhole it
#
# It needs a checkout of the SDK (Apache-2.0), which carries a prebuilt static library and is not
# vendored here:
#
#   git clone --depth 1 https://github.com/BoosterRobotics/booster_robotics_sdk /tmp/booster_sdk
#   make check BOOSTER_SDK=/tmp/booster_sdk
#
# It refuses rather than building half of itself when that is absent, for the reason
# `drivers/ros2_control` gives about its own framework: a driver that compiled without its SDK
# would be a driver nobody could load.

BOOSTER_SDK ?=
ARCH         ?= $(shell uname -m)
CXX          ?= c++
CARGO        ?= cargo
PYTHON       ?= python3
CXXFLAGS     ?= -std=c++17 -O2 -Wall -Wextra -fPIC
REPO         := ../..

# `-isystem` rather than `-I`: the SDK's own headers are not this driver's code and `-Wall
# -Wextra` over FastDDS's templates buries a real warning under a thousand lines of somebody
# else's. Warnings from `booster_bridge.cpp` still arrive.
SDK_INCLUDES := -isystem $(BOOSTER_SDK)/include
SDK_LIBS     := $(BOOSTER_SDK)/lib/$(ARCH)/libbooster_robotics_sdk.a -lpthread -ldl -lrt

LIB  := libskynet_booster.so
FAKE := fake_robot
SUITE := selftest

#: Which interface DDS runs on. `lo` keeps the whole exchange inside this machine, which is what a
#: substrate should do — a test that discovered a robot on the office network would be a test that
#: commanded one.
IFACE ?= lo
HZ    ?= 500

.PHONY: all check lib fake suite chaos clean require-sdk

all: check

require-sdk:
	@if [ -z "$(BOOSTER_SDK)" ]; then \
	  echo "::error::BOOSTER_SDK is not set. This driver links Booster's own SDK (Apache-2.0),"; \
	  echo "which carries a prebuilt static library and is not vendored here:"; \
	  echo ""; \
	  echo "  git clone --depth 1 https://github.com/BoosterRobotics/booster_robotics_sdk /tmp/booster_sdk"; \
	  echo "  make check BOOSTER_SDK=/tmp/booster_sdk"; \
	  echo ""; \
	  echo "Refusing rather than building half of itself: a driver that compiled without its SDK"; \
	  echo "would be a driver nobody could load."; \
	  exit 1; \
	fi
	@if [ ! -f "$(BOOSTER_SDK)/lib/$(ARCH)/libbooster_robotics_sdk.a" ]; then \
	  echo "::error::$(BOOSTER_SDK)/lib/$(ARCH)/libbooster_robotics_sdk.a does not exist."; \
	  echo "The SDK ships prebuilt libraries for x86_64 and aarch64 only, and this machine is"; \
	  echo "$(ARCH). There is nothing to link against here."; \
	  exit 1; \
	fi

lib: require-sdk $(LIB)

$(LIB): booster_bridge.cpp $(REPO)/bindings/cpp/skynet_rhal.hpp $(REPO)/bindings/c/skynet_rhal.h
	$(CXX) $(CXXFLAGS) -shared -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c $(SDK_INCLUDES) \
	  -o $@ booster_bridge.cpp $(SDK_LIBS)

fake: require-sdk $(FAKE)

$(FAKE): fake_robot.cpp
	$(CXX) $(CXXFLAGS) $(SDK_INCLUDES) -o $@ fake_robot.cpp $(SDK_LIBS)

suite: require-sdk $(SUITE)

$(SUITE): selftest.cpp booster_bridge.cpp
	$(CXX) $(CXXFLAGS) -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c $(SDK_INCLUDES) \
	  -o $@ selftest.cpp booster_bridge.cpp $(SDK_LIBS)

# The whole path, through the real ABI: the fake robot on DDS, then the conformance run against it,
# then the fake's own report of what the driver sent. The last part is what makes this more than a
# tier: a green run that had sent the wrong layout would be reported here.
check: lib fake suite
	sh ./run-check.sh "$(IFACE)" "$(HZ)"

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4). DDS names its peers in discovery messages, so there is no address to put a relay in front of: the kernel's own link emulation (`netem`)
# runs on the loopback of a network namespace of its own, which the driver and the fake robot share (`tools/faultproxy/relays.py`). The driver and the fake are built outside the namespace, so
# nothing inside it compiles; the fake's `tap=<fd>` is a pipe, which the queue cannot touch.
chaos: lib fake
	PYTHONPATH=$(REPO)/bindings/python sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -f $(LIB) $(FAKE) $(SUITE)
