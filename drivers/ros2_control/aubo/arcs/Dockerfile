# ARCS, AUBO's robot controller software, as the AUBO row's controller (DRY-COVERAGE-PLAN.md
# Phase 8, item 8.15). Rung 1: the code that runs inside an AUBO control box, with AUBO's own
# simulated arm (`extensions/fake_robot`) where the joint drives would be.
#
# AUBO documents two ways to run ARCS without a robot: its `aubo_sim` VMware image, and its
# installer on an Ubuntu machine (developer.aubo-robotics.cn, application note 06). This is the
# second, in a container: the installer is fetched from AUBO's own download server at a pinned
# version and checked against the md5 AUBO publishes beside it, then run in its documented
# `install` mode, unmodified. Nothing of it is copied into this repository; its licence is AUBO's
# (`"license": "aubo"` in every extension's package.json), which is why it is fetched, never
# vendored.
#
# Two things differ from AUBO's instructions, and both are the machine rather than the software:
#   * There is no init in a container, so `systemctl` and `udevadm` are stubs that say what they
#     were asked and succeed. The installer uses them to register services — aubo_control,
#     aubo_scope, webscope, nginx, pn_server — and to reload udev for EtherCAT; none of those is
#     needed to run the controller, which is started directly below as its service unit starts it.
#   * AUBO's guide says to edit `aubo_control.conf` to `hw = "fake_robot1"`. AUBO ships that file
#     already edited, as `config/aubo_control_fake.conf`, and it is copied in whole rather than
#     edited here: the simulated arm is an aubo_i5 (the only model its `model_type` names) with
#     AUBO's own home pose, at AUBO's 5 ms servo cycle.
#
# And one thing the image cannot carry: ARCS puts its servo thread on a real-time policy at start
# and, refused one, waits for ever (DRY-COVERAGE-PLAN.md F139), so the container is run with
# `--ulimit rtprio=99` — `make check-arcs-docker` does.
#
# The pendant (aubo_scope) and the browser pendant (webscope) are installed and never started: the
# plugin speaks RPC on 30004 and RTDE on 30010, and power-on is an RPC call (`../run-arcs.sh`).
#
#   docker build -t skynet-aubo-arcs drivers/ros2_control/aubo/arcs
#   docker build -t skynet-aubo-arcs:maker-latest --build-arg ARCS_RUN=latest \
#     drivers/ros2_control/aubo/arcs

FROM ubuntu:22.04

# The newest x86_64 installer on AUBO's server on 2026-09-26, and the md5 AUBO published with it.
# `latest` resolves the newest `arcs_update-*-x86_64.run` in the listing at build time and checks
# it against its own `.md5`, which is integrity and nothing more.
ARG ARCS_RUN=arcs_update-0.32.1-rc.54+9a220cc-x86_64.run
ARG ARCS_MD5=cc49c8d0ca62e8952a9e9fe286073b92
ARG ARCS_URL=https://download.aubo-robotics.cn/arcs

ENV DEBIAN_FRONTEND=noninteractive
# What the installer calls: lsb_release and bc for its system-version check, envsubst for its nginx
# template, logger for syslog; and the few tools run-arcs.sh reads the controller with.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl lsb-release bc gettext-base bsdutils procps iproute2 \
    && rm -rf /var/lib/apt/lists/*

RUN set -e; for tool in systemctl udevadm; do \
      printf '#!/bin/sh\necho "%s $*: no init in this container, not run" >&2\nexit 0\n' "$tool" \
        > /usr/local/bin/$tool; chmod +x /usr/local/bin/$tool; done; \
    mkdir -p /lib/systemd/system /etc/udev/rules.d

RUN set -e; cd /tmp; \
    if [ "$ARCS_RUN" = latest ]; then \
      ARCS_RUN=$(curl -fsSL --retry 5 "$ARCS_URL/" \
        | grep -o 'arcs_update-[^"<>]*-x86_64\.run' | sort -uV | tail -1); \
      ARCS_MD5=$(curl -fsSL --retry 5 "$ARCS_URL/$(echo "$ARCS_RUN" | sed 's/+/%2B/g').md5" \
        | cut -d' ' -f1); \
    fi; \
    echo "ARCS installer: $ARCS_RUN (md5 $ARCS_MD5)"; \
    curl -fsSL --retry 5 -o "$ARCS_RUN" "$ARCS_URL/$(echo "$ARCS_RUN" | sed 's/+/%2B/g')"; \
    echo "$ARCS_MD5  $ARCS_RUN" | md5sum -c -; \
    sh "./$ARCS_RUN" --nox11 --noprogress install; \
    rm -f "$ARCS_RUN"; \
    echo "$ARCS_RUN $ARCS_MD5" > /opt/arcs/installer

# The simulated arm, as AUBO ships its configuration. `/etc/default/arcs` is read, not sourced —
# see aubo-control.sh.
RUN set -e; arcs() { sed -n "s/^$1=//p" /etc/default/arcs; }; \
    cp "$(arcs PREFIX)/$(arcs VERSION)/config/aubo_control_fake.conf" \
      /root/arcs_ws/config/aubo_control.conf
COPY aubo-control.sh /usr/local/bin/aubo-control
RUN chmod +x /usr/local/bin/aubo-control

EXPOSE 30004 30010
CMD ["/usr/local/bin/aubo-control"]
