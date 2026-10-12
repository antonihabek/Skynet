#!/bin/sh
# The link-chaos contract against ros2_control's own diff_drive_controller (DRY-COVERAGE-PLAN.md item 12.C.4), run by `make chaos-docker` inside `sim/Dockerfile`'s image:
#
#   `tools/faultproxy/chaos.py drivers/ros2_base/chaos.py`: for each profile a fresh controller (`sim/chaos-controller.sh`), a second reader of the base on a path the faults do not touch
#   (`sim/chaos_tap.py`), the driver opened over a clean loopback, the kernel's own link emulation set on that loopback, and the contract's questions asked of the driver.
#
# The container's network is its own: `tc` here changes this container's loopback and nothing of the machine's, which is what `SKYNET_FAULTPROXY_NETEM_OK` says to the relay (outside a
# user namespace of its own it refuses, because the interface may be the machine's). It needs `--cap-add NET_ADMIN` and a kernel with `sch_netem`.
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-chaos.sh [--only profile]...`, never `./`.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)

# ROS's own setup scripts read variables they have not set, so they are sourced before `set -u` would refuse them.
set +u
. "/opt/ros/$ROS_DISTRO/setup.sh"
set -u
export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
# This container runs as root on a checkout that is somebody else's: no bytecode is written into it.
export PYTHONDONTWRITEBYTECODE=1
export PYTHONWARNINGS=ignore
export RCUTILS_COLORIZED_OUTPUT=0
export ROS_DOMAIN_ID=${ROS_DOMAIN_ID:-42}
export SKYNET_FAULTPROXY_NETEM_OK=1

if ! tc qdisc show dev lo >/dev/null 2>&1; then
  echo "::error::this container cannot use tc on its loopback. Run it with --cap-add NET_ADMIN, on a kernel with sch_netem. Nothing was run."
  exit 1
fi

echo "ROS 2 $(printenv ROS_DISTRO), $(sed 's/ /=/' /opt/ros-packages.txt | tr '\n' ' ')"
exec python3 "$REPO/tools/faultproxy/chaos.py" "$BASE/chaos.py" "$@"
