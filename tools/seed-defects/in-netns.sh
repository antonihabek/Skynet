#!/bin/sh
# Run a command in a network namespace of its own, with a loopback interface up.
#
#   sh in-netns.sh <command> [args...]
#
# For a suite that binds fixed ports on 127.0.0.1 — the scripted controller in `drivers/ur_rtde-rs/tests/scripted.rs` listens on
# 29999, 30002 and 30004, which are a UR's — so that the seeding campaign's workers, which run suites side by side, do not meet
# on them, and a URSim container listening on the machine's own loopback cannot be in their way. The namespace is this
# process's, needs no root, and goes when the command does; nothing on the machine's own network changes (the same idea as
# `drivers/canopen/vcan.sh`, which makes a vcan0 the same way).
#
# Where the machine will not make one it says so and fails, rather than running the command on the shared loopback: a suite that
# binds a port somebody else holds fails for that reason, and a campaign would count it as a defect noticed.
set -eu

if [ "$#" -eq 0 ]; then
  echo "usage: sh in-netns.sh <command> [args...]" >&2
  exit 2
fi

if unshare -rn /bin/true 2>/dev/null; then
  exec unshare -rn sh -c 'ip link set lo up && exec "$@"' sh "$@"
fi

echo "::error::this machine will not make an unprivileged network namespace (unshare -rn), which the seeding campaign needs so its" >&2
echo "workers do not share ports. On Ubuntu 24.04 the cause is usually AppArmor: sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0" >&2
exit 1
