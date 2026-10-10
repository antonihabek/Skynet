# The Rainbow row's substrate: Rainbow Robotics' own client for its control boxes, built and run (DRY-COVERAGE-PLAN.md item 12.E.14).
#
# A Rainbow RB arm is a control box with a text port and a packet port, and its maker publishes the client for both (`RainbowRobotics/rbpodo`, Apache-2.0): C++ with a Python module over it. The image builds that module into a
# virtual environment of its own, at the commit `fetch-sdk.sh` pins (`RBPODO_COMMIT=main` for the tip, which is not checked against a hash and says so), with numpy by the hash in `requirements-layout.txt` (copied in). The build needs CMake,
# a C++17 compiler and a network: it downloads Eigen 3.4.0 and pybind11 3.0.0rc1 itself, each by the MD5 rbpodo's own CMake files pin. Nothing of any of it is in this repository, and nothing installed here leaves the container.
#
#   docker build -t skynet-rainbow-sdk -f drivers/rainbow/sim/Dockerfile drivers/rainbow/sim

FROM ubuntu:24.04

ARG RBPODO_COMMIT=f6ef41adf629dd27c96b0d3e7ebf3d3fc0cc53c4

ENV DEBIAN_FRONTEND=noninteractive
# python3 and a C compiler because `make check` builds the Python host's shared object in the container; cmake, g++ and python3-dev because rbpodo is built here; curl for its tarball.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl python3 python3-dev python3-venv gcc g++ make cmake libc6-dev \
    && rm -rf /var/lib/apt/lists/*

# The script looks for requirements-layout.txt in its parent's directory, as the two sit in the repository (drivers/rainbow/sim and drivers/rainbow).
COPY fetch-sdk.sh /opt/rainbow/sim/
COPY requirements-layout.txt /opt/rainbow/
# fetch-sdk.sh makes its own virtual environment (/opt/rainbow-vendor/venv) and installs numpy by hash into it (for the pinned commit), then builds rbpodo there.
RUN RBPODO_COMMIT="$RBPODO_COMMIT" sh /opt/rainbow/sim/fetch-sdk.sh /opt/rainbow-vendor \
    && /opt/rainbow-vendor/venv/bin/pip list 2>/dev/null | grep -i -E "numpy|rbpodo" > /opt/rainbow.txt \
    && echo "RainbowRobotics/rbpodo $RBPODO_COMMIT" >> /opt/rainbow.txt
