#!/usr/bin/env python3
"""A fault-injecting proxy between a driver and its substrate (DRY-COVERAGE-PLAN.md item 12.C.4).

    faultproxy.py tcp    --listen 127.0.0.1:9000 --upstream 127.0.0.1:9001 --profile 'delay=40ms,jitter=15ms,loss=3%'
    faultproxy.py udp    --listen 127.0.0.1:9000 --upstream 127.0.0.1:9001 [--downstream 127.0.0.1:9002] --profile ...
    faultproxy.py serial --link-driver /tmp/driver-tty --link-substrate /tmp/substrate-tty --profile ...
    faultproxy.py can    --driver-side vcan0 --substrate-side vcan1 --profile ...
    faultproxy.py raw    --driver-side ecat0p --substrate-side ecat1p --profile ...

Point the driver at the proxy's address instead of the substrate's. The profile language, every fault and what each means on each
transport are in `engine.py` and `relays.py`; a clean profile (`''`) is a wire.

    --exec ... -- CMD ARGS        the substrate, run under the proxy's supervision so that `kill=` can kill it with SIGKILL and
                                  start it again (TCP and UDP: the relay waits until --upstream accepts again)
    --control PATH                a Unix socket to change the profile while the proxy runs: `set PROFILE`, `heal`, `stats`, `quit`
    --seed N                      the generator's seed: the same messages meet the same faults (default 1)

It prints `ready ...` once it is relaying (the line a test waits for), `stats {...}` as JSON when it is asked and when it exits,
and nothing else on stdout. The standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import socket
import sys
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from engine import Engine, ProfileError  # noqa: E402
from relays import CanRelay, RawRelay, SerialRelay, Supervised, TcpRelay, UdpRelay  # noqa: E402


def address(text: str) -> tuple[str, int]:
    host, _, port = text.rpartition(":")
    if not host or not port.isdigit():
        raise argparse.ArgumentTypeError(f"`{text}` is not HOST:PORT, like 127.0.0.1:9000")
    return host, int(port)


def tcp_ready(target: tuple[str, int]):
    def ready() -> bool:
        try:
            with socket.create_connection(target, timeout=0.2):
                return True
        except OSError:
            return False

    return ready


def control(path: str, engine: Engine, stop: threading.Event) -> None:
    """One line per request on a Unix socket; one line back."""
    if os.path.exists(path):
        os.unlink(path)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    server.listen(4)
    server.settimeout(0.1)

    def handle(connection: socket.socket) -> None:
        with connection, connection.makefile("rw") as stream:
            for line in stream:
                command, _, rest = line.strip().partition(" ")
                if command == "set":
                    try:
                        engine.set_profile(rest)
                        reply = "ok"
                    except ProfileError as error:
                        reply = f"error {error}"
                elif command == "heal":
                    engine.set_profile("")
                    reply = "ok"
                elif command == "stats":
                    reply = "stats " + json.dumps(engine.counters())
                elif command == "quit":
                    stop.set()
                    reply = "ok"
                else:
                    reply = "error the commands are set PROFILE, heal, stats and quit"
                stream.write(reply + "\n")
                stream.flush()

    while not stop.is_set():
        try:
            connection, _ = server.accept()
        except socket.timeout:
            continue
        except OSError:
            return
        threading.Thread(target=handle, args=(connection,), daemon=True).start()
    server.close()
    os.unlink(path)


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    substrate: list[str] = []
    if "--" in arguments:
        cut = arguments.index("--")
        arguments, substrate = arguments[:cut], arguments[cut + 1 :]
    parser = argparse.ArgumentParser(prog="faultproxy.py", description=__doc__.split("\n")[0])
    parser.add_argument("transport", choices=["tcp", "udp", "serial", "can", "raw"])
    parser.add_argument("--profile", default="", help="the faults, e.g. 'delay=40ms,loss=3%%'; empty is a clean link")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--listen", type=address)
    parser.add_argument("--upstream", type=address)
    parser.add_argument("--downstream", type=address, help="UDP: where datagrams the substrate sends unprompted go")
    parser.add_argument("--link-driver")
    parser.add_argument("--link-substrate")
    parser.add_argument("--driver-side")
    parser.add_argument("--substrate-side")
    parser.add_argument("--buffer", type=int, help="TCP: a small kernel buffer on the relay's sockets, for a slow link to push back sooner")
    parser.add_argument("--control")
    options = parser.parse_args(arguments)

    ordered = options.transport in ("tcp", "serial")
    try:
        engine = Engine(options.profile, options.seed, ordered=ordered)
    except ProfileError as error:
        parser.error(str(error))

    supervised = None
    if substrate:
        target = options.upstream if options.transport in ("tcp", "udp") else None
        supervised = Supervised(substrate, tcp_ready(target) if options.transport == "tcp" and target else None)

    if options.transport in ("tcp", "udp"):
        if not options.listen or not options.upstream:
            parser.error(f"`{options.transport}` needs --listen and --upstream, each HOST:PORT")
    if options.transport in ("can", "raw") and not (options.driver_side and options.substrate_side):
        parser.error(f"`{options.transport}` needs --driver-side and --substrate-side, each an interface name")

    if options.transport == "tcp":
        relay = TcpRelay(engine, options.listen, options.upstream, supervised, buffer=options.buffer)
    elif options.transport == "udp":
        relay = UdpRelay(engine, options.listen, options.upstream, options.downstream, supervised)
    elif options.transport == "serial":
        relay = SerialRelay(engine, options.link_driver, options.link_substrate)
    elif options.transport == "can":
        relay = CanRelay(engine, options.driver_side, options.substrate_side)
    else:
        relay = RawRelay(engine, options.driver_side, options.substrate_side)

    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    relay.start()
    if options.control:
        threading.Thread(target=control, args=(options.control, engine, stop), daemon=True).start()
    print(relay.ready(), flush=True)
    while not stop.wait(0.1):
        pass
    relay.stop()
    print("stats " + json.dumps(engine.counters()), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
