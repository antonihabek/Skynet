# The OnRobot row's substrate: OnRobot's own client library, run (DRY-COVERAGE-PLAN.md item 12.E.8).
#
# `OnRobot/onrobot-tool-api` is the maker's repository (BSD-3-Clause): its GitHub release carries the closed client library `libonrobot_tool_api` as two Debian packages, which `fetch.sh` downloads, checks against a
# SHA-256 pinned there (for the release 1.2.0; the nightly's `ONROBOT_RELEASE=latest` is not checked against a pin, and says so) and unpacks without installing. `build.sh` builds `oracle.cpp` against them: the maker's
# own client as a process. The image holds the toolchain and libmodbus (which the maker's library links); nothing of the maker's is in this repository, and nothing built here leaves the container.
#
#   docker build -t skynet-onrobot-oracle -f drivers/onrobot/sim/Dockerfile drivers/onrobot/sim

FROM ubuntu:24.04

ARG ONROBOT_RELEASE=1.2.0

ENV DEBIAN_FRONTEND=noninteractive
# curl and dpkg for the packages; a C++17 compiler and libmodbus for the oracle; python3 and a C compiler because `make check` builds the Python host's shared object in the container.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl python3 gcc g++ make libc6-dev libmodbus5 libmodbus-dev \
    && rm -rf /var/lib/apt/lists/*

COPY fetch.sh build.sh oracle.cpp /opt/onrobot-sim/
RUN ONROBOT_RELEASE="$ONROBOT_RELEASE" sh /opt/onrobot-sim/fetch.sh /opt/onrobot \
    && sh /opt/onrobot-sim/build.sh /opt/onrobot/sdk /opt/onrobot-oracle \
    && echo "onrobot-tool-api $ONROBOT_RELEASE" > /opt/onrobot.txt
