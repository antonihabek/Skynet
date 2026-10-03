# The ROS-Industrial row's substrate: ROS-Industrial's own `simple_message` library, built without ROS into a server (DRY-COVERAGE-PLAN.md item 12.E.6).
#
# `industrial_core` is the maker's repository (BSD-3-Clause); it is cloned at a pinned commit and built here, in the image, in each of the four ways a controller's build can spell an int and a
# real (`-DBYTE_SWAPPING`, `-DFLOAT64`: the maker's own switches), with `server.cpp` (the arm behind the maker's sockets, written here) and `codec_dump.cpp` (the maker's codec, asked to
# encode and decode). Nothing of the maker's is in this repository, and nothing built here leaves the image.
#
#   docker build -t skynet-ros-industrial-sim -f drivers/ros_industrial/sim/Dockerfile drivers/ros_industrial/sim

FROM ubuntu:24.04

ARG INDUSTRIAL_CORE_COMMIT=2b7ef6505d3a4742a35a33345a30b92d5c5e9080

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git g++ gcc make libc6-dev python3 iproute2 procps \
    && rm -rf /var/lib/apt/lists/*

RUN git init /opt/industrial_core \
    && git -C /opt/industrial_core remote add origin https://github.com/ros-industrial/industrial_core.git \
    && git -C /opt/industrial_core fetch --depth 1 origin "$INDUSTRIAL_CORE_COMMIT" \
    && git -C /opt/industrial_core checkout --detach FETCH_HEAD \
    && git -C /opt/industrial_core log -1 --format='industrial_core %H %cs' > /opt/industrial_core.txt

COPY build-reference.sh server.cpp codec_dump.cpp /opt/rosi-src/
RUN for variant in "little 4" "big 4" "little 8" "big 8"; do \
      set -- $variant; flags=""; \
      [ "$1" = big ] && flags="$flags -DBYTE_SWAPPING"; [ "$2" = 8 ] && flags="$flags -DFLOAT64"; \
      sh /opt/rosi-src/build-reference.sh /opt/industrial_core /opt/rosi-src/rosi_server-$1-$2 /opt/rosi-src/server.cpp $flags; \
    done \
    && ls /opt/rosi-src
