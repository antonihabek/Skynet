#!/bin/sh
# aubo_control, started as ARCS's own systemd unit starts it (`share/service/aubo_control.service`:
# `${PREFIX}/${VERSION}/bin/aubo_control ${AUBO_CONTROL_OPT}`, the values from /etc/default/arcs).
#
# The file is read rather than sourced. It is written for systemd's `EnvironmentFile=`, and the
# installer writes `AUBO_CONTROL_OPT=--workspace /root/arcs_ws` unquoted, which systemd reads as one
# value and a shell reads as an assignment followed by a command.
set -eu
arcs() { sed -n "s/^$1=//p" /etc/default/arcs; }
PREFIX=$(arcs PREFIX)
VERSION=$(arcs VERSION)
# Word-split on purpose: the value is an option and its argument, as the unit's `bash -c` splits it.
# shellcheck disable=SC2046
exec "$PREFIX/$VERSION/bin/aubo_control" $(arcs AUBO_CONTROL_OPT)
