# The Tello row's substrate: djitellopy, run (DRY-COVERAGE-PLAN.md item 12.E.11).
#
# A Tello is a UDP socket that takes the SDK 2.0 User Guide's text commands, and its maker's own sample client is on an address that no longer answers. `djitellopy` (MIT) is the client of the same guide that everyone
# else uses: another person's reading of the document, which the layout check runs unmodified against a drone written in this repository. The image installs it into a virtual environment with `numpy` and `av`, the two
# imports its `tello` module needs (its metadata also names `opencv-python` and `pillow`, which nothing this check runs imports, so the install is `--no-deps`): by the hashes in `requirements-layout.txt` (copied in),
# or unpinned when TELLO_DJITELLOPY is `latest` (the nightly's second run, which says so). Nothing installed here leaves the container, and none of it is in this repository.
#
#   docker build -t skynet-tello-sdk -f drivers/tello/sim/Dockerfile drivers/tello/sim

FROM ubuntu:24.04

ARG TELLO_DJITELLOPY=2.5.0

ENV DEBIAN_FRONTEND=noninteractive
# python3 and a C compiler because `make check` builds the Python host's shared object in the container.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates python3 python3-venv gcc make libc6-dev \
    && rm -rf /var/lib/apt/lists/*

COPY requirements-layout.txt /opt/tello-sim/
RUN python3 -m venv /opt/tello-venv \
    && if [ "$TELLO_DJITELLOPY" = latest ]; then /opt/tello-venv/bin/pip install --quiet --no-deps djitellopy numpy av; \
       else /opt/tello-venv/bin/pip install --quiet --require-hashes --no-deps -r /opt/tello-sim/requirements-layout.txt; fi \
    && /opt/tello-venv/bin/pip list 2>/dev/null | grep -i -E "djitellopy|numpy|^av " > /opt/tello.txt
