# skynet-x1 — an AgiBot X1 humanoid over its own ROS 2 topics, and its substrate.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 7 item 3: the X1 against the vendor's own `agibot_x1_infer`
# MuJoCo deployment. The driver is a plain rclcpp node speaking the two topics the sim owns —
# `/joint_states` in, `/joint_cmd` out — the way the vendor's own ControlModule does. The
# substrate is two-headed: a deterministic C++ fake (fixed joints, scripted states, recorded
# commands, on DDS domain 42) for the suite's logic, and the vendor's sim itself (domain 0) for
# the motion proof — no transcription of the wire anywhere, because the sim IS the wire.
#
#   make check    fake + sim up, the suite, `skynet driver check`, the fake's report
#   make lib      libskynet_x1.so
#   make fake     the fake on its own, for driving by hand
#   make suite    the suite on its own (needs the fake and the sim already up — see run-check.sh)
#   make check-sim-docker   all of `check`, inside sim/Dockerfile's image: the vendor tree built
#                 from a pinned commit, headless under Xvfb, nothing needed on the host but Docker
#
# It needs the vendor's built tree (AimRT + MuJoCo + generated messages — none of it vendored
# here) and a ROS 2 with rclcpp:
#
#   git clone https://github.com/AgibotTech/agibot_x1_infer /tmp/x1_infer   # then build it
#   make check X1_INFER=/tmp/x1_infer ROS_ROOT=$CONDA_PREFIX
#
# Building the vendor tree takes a while (AimRT, protobuf, MuJoCo) and needs four local
# patches on a jazzy machine — see README.md. `make check` refuses rather than building half
# of itself when either is absent.

X1_INFER ?=
ROS_ROOT ?=
ARCH     ?= $(shell uname -m)
CXX      ?= c++
CARGO    ?= cargo
CXXFLAGS ?= -std=c++17 -O2 -Wall -Wextra -fPIC
REPO     := ../..

X1_INSTALL := $(X1_INFER)/build/install

# `-isystem`: the vendor's and the framework's headers are not this driver's code.
X1_INCLUDES := -isystem $(X1_INSTALL)/include/my_ros2_proto \
               -isystem $(X1_INSTALL)/include
ROS_INCLUDES := -I$(ROS_ROOT)/include $(shell for dir in $(ROS_ROOT)/include/*/; do \
                  [ -d "$$dir$$(basename "$$dir")" ] && echo "-I$$dir"; done 2>/dev/null)
X1_LIBS := -L$(X1_INSTALL)/lib -lmy_ros2_proto__rosidl_typesupport_cpp \
           -lmy_ros2_proto__rosidl_generator_c \
           -Wl,-rpath,$(X1_INSTALL)/lib
# `rcl` and below: executables (fake, suite) cannot hide behind
# `--allow-shlib-undefined` the way the shared library can — rclcpp's timer, subscription and
# statistics templates reference them directly, and which template instantiates where decides
# which binary needs which (the suite's spy pulls in the statistics collector; the fake's
# subscriptions do not).
ROS_LIBS := -L$(ROS_ROOT)/lib -lrclcpp -lrcl -lrmw -ltracetools \
            -llibstatistics_collector \
            -lsensor_msgs__rosidl_typesupport_cpp \
            -lstatistics_msgs__rosidl_typesupport_cpp \
            -lrcutils -Wl,--allow-shlib-undefined

VENDOR_LIBS := $(shell ls -d $(ROS_ROOT)/opt/*/lib 2>/dev/null | paste -sd: )
RUN_ENV := AMENT_PREFIX_PATH=$(ROS_ROOT):$(X1_INSTALL) ROS_DISTRO=jazzy \
           LD_LIBRARY_PATH=$(ROS_ROOT)/lib:$(ROS_ROOT)/lib/x86_64-linux-gnu:$(VENDOR_LIBS):$(X1_INSTALL)/lib \
           LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe MESA_LOADER_DRIVER_OVERRIDE=swrast \
           __GLX_VENDOR_LIBRARY_NAME=mesa

LIB   := libskynet_x1.so
FAKE  := fake_x1
SUITE := selftest
MOTION := sim_motion

#: The image `check-sim-docker` builds, and extra `docker build` arguments for it. Empty builds the
#: vendor tree at the pin in sim/Dockerfile; the nightly substrate workflow
#: (`.github/workflows/substrates.yml`, Phase 8 item 8.11) passes the vendor's branch here, under
#: another image name, to find a vendor change the night it lands.
SIM_IMAGE ?= skynet-x1-sim
DOCKER_BUILD_ARGS ?=

.PHONY: all check check-sim-docker chaos-docker lib fake suite motion clean require-x1 require-ros

all: check

require-x1:
	@if [ -z "$(X1_INFER)" ]; then \
	  echo "::error::X1_INFER is not set. This driver links the vendor's built tree"; \
	  echo "(agibot_x1_infer: AimRT + MuJoCo + generated messages), which is not vendored here."; \
	  echo "See README.md for the build, then:"; \
	  echo ""; \
	  echo "  make check X1_INFER=/tmp/x1_infer ROS_ROOT=\$$CONDA_PREFIX"; \
	  exit 1; fi
	@test -f "$(X1_INSTALL)/lib/libmy_ros2_proto__rosidl_typesupport_cpp.so" || { \
	  echo "::error::no built vendor tree at $(X1_INFER). The driver links its message"; \
	  echo "libraries and runs its sim; an unbuilt checkout is neither. See README.md."; \
	  exit 1; }

require-ros:
	@test -f "$(ROS_ROOT)/lib/librclcpp.so" || { \
	  echo "::error::no rclcpp at $(ROS_ROOT). Set ROS_ROOT to a ROS 2 install."; \
	  exit 1; }

lib: require-x1 require-ros $(LIB)

$(LIB): x1_bridge.cpp $(REPO)/bindings/c/skynet_rhal.h $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -shared -o $@ x1_bridge.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(X1_INCLUDES) $(ROS_INCLUDES) \
	  $(X1_LIBS) $(ROS_LIBS)

fake: require-x1 require-ros $(FAKE)

$(FAKE): fake_x1.cpp
	$(CXX) $(CXXFLAGS) -o $@ fake_x1.cpp $(X1_INCLUDES) $(ROS_INCLUDES) \
	  $(X1_LIBS) $(ROS_LIBS)

suite: require-x1 require-ros $(SUITE)

$(SUITE): x1_bridge.cpp selftest.cpp $(REPO)/bindings/c/skynet_rhal.h \
          $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -o $@ x1_bridge.cpp selftest.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(X1_INCLUDES) $(ROS_INCLUDES) \
	  $(X1_LIBS) $(ROS_LIBS)

motion: require-x1 require-ros $(MOTION)

$(MOTION): x1_bridge.cpp sim_motion.cpp $(REPO)/bindings/c/skynet_rhal.h \
          $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -o $@ x1_bridge.cpp sim_motion.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(X1_INCLUDES) $(ROS_INCLUDES) \
	  $(X1_LIBS) $(ROS_LIBS)

check: $(LIB) $(FAKE) $(SUITE) $(MOTION)
	X1_INFER=$(X1_INFER) ROS_ROOT=$(ROS_ROOT) sh ./run-check.sh

# `check` inside the pinned image, with the repository mounted and a named volume keeping cargo's
# build between runs. Xvfb is the sim's display: its MuJoCo viewer opens a window whether or not
# anybody looks. `clean` first, inside the container, because the driver links the image's
# generated message types, and a library another tree left behind is newer than its sources.
check-sim-docker:
	docker build $(DOCKER_BUILD_ARGS) -t $(SIM_IMAGE) sim
	docker run --rm -v "$(abspath $(REPO))":/src -v skynet-x1-sim-target:/cache \
	  -e CARGO_TARGET_DIR=/cache/target -w /src/drivers/x1 $(SIM_IMAGE) \
	  sh -c 'make clean && xvfb-run -a make check X1_INFER=/opt/src/x1_infer ROS_ROOT=/opt/ros/jazzy'

# The link-chaos contract (item 12.C.4) in the same pinned image: `sim/run-chaos.sh` builds the driver and the fake there, and `chaos.py` puts the kernel's own link emulation on the
# container's loopback, which the driver and the fake share. `--cap-add NET_ADMIN` is what lets `tc` change that loopback; nothing of the host's network is touched.
chaos-docker:
	docker build $(DOCKER_BUILD_ARGS) -t $(SIM_IMAGE) sim
	docker run --rm --cap-add NET_ADMIN -v "$(abspath $(REPO))":/src -w /src/drivers/x1 $(SIM_IMAGE) sh sim/run-chaos.sh

clean:
	rm -f $(LIB) $(FAKE) $(SUITE) $(MOTION)
