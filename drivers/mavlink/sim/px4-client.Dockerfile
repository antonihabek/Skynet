# The client half of the PX4 row's substrate. PX4's own image (`px4io/px4-sitl-sih`, run by `sim/run-px4.sh`) holds the firmware and
# nothing else — no Python, no compiler — so this driver runs beside it, in its network namespace, from here: the interpreter, the C
# compiler the Python host needs, `tc` to impair the loopback, and `pymavlink`, the reference MAVLink implementation the live block
# reads the same vehicle with as a second opinion. A check-time dependency, never the driver's.
#
#   docker build -t skynet-px4-client -f drivers/mavlink/sim/px4-client.Dockerfile drivers/mavlink/sim

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      python3 python3-venv procps ca-certificates gcc make libc6-dev iproute2 \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m venv /opt/venv && /opt/venv/bin/pip install --no-cache-dir pymavlink==2.4.49
