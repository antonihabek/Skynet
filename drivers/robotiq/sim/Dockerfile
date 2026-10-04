# The Robotiq row's substrate: Robotiq's own SDK and its own fake gripper (DRY-COVERAGE-PLAN.md item 12.E.8).
#
# `Robotiq/grippers` is the maker's repository (BSD-3-Clause): it is fetched at a pinned commit (or, for the nightly's `ROBOTIQ_COMMIT=HEAD`, the tip of its default branch, which is then not verified against a pin) and its
# C++ SDK is built by `sim/build.sh` at run time, unmodified, with four small programs against it (`robotiq-fake-server`, `robotiq-codec-dump`, `robotiq-second-opinion`, `robotiq-client-probe`). The image holds the toolchain
# and the checkout; nothing of the maker's is in this repository, and nothing built here leaves the container.
#
#   docker build -t skynet-robotiq-sim -f drivers/robotiq/sim/Dockerfile drivers/robotiq/sim

FROM ubuntu:24.04

ARG ROBOTIQ_COMMIT=49f56f14a91327c45e10e000687ba030b595ac39

ENV DEBIAN_FRONTEND=noninteractive
# git for the pinned fetch; cmake and a C++17 compiler for the maker's SDK and the harnesses; python3 and a C compiler because `make check` builds the Python host's shared object in the container.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git python3 gcc g++ make cmake libc6-dev \
    && rm -rf /var/lib/apt/lists/*

RUN git init /opt/robotiq_grippers \
    && git -C /opt/robotiq_grippers remote add origin https://github.com/Robotiq/grippers.git \
    && git -C /opt/robotiq_grippers fetch --depth 1 origin "$ROBOTIQ_COMMIT" \
    && git -C /opt/robotiq_grippers checkout --detach FETCH_HEAD \
    && { [ "${#ROBOTIQ_COMMIT}" -ne 40 ] || test "$(git -C /opt/robotiq_grippers rev-parse HEAD)" = "$ROBOTIQ_COMMIT"; } \
    && git -C /opt/robotiq_grippers log -1 --format='robotiq grippers %H %cs' > /opt/robotiq.txt
