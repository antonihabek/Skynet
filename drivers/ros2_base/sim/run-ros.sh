#!/bin/sh
# The ROS 2 base row against ros2_control's own controller (DRY-COVERAGE-PLAN.md item 12.E.3), run by `make check-ros-docker` inside
# `sim/Dockerfile`'s image:
#
#   1. start `robot_state_publisher` and `ros2_control_node` (the controller manager, with mock hardware), then load and activate
#      `joint_state_broadcaster` and `diff_drive_controller`, which is ros2_control's own differential-drive controller;
#   2. `live.py`: the driver against that controller in real time, with a second reader of the same base (`observer.py`, `rclpy` and nothing of this
#      repository's) and ROS's own command-line tools for the facts about the graph;
#   3. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the controller).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-ros.sh <skynet binary>`, never `./`.
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
# One host, one graph: DDS shared memory is the default transport between two processes of one container and it is what a robot's own computer does.
export ROS_DOMAIN_ID=${ROS_DOMAIN_ID:-42}

echo "ROS 2 $(printenv ROS_DISTRO), $(sed 's/ /=/' /opt/ros-packages.txt | tr '\n' ' ')"
status=0
PIDS=""

stop_controller() {
  for pid in $PIDS; do kill "$pid" 2>/dev/null || true; done
  for pid in $PIDS; do wait "$pid" 2>/dev/null || true; done
  PIDS=""
}
trap stop_controller EXIT INT TERM

# $1 is a controllers file; the controller manager reads its parameters from it and the robot's description from the topic robot_state_publisher puts it on.
start_controller() {
  # ROS reads a `-p name:=value` as YAML, and the URDF's own comment has a colon and a space in it: the description goes in without its comments and on one line.
  urdf=$(python3 -c "import re, sys; print(re.sub(r'<!--.*?-->', '', open(sys.argv[1]).read(), flags=re.S).replace(chr(10), ' '))" "$HERE/diffbot.urdf")
  ros2 run robot_state_publisher robot_state_publisher --ros-args -p "robot_description:=$urdf" >/tmp/robot_state_publisher.log 2>&1 &
  PIDS="$!"
  ros2 run controller_manager ros2_control_node --ros-args --params-file "$1" -r '~/robot_description:=/robot_description' >/tmp/controller_manager.log 2>&1 &
  PIDS="$PIDS $!"
  waited=0
  until timeout 5 ros2 control list_controllers >/dev/null 2>&1; do
    waited=$((waited + 1))
    if [ "$waited" -gt 30 ]; then echo "::error::the controller manager never came up."; tail -20 /tmp/controller_manager.log; return 1; fi
    sleep 1
  done
  timeout 90 ros2 run controller_manager spawner joint_state_broadcaster --controller-manager /controller_manager >/tmp/spawn_jsb.log 2>&1 || { cat /tmp/spawn_jsb.log; return 1; }
  timeout 90 ros2 run controller_manager spawner diff_drive_controller --controller-manager /controller_manager >/tmp/spawn_ddc.log 2>&1 || { cat /tmp/spawn_ddc.log; return 1; }
  ros2 control list_controllers
}

# $1 names the pass, $2 is the controllers file, $3 the command topic and $4 the message type the controller then takes (the driver is not told: it reads it off the graph),
# $5 whether to run the sim half of SCTS through the real ABI as well ("abi"). `$SKYNET` is a binary built where the runner builds it, which an older distribution's C
# library may not run, so the older distribution's pass is the live block alone and the ABI is held by the pass that can run it: it does not depend on the message type.
run_pass() {
  echo ""
  echo "== $1"
  start_controller "$2" || { status=1; stop_controller; return; }
  (cd "$BASE" && python3 live.py "$3" /diff_drive_controller/odom /joint_states "$4") || status=1
  if [ "${5:-}" = "abi" ]; then
    echo ""
    echo "== the sim half of SCTS, through the real ABI ($1)"
    make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
    SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_ros2_base \
      "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
      --config "{\"command_topic\":\"$3\",\"odometry_topic\":\"/diff_drive_controller/odom\",\"max_linear\":0.5,\"max_angular\":1.0,\"simulator\":true,\"open_wait_s\":10}" \
      > "/tmp/skynet-ros2-base-scts.txt" 2>&1 || status=1
    sed 's/\x1b\[[0-9;]*m//g' "/tmp/skynet-ros2-base-scts.txt" | grep -E "FAIL|passed|guest|no tier" | head -20
  fi
  stop_controller
}

# Jazzy and what follows it: the controller takes TwistStamped on `<name>/cmd_vel` and nothing else. Humble: `use_stamped_vel: false` makes it take Twist, on `<name>/cmd_vel_unstamped`.
case "$ROS_DISTRO" in
  humble) run_pass "diff_drive_controller with use_stamped_vel false (Twist)" "$HERE/controllers-humble.yaml" /diff_drive_controller/cmd_vel_unstamped Twist ;;
  *) run_pass "diff_drive_controller as shipped (TwistStamped)" "$HERE/controllers.yaml" /diff_drive_controller/cmd_vel TwistStamped abi ;;
esac

exit $status
