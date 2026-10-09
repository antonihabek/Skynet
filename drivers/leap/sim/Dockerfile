# The LEAP Hand row's substrate: ROBOTIS's own SDK for its servos, run (DRY-COVERAGE-PLAN.md item 12.E).
#
# A LEAP Hand is sixteen Dynamixel servos and no controller, so the maker's software is ROBOTIS's `dynamixel_sdk` (Apache-2.0, PyPI), which every program for the servos is built on. The image installs it into a
# virtual environment at the version DYNAMIXEL_SDK names: 4.1.0 by the hashes in `requirements-layout.txt` (copied in), `latest` unpinned (the nightly's second run, which says so). LeRobot's control table is
# fetched by `fetch-vendor.sh` at its pinned commit (`LEROBOT_COMMIT=HEAD` for the tip). Nothing of either is in this repository, and nothing installed here leaves the container. The hand's authors' own client is
# CC BY-NC 4.0 and is not in the image.
#
#   docker build -t skynet-leap-sdk -f drivers/leap/sim/Dockerfile drivers/leap/sim

FROM ubuntu:24.04

ARG DYNAMIXEL_SDK=4.1.0
ARG LEROBOT_COMMIT=a855570097e6233b23dd42341d02c64f3ccc9267

ENV DEBIAN_FRONTEND=noninteractive
# python3 and a C compiler because `make check` builds the Python host's shared object in the container; curl for LeRobot's table.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl python3 python3-venv gcc make libc6-dev \
    && rm -rf /var/lib/apt/lists/*

COPY fetch-vendor.sh /opt/leap-sim/
COPY requirements-layout.txt /opt/leap-sim/
RUN python3 -m venv /opt/leap-venv \
    && if [ "$DYNAMIXEL_SDK" = latest ]; then /opt/leap-venv/bin/pip install --quiet dynamixel-sdk pyserial; \
       else /opt/leap-venv/bin/pip install --quiet --require-hashes -r /opt/leap-sim/requirements-layout.txt; fi \
    && LEROBOT_COMMIT="$LEROBOT_COMMIT" sh /opt/leap-sim/fetch-vendor.sh /opt/leap-vendor \
    && /opt/leap-venv/bin/pip list 2>/dev/null | grep -i -E "dynamixel|pyserial" > /opt/leap.txt
