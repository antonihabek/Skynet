# The Unitree row's rung-2 substrate (DRY-COVERAGE-PLAN.md Phase 8, item 8.1): Unitree's own
# simulator, `unitree_mujoco`, built unmodified and run headless.
#
# Why an image rather than a list of packages: the simulator links GLFW, yaml-cpp, Boost and an
# installed unitree_sdk2, and wants a display. None of that is this repository's to vendor, and a
# workstation without root cannot install it — so the whole substrate is one pinned build, the same
# one CI runs. Xvfb is the display; the simulator does not know it is not on a screen.
#
# Every input is pinned by commit or by digest, and each pin is an ARG so the nightly workflow can
# build the same recipe at the makers' latest (8.11) and tell a driver defect from a vendor change:
#
#   docker build -t skynet-unitree-sim drivers/unitree/sim
#   docker build -t skynet-unitree-sim:maker-latest --build-arg UNITREE_MUJOCO_REF=main \
#                --build-arg UNITREE_SDK2_REF=main drivers/unitree/sim
#
# Licences: unitree_sdk2 and unitree_mujoco are BSD-3-Clause, MuJoCo is Apache-2.0. Nothing of any
# of them is copied into this repository; the image fetches them.

FROM ubuntu:24.04

ARG UNITREE_SDK2_REF=63096d0ac0c5d2dec9d6e0c22cd5233410ca2f36
ARG UNITREE_MUJOCO_REF=1eb6642e3f3fdfb7fb13a9794fd6a2dd93ea0e7d
# The version unitree_mujoco's own readme links against, and the digest of its Linux release.
ARG MUJOCO_VERSION=3.3.6
ARG MUJOCO_SHA256=049204172901afad251070385a6badf46d795ebe47403d093f8469557eeeab5a
ARG RUST_TOOLCHAIN=1.98.1

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake git ca-certificates curl python3 \
      libyaml-cpp-dev libboost-program-options-dev libfmt-dev libspdlog-dev libglfw3-dev \
      libeigen3-dev \
      xvfb xauth libgl1-mesa-dri libglx-mesa0 libegl1 \
    && rm -rf /var/lib/apt/lists/*

# A commit, fetched by name: `--depth 1` on a SHA needs the server's allow-reachable-sha1, which
# GitHub grants, and a branch name works the same way for the nightly's `main`.
RUN git init -q /opt/src/unitree_sdk2 \
    && git -C /opt/src/unitree_sdk2 fetch -q --depth 1 \
         https://github.com/unitreerobotics/unitree_sdk2 "$UNITREE_SDK2_REF" \
    && git -C /opt/src/unitree_sdk2 checkout -q FETCH_HEAD \
    && git -C /opt/src/unitree_sdk2 rev-parse HEAD > /opt/src/unitree_sdk2.commit \
    && cmake -S /opt/src/unitree_sdk2 -B /tmp/sdk-build -DBUILD_EXAMPLES=OFF \
         -DCMAKE_INSTALL_PREFIX=/opt/unitree_robotics \
    && cmake --build /tmp/sdk-build -j"$(nproc)" \
    && cmake --install /tmp/sdk-build \
    && rm -rf /tmp/sdk-build

RUN curl -sSfLo /tmp/mujoco.tar.gz \
      "https://github.com/google-deepmind/mujoco/releases/download/${MUJOCO_VERSION}/mujoco-${MUJOCO_VERSION}-linux-x86_64.tar.gz" \
    && echo "${MUJOCO_SHA256}  /tmp/mujoco.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/mujoco.tar.gz -C /opt \
    && rm /tmp/mujoco.tar.gz

# The simulator, built exactly as its readme says: MuJoCo linked beside `simulate/`, cmake, make.
# The readme's package list omits Eigen, which the SDK's `dds_wrapper` headers include; it is in the
# apt line above for that reason.
RUN git init -q /opt/src/unitree_mujoco \
    && git -C /opt/src/unitree_mujoco fetch -q --depth 1 \
         https://github.com/unitreerobotics/unitree_mujoco "$UNITREE_MUJOCO_REF" \
    && git -C /opt/src/unitree_mujoco checkout -q FETCH_HEAD \
    && git -C /opt/src/unitree_mujoco rev-parse HEAD > /opt/src/unitree_mujoco.commit \
    && ln -s "/opt/mujoco-${MUJOCO_VERSION}" /opt/src/unitree_mujoco/simulate/mujoco \
    && cmake -S /opt/src/unitree_mujoco/simulate -B /opt/src/unitree_mujoco/simulate/build \
         -DCMAKE_BUILD_TYPE=Release \
    && cmake --build /opt/src/unitree_mujoco/simulate/build -j"$(nproc)" --target unitree_mujoco

ENV LD_LIBRARY_PATH=/opt/mujoco-${MUJOCO_VERSION}/lib:/opt/unitree_robotics/lib
ENV UNITREE_MUJOCO=/opt/src/unitree_mujoco
ENV UNITREE_SDK2=/opt/src/unitree_sdk2

# The `skynet` command, built from the mounted checkout at run time: the conformance run is the
# repository's, not the image's.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
ENV PATH=/root/.cargo/bin:$PATH
