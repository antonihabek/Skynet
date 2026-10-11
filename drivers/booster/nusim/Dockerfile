# The client side of the Booster K1 row's live run (DRY-COVERAGE-PLAN.md item 12.D): the container the driver runs in, beside NUbots' simulator.
#
# Docker Desktop and a Linux runner agree on one thing that matters here: `--network host` is the host of the Docker daemon, which is not always the machine a script runs on. So the driver, the helper that puts
# the simulator in CUSTOM mode and the harness all run in a container on the same host network as the simulator's, and the repository and the Booster SDK are mounted into it. Nothing in this image is the
# driver's or the simulator's: a compiler, make, and Python.
#
#   docker build -t skynet-booster-nusim -f drivers/booster/nusim/Dockerfile drivers/booster/nusim

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates g++ make python3 \
    && rm -rf /var/lib/apt/lists/*
