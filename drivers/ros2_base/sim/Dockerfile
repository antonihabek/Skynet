# The ROS 2 base row's substrate: ros2_control's own diff_drive_controller on mock hardware, with ROS 2's command-line tools and the client half
# of this repository (the interpreter, a C compiler for the Python host) in one container.
#
# `ros:<distro>-ros-base` is the ROS project's image of ROS 2 on Ubuntu. The packages added to it are the maker's own and come from the ROS apt
# archive that image already has configured: `controller_manager` (the node that loads and runs controllers), `diff_drive_controller` (the
# controller the driver is held to) and `joint_state_broadcaster` (which publishes the wheels' joint state, the second opinion on what the driver's
# commands made the wheels do). `hardware_interface` carries `mock_components/GenericSystem`, the hardware that echoes its commands back as state.
# `ros2controlcli` is `ros2 control`, the maker's own tool for saying which controller is active.
#
#   docker build -t skynet-ros2-base-sim -f drivers/ros2_base/sim/Dockerfile drivers/ros2_base/sim
#
# `--build-arg ROS_DISTRO=rolling` is the nightly's run against the ROS distribution after Jazzy.

ARG ROS_DISTRO=jazzy
FROM ros:${ROS_DISTRO}-ros-base

# The base image sets ROS_DISTRO in its environment; the package names below are built from it.
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ros-$ROS_DISTRO-controller-manager ros-$ROS_DISTRO-diff-drive-controller ros-$ROS_DISTRO-joint-state-broadcaster \
      ros-$ROS_DISTRO-hardware-interface ros-$ROS_DISTRO-ros2controlcli ros-$ROS_DISTRO-robot-state-publisher \
      gcc make libc6-dev iproute2 procps \
    && rm -rf /var/lib/apt/lists/* \
    && dpkg-query -W -f='${Package} ${Version}\n' ros-$ROS_DISTRO-controller-manager ros-$ROS_DISTRO-diff-drive-controller \
      ros-$ROS_DISTRO-joint-state-broadcaster ros-$ROS_DISTRO-hardware-interface > /opt/ros-packages.txt
