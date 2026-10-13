# The host's half of `frozen.py` (DRY-COVERAGE-PLAN.md item 12.C.4), shared by every vendor's runner: source it, then call `frozen_cycle`.
#
#   . ./frozen-host.sh
#   frozen_cycle <controller container> <driver image> <repository root> <the command the driver's container runs>
#
# The driver's container has no Docker, so the two talk through files in a directory both can see: `frozen.py` writes `ready` when the body is open and healthy and waits for `frozen`;
# this script pauses the controller's container (the cgroup freezer: every process of the simulator or emulator stops at once, its connections stay open and nothing is sent) and writes
# `frozen`; `frozen.py` measures, writes `measured` and waits for `thawed`; this script lets the controller go and writes `thawed`; the last word is `done`. The driver's container joins the
# controller's network namespace, so the plugin dials 127.0.0.1 as it does in every other live block of this driver. Returns the driver container's exit status.
frozen_cycle() {
  controller=$1; image=$2; repo=$3; command=$4
  handshake=$(mktemp -d)
  chmod 777 "$handshake"
  driver_name=${controller}-driver
  docker rm -f "$driver_name" >/dev/null 2>&1
  docker run --rm --name "$driver_name" --network "container:$controller" -v "$repo":/src -v "$handshake":/handshake -w /src/drivers/ros2_control "$image" \
    sh -c "$command" &
  driver=$!
  waiting_for() {   # a file, or the driver's end: 0 when the file came, 1 when the driver's container ended without it
    while [ ! -e "$handshake/$1" ]; do
      kill -0 "$driver" 2>/dev/null || return 1
      sleep 0.1
    done
  }
  if waiting_for ready; then
    docker pause "$controller" >/dev/null && echo "== $controller paused $(date +%T)"
    touch "$handshake/frozen"
    if waiting_for measured; then
      docker unpause "$controller" >/dev/null && echo "== $controller let go $(date +%T)"
      touch "$handshake/thawed"
    fi
  fi
  wait "$driver"
  status=$?
  docker unpause "$controller" >/dev/null 2>&1
  docker rm -f "$driver_name" >/dev/null 2>&1
  rm -rf "$handshake"
  return $status
}
