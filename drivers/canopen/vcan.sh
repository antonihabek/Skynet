#!/bin/sh
# Run a command with a kernel CAN bus called `vcan0` under it.
#
#   sh vcan.sh <command> [args...]
#
# Uses the machine's own `vcan0` when one is up. Otherwise it makes one inside an unprivileged user
# and network namespace (`unshare -rn`), which needs no root: the namespace is this process's, the
# interface is created in it and disappears with it, and nothing on the machine's own network
# changes. That is how `make check-vcan` measures this driver on a laptop, a container or a CI runner
# with nobody's permission — DRY-COVERAGE-PLAN.md F71 is what the first such run found.
#
# Where neither is possible it fails and says why, rather than running the command on no bus: a
# measurement asked for and not taken is not a pass.
set -eu

if [ "$#" -eq 0 ]; then
  echo "usage: sh vcan.sh <command> [args...]" >&2
  exit 2
fi

if ip link show vcan0 >/dev/null 2>&1; then
  exec "$@"
fi

if unshare -rn /bin/true 2>/dev/null; then
  # `sh -c` rather than a script path, for `drivers/python-host/loop.sh`'s reason: a file's execute
  # bit is not carried by every checkout.
  exec unshare -rn sh -c '
    ip link add dev vcan0 type vcan 2>/dev/null || {
      echo "::error::this kernel will not create a vcan interface (CONFIG_CAN_VCAN, or a module"
      echo "the namespace cannot load). Nothing was measured."
      exit 1
    }
    ip link set up vcan0
    exec "$@"
  ' sh "$@"
fi

echo "::error::no vcan0 on this machine and no unprivileged network namespace to create one in."
echo "Bring one up as root — modprobe vcan && ip link add dev vcan0 type vcan && ip link set up vcan0"
echo "— or run where user namespaces are allowed. Nothing was measured."
exit 1
