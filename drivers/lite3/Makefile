# skynet-lite3 — a Jueying Lite3 quadruped over MotionSDK's wire, and its substrate.
#
# `DRY-COVERAGE-PLAN.md` Phase 7's quadruped row, specified by F92 as *"Fourier-shaped, SDK client
# unmodified against a fake motion host"*. Four pieces:
#
#   lite3_bridge.cpp      the driver — speaks the wire directly, links nothing of the vendor's
#   fake_motion_host.cpp  the substrate — the robot's side of the same wire, with a plant behind it
#   selftest.cpp          the driver's own suite, which is its own motion host and reads the bytes
#   layout_check.cpp      this tree's structs against the vendor's headers, at compile time
#   cross_check.cpp       the vendor's own Sender and Receiver, unmodified, against the fake
#
#   make check    everything, in the order it has to happen in
#   make suite    the driver's own suite alone, for driving by hand — needs no SDK
#   make lib      libskynet_lite3.so
#   make fake     the fake motion host alone (`./fake_motion_host 43893 127.0.0.1 43897 500`)
#
# The driver, the fake and the suite need nothing but this tree: the wire is three words and a
# struct, transcribed in `lite3_wire.hpp` with the vendor's file named beside every constant. The
# two cross-checks are what hold that transcription to the vendor's own code, and they need a
# checkout of MotionSDK, which is not vendored here:
#
#   git clone --depth 1 https://github.com/DeepRoboticsLab/lite3_motionsdk /tmp/lite3_motionsdk
#   make check LITE3_SDK=/tmp/lite3_motionsdk
#
# `check` refuses rather than running half of itself without it, for the reason the Fourier row
# gives about its own vendor client: a wire claim with only one implementation behind it is this
# tree agreeing with itself.

LITE3_SDK ?=
ARCH      ?= $(shell uname -m)
CXX       ?= c++
CARGO     ?= cargo
CXXFLAGS  ?= -std=c++17 -O2 -Wall -Wextra -fPIC
REPO      := ../..

SDK_INCLUDES := -isystem $(LITE3_SDK)/include -isystem $(LITE3_SDK)/include/common
SDK_LIB      := $(LITE3_SDK)/lib/libdeeprobotics_legged_sdk_$(ARCH).so

LIB    := libskynet_lite3.so
FAKE   := fake_motion_host
SUITE  := selftest
LAYOUT := layout_check
CROSS  := cross_check

# The suite and `skynet driver check` run on ports beside the vendor's, because the vendor's own
# `Receiver` has 43897 fixed inside the prebuilt library and the cross-check has to use it.
SUITE_COMMAND_PORT ?= 44893
SUITE_STATE_PORT   ?= 44897
HZ                 ?= 500

.PHONY: all check suite lib fake layout cross clean require-sdk

all: check

require-sdk:
	@if [ -z "$(LITE3_SDK)" ]; then \
	  echo "::error::LITE3_SDK is not set. The two cross-checks hold this tree's transcription of"; \
	  echo "the wire against Deep Robotics' own headers and their own prebuilt client (MIT), and"; \
	  echo "neither is vendored here:"; \
	  echo ""; \
	  echo "  git clone --depth 1 https://github.com/DeepRoboticsLab/lite3_motionsdk /tmp/lite3_motionsdk"; \
	  echo "  make check LITE3_SDK=/tmp/lite3_motionsdk"; \
	  echo ""; \
	  echo "Refusing rather than running half of itself: a wire claim with only one implementation"; \
	  echo "behind it is this tree agreeing with itself. \`make suite\` is the half that needs no SDK."; \
	  exit 1; \
	fi
	@if [ ! -f "$(LITE3_SDK)/include/robot_types.h" ]; then \
	  echo "::error::$(LITE3_SDK)/include/robot_types.h does not exist, so LITE3_SDK does not"; \
	  echo "point at a MotionSDK checkout."; \
	  exit 1; \
	fi
	@if [ ! -f "$(SDK_LIB)" ]; then \
	  echo "::error::$(SDK_LIB) does not exist."; \
	  echo "MotionSDK ships prebuilt libraries for x86_64 and aarch64 only, and this machine is"; \
	  echo "$(ARCH). The layout check still compiles from the headers; the wire cross-check has"; \
	  echo "nothing to link against here."; \
	  exit 1; \
	fi

lib: $(LIB)

$(LIB): lite3_bridge.cpp lite3_wire.hpp $(REPO)/bindings/cpp/skynet_rhal.hpp $(REPO)/bindings/c/skynet_rhal.h
	$(CXX) $(CXXFLAGS) -shared -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c -I. \
	  -o $@ lite3_bridge.cpp

fake: $(FAKE)

$(FAKE): fake_motion_host.cpp lite3_wire.hpp
	$(CXX) $(CXXFLAGS) -I. -o $@ fake_motion_host.cpp -lpthread

suite: $(SUITE)
	./$(SUITE) $(SUITE_COMMAND_PORT) $(SUITE_STATE_PORT)

$(SUITE): selftest.cpp lite3_bridge.cpp lite3_wire.hpp
	$(CXX) $(CXXFLAGS) -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c -I. \
	  -o $@ selftest.cpp lite3_bridge.cpp -lpthread

layout: require-sdk $(LAYOUT)
	./$(LAYOUT)

$(LAYOUT): layout_check.cpp lite3_wire.hpp
	$(CXX) $(CXXFLAGS) -I. $(SDK_INCLUDES) -o $@ layout_check.cpp

cross: require-sdk $(CROSS)

$(CROSS): cross_check.cpp
	$(CXX) $(CXXFLAGS) -I. $(SDK_INCLUDES) -o $@ cross_check.cpp $(SDK_LIB) \
	  -Wl,-rpath,$(LITE3_SDK)/lib -lpthread

check: require-sdk $(LIB) $(FAKE) $(SUITE) $(LAYOUT) $(CROSS)
	sh ./run-check.sh "$(SUITE_COMMAND_PORT)" "$(SUITE_STATE_PORT)" "$(HZ)"

clean:
	rm -f $(LIB) $(FAKE) $(SUITE) $(LAYOUT) $(CROSS)
