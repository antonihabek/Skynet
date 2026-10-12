#!/bin/sh
# ros2_control's own diff_drive_controller on mock hardware, started and left running, for the link-chaos subject (`drivers/ros2_base/chaos.py`,
# DRY-COVERAGE-PLAN.md item 12.C.4). The same four processes `sim/run-ros.sh` starts for the live block, in the same order, and then they are
# waited on instead of used: the subject starts this once per profile, so that every profile meets a controller that has not been driving for the ones
# before it, and stops it by signalling this script's process group.
#
#   sh sim/chaos-controller.sh <controllers file>      prints `controllers up` when both controllers are active, then waits
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. ROS 2's setup scripts read variables they have not set, so they are sourced
# before `set -u` would refuse them.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
set +u
. "/opt/ros/$ROS_DISTRO/setup.sh"
set -u
export PYTHONWARNINGS=ignore
export RCUTILS_COLORIZED_OUTPUT=0

# ROS reads a `-p name:=value` as YAML, and the URDF's own comment has a colon and a space in it: the description goes in without its comments and on one line.
urdf=$(python3 -c "import re, sys; print(re.sub(r'<!--.*?-->', '', open(sys.argv[1]).read(), flags=re.S).replace(chr(10), ' '))" "$HERE/diffbot.urdf")

PIDS=""
cleanup() {
  for pid in $PIDS; do kill "$pid" 2>/dev/null || true; done
  for pid in $PIDS; do wait "$pid" 2>/dev/null || true; done
  PIDS=""
}
trap 'cleanup; exit 0' EXIT INT TERM

ros2 run robot_state_publisher robot_state_publisher --ros-args -p "robot_description:=$urdf" >/tmp/robot_state_publisher.log 2>&1 &
PIDS="$!"
ros2 run controller_manager ros2_control_node --ros-args --params-file "$1" -r '~/robot_description:=/robot_description' >/tmp/controller_manager.log 2>&1 &
PIDS="$PIDS $!"

waited=0
until timeout 5 ros2 control list_controllers >/dev/null 2>&1; do
  waited=$((waited + 1))
  if [ "$waited" -gt 30 ]; then echo "::error::the controller manager never came up."; tail -20 /tmp/controller_manager.log; exit 1; fi
  sleep 1
done
timeout 90 ros2 run controller_manager spawner joint_state_broadcaster --controller-manager /controller_manager >/tmp/spawn_jsb.log 2>&1 || { cat /tmp/spawn_jsb.log; exit 1; }
timeout 90 ros2 run controller_manager spawner diff_drive_controller --controller-manager /controller_manager >/tmp/spawn_ddc.log 2>&1 || { cat /tmp/spawn_ddc.log; exit 1; }
echo "controllers up"

# Waited on, so that a signal to this script is what ends the controller.
while :; do sleep 1; done
