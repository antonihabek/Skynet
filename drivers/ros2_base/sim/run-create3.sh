#!/bin/sh
# The ROS 2 base row against iRobot's own Create 3 simulation (DRY-COVERAGE-PLAN.md item 12.E.3), run by `make check-create3-docker` inside
# `sim/Dockerfile.create3`'s image:
#
#   1. start Gazebo's *server* on `sim/create3-flat.sdf` (the maker's `depot.sdf` with its Fuel model, a download at run time, replaced by a floor), under a virtual display
#      because the Create 3's description loads Gazebo's sensors system with the `ogre` renderer and that opens a GL context; start the clock bridge; then run the maker's own
#      `create3_spawn.launch.py`: the description, the spawn, the bridges, `ros2_control`'s differential-drive controller with the Create 3's own parameters, and iRobot's
#      `motion_control` node, which sits between `/cmd_vel` and the controller as it does on the robot. These are the maker's launch files minus the window and the dock;
#   2. `live_create3.py`: the driver against that robot's simulation in real time, with Gazebo's own pose of the model as the second opinion;
#   3. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the simulation).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-create3.sh <skynet binary>`, never `./`.
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
export ROS_DOMAIN_ID=${ROS_DOMAIN_ID:-44}
# The renderer is Mesa's software rasteriser on a virtual screen: nothing here has a GPU, and nothing the driver reads goes through the renderer.
export LIBGL_ALWAYS_SOFTWARE=1
DESCRIPTION=$(ros2 pkg prefix irobot_create_description)/share
export GZ_SIM_RESOURCE_PATH=$HERE:$DESCRIPTION

echo "ROS 2 $(printenv ROS_DISTRO), $(sed 's/ /=/' /opt/ros-packages.txt | tr '\n' ' ')"
status=0
PIDS=""
stop_simulation() {
  for pid in $PIDS; do kill "$pid" 2>/dev/null || true; done
  for pid in $PIDS; do wait "$pid" 2>/dev/null || true; done
  PIDS=""
}
trap stop_simulation EXIT INT TERM

# Gazebo's server alone: `-r` runs it, `-s` leaves the GUI out. `on_exit_shutdown` ends the launch with it.
xvfb-run -a -s "-screen 0 1280x1024x24" ros2 launch ros_gz_sim gz_sim.launch.py gz_args:="-r -s -v2 $HERE/create3-flat.sdf" on_exit_shutdown:=true >/tmp/gz.log 2>&1 &
PIDS="$!"
waited=0
until gz topic -l 2>/dev/null | grep -q "/world/flat/pose/info"; do
  waited=$((waited + 1))
  if [ "$waited" -gt 90 ]; then echo "::error::Gazebo's server never came up."; tail -20 /tmp/gz.log; exit 1; fi
  sleep 1
done
ros2 run ros_gz_bridge parameter_bridge '/clock@rosgraph_msgs/msg/Clock[ignition.msgs.Clock' >/tmp/clock.log 2>&1 &
PIDS="$PIDS $!"
ros2 launch irobot_create_gz_bringup create3_spawn.launch.py use_rviz:=false spawn_dock:=false world:=flat >/tmp/spawn.log 2>&1 &
PIDS="$PIDS $!"

# The robot is up when the controller's odometry is on the graph and iRobot's motion_control subscribes to the command topic.
waited=0
until timeout 5 ros2 topic echo --once /odom >/dev/null 2>&1 && ros2 topic info /cmd_vel 2>/dev/null | grep -q "Subscription count: [1-9]"; do
  waited=$((waited + 1))
  if [ "$waited" -gt 90 ]; then echo "::error::the Create 3 never appeared on the graph."; tail -20 /tmp/spawn.log; tail -20 /tmp/gz.log; exit 1; fi
  sleep 1
done
sleep 3

(cd "$BASE" && python3 live_create3.py) || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (Create 3 in Gazebo)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_ros2_base \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config '{"command_topic":"/cmd_vel","odometry_topic":"/odom","max_linear":0.3,"max_angular":1.8,"simulator":true,"open_wait_s":30,"stop_settle_s":8}' \
  > /tmp/skynet-ros2-base-scts-create3.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-ros2-base-scts-create3.txt | grep -E "FAIL|passed|guest|no tier" | head -20

exit $status
