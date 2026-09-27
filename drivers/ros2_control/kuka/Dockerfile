# The KUKA row's RSI substrate (DRY-COVERAGE-PLAN.md Phase 8, item 8.14): the `ros2_control` bridge
# hosting KUKA's own hardware plugin, `kuka_rsi_driver/KukaRSIHardwareInterface`, against KUKA's own
# `kuka_rsi_simulator`, which speaks the Robot Sensor Interface's XML-over-UDP from the controller's
# side. Both are KUKA Hungaria's, in `kroshu/kuka_drivers`. No WorkVisual, no OfficeLite, no account.
#
# What the simulator is, from its own source: every 40 ms it sends the controller's telegram — the
# axis positions in degrees, the missed-reply count, the IPOC — waits for the client's reply and
# moves each axis to its start pose plus the correction the reply carries. It checks nothing else.
# So this is the protocol (the telegram, the units, the correction's meaning, the IPOC echo and the
# stop flag) exercised on both ends by the maker's code, and not a controller: no limits, no
# filters, and a tenth of RSI's 4 ms rate.
#
# Everything is fetched at a pinned commit and nothing of it is copied into this repository. The
# pins are the release set ROS Jazzy ships (rosdistro, 2026-09-25): kuka_drivers 2.0.0 (`1a0acc3`,
# the commit that set the version), kuka_external_control_sdk 2.0.0 (its tag), and
# kuka_robot_descriptions 2.0.2 (its tag). All three are Apache-2.0.
#
#   docker build -t skynet-kuka-rsi drivers/ros2_control/kuka
#   docker build -t skynet-kuka-rsi:maker-latest --build-arg KUKA_DRIVERS_REF=master \
#     --build-arg KUKA_SDK_REF=master --build-arg KUKA_DESCRIPTIONS_REF=master \
#     drivers/ros2_control/kuka

FROM ros:jazzy-ros-base

ARG KUKA_DRIVERS_REF=1a0acc3265180d7630eeacd32e6aff0cce8fd861
ARG KUKA_SDK_REF=6e1992cb20a113dd41be8820f8f6584928a2eb94
ARG KUKA_DESCRIPTIONS_REF=fbda927964caa1eb4e408fb0c25fe46b5a0bde3c

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git python3 python3-colcon-common-extensions python3-rosdep python3-numpy \
      ros-jazzy-ros2-control ros-jazzy-hardware-interface ros-jazzy-pluginlib ros-jazzy-xacro \
      ros-jazzy-hardware-interface-testing \
    && rm -rf /var/lib/apt/lists/*

# One shallow fetch per repository at its pin, the commit recorded beside it.
RUN set -e; mkdir -p /opt/kuka_ws/src; cd /opt/kuka_ws/src; \
    fetch() { git init -q "$1" && git -C "$1" fetch -q --depth 1 "$2" "$3" \
      && git -C "$1" checkout -q FETCH_HEAD && echo "$1 $(git -C "$1" rev-parse HEAD)" >> /opt/kuka_ws/commits; }; \
    fetch kuka_drivers https://github.com/kroshu/kuka_drivers "$KUKA_DRIVERS_REF"; \
    fetch kuka-external-control-sdk https://github.com/kroshu/kuka-external-control-sdk "$KUKA_SDK_REF"; \
    fetch kuka_robot_descriptions https://github.com/kroshu/kuka_robot_descriptions "$KUKA_DESCRIPTIONS_REF"

# The RSI plugin, the simulator, the arm's description and what they need, and nothing else:
# the iiQKA and Sunrise drivers, Gazebo and the MoveIt configurations stay out of the image.
# rosdep reads the build dependencies of the packages named here only.
RUN set -e; . /opt/ros/jazzy/setup.sh; cd /opt/kuka_ws; \
    apt-get update; rosdep update --rosdistro jazzy >/dev/null; \
    rosdep install -y --rosdistro jazzy --ignore-src \
      --dependency-types build --dependency-types build_export \
      --dependency-types buildtool --dependency-types buildtool_export \
      --from-paths src/kuka_drivers/kuka_rsi_driver src/kuka_drivers/kuka_drivers_core \
                   src/kuka_drivers/kuka_driver_interfaces src/kuka_drivers/controllers \
                   src/kuka_drivers/kuka_rsi_simulator \
                   src/kuka-external-control-sdk/kuka_external_control_sdk \
                   src/kuka_robot_descriptions/kuka_agilus_support \
                   src/kuka_robot_descriptions/kuka_resources; \
    rm -rf /var/lib/apt/lists/*; \
    colcon build --merge-install \
      --packages-up-to kuka_rsi_driver kuka_rsi_simulator kuka_agilus_support \
      --cmake-args -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF; \
    rm -rf build log

# The live body, by KUKA's own xacro with every argument at its default: `mode` hardware,
# `driver_version` rsi_only, listening on 0.0.0.0:59152, no GPIO — what KUKA's `startup.launch.py`
# brings up for an RSI-only controller.
RUN set -e; . /opt/ros/jazzy/setup.sh; . /opt/kuka_ws/install/setup.sh; \
    xacro /opt/kuka_ws/install/share/kuka_agilus_support/urdf/kr6_r900_sixx.urdf.xacro \
      > /opt/kuka_ws/kr6_r900_sixx_rsi.urdf
