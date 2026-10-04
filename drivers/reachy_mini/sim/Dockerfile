# The Reachy Mini row's substrate: Pollen Robotics' own daemon, running its own MuJoCo simulator (DRY-COVERAGE-PLAN.md item 12.E.12).
#
# `reachy_mini` is the maker's repository (Apache-2.0): it is cloned at a pinned commit and installed here, with its `mujoco` extra, into a virtual environment, and `reachy-mini-daemon --sim --headless
# --no-media` is the maker's own simulator behind the maker's own REST interface. PyGObject, which the package requires on Linux for its media code, is built from source against the packages below; the
# daemon is started with no media and this driver never touches any. Nothing of the maker's is in this repository, and nothing built here leaves the image.
#
#   docker build -t skynet-reachy-mini-sim -f drivers/reachy_mini/sim/Dockerfile drivers/reachy_mini/sim

FROM ubuntu:24.04

ARG REACHY_MINI_COMMIT=a4ee426724c2cc6411c238094d129d47fa8ad836

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git git-lfs python3 python3-venv python3-dev gcc g++ make pkg-config libc6-dev libgirepository1.0-dev libcairo2-dev libusb-1.0-0 iproute2 procps \
    && rm -rf /var/lib/apt/lists/*

# The package's face-tracking module imports GStreamer's typelib at import time (`gi.require_version("Gst", "1.0")`), even for a daemon started with no media: found by the first run, which the daemon never survived.
RUN apt-get update && apt-get install -y --no-install-recommends gir1.2-gstreamer-1.0 gir1.2-gst-plugins-base-1.0 gstreamer1.0-plugins-base     && rm -rf /var/lib/apt/lists/*

RUN git init /opt/reachy_mini \
    && git -C /opt/reachy_mini remote add origin https://github.com/pollen-robotics/reachy_mini.git \
    && git -C /opt/reachy_mini fetch --depth 1 origin "$REACHY_MINI_COMMIT" \
    && git -C /opt/reachy_mini checkout --detach FETCH_HEAD \
    && git -C /opt/reachy_mini log -1 --format='reachy_mini %H %cs' > /opt/reachy_mini.txt

# The maker's meshes (every `.stl` under `descriptions/`) are Git LFS objects. Without this step `pip install` packages 160-byte pointer files and MuJoCo refuses the first mesh it reads ("number of faces should be between 1 and
# 200000 in STL file ... perhaps this is an ASCII file?"), which the daemon reports as `state: error` after it has started serving. Found by the first run of `sim/run-sim.sh`.
RUN git -C /opt/reachy_mini lfs install --local && git -C /opt/reachy_mini lfs pull \
    && ! grep -rl "git-lfs.github.com/spec" /opt/reachy_mini/src/reachy_mini/descriptions

RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir --upgrade pip \
    && /opt/venv/bin/pip install --no-cache-dir "/opt/reachy_mini[mujoco]" \
    && /opt/venv/bin/pip freeze > /opt/pip-freeze.txt
