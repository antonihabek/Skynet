# The MAVLink row's substrate: ArduPilot's own flight code, built for x86 ("SITL"), one binary per
# vehicle family. This is not a model of an autopilot. `arducopter`, `ardurover`, `ardusub` and
# `arduplane` are the firmware that runs on a flight controller — the same source, the same
# parameter tree, the same arming checks, the same EKF, the same failsafes and the same MAVLink stack —
# built against a simulated board (physics, sensors and motors are the simulator's; everything above
# the hardware abstraction layer is the vehicle's). That is F118's rung 1 for a class of robot this
# repository had no driver for: multirotors, fixed wings, ground vehicles and boats, and submarines.
#
# What is fetched: ArduPilot at a pinned commit (`Copter-4.7.1`, GPL-3.0). Nothing of it is copied into
# this repository and nothing of it is linked: the binaries run as separate processes and are spoken to
# over a socket, which is what makes the licence boundary a process boundary.
#
# What runs beside it is the second opinion: `pymavlink`, the reference MAVLink implementation, which
# `layout_check.py` uses to compare this driver's bytes with the reference's and the live block uses to
# read the same vehicle the driver reads. It is a check-time dependency and never the driver's.
#
#   docker build -t skynet-mavlink-sim drivers/mavlink/sim
#   docker build -t skynet-mavlink-sim:maker-latest --build-arg ARDUPILOT_REF=master drivers/mavlink/sim

FROM ubuntu:24.04 AS build

ARG ARDUPILOT_REF=dbe792162d06cab66c3475fd5556bf7a120f119e

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git ca-certificates python3 python3-venv python3-dev gawk ccache \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir 'empy==3.3.4' pexpect lxml setuptools wheel future

# One fetch at the pin, the commit recorded beside it. Every submodule but the boards' real-time
# operating systems (ChibiOS, NuttX, PX4's): a SITL build reads none of those.
RUN set -e; git init -q /src; cd /src \
    && git fetch -q --depth 1 https://github.com/ArduPilot/ardupilot "$ARDUPILOT_REF" \
    && git checkout -q FETCH_HEAD \
    && git submodule sync -q \
    && git submodule update -q --init --depth 1 \
         $(git config -f .gitmodules --get-regexp '^submodule\..*\.path$' | awk '{print $2}' \
           | grep -v -e ChibiOS -e NuttX -e PX4) \
    && git rev-parse HEAD > /ardupilot.commit

RUN cd /src && . /opt/venv/bin/activate \
    && ./waf configure --board sitl >/dev/null \
    && ./waf -j"$(nproc)" copter rover sub plane 2>&1 | tail -15 \
    && ls -la build/sitl/bin/

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      python3 python3-venv procps ca-certificates libstdc++6 gcc make libc6-dev iproute2 \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m venv /opt/venv && /opt/venv/bin/pip install --no-cache-dir pymavlink==2.4.49

COPY --from=build /src/build/sitl/bin/arducopter /src/build/sitl/bin/ardurover /src/build/sitl/bin/ardusub \
     /src/build/sitl/bin/arduplane /opt/ardupilot/bin/
COPY --from=build /src/Tools/autotest/default_params /opt/ardupilot/default_params
COPY --from=build /ardupilot.commit /opt/ardupilot/commit
COPY plane.parm /opt/ardupilot/default_params/skynet-plane.parm
COPY second.parm /opt/ardupilot/default_params/skynet-second.parm
