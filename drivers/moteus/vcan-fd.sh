#!/bin/sh
# Run a command with a kernel CAN-FD bus called `vcan0` under it.
#
#   sh vcan-fd.sh <command> [args...]
#
# `../canopen/vcan.sh`'s way, with one difference that is the whole point of this file: a moteus's frames are CAN-FD, and a virtual CAN interface carries one only
# if its MTU is 72 (a classic frame is 16). So the interface is made with `mtu 72`. It uses the machine's own `vcan0` when one is up *and* already FD-capable;
# otherwise it makes one inside an unprivileged user and network namespace (`unshare -rn`), which needs no root: the namespace is this process's, the interface is
# created in it and disappears with it, and nothing on the machine's own network changes.
#
# Where neither is possible it fails and says why, rather than running the command on no bus: a measurement asked for and not taken is not a pass.
set -eu

if [ "$#" -eq 0 ]; then
  echo "usage: sh vcan-fd.sh <command> [args...]" >&2
  exit 2
fi

if ip -d link show vcan0 2>/dev/null | grep -q "mtu 72"; then
  exec "$@"
fi

if unshare -rn /bin/true 2>/dev/null; then
  # `sh -c` rather than a script path, for `drivers/python-host/loop.sh`'s reason: a file's execute bit is not carried by every checkout.
  exec unshare -rn sh -c '
    ip link add dev vcan0 type vcan 2>/dev/null || {
      echo "::error::this kernel will not create a vcan interface (CONFIG_CAN_VCAN, or a module"
      echo "the namespace cannot load). Nothing was measured."
      exit 1
    }
    ip link set vcan0 mtu 72 || {
      echo "::error::this kernel will not give a vcan interface the MTU a CAN-FD frame needs (72). Nothing was measured."
      exit 1
    }
    ip link set up vcan0
    exec "$@"
  ' sh "$@"
fi

echo "::error::no CAN-FD vcan0 on this machine and no unprivileged network namespace to create one in."
echo "Bring one up as root — modprobe vcan && ip link add dev vcan0 type vcan && ip link set vcan0 mtu 72 && ip link set up vcan0"
echo "— and nothing was measured."
exit 1
