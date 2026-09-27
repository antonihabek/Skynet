# The Doosan row's rung-1 substrate (DRY-COVERAGE-PLAN.md Phase 8, item 8.3): the `ros2_control`
# bridge hosting Doosan's own hardware plugin, `dsr_hardware2/DRHWInterface`, against DRCF — Doosan's
# controller emulator, which runs as its own container (`doosanrobot/dsr_emulator`, pinned in the
# Makefile beside `check-drcf-docker`).
#
# This image is the ROS side: Jazzy with ros2_control, and Doosan's `doosan-robot2` built from a
# pinned commit of its `jazzy` branch, up to the plugin and the description. Built with
# `DRCF_VER=3` because the emulator reports controller version GF03020000, and Doosan's readme
# builds that way "for Version 3.x Controller". Doosan's packages are Apache-2.0; the plugin
# links Doosan's prebuilt `libDRFL.a`, which the same repository ships. Nothing of either is copied
# into this repository.
#
#   docker build -t skynet-doosan-drcf drivers/ros2_control/doosan
#   docker build -t skynet-doosan-drcf:maker-latest --build-arg DOOSAN_ROBOT2_REF=jazzy drivers/ros2_control/doosan

FROM ros:jazzy-ros-base

ARG DOOSAN_ROBOT2_REF=e5e2d7f4fe11865b9ddf4ad658362bc8e73296e0

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git curl python3-colcon-common-extensions \
      libpoco-dev libyaml-cpp-dev libfmt-dev \
      ros-jazzy-ros2-control ros-jazzy-hardware-interface ros-jazzy-controller-interface \
      ros-jazzy-controller-manager ros-jazzy-realtime-tools ros-jazzy-control-msgs \
      ros-jazzy-trajectory-msgs ros-jazzy-kdl-parser ros-jazzy-moveit-msgs ros-jazzy-xacro \
      ros-jazzy-pluginlib ros-jazzy-rcpputils ros-jazzy-hardware-interface-testing \
    && rm -rf /var/lib/apt/lists/*

# Only what the plugin needs, merged into one prefix so a single path puts it on the ament index.
RUN git init -q /opt/doosan_ws/src/doosan-robot2 \
    && git -C /opt/doosan_ws/src/doosan-robot2 fetch -q --depth 1 \
         https://github.com/DoosanRobotics/doosan-robot2 "$DOOSAN_ROBOT2_REF" \
    && git -C /opt/doosan_ws/src/doosan-robot2 checkout -q FETCH_HEAD \
    && git -C /opt/doosan_ws/src/doosan-robot2 rev-parse HEAD > /opt/doosan_ws/doosan-robot2.commit \
    && . /opt/ros/jazzy/setup.sh \
    && cd /opt/doosan_ws \
    && colcon build --merge-install --packages-up-to dsr_hardware2 dsr_description2 \
         --cmake-args -DDRCF_VER=3 -DCMAKE_BUILD_TYPE=Release \
    && rm -rf build log
