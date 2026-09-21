"""The wire cross-check: the vendor's own `bosdyn-client` against this fake's services.

No arguments and no checkout: the vendor client is the PyPI package in the project
environment, imported unmodified, and the fake runs in-process. It walks the real bring-up
(auth, directory, estop, lease, time sync, power, motion) and then attacks the fake's
discipline the way only the vendor's shapes can: a tampered token, a lease the robot never
issued, a command with no clock, a forged challenge response.

If the fake runs the vendor's flows and refuses the vendor's forgeries, the wire the driver
speaks is Spot's. Either half failing is a defect in this tree, not in theirs.
"""

from __future__ import annotations

import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)

try:
    import grpc
    from bosdyn.api import estop_pb2, estop_service_pb2_grpc
    from bosdyn.api import lease_pb2
    from bosdyn.api import power_pb2
    from bosdyn.api import robot_command_pb2, robot_command_service_pb2_grpc
    from bosdyn.api import robot_state_pb2, robot_state_service_pb2_grpc
    from bosdyn.client import create_standard_sdk
    from bosdyn.client.estop import EstopClient, EstopEndpoint, EstopKeepAlive
    from bosdyn.client.lease import LeaseClient, LeaseKeepAlive, ResourceAlreadyClaimedError
    from bosdyn.client.power import PowerClient
    from bosdyn.client.robot_command import RobotCommandBuilder, RobotCommandClient
    from bosdyn.client.robot_state import RobotStateClient
except ImportError:
    print("::error::the cross-check needs the project environment (bosdyn-client, "
          "grpcio). See drivers/spot/README.md.", flush=True)
    sys.exit(1)

from skynet_spot.fake_spot import JOINTS, STAND, serve

CERTS = os.path.join(HERE, "certs")
CA = f"{CERTS}/ca.pem"

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def main() -> int:
    server, world, port = serve(0)
    try:
        section("the vendor's bring-up, unmodified")
        sdk = create_standard_sdk("cross-check", cert_resource_glob=CA)
        robot = sdk.create_robot("127.0.0.1")
        robot.update_secure_channel_port(port)
        robot.authenticate("admin", "password")
        check(True, "authenticate answers a token over the test CA's TLS")
        robot.sync_with_directory()
        names = sorted(robot.authorities_by_name)
        check(names == ["auth", "directory", "estop", "lease", "power",
                        "robot-command", "robot-state", "time-sync"],
              "the directory lists the eight services the driver speaks")
        check(all(authority.endswith(".spot.robot")
                  for authority in robot.authorities_by_name.values()),
              "under the SDK's own authority pattern")

        section("estop: the handshake the driver inherits")
        estop = robot.ensure_client(EstopClient.default_service_name)
        endpoint = EstopEndpoint(estop, "cross-check", 5.0)
        endpoint.force_simple_setup()
        check(True, "force_simple_setup: get, set, register, seed")
        alive = EstopKeepAlive(endpoint)
        time.sleep(0.5)
        check(world.counts["challenge_refused"] == 0,
              "keepalive check-ins pass the challenge the fake verifies")
        alive.shutdown()
        cuts = world.counts["estop_cut"]
        endpoint.stop()
        time.sleep(0.3)
        check(world.estopped and world.counts["estop_cut"] == cuts + 1,
              "endpoint.stop() CUTs with the keepalive down")
        endpoint.allow()
        time.sleep(0.3)
        check(not world.estopped, "allow() at NONE clears it")
        endpoint.deregister()
        check(True, "deregister with the motors off answers")

        section("lease, time sync, power, motion")
        lease_client = robot.ensure_client(LeaseClient.default_service_name)
        lease = lease_client.acquire("body")
        check(lease.lease_proto.resource == "body", "acquire takes the body")
        try:
            lease_client.acquire("body")
            check(False, "a second acquire is refused while held")
        except ResourceAlreadyClaimedError:
            check(True, "a second acquire is refused while held")
        taken = lease_client.take("body")
        check(taken.lease_proto.epoch != lease.lease_proto.epoch,
              "take force-takes with a new epoch")
        keeper = LeaseKeepAlive(lease_client)
        robot.time_sync.wait_for_sync(timeout_sec=10)
        check(True, "time sync establishes against the fake clock")
        power = robot.ensure_client(PowerClient.default_service_name)
        power.power_command(power_pb2.PowerCommandRequest.REQUEST_ON_MOTORS,
                            lease=taken.lease_proto)
        states = robot.ensure_client(RobotStateClient.default_service_name)
        check(world.powered, "power_on answers and the state reads on")
        commands = robot.ensure_client(RobotCommandClient.default_service_name)
        endpoint2 = commands.timesync_endpoint
        stand_id = commands.robot_command(
            RobotCommandBuilder.synchro_stand_command(), lease=taken.lease_proto,
            timesync_endpoint=endpoint2)
        check(isinstance(stand_id, int), "stand answers a command id")
        state = states.get_robot_state()
        got = {joint.name: joint.position.value
               for joint in state.kinematic_state.joint_states}
        check([joint.name for joint in state.kinematic_state.joint_states]
              == list(JOINTS), "the state enumerates the twelve vendor joints")
        check(all(abs(got[name] - STAND[name.rsplit(".", 1)[1]]) < 1e-9
                  for name in JOINTS), "standing reads the stand pose")
        commands.robot_command(RobotCommandBuilder.synchro_sit_command(),
                               lease=taken.lease_proto, timesync_endpoint=endpoint2)
        check(world.last_command == "sit", "sit sits")
        commands.robot_command(RobotCommandBuilder.stop_command(),
                               lease=taken.lease_proto, timesync_endpoint=endpoint2)
        check(world.last_command == "stop", "stop stops")
        commands.robot_command(RobotCommandBuilder.selfright_command(),
                               lease=taken.lease_proto, timesync_endpoint=endpoint2)
        check(world.last_command == "selfright", "selfright self-rights")
        power.power_command(power_pb2.PowerCommandRequest.REQUEST_OFF_MOTORS,
                            lease=taken.lease_proto)
        check(not world.powered, "power_off answers and the state reads off")
        section("forgeries the fake refuses (the lease still live)")
        with open(CA, "rb") as handle:
            creds = grpc.ssl_channel_credentials(root_certificates=handle.read())

        def channel(authority: str) -> grpc.Channel:
            return grpc.secure_channel(
                f"127.0.0.1:{port}", creds,
                [("grpc.ssl_target_name_override", authority)])

        state_stub = robot_state_service_pb2_grpc.RobotStateServiceStub(
            channel("robot-state.spot.robot"))
        try:
            state_stub.GetRobotState(
                robot_state_pb2.RobotStateRequest(),
                metadata=[("authorization", "Bearer wrong-token")], timeout=10)
            check(False, "a forged token reads state")
        except grpc.RpcError as error:
            check(error.code() == grpc.StatusCode.UNAUTHENTICATED,
                  "a forged token is UNAUTHENTICATED, not data")
        cmd_stub = robot_command_service_pb2_grpc.RobotCommandServiceStub(
            channel("robot-command.spot.robot"))
        forged = lease_pb2.Lease(resource="body", epoch="epoch-never")
        forged.sequence.append(9)
        before = world.counts["lease_refused"]
        reply = cmd_stub.RobotCommand(
            robot_command_pb2.RobotCommandRequest(
                lease=forged,
                command=RobotCommandBuilder.synchro_stand_command()),
            metadata=[("authorization", f"Bearer {robot.user_token}")], timeout=10)
        values = robot_command_pb2.RobotCommandResponse.DESCRIPTOR.fields_by_name[
            "status"].enum_type.values_by_name
        check(reply.status != values["STATUS_OK"].number
              and world.counts["lease_refused"] == before + 1,
              "a lease the robot never issued is refused and counted")
        reply = cmd_stub.RobotCommand(
            robot_command_pb2.RobotCommandRequest(
                lease=taken.lease_proto,
                command=RobotCommandBuilder.synchro_stand_command()),
            metadata=[("authorization", f"Bearer {robot.user_token}")], timeout=10)
        check(reply.status == values["STATUS_NO_TIMESYNC"].number,
              "a command with no clock is refused as NO_TIMESYNC")
        # A second endpoint, seeded honestly, then a forged echo on a raw stub.
        probe = EstopEndpoint(estop, "tamper-probe", 5.0)
        probe.register(world.config_id)
        before = world.counts["challenge_refused"]
        estop_stub = estop_service_pb2_grpc.EstopServiceStub(channel("estop.spot.robot"))
        answer = estop_stub.EstopCheckIn(
            estop_pb2.EstopCheckInRequest(
                endpoint=estop_pb2.EstopEndpoint(unique_id=probe.unique_id),
                challenge=probe.get_challenge(), response=0xDEAD,
                stop_level=estop_pb2.ESTOP_LEVEL_NONE),
            metadata=[("authorization", f"Bearer {robot.user_token}")], timeout=10)
        values = estop_pb2.EstopCheckInResponse.DESCRIPTOR.fields_by_name[
            "status"].enum_type.values_by_name
        check(answer.status == values["STATUS_INCORRECT_CHALLENGE_RESPONSE"].number
              and world.counts["challenge_refused"] == before + 1,
              "a forged challenge response is refused and counted")
        probe.deregister()

        keeper.shutdown()
        lease_client.return_lease(taken)
        check(True, "the lease returns")
    finally:
        server.stop(grace=2).wait()
        world.halt()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())
