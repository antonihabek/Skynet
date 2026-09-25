# The ABB row's EGM substrate (DRY-COVERAGE-PLAN.md Phase 8, item 8.4; docs/robotstudio-runbook.md
# section 6): the `ros2_control` bridge hosting PickNik's `abb_hardware_interface` — the open EGM
# client, over ros-industrial's `abb_libegm` — unmodified, against a fake EGM controller that speaks
# ABB's own `egm.proto` over UDP (`fake_egm.py`, beside this file). No RobotStudio, no account, no
# Windows. Only controller software corroborates the controller, so this complements the runbook's
# virtual controller rather than replacing it.
#
# Everything here is fetched at a pinned commit and nothing of it is copied into this repository:
# abb_ros2 (Apache-2.0; the interface is Apache-2.0/BSD-3), abb_libegm and abb_librws (BSD-3,
# `egm.proto` among them), abb_egm_rws_managers (BSD-3).
#
#   docker build -t skynet-abb-egm drivers/ros2_control/abb
#   docker build -t skynet-abb-egm:maker-latest --build-arg ABB_ROS2_REF=rolling \
#     --build-arg ABB_LIBEGM_REF=master --build-arg ABB_LIBRWS_REF=master \
#     --build-arg ABB_MANAGERS_REF=master drivers/ros2_control/abb

FROM ros:jazzy-ros-base

ARG ABB_ROS2_REF=e6486f7409073cdb2a10d6878bb50191dd50172b
ARG ABB_LIBEGM_REF=2c9350abfb67131cae796805fed8cd6eeb2ab7f9
ARG ABB_LIBRWS_REF=9ba08371482f3975f364011db9b1368fa0fc6b32
ARG ABB_MANAGERS_REF=7e554e53b415df1bbd743392b84a055c499b63d2

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git python3-colcon-common-extensions \
      libboost-all-dev libpoco-dev protobuf-compiler libprotobuf-dev python3-protobuf \
      ros-jazzy-ros2-control ros-jazzy-hardware-interface ros-jazzy-pluginlib \
      ros-jazzy-rclcpp-lifecycle ros-jazzy-realtime-tools ros-jazzy-xacro \
    && rm -rf /var/lib/apt/lists/*

# One shallow fetch per repository at its pin, the commit recorded beside it.
RUN set -e; mkdir -p /opt/abb_ws/src; cd /opt/abb_ws/src; \
    fetch() { git init -q "$1" && git -C "$1" fetch -q --depth 1 "$2" "$3" \
      && git -C "$1" checkout -q FETCH_HEAD && echo "$1 $(git -C "$1" rev-parse HEAD)" >> /opt/abb_ws/commits; }; \
    fetch abb_ros2 https://github.com/PickNikRobotics/abb_ros2 "$ABB_ROS2_REF"; \
    fetch abb_libegm https://github.com/ros-industrial/abb_libegm "$ABB_LIBEGM_REF"; \
    fetch abb_librws https://github.com/ros-industrial/abb_librws "$ABB_LIBRWS_REF"; \
    fetch abb_egm_rws_managers https://github.com/ros-industrial/abb_egm_rws_managers "$ABB_MANAGERS_REF"

# The client and the description, merged into one prefix. The fake's Python bindings are generated
# from the same `egm.proto` the client compiles, so the two ends share ABB's definition and nothing
# else.
RUN . /opt/ros/jazzy/setup.sh \
    && cd /opt/abb_ws \
    && colcon build --merge-install \
         --packages-up-to abb_hardware_interface abb_irb1200_support abb_resources \
         --cmake-args -DCMAKE_BUILD_TYPE=Release \
    && mkdir -p /opt/abb_ws/egm_py \
    && protoc --proto_path=src/abb_libegm/proto --python_out=/opt/abb_ws/egm_py \
         src/abb_libegm/proto/egm.proto \
    && rm -rf build log
