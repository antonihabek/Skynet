# URSim — Universal Robots' own controller software, in a container — as the three UR rows use it:
# this driver, `drivers/ur_rtde-rs`, and the UR5e behind `drivers/ros2_control`. Included by all
# three Makefiles, so the pin is written once; three copies of a digest drift apart.
#
# The digest is the `5.26` tag's (URSoftware 5.26) as of 2026-09-25, when `latest` pointed at the
# same image. The nightly substrate workflow (`.github/workflows/substrates.yml`, DRY-COVERAGE-PLAN.md
# Phase 8 item 8.11) runs every UR row again with URSIM_IMAGE=universalrobots/ursim_e-series:latest,
# so a URSoftware release that breaks a driver is found the night it is published.

URSIM_IMAGE ?= universalrobots/ursim_e-series:5.26@sha256:91fe32e3517b069dfc6b7b72e8f55f2250e29efcdcb22809891b52a48af6e5a9
URSIM_NAME  ?= skynet-ursim
#: The ports a check on the host reaches URSim through. A check that joins the container's network
#: namespace instead (the UR5e behind `ros2_control`) publishes nothing.
URSIM_PORTS := -p 29999:29999 -p 30001:30001 -p 30002:30002 -p 30003:30003 -p 30004:30004
#: The dashboard opens while URControl is still booting; this waits for a mode (see the script).
URSIM_WAIT  := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))/ursim_wait.py
