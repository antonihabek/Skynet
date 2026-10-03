# skynet-ros-industrial — a robot arm behind a ROS-Industrial `simple_message` server, and its suite.
#
#   make selftest            the wire and every refusal, against a simulated controller. Needs python3
#   make check               the whole path: skynet driver check → python-host → this bridge → a simulated controller in the same process
#   make check-layout        this driver's constants and bytes against the maker's own library, built here (INDUSTRIAL_CORE=...; needs g++)
#   make chaos               the link-chaos contract: the driver's two sockets through a relay that delays, stalls, blackholes and resets them
#   make check-sim-docker    all of it against a server built from the maker's own library (sim/server.cpp), in a container
#
# `check-sim-docker` is DRY-COVERAGE-PLAN.md item 12.E.6: ROS-Industrial's own `simple_message` library as the other end of the wire, which is F118's rung 3 — an independent implementation of
# the protocol, written by the people who wrote the protocol, and not any robot maker's controller.

PYTHON ?= python3
CARGO  ?= cargo
REPO   := ../..
HOST   := $(REPO)/drivers/python-host

#: The maker's repository, for `check-layout` and the substrate image: `git clone --depth 1 https://github.com/ros-industrial/industrial_core`. Pinned in sim/Dockerfile.
INDUSTRIAL_CORE ?=
ROSI_IMAGE      ?= skynet-ros-industrial-sim
DOCKER_BUILD_ARGS ?=

.PHONY: all check selftest chaos check-layout check-sim-docker clean

all: check

selftest:
	PYTHONPATH=$(REPO)/bindings/python:. $(PYTHON) selftest.py

check: selftest
	$(MAKE) --no-print-directory -C $(HOST) libskynet_python_host.so PYTHON=$(PYTHON)
	SKYNET_PYTHON=$(PYTHON) \
	SKYNET_PYTHON_DRIVER=skynet_ros_industrial \
	SKYNET_ROSI_SIM=1 \
	PYTHONPATH=$(REPO)/bindings/python:. \
	$(CARGO) run --manifest-path $(REPO)/Cargo.toml -q -p skynet-cli -- \
	  driver check $(HOST)/libskynet_python_host.so --config '{}'

check-layout:
	@test -n "$(INDUSTRIAL_CORE)" || { echo "::error::INDUSTRIAL_CORE names no checkout of ROS-Industrial's industrial_core. Nothing was compared."; exit 1; }
	INDUSTRIAL_CORE=$(INDUSTRIAL_CORE) PYTHONWARNINGS=ignore PYTHONPATH=. $(PYTHON) layout_check.py

# The maker's library, built without ROS into a server (sim/server.cpp), the client image in the same container (the driver dials 127.0.0.1 and nothing is published).
check-sim-docker:
	$(CARGO) build --release -q --manifest-path $(REPO)/Cargo.toml -p skynet-cli
	docker build -q $(DOCKER_BUILD_ARGS) -t $(ROSI_IMAGE) -f sim/Dockerfile sim
	docker rm -f skynet-ros-industrial-run >/dev/null 2>&1 || true
	docker run --rm --cap-add NET_ADMIN --name skynet-ros-industrial-run -v "$(abspath $(REPO))":/src \
	  -w /src/drivers/ros_industrial $(ROSI_IMAGE) \
	  sh sim/run-sim.sh /src/target/release/skynet

# The link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4): the bridge's two sockets, each through a relay that delays, stalls, blackholes and resets them (`tools/faultproxy`). In a
# network namespace of its own, so the relays' loopback is nobody else's.
chaos:
	PYTHONPATH=$(REPO)/bindings/python:. sh $(REPO)/tools/seed-defects/in-netns.sh $(PYTHON) $(REPO)/tools/faultproxy/chaos.py chaos.py

clean:
	rm -rf skynet_ros_industrial/__pycache__ __pycache__
