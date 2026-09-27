# The AUBO row's ROS side (DRY-COVERAGE-PLAN.md Phase 8, item 8.15): AUBO's own `ros2_control`
# hardware plugin, `aubo_driver/AuboHardwareInterface` from `AuboRobot/aubo_ros2_driver`, built on
# ROS 2 Jazzy with one line of it deleted (`aubo-jazzy.patch`, DRY-COVERAGE-PLAN.md F144), for the
# bridge to host against ARCS — AUBO's controller software, in `arcs/Dockerfile`'s image.
#
# The plugin speaks AUBO's SDK: JSON-RPC on 30004 for servo mode and `servoJoint`, RTDE on 30010 for
# the joint stream. The SDK is a binary AUBO publishes, and the plugin's own CMakeLists fetches it
# (`aubo_sdk-0.24.1-rc.3`) from AUBO's download server at configure time — left as AUBO wrote it.
#
# The plugin commands nothing until the arm is powered on and its brakes released, which on a real
# controller is the operator at the pendant. Here it is AUBO's own SDK example, `example_startup.cpp`
# from the same SDK archive, compiled unmodified as `aubo_example_startup`. `arcs_state.cpp` beside
# this file is the one thing written here: it reads the controller's own account of itself (mode,
# safety mode, servo mode, joints) so `run-arcs.sh` can say what ARCS saw.
#
# Pins: aubo_ros2_driver at `8568407` (its `main` head on 2026-09-26; the repository has no tags),
# and its `aubo_description` submodule at the commit that head records, `47fa5e0` — the same
# commit `../urdf/aubo_i5s_mock.urdf` was taken from. BSD-3-Clause and BSD. The image also carries
# `ros2_control`'s own test components, which the suite's default half needs (F142's fixture).
#
#   docker build -t skynet-aubo-ros drivers/ros2_control/aubo
#   docker build -t skynet-aubo-ros:maker-latest --build-arg AUBO_DRIVER_REF=main \
#     --build-arg AUBO_DESCRIPTION_REF=main drivers/ros2_control/aubo

FROM ros:jazzy-ros-base

ARG AUBO_DRIVER_REF=85684075d6ff06c5385e39611208e99ebf0f94c6
ARG AUBO_DESCRIPTION_REF=47fa5e02fa873f27f7e812d31f31e3f4cf5e56b1

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git python3 python3-colcon-common-extensions python3-rosdep \
      ros-jazzy-ros2-control ros-jazzy-hardware-interface ros-jazzy-pluginlib ros-jazzy-xacro \
      ros-jazzy-hardware-interface-testing libboost-dev \
    && rm -rf /var/lib/apt/lists/*

# One shallow fetch per repository at its pin, the commit recorded beside it. The description is
# fetched into the submodule's path, where the driver repository expects it.
RUN set -e; mkdir -p /opt/aubo_ws/src; cd /opt/aubo_ws/src; \
    fetch() { git init -q "$1" && git -C "$1" fetch -q --depth 1 "$2" "$3" \
      && git -C "$1" checkout -q FETCH_HEAD && echo "$1 $(git -C "$1" rev-parse HEAD)" >> /opt/aubo_ws/commits; }; \
    fetch aubo_ros2_driver https://github.com/AuboRobot/aubo_ros2_driver "$AUBO_DRIVER_REF"; \
    rmdir aubo_ros2_driver/aubo_description; \
    fetch aubo_ros2_driver/aubo_description https://github.com/AuboRobot/aubo_description "$AUBO_DESCRIPTION_REF"

# AUBO's plugin is written against Humble, and one line of it does not build on Jazzy: its header
# includes `hardware_interface/visibility_control.h`, which Jazzy's `hardware_interface` no longer
# ships, and uses nothing from it. `aubo-jazzy.patch` deletes that include and nothing else; a run
# at AUBO's latest applies it to the new head and fails loudly if it no longer fits.
COPY aubo-jazzy.patch /opt/aubo_ws/aubo-jazzy.patch
RUN git -C /opt/aubo_ws/src/aubo_ros2_driver apply /opt/aubo_ws/aubo-jazzy.patch

# The plugin, the two message packages it includes, and the description, and nothing else: Gazebo,
# MoveIt and the demo planner stay out. rosdep reads build dependencies only, as the KUKA image does.
# No build type is passed: AUBO's README builds with a bare `colcon build`, its CMakeLists then
# chooses RelWithDebInfo, and that is the configuration whose SDK library its install rule copies.
RUN set -e; . /opt/ros/jazzy/setup.sh; cd /opt/aubo_ws; \
    apt-get update; rosdep update --rosdistro jazzy >/dev/null; \
    rosdep install -y --rosdistro jazzy --ignore-src \
      --dependency-types build --dependency-types build_export \
      --dependency-types buildtool --dependency-types buildtool_export \
      --from-paths src/aubo_ros2_driver/aubo_ros2_driver src/aubo_ros2_driver/aubo_msgs \
                   src/aubo_ros2_driver/aubo_dashboard_msgs src/aubo_ros2_driver/aubo_description; \
    rm -rf /var/lib/apt/lists/*; \
    colcon build --merge-install \
      --packages-up-to aubo_ros2_driver aubo_description \
      --cmake-args -DBUILD_TESTING=OFF

# The operator's two tools, against the SDK the plugin's build fetched: AUBO's startup example,
# unmodified, and the state reader written here. Then the build tree goes.
COPY arcs_state.cpp /opt/aubo_ws/arcs_state.cpp
RUN set -e; cd /opt/aubo_ws; \
    sdk=$(ls -d build/aubo_ros2_driver/_deps/aubo_sdk-src); \
    cp -a "$sdk/lib/." install/lib/; \
    mkdir -p bin; \
    for tool in "$sdk/share/example/c++/example_startup.cpp:aubo_example_startup" \
                "arcs_state.cpp:arcs_state"; do \
      g++ -std=c++17 -O2 -o "bin/${tool#*:}" "${tool%%:*}" -I"$sdk/include" \
        -Linstall/lib -laubo_sdk -Wl,-rpath,/opt/aubo_ws/install/lib -lpthread; \
    done; \
    cat "$sdk/VERSION" > sdk-version; \
    rm -rf build log

# The live body, by AUBO's own xacro: `aubo_ros2.xacro` with the arm ARCS simulates (aubo_i5, the
# model its simulated arm is configured as), the controller at 127.0.0.1, and AUBO's plugin rather
# than ros2_control's mock — what AUBO's `aubo_control.launch.py` builds for `use_fake_hardware:=false`.
RUN set -e; . /opt/ros/jazzy/setup.sh; . /opt/aubo_ws/install/setup.sh; \
    xacro /opt/aubo_ws/install/share/aubo_description/urdf/xacro/inc/aubo_ros2.xacro \
      aubo_type:=aubo_i5 robot_ip:=127.0.0.1 use_fake_hardware:=false \
      > /opt/aubo_ws/aubo_i5_arcs.urdf
