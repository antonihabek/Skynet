# skynet-ros2-control — a body behind `ros2_control`, and its suite.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 2 table asks for a *"fake `hardware_interface`"* as the substrate,
# and `ros2_control` ships one: `mock_components/GenericSystem`, the fake its own tests run against.
# So this driver is exercised against the real framework with no robot, and against a fake nobody
# in this repository wrote.
#
#   make check     the suite, against mock_components
#   make lib       libskynet_ros2_control.so
#   make check-conformance   the sim half of SCTS through the real ABI
#   make check-live   the suite with its URSim block, when URSim answers
#
# Needs ROS 2 with ros2_control. It refuses rather than building half of itself when that is absent,
# because a driver that compiled without its framework would be a driver nobody could load.
#
# Either ROS works: apt's (`/opt/ros/jazzy`, the default and what CI installs), or RoboStack's
# conda-forge build in user space, with that environment's own compiler —
#   make check ROS_ROOT=$CONDA_PREFIX CXX=$CONDA_PREFIX/bin/x86_64-conda-linux-gnu-c++

ROS_DISTRO ?= jazzy
ROS_ROOT   ?= /opt/ros/$(ROS_DISTRO)

CXX      ?= c++
CXXFLAGS ?= -std=c++17 -O2 -Wall -Wextra -fPIC -DSOURCE_DIR=\"$(CURDIR)\"
REPO     := ../..

# Every ROS package's include directory, because a message package's headers include another's by
# name and there is no single root that satisfies them all. *Package* directories only — ament
# installs a package's headers as `include/<pkg>/<pkg>/`, so a directory is added when it holds one
# named after itself. Adding every directory under `include/` worked on `/opt/ros`, where nothing
# else lives there, and broke on a conda-forge ROS (RoboStack), where `include/dds/features.h` then
# shadowed glibc's `<features.h>` and the standard library stopped compiling. Anything that is not an
# ament package is reached through `include/` itself, as `<fmt/format.h>` is.
ROS_INCLUDES := -I$(ROS_ROOT)/include $(shell for dir in $(ROS_ROOT)/include/*/; do \
                  [ -d "$$dir$$(basename "$$dir")" ] && echo "-I$$dir"; done 2>/dev/null)
ROS_LIBS     := -L$(ROS_ROOT)/lib -lhardware_interface -lrclcpp -lrclcpp_lifecycle -lrcutils -lrmw -lrcl -lrcl_lifecycle -ltracetools -lrealtime_tools -lrcpputils -lfmt -Wl,--allow-shlib-undefined

# The plugin loader dlopens hardware plugins and the URDF parser plugins beside them, and those pull
# in vendored libraries that are not on the default search path.
VENDOR_LIBS := $(shell ls -d $(ROS_ROOT)/opt/*/lib 2>/dev/null | paste -sd: )
RUN_ENV := AMENT_PREFIX_PATH=$(ROS_ROOT) \
           LD_LIBRARY_PATH=$(ROS_ROOT)/lib:$(ROS_ROOT)/lib/x86_64-linux-gnu:$(VENDOR_LIBS)

LIB := libskynet_ros2_control.so

.PHONY: all lib check check-conformance check-live require-ros clean

all: check

require-ros:
	@test -d "$(ROS_ROOT)/include/hardware_interface" || { \
	  echo "::error::no ros2_control at $(ROS_ROOT). This driver is a ros2_control host and there"; \
	  echo "is nothing to host without it. On Ubuntu: apt install ros-$(ROS_DISTRO)-ros2-control,"; \
	  echo "or set ROS_ROOT. Nothing was built and nothing was measured."; \
	  exit 1; }

lib: require-ros $(LIB)

$(LIB): ros2_control_bridge.cpp $(REPO)/bindings/c/skynet_rhal.h $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -shared -o $@ ros2_control_bridge.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(ROS_INCLUDES) $(ROS_LIBS)

selftest: require-ros ros2_control_bridge.cpp selftest.cpp $(REPO)/bindings/c/skynet_rhal.h \
          $(REPO)/bindings/cpp/skynet_rhal.hpp
	$(CXX) $(CXXFLAGS) -o selftest ros2_control_bridge.cpp selftest.cpp \
	  -I$(REPO)/bindings/c -I$(REPO)/bindings/cpp $(ROS_INCLUDES) $(ROS_LIBS)

check: selftest
	$(RUN_ENV) ./selftest

# The sim half of SCTS against this body, through `dlopen` and the real ABI.
check-conformance: $(LIB)
	$(RUN_ENV) cargo run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check ./$(LIB) --config 'urdf=$(CURDIR)/urdf/two_joint_mock.urdf roles=shoulder:right_arm.shoulder.yaw;elbow:right_arm.elbow.flex'

# The suite with its URSim block: the dashboard is probed first, and when nothing
# answers the target says so and passes, because CI has no simulator. When URSim is
# up, SKYNET_URSIM_LIVE=1 runs the live block — power on, brake release, and the
# elbow out 0.08 rad and back — against urdf/ur5e_live.urdf.
check-live: selftest
	@python3 -c "import socket; s = socket.create_connection(('127.0.0.1', 29999), timeout=5); s.close()" 2>/dev/null || { \
	  echo "URSim's dashboard is not answering on 127.0.0.1:29999 — the live block is skipped (see the README)."; \
	  exit 0; }
	$(RUN_ENV) SKYNET_URSIM_LIVE=1 ./selftest

clean:
	rm -f selftest $(LIB)
