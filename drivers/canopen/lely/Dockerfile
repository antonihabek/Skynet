# The CANopen drivers' independent counterpart (DRY-COVERAGE-PLAN.md Phase 10, item 10.7):
# `ros2_canopen`'s `canopen_fake_slaves`, a CiA 402 drive built on Lely's CANopen stack, on a kernel
# CAN bus. Both CANopen drivers here — the Python one and its Rust port — were written against
# simulated drives from this repository (`skynet_canopen.slave`), so every check they passed was the
# driver agreeing with its author's reading of CiA 301 and CiA 402. Lely's stack is somebody else's
# reading of both, in a form industrial masters run against, and `ros2_canopen`'s slave adds its own
# reading of the CiA 402 state machine on top. Nothing here is written by this repository except the
# device configuration `make_dcf.py` derives from the slave's own (see its header).
#
# What the slave is, from its own source: an SDO server, NMT and PDO engine from Lely; a CiA 402
# state machine and five mode threads (profile position, interpolated position, cyclic
# synchronous position, profile velocity, homing) from `ros2_canopen`, each a mock — no current
# loop, no following error, integer arithmetic in units of a thousand. It is a protocol
# counterpart, not a drive.
#
# Two things are fetched and neither is copied into this repository: `ros2_canopen` at a pinned
# commit — 0.3.4, the release ROS Jazzy ships — and Lely's libraries, which ROS packages
# (`ros-jazzy-lely-core-libraries`, Apache-2.0). The slave is built from source rather than taken
# from apt so the maker-latest run can build `master`; apt's package installs `libmotion_generator.so`
# with no run path, so the run script names it in LD_LIBRARY_PATH for the same reason the build does.
#
#   docker build -t skynet-canopen-lely drivers/canopen/lely
#   docker build -t skynet-canopen-lely:maker-latest --build-arg CANOPEN_REF=master \
#     drivers/canopen/lely

FROM ros:jazzy-ros-base

ARG CANOPEN_REF=169c796cb4b7b13aeb779960a5855d08c30cd6d7

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git python3 python3-colcon-common-extensions \
      ros-jazzy-lely-core-libraries ros-jazzy-rclcpp ros-jazzy-rclcpp-lifecycle \
      ros-jazzy-lifecycle-msgs ros-jazzy-launch-ros ros-jazzy-ros2launch \
      iproute2 can-utils procps \
    && rm -rf /var/lib/apt/lists/*

# One shallow fetch at the pin, the commit recorded beside it.
RUN set -e; mkdir -p /opt/lely_ws/src; cd /opt/lely_ws/src; \
    git init -q ros2_canopen \
      && git -C ros2_canopen fetch -q --depth 1 https://github.com/ros-industrial/ros2_canopen "$CANOPEN_REF" \
      && git -C ros2_canopen checkout -q FETCH_HEAD \
      && echo "ros2_canopen $(git -C ros2_canopen rev-parse HEAD)" > /opt/lely_ws/commits; \
    dpkg-query -W -f='lely_core_libraries ${Version}\n' ros-jazzy-lely-core-libraries >> /opt/lely_ws/commits

# The slave and nothing else: the master, the 402 driver and the controllers stay out.
RUN set -e; . /opt/ros/jazzy/setup.sh; cd /opt/lely_ws; \
    colcon build --merge-install --packages-select canopen_fake_slaves \
      --cmake-args -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF; \
    rm -rf build log
