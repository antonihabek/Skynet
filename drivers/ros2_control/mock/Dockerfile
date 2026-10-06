# The mock-components block's image: ROS 2 Jazzy, `ros2_control`'s hardware interface and its own test components, and a C++
# toolchain — what `make check` and `make check-conformance` need and what `ci.yml`'s `ros2_control` step installs on a runner.
#
# `make check` runs the bridge against `mock_components` (the fake `ros2_control`'s own tests run against) with no vendor plugin and
# no controller, so nothing here is a vendor's. The image exists so the seeding campaign (`tools/seed-defects`, item 12.C.3) can
# build and run the suite per seed on a machine with no ROS 2 of its own (DRY-COVERAGE-PLAN.md, fifty-sixth session) and so a
# contributor on such a machine can run the block:
#
#   docker build -t skynet-ros2-control-mock drivers/ros2_control/mock
#   docker run --rm -v "$PWD":/src -w /src/drivers/ros2_control skynet-ros2-control-mock sh -c 'make clean && make check'
#
# `ros-jazzy-hardware-interface-testing` is the one the suite refuses to run without: the F142 fixture is its own test component.

ARG ROS_DISTRO=jazzy
FROM ros:${ROS_DISTRO}-ros-base

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ros-$ROS_DISTRO-hardware-interface ros-$ROS_DISTRO-hardware-interface-testing \
      build-essential iproute2 procps \
    && rm -rf /var/lib/apt/lists/* \
    && dpkg-query -W -f='${Package} ${Version}\n' ros-$ROS_DISTRO-hardware-interface ros-$ROS_DISTRO-hardware-interface-testing \
      > /opt/ros-packages.txt
