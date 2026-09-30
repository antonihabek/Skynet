# A disposable systemd, for the tests that need the real thing (packaging/test-install-needs.sh).
#
# `install.sh` writes units and drop-ins and then asks *systemd* what it parsed — the only honest way
# to know a sandbox directive took effect (DRY-COVERAGE-PLAN.md F185). That needs systemd as PID 1,
# root, and a machine you are willing to install units onto. A privileged container is all three
# without being the developer's own machine:
#
#   docker build -t skynet-systemd-test:24.04 -f packaging/test/systemd.Dockerfile packaging/test
#
# Ubuntu 24.04 because it is what the CI runners and the reference machine run. Nothing here is
# shipped and nothing in the image is Skynet's: the repository is mounted read-only at run time.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends systemd systemd-sysv dbus python3 iproute2 kmod procps ca-certificates \
 && apt-get clean \
 && find /var/lib/apt/lists -type f -delete
STOPSIGNAL SIGRTMIN+3
CMD ["/lib/systemd/systemd"]
