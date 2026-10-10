# The Dobot row's substrate: Dobot's own Python client for its V4 controllers, run (DRY-COVERAGE-PLAN.md item 12.E.13).
#
# A Dobot V4 arm is a controller with a text port and a packet port, and its maker publishes the client for both (`Dobot-Arm/TCP-IP-Python-V4`, MIT). The image installs numpy, which the client reads its feedback
# packet through, into a virtual environment: by the hash in `requirements-layout.txt` (copied in), or unpinned when DOBOT_NUMPY is `latest` (the nightly's second run, which says so). The client itself is fetched by
# `fetch-sdk.sh` at its pinned commit (`DOBOT_SDK_COMMIT=main` for the tip). Nothing of either is in this repository, and nothing installed here leaves the container.
#
#   docker build -t skynet-dobot-sdk -f drivers/dobot/sim/Dockerfile drivers/dobot/sim

FROM ubuntu:24.04

ARG DOBOT_NUMPY=2.5.3
ARG DOBOT_SDK_COMMIT=55ec1ec82201aaf2e6d54aa47b6272bbaa14c815

ENV DEBIAN_FRONTEND=noninteractive
# python3 and a C compiler because `make check` builds the Python host's shared object in the container; curl for the client's tarball.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl python3 python3-venv gcc make libc6-dev \
    && rm -rf /var/lib/apt/lists/*

COPY fetch-sdk.sh /opt/dobot-sim/
COPY requirements-layout.txt /opt/dobot-sim/
RUN python3 -m venv /opt/dobot-venv \
    && if [ "$DOBOT_NUMPY" = latest ]; then /opt/dobot-venv/bin/pip install --quiet numpy; \
       else /opt/dobot-venv/bin/pip install --quiet --require-hashes -r /opt/dobot-sim/requirements-layout.txt; fi \
    && DOBOT_SDK_COMMIT="$DOBOT_SDK_COMMIT" sh /opt/dobot-sim/fetch-sdk.sh /opt/dobot-vendor \
    && /opt/dobot-venv/bin/pip list 2>/dev/null | grep -i -E "numpy" > /opt/dobot.txt \
    && echo "Dobot-Arm/TCP-IP-Python-V4 $DOBOT_SDK_COMMIT" >> /opt/dobot.txt
