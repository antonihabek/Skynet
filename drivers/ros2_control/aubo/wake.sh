#!/bin/sh
# Bring ARCS's arm to Running, as a person at the pendant does before a ros2_control session, and be a no-op when it is there already (DRY-COVERAGE-PLAN.md item 12.C.4: `frozen.py` runs
# this before it opens the body and again after a frozen controller is let go, because a simulator stopped for six seconds may have fallen back).
#
#   sh aubo/wake.sh <aubo-workspace>
WS=${1:?the AUBO workspace, /opt/aubo_ws in the image}
running() { "$WS/bin/arcs_state" 2>/dev/null | grep -q " mode=RobotModeType.Running "; }
running && exit 0
"$WS/bin/aubo_example_startup" >/dev/null 2>&1
running
