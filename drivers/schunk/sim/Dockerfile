# The SCHUNK row's substrate: SCHUNK's own protocol simulator and its own client (DRY-COVERAGE-PLAN.md item 12.E.9).
#
# `SCHUNK-SE-Co-KG/schunk_mechatronic_gripper` is the maker's repository (GPL-3.0): it is fetched at a pinned commit (or, for the nightly's `SCHUNK_COMMIT=HEAD`, the tip of its default branch, which is then not verified against a pin) and its two Python packages are installed here into a virtual environment, `schunk_gripper_dummy`
# (the maker's "minimalist protocol simulator" of an EGU, EGK or EZU module's web interface) and `schunk_gripper_library` (the maker's client, which `layout_check.py` and `live.py` import unmodified). Nothing of the
# maker's is in this repository, and nothing built here leaves the image. The maker's client imports `pytest` at import time and declares no dependency on it (F250), so it is installed beside them.
#
#   docker build -t skynet-schunk-sim -f drivers/schunk/sim/Dockerfile drivers/schunk/sim

FROM ubuntu:24.04

ARG SCHUNK_COMMIT=76df296c49054ba74500fb09c63012f092a65573

ENV DEBIAN_FRONTEND=noninteractive
# git for the pinned fetch; python3 and a C compiler because `make check` builds the Python host's shared object in the container.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git python3 python3-venv gcc make libc6-dev \
    && rm -rf /var/lib/apt/lists/*

RUN git init /opt/schunk_mechatronic_gripper \
    && git -C /opt/schunk_mechatronic_gripper remote add origin https://github.com/SCHUNK-SE-Co-KG/schunk_mechatronic_gripper.git \
    && git -C /opt/schunk_mechatronic_gripper fetch --depth 1 origin "$SCHUNK_COMMIT" \
    && git -C /opt/schunk_mechatronic_gripper checkout --detach FETCH_HEAD \
    && { [ "${#SCHUNK_COMMIT}" -ne 40 ] || test "$(git -C /opt/schunk_mechatronic_gripper rev-parse HEAD)" = "$SCHUNK_COMMIT"; } \
    && git -C /opt/schunk_mechatronic_gripper log -1 --format='schunk_mechatronic_gripper %H %cs' > /opt/schunk.txt

RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir --upgrade pip \
    && /opt/venv/bin/pip install --no-cache-dir /opt/schunk_mechatronic_gripper/schunk_gripper_library /opt/schunk_mechatronic_gripper/schunk_gripper_dummy pytest \
    && /opt/venv/bin/pip freeze > /opt/pip-freeze.txt
