"""Pollen Robotics' Reachy 2 core in fake mode, as the open image Pollen publishes runs it: the substrate `selftest_wire_reachy2.py` holds the bridge to.

The image is `pollenrobotics/reachy2` on Docker Hub (open, no login), pinned by digest. It is Pollen's whole ROS 2 stack: `reachy2_core` with mock hardware (`fake:=true`, the
URDF's `use_fake_hardware`, the same controllers a real Reachy 2 runs) and the SDK server (`start_sdk_server:=true`), the gRPC server on port 50051 that `reachy2-sdk` speaks to,
and so LeRobot's `Reachy2Robot`, which opens `ReachySDK(ip_address)` and nothing else. Nothing of Pollen's is in this repository, and this file does not import any of it: it starts
the container, waits until the server answers, and stops it.

`fake:=true` is Pollen's own mode for "no robot": the mock hardware follows what it is commanded, there is no physics, no camera (`orbbec:=false`), and no WebRTC (so no teleoperation).
It is **rung 2** for the wire (`docs/robot-support.md` section 3): the maker's own server answering the maker's own client, which is why a pass is evidence the bridge speaks to a
Reachy 2 core as Pollen's software expects, and is not evidence about an arm, a motor or a person standing next to one.

The container's port 50051 is published on the host's loopback only, and on 50051: `Reachy2Robot` passes the SDK nothing but a host, so the SDK's default port is the only one it can reach.
"""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import time
import uuid

#: Pollen's `pollenrobotics/reachy2:1.7.5.9` (amd64), by the digest Docker Hub reported on 2026-10-12. `REACHY2_IMAGE` names another.
IMAGE = os.environ.get(
    "REACHY2_IMAGE",
    "pollenrobotics/reachy2@sha256:d5dd5768120f3cf36a9f1b88e0a217e97c4384abb7535496a6cf68aefaf6dff9",
)
SDK_PORT = 50051

#: What `/package/launch.sh` in the image does, less the VNC server and the Jupyter notebook it also starts: the ROS environment, then `reachy.launch.py` in fake mode with the SDK server.
#:
#: And one thing the image does not do for itself, F329: the video server's camera search runs `lsusb`, which exits 1 in a container with no USB bus (`unable to initialize libusb`),
#: the server dies on the `CalledProcessError`, and `reachy.launch.py` treats a dead video server as a critical node and shuts the whole core down with it, the SDK server among
#: the nodes. Docker Desktop on Windows gives a container no `/dev/bus/usb`, so the image as published does not stay up there. The search is for a camera the fake has none of
#: (`orbbec:=false`), so a `lsusb` that finds no device is the true answer; it is put first on the PATH of this one process tree and nothing in the image is changed.
LAUNCH = (
    "shopt -s expand_aliases; "
    "mkdir -p /tmp/no-usb && printf '#!/bin/sh\\nexit 0\\n' > /tmp/no-usb/lsusb && chmod +x /tmp/no-usb/lsusb && export PATH=/tmp/no-usb:$PATH; "
    "source /opt/ros/humble/setup.bash; source /home/reachy/reachy_ws/install/setup.bash; "
    "source /home/reachy/.utils.bash; source /package/reachy.bash; "
    "exec ros2 launch reachy_bringup reachy.launch.py start_rviz:=false start_sdk_server:=true fake:=true orbbec:=false"
)


class CoreError(RuntimeError):
    """The core could not be started or did not come up; the message says what to do."""


def docker_available() -> str | None:
    """Why Docker cannot be used, or `None` when it can."""
    if shutil.which("docker") is None:
        return "docker is not installed or not on PATH"
    try:
        done = subprocess.run(["docker", "info"], capture_output=True, timeout=30, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"docker did not answer: {error}"
    return None if done.returncode == 0 else "the docker daemon is not running"


def port_is_free(port: int = SDK_PORT) -> bool:
    with socket.socket() as probe:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) != 0


class Core:
    """One container running Reachy 2's fake core and SDK server. A context manager: it is stopped on the way out whatever happened."""

    def __init__(self, image: str = IMAGE, cpus: str | None = None) -> None:
        self.image = image
        self.cpus = cpus
        self.name = os.environ.get("REACHY2_NAME") or f"skynet-reachy2-{os.getpid()}-{uuid.uuid4().hex[:6]}"
        self.started = False
        self.external = False

    def __enter__(self) -> "Core":
        self.start()
        return self

    def __exit__(self, *_exc) -> None:
        self.stop()

    def start(self) -> None:
        if os.environ.get("REACHY2_EXTERNAL") == "1":
            # A core somebody else started and will stop, for writing checks against without paying 18 s of ROS start-up each time: the run that counts starts its own.
            self.external = True
            return
        why = docker_available()
        if why:
            raise CoreError(f"{why}. This file runs Pollen's image; start Docker (or point REACHY2_IMAGE at an image that is here) and run it again.")
        if not port_is_free():
            raise CoreError(
                f"port {SDK_PORT} on this machine's loopback is in use, and LeRobot's Reachy2Robot can only reach the SDK on that one. "
                "Stop whatever holds it (another Reachy 2 container: `docker ps`) and run this again."
            )
        command = ["docker", "run", "-d", "--rm", "--name", self.name, "-p", f"127.0.0.1:{SDK_PORT}:{SDK_PORT}"]
        if self.cpus:
            command += ["--cpus", self.cpus]
        command += ["--entrypoint", "bash", self.image, "-c", LAUNCH]
        done = subprocess.run(command, capture_output=True, text=True, timeout=600, check=False)
        if done.returncode != 0:
            raise CoreError(
                f"docker could not start {self.image}: {done.stderr.strip() or done.stdout.strip()}. "
                f"If the image is not here, `docker pull {self.image}` fetches it (about 4.5 GB, no login)."
            )
        self.started = True

    def logs(self, tail: int = 40) -> str:
        done = subprocess.run(["docker", "logs", "--tail", str(tail), self.name], capture_output=True, text=True, timeout=60, check=False)
        return (done.stdout + done.stderr).strip()

    def running(self) -> bool:
        if self.external:
            return True
        done = subprocess.run(["docker", "inspect", "-f", "{{.State.Running}}", self.name], capture_output=True, text=True, timeout=30, check=False)
        return done.returncode == 0 and done.stdout.strip() == "true"

    def wait_ready(self, seconds: float = 240.0) -> float:
        """Until the SDK server answers `GetReachy` with a robot that has parts, and returns how long that took. Raises with the container's own last lines when it does not."""
        import grpc
        from google.protobuf.empty_pb2 import Empty
        from reachy2_sdk_api import reachy_pb2_grpc

        began = time.monotonic()
        last = "no answer yet"
        while time.monotonic() - began < seconds:
            if not self.running():
                raise CoreError(f"the container stopped before the SDK server answered. Its last lines:\n{self.logs()}")
            try:
                with grpc.insecure_channel(f"127.0.0.1:{SDK_PORT}") as channel:
                    reachy = reachy_pb2_grpc.ReachyServiceStub(channel).GetReachy(Empty(), timeout=3)
                if reachy.HasField("r_arm") or reachy.HasField("head"):
                    return time.monotonic() - began
                last = "the server answered and the robot it described had no parts"
            except grpc.RpcError as error:
                last = f"{error.code().name}: {error.details()}"
            time.sleep(1.0)
        raise CoreError(f"the SDK server did not answer in {seconds:.0f} s ({last}). Its last lines:\n{self.logs()}")

    def pause(self) -> None:
        """Freeze every process in the container (the cgroup freezer): the SDK server stops answering and stops streaming, and the client's connection stays open. A producer that has stalled."""
        subprocess.run(["docker", "pause", self.name], capture_output=True, timeout=60, check=True)

    def unpause(self) -> None:
        subprocess.run(["docker", "unpause", self.name], capture_output=True, timeout=60, check=True)

    def stop(self) -> None:
        if not self.started:
            return
        subprocess.run(["docker", "stop", "-t", "5", self.name], capture_output=True, timeout=120, check=False)
        self.started = False
