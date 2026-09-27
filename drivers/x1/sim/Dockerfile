# The X1 row's rung-2 substrate (DRY-COVERAGE-PLAN.md Phase 7 item 3, made reproducible for Phase 8
# item 8.11): AgiBot's own open deployment, `agibot_x1_infer` — AimRT modules, the vendor's MuJoCo
# sim, the X1 model — built from a pinned commit and run headless under Xvfb. The driver speaks the
# two ROS 2 topics the sim owns, so nothing of the wire is transcribed: the sim is the wire.
#
# Until this image, the sim half of `make check` had run on one machine with a hand-built vendor
# tree and four hand-made patches (the README's list). What this image needs instead:
#
# * The vendor's `sim_module` links `dart-external-lodepng`, which it never defines — AgiBot's
#   machines carry DART. Ubuntu packages DART's lodepng as `libdart-external-lodepng-dev`, with the
#   header at the path `simulate.cc` includes, so it is installed rather than patched in.
# * AimRT, which the vendor's CMake fetches at a tag it names, does not compile against ROS 2 Jazzy:
#   `aimrt-jazzy.patch` beside this file is the three-file change (the subscription options call,
#   the `SubscriptionBase` constructor Jazzy grew, and the dynamic-type pure virtuals Jazzy added,
#   which throw as rclcpp's own Subscription does). The tag is read out of the vendor's own
#   `cmake/GetAimRT.cmake` rather than pinned here, so when AgiBot moves it, the nightly's build at
#   the makers' latest applies the patch to the new tag and fails loudly if it no longer fits.
#
# The vendor tree carries its own prebuilt MuJoCo, ONNX Runtime and qpOASES; nothing of any of it
# is copied into this repository. Licences: agibot_x1_infer and AimRT are both Mulan PSL v2 (each
# repository's LICENSE); the patch beside this file is a change to AimRT and is offered under it.
#
#   docker build -t skynet-x1-sim drivers/x1/sim
#   docker build -t skynet-x1-sim:maker-latest --build-arg X1_INFER_REF=main drivers/x1/sim

FROM ros:jazzy-ros-base

# The vendor's last commit as of 2026-09-25 (2025-04-03, "update docs").
ARG X1_INFER_REF=9e0b818804d644fb9c9663e932dd33b03b24dfa4
ARG RUST_TOOLCHAIN=1.98.1

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake git curl ca-certificates python3 \
      libglfw3-dev libsdl2-dev libdart-external-lodepng-dev \
      xvfb xauth libgl1-mesa-dri libglx-mesa0 libegl1 \
      ros-jazzy-sensor-msgs ros-jazzy-geometry-msgs ros-jazzy-std-msgs \
    && rm -rf /var/lib/apt/lists/*

COPY aimrt-jazzy.patch /opt/src/aimrt-jazzy.patch

RUN git init -q /opt/src/x1_infer \
    && git -C /opt/src/x1_infer fetch -q --depth 1 \
         https://github.com/AgibotTech/agibot_x1_infer "$X1_INFER_REF" \
    && git -C /opt/src/x1_infer checkout -q FETCH_HEAD \
    && git -C /opt/src/x1_infer rev-parse HEAD > /opt/src/x1_infer.commit

# AimRT at the tag the vendor names, with the Jazzy patch, handed to the vendor's FetchContent so
# its own build uses this checkout instead of fetching another.
RUN set -e; \
    tag=$(sed -n 's/^ *GIT_TAG \([^ )]*\).*/\1/p' /opt/src/x1_infer/cmake/GetAimRT.cmake); \
    test -n "$tag" || { echo "no GIT_TAG in the vendor's GetAimRT.cmake"; exit 1; }; \
    git init -q /opt/src/aimrt; \
    git -C /opt/src/aimrt fetch -q --depth 1 https://github.com/AimRT/AimRT "refs/tags/$tag"; \
    git -C /opt/src/aimrt checkout -q FETCH_HEAD; \
    echo "$tag $(git -C /opt/src/aimrt rev-parse HEAD)" > /opt/src/aimrt.commit; \
    git -C /opt/src/aimrt apply /opt/src/aimrt-jazzy.patch

# The vendor's own build script, with the simulation on.
RUN . /opt/ros/jazzy/setup.sh \
    && cd /opt/src/x1_infer \
    && ./build.sh -DFETCHCONTENT_SOURCE_DIR_AIMRT=/opt/src/aimrt

# The `skynet` command, built from the mounted checkout at run time: the conformance run is the
# repository's, not the image's.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
ENV PATH=/root/.cargo/bin:$PATH
