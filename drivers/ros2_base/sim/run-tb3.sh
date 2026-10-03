#!/bin/sh
# The ROS 2 base row against ROBOTIS's own TurtleBot3 simulation (DRY-COVERAGE-PLAN.md item 12.E.3), run by `make check-tb3-docker` inside
# `sim/Dockerfile.turtlebot3`'s image:
#
#   1. start Gazebo's *server* on the package's empty world (no GUI: nothing here renders), publish the burger's description, spawn it and start the bridge that puts
#      `cmd_vel` and `odom` on the ROS 2 graph. These are the maker's own launch files, run the way the maker's `turtlebot3_world.launch.py` runs them minus the window;
#   2. `live_tb3.py`: the driver against that robot's simulation in real time, with Gazebo's own pose of the model as the second opinion;
#   3. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the simulation).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-tb3.sh <skynet binary>`, never `./`.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)

# ROS's own setup scripts read variables they have not set, so they are sourced before `set -u` would refuse them.
set +u
. "/opt/ros/$ROS_DISTRO/setup.sh"
set -u
export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
export RCUTILS_COLORIZED_OUTPUT=0
export TURTLEBOT3_MODEL=${TURTLEBOT3_MODEL:-burger}
export ROS_DOMAIN_ID=${ROS_DOMAIN_ID:-43}
PACKAGE=$(ros2 pkg prefix turtlebot3_gazebo)/share/turtlebot3_gazebo
export GZ_SIM_RESOURCE_PATH=$PACKAGE/models

echo "ROS 2 $(printenv ROS_DISTRO), $(sed 's/ /=/' /opt/ros-packages.txt | tr '\n' ' ')"
echo "TurtleBot3 model: $TURTLEBOT3_MODEL"
status=0
PIDS=""
stop_simulation() {
  for pid in $PIDS; do kill "$pid" 2>/dev/null || true; done
  for pid in $PIDS; do wait "$pid" 2>/dev/null || true; done
  PIDS=""
}
trap stop_simulation EXIT INT TERM

# Gazebo's server alone: `-r` runs it, `-s` leaves the GUI out. `on_exit_shutdown` ends the launch with it.
ros2 launch ros_gz_sim gz_sim.launch.py gz_args:="-r -s -v2 $PACKAGE/worlds/empty_world.world" on_exit_shutdown:=true >/tmp/gz.log 2>&1 &
PIDS="$!"
waited=0
until gz topic -l 2>/dev/null | grep -q "/world/default/pose/info"; do
  waited=$((waited + 1))
  if [ "$waited" -gt 60 ]; then echo "::error::Gazebo's server never came up."; tail -20 /tmp/gz.log; exit 1; fi
  sleep 1
done
ros2 launch turtlebot3_gazebo robot_state_publisher.launch.py use_sim_time:=true >/tmp/robot_state_publisher.log 2>&1 &
PIDS="$PIDS $!"
ros2 launch turtlebot3_gazebo spawn_turtlebot3.launch.py x_pose:=0.0 y_pose:=0.0 >/tmp/spawn.log 2>&1 &
PIDS="$PIDS $!"

# The model is spawned and the bridge is up when the plugin's odometry is on the graph and something subscribes to the command topic.
waited=0
until timeout 5 ros2 topic echo --once /odom >/dev/null 2>&1 && ros2 topic info /cmd_vel 2>/dev/null | grep -q "Subscription count: [1-9]"; do
  waited=$((waited + 1))
  if [ "$waited" -gt 40 ]; then echo "::error::the TurtleBot3 never appeared on the graph."; tail -20 /tmp/spawn.log; exit 1; fi
  sleep 1
done
sleep 2

(cd "$BASE" && python3 live_tb3.py) || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (TurtleBot3 in Gazebo)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_ros2_base \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config '{"command_topic":"/cmd_vel","odometry_topic":"/odom","max_linear":0.22,"max_angular":2.0,"simulator":true,"open_wait_s":20,"stop_settle_s":4}' \
  > /tmp/skynet-ros2-base-scts-tb3.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-ros2-base-scts-tb3.txt | grep -E "FAIL|passed|guest|no tier" | head -20

exit $status
