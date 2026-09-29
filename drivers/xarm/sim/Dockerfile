# The xArm row's client image (DRY-COVERAGE-PLAN.md Phase 10, item 10.5): what runs *beside* UFACTORY's
# simulator, in its network namespace, to drive it.
#
# The simulator is UFACTORY's own image (`danielwang123321/uf-ubuntu-docker` — the one their support
# article "How to install UFACTORY Studio in Docker" points to), which runs the controller's own
# firmware, `xarm6-type6-controller` v2.4.0, with no motors behind it. It speaks the two kinds of
# socket a real controller does — the register table on 502 and the report streams on 30000 to
# 30003 — so this driver is measured against the code that runs in the robot's control box. It is
# Ubuntu 22.04, and the `skynet` binary the check mounts is built on a newer glibc, so the client is
# its own image rather than a process inside the simulator's.
#
# What is fetched, and nothing of it is copied into this repository: UFACTORY's Python SDK
# (BSD-3-Clause) at a pinned commit, which `layout_check.py` opens and which the live block uses
# as a second opinion — its own `XArmAPI` reading the same controller.
#
#   docker build -t skynet-xarm-client drivers/xarm/sim
#   docker build -t skynet-xarm-client:maker-latest --build-arg XARM_SDK_REF=master drivers/xarm/sim

FROM ubuntu:24.04

ARG XARM_SDK_REF=d911319cebc45142613e18086aabb8067b18ab7f

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      python3 gcc make git ca-certificates libc6-dev \
    && rm -rf /var/lib/apt/lists/*

RUN git init -q /opt/xarm-sdk \
    && git -C /opt/xarm-sdk fetch -q --depth 1 https://github.com/xArm-Developer/xArm-Python-SDK "$XARM_SDK_REF" \
    && git -C /opt/xarm-sdk checkout -q FETCH_HEAD \
    && git -C /opt/xarm-sdk rev-parse HEAD > /opt/xarm-sdk.commit
