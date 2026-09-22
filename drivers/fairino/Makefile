# skynet-fairino — a FAIRINO FR3 cobot over the vendor's own wires, and its substrate.
#
# `DRY-COVERAGE-PLAN.md` Phase 7's cobot class, which F92 had closed as *"Fairino … unlicensed"* on
# the strength of `frcobot_ros2`'s `TODO: License declaration`. That is the ROS 2 wrapper; the SDK
# is Apache-2.0 **with source**, and this row is built on the SDK. Six pieces:
#
#   fairino_bridge.cpp    the driver — speaks XML-RPC and CNDE directly, links nothing vendor
#   fairino_wire.hpp      the wire truth the three share, with the vendor's file named per constant
#   fairino_xmlrpc.hpp    the subset of XML-RPC this wire uses, and the shapes it refuses
#   fake_controller.hpp   the substrate: an FR controller's three sockets with a plant behind them
#   selftest.cpp          the driver's own suite, which runs that controller in-process
#   layout_check.cpp      this tree's sizes against the vendor's struct, and its ports and its stop
#                         frame against the vendor's own source
#   cross_check.cpp       the vendor's own FRRobot, unmodified, against the fake
#
#   make check    everything, in the order it has to happen in
#   make suite    the driver's own suite alone, for driving by hand — needs no SDK
#   make lib      libskynet_fairino.so
#   make fake     the fake controller alone (`./fake_controller 20003 8080 20005`)
#
# The driver, the fake and the suite need nothing but this tree. The two checks that hold this
# tree's reading of the wire against the vendor's own need a checkout, which is not vendored here:
#
#   git clone --depth 1 -b v2.3.9-robot3.9.9 \
#     https://github.com/FAIR-INNOVATION/fairino-cpp-sdk /tmp/fairino-cpp-sdk
#   make check FAIRINO_SDK=/tmp/fairino-cpp-sdk
#
# `check` refuses rather than running half of itself without it: a wire claim with only one
# implementation behind it is this tree agreeing with itself. `make suite` is the half that needs
# no SDK.

FAIRINO_SDK ?=
ARCH        ?= $(shell uname -m)
CXX         ?= c++
CARGO       ?= cargo
CXXFLAGS    ?= -std=c++17 -O2 -Wall -Wextra -fPIC
REPO        := ../..

SDK_SRC      := $(FAIRINO_SDK)/libfairino/src
SDK_INCLUDES := -isystem $(SDK_SRC)/include/Robot-EN -isystem $(SDK_SRC)/include/CNDE \
                -isystem $(SDK_SRC)/include/Base
# The vendor ships the Linux binaries zipped, one archive per architecture. Unpacked beside them
# rather than into this tree.
SDK_ZIP     := $(FAIRINO_SDK)/linux/libfairino/lib/x86.zip
SDK_LIB_DIR := $(FAIRINO_SDK)/linux/libfairino/lib/x86/lib
SDK_LIB     := $(SDK_LIB_DIR)/libfairino.so

LIB    := libskynet_fairino.so
FAKE   := fake_controller
SUITE  := selftest
LAYOUT := layout_check
CROSS  := cross_check

# The suite and `skynet driver check` run beside the vendor's ports, because the vendor's library
# has 20003, 8080 and 20005 compiled into it and the cross-check has to use them.
SUITE_XMLRPC_PORT ?= 31003
SUITE_CMD_PORT    ?= 31080
SUITE_STATE_PORT  ?= 31005
CHECK_XMLRPC_PORT ?= 32003
CHECK_CMD_PORT    ?= 32080
CHECK_STATE_PORT  ?= 32005

.PHONY: all check suite lib fake layout cross clean require-sdk

all: check

require-sdk:
	@if [ -z "$(FAIRINO_SDK)" ]; then \
	  echo "::error::FAIRINO_SDK is not set. The layout check and the wire cross-check hold this"; \
	  echo "tree's reading of the FR wire against FAIR INNOVATION's own source and their own"; \
	  echo "prebuilt client (Apache-2.0), and neither is vendored here:"; \
	  echo ""; \
	  echo "  git clone --depth 1 -b v2.3.9-robot3.9.9 \\"; \
	  echo "    https://github.com/FAIR-INNOVATION/fairino-cpp-sdk /tmp/fairino-cpp-sdk"; \
	  echo "  make check FAIRINO_SDK=/tmp/fairino-cpp-sdk"; \
	  echo ""; \
	  echo "Refusing rather than running half of itself: a wire claim with only one implementation"; \
	  echo "behind it is this tree agreeing with itself. \`make suite\` is the half that needs no SDK."; \
	  exit 1; \
	fi
	@if [ ! -f "$(SDK_SRC)/include/Robot-EN/robot_types.h" ]; then \
	  echo "::error::$(SDK_SRC)/include/Robot-EN/robot_types.h does not exist, so FAIRINO_SDK does"; \
	  echo "not point at a fairino-cpp-sdk checkout with its source. The main branch carries only a"; \
	  echo "README — check out the tag: git -C \$$FAIRINO_SDK checkout v2.3.9-robot3.9.9"; \
	  exit 1; \
	fi
	@if [ "$(ARCH)" != "x86_64" ]; then \
	  echo "::error::the vendor ships Linux binaries for x86_64 and three ARM boards, and this"; \
	  echo "machine is $(ARCH). The layout check still compiles from the source; the wire"; \
	  echo "cross-check has nothing to link against here."; \
	  exit 1; \
	fi

$(SDK_LIB): require-sdk
	@if [ ! -f "$(SDK_ZIP)" ]; then \
	  echo "::error::$(SDK_ZIP) does not exist."; exit 1; \
	fi
	unzip -o -q "$(SDK_ZIP)" -d "$(FAIRINO_SDK)/linux/libfairino/lib/x86"
	@test -f "$(SDK_LIB)"

lib: $(LIB)

$(LIB): fairino_bridge.cpp fairino_wire.hpp fairino_xmlrpc.hpp \
        $(REPO)/bindings/cpp/skynet_rhal.hpp $(REPO)/bindings/c/skynet_rhal.h
	$(CXX) $(CXXFLAGS) -shared -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c -I. \
	  -o $@ fairino_bridge.cpp

fake: $(FAKE)

$(FAKE): fake_controller.cpp fake_controller.hpp fairino_wire.hpp fairino_xmlrpc.hpp
	$(CXX) $(CXXFLAGS) -I. -o $@ fake_controller.cpp -lpthread

suite: $(SUITE)
	./$(SUITE) $(SUITE_XMLRPC_PORT) $(SUITE_CMD_PORT) $(SUITE_STATE_PORT)

$(SUITE): selftest.cpp fairino_bridge.cpp fake_controller.hpp fairino_wire.hpp fairino_xmlrpc.hpp
	$(CXX) $(CXXFLAGS) -I$(REPO)/bindings/cpp -I$(REPO)/bindings/c -I. \
	  -o $@ selftest.cpp fairino_bridge.cpp -lpthread

layout: require-sdk $(LAYOUT)
	./$(LAYOUT) $(FAIRINO_SDK)

$(LAYOUT): layout_check.cpp fairino_wire.hpp
	$(CXX) $(CXXFLAGS) -I. $(SDK_INCLUDES) -DFAIRINO_SDK_PATH='"$(FAIRINO_SDK)"' \
	  -o $@ layout_check.cpp

cross: require-sdk $(CROSS)

$(CROSS): cross_check.cpp $(SDK_LIB)
	$(CXX) $(CXXFLAGS) -I. $(SDK_INCLUDES) -o $@ cross_check.cpp $(SDK_LIB) \
	  -Wl,-rpath,$(SDK_LIB_DIR) -lpthread

check: require-sdk $(LIB) $(FAKE) $(SUITE) $(LAYOUT) $(CROSS)
	sh ./run-check.sh "$(FAIRINO_SDK)" "$(SUITE_XMLRPC_PORT)" "$(SUITE_CMD_PORT)" \
	  "$(SUITE_STATE_PORT)" "$(CHECK_XMLRPC_PORT)" "$(CHECK_CMD_PORT)" "$(CHECK_STATE_PORT)"

clean:
	rm -f $(LIB) $(FAKE) $(SUITE) $(LAYOUT) $(CROSS)
