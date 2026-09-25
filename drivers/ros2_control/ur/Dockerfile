# The UR5e row's rung-1 substrate behind this bridge: the `ros2_control` bridge hosting Universal
# Robots' own hardware plugin, `ur_robot_driver/URPositionHardwareInterface`, against URSim —
# UR's controller software, which runs as its own container at the pin in `../ur_rtde/ursim.mk`.
#
# This image is the ROS side: Jazzy, and Universal Robots' four ROS 2 repositories built from the
# commits of the release set ROS Jazzy ships (rosdistro, 2026-09-25): ur_robot_driver 3.9.0,
# ur_client_library 2.15.2, ur_description 3.5.1, ur_msgs 2.6.0. All four are BSD-3-Clause or
# Apache-2.0, and nothing of any of them is copied into this repository; the image fetches them.
#
# The live body the suite drives, `urdf/ur5e_live.urdf`, is generated inside this image by the
# vendor's own xacro (below) and committed with its provenance. The topology it bakes in is the one
# `make check-ursim-docker` builds: the check joins URSim's network namespace, so the controller is
# 127.0.0.1 and so is the driver's reverse server the controller dials back. It was a Docker
# Desktop topology before 2026-09-25 (`host.docker.internal`, and script paths into one machine's
# conda environment), which no runner and no other machine could reproduce.
#
#   docker build -t skynet-ur-ros drivers/ros2_control/ur
#   docker build -t skynet-ur-ros:maker-latest --build-arg UR_ROS2_DRIVER_REF=jazzy \
#     --build-arg UR_CLIENT_LIBRARY_REF=master --build-arg UR_DESCRIPTION_REF=jazzy \
#     --build-arg UR_MSGS_REF=humble drivers/ros2_control/ur

FROM ros:jazzy-ros-base

ARG UR_ROS2_DRIVER_REF=55854cceb1b319dfa6e9333d2c64671c80f4e5f1
ARG UR_CLIENT_LIBRARY_REF=7747b683a68ca5f23d33fde4405e24e55d390b85
ARG UR_DESCRIPTION_REF=710505166d0399b506838d55663f634880d235fe
ARG UR_MSGS_REF=3c584014d9b228e0b2da819db0ca1d887c373e1d

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git python3 python3-colcon-common-extensions python3-rosdep \
      ros-jazzy-ros2-control ros-jazzy-hardware-interface ros-jazzy-pluginlib ros-jazzy-xacro \
    && rm -rf /var/lib/apt/lists/*

# One shallow fetch per repository at its pin, the commit recorded beside it.
RUN set -e; mkdir -p /opt/ur_ws/src; cd /opt/ur_ws/src; \
    fetch() { git init -q "$1" && git -C "$1" fetch -q --depth 1 "$2" "$3" \
      && git -C "$1" checkout -q FETCH_HEAD && echo "$1 $(git -C "$1" rev-parse HEAD)" >> /opt/ur_ws/commits; }; \
    fetch Universal_Robots_ROS2_Driver https://github.com/UniversalRobots/Universal_Robots_ROS2_Driver "$UR_ROS2_DRIVER_REF"; \
    fetch Universal_Robots_Client_Library https://github.com/UniversalRobots/Universal_Robots_Client_Library "$UR_CLIENT_LIBRARY_REF"; \
    fetch Universal_Robots_ROS2_Description https://github.com/UniversalRobots/Universal_Robots_ROS2_Description "$UR_DESCRIPTION_REF"; \
    fetch ur_msgs https://github.com/ros-industrial/ur_msgs "$UR_MSGS_REF"

# Only what the plugin and the description need. rosdep reads the build dependencies of the
# packages built here and nothing else, so rviz, Gazebo and MoveIt stay out of the image.
RUN set -e; . /opt/ros/jazzy/setup.sh; cd /opt/ur_ws; \
    apt-get update; rosdep update --rosdistro jazzy >/dev/null; \
    rosdep install -y --rosdistro jazzy --ignore-src \
      --dependency-types build --dependency-types build_export \
      --dependency-types buildtool --dependency-types buildtool_export \
      --from-paths src/Universal_Robots_ROS2_Driver/ur_robot_driver \
                   src/Universal_Robots_ROS2_Driver/ur_controllers \
                   src/Universal_Robots_ROS2_Driver/ur_dashboard_msgs \
                   src/Universal_Robots_Client_Library src/Universal_Robots_ROS2_Description \
                   src/ur_msgs; \
    rm -rf /var/lib/apt/lists/*; \
    colcon build --merge-install --packages-up-to ur_robot_driver ur_description \
      --cmake-args -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF; \
    rm -rf build log

# The live body, by the vendor's own xacro, with the addresses the check's topology gives it. The
# script and the two RTDE recipes are the files the driver ships, unmodified.
RUN set -e; . /opt/ros/jazzy/setup.sh; . /opt/ur_ws/install/setup.sh; \
    SHARE=/opt/ur_ws/install/share; \
    xacro "$SHARE/ur_robot_driver/urdf/ur.urdf.xacro" name:=ur5e_live ur_type:=ur5e \
      robot_ip:=127.0.0.1 reverse_ip:=127.0.0.1 headless_mode:=true \
      script_filename:="$SHARE/ur_client_library/resources/external_control.urscript" \
      output_recipe_filename:="$SHARE/ur_robot_driver/resources/rtde_output_recipe.txt" \
      input_recipe_filename:="$SHARE/ur_robot_driver/resources/rtde_input_recipe.txt" \
      > /opt/ur_ws/ur5e_live.urdf
