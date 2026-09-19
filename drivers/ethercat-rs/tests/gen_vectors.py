"""Golden EtherCAT frames for `drivers/ethercat-rs`, generated from the Python driver.

    cd drivers/ethercat-rs/tests
    PYTHONPATH=../../ethercat:../../canopen:../../../bindings/python python3 gen_vectors.py > vectors.json

Every frame is `skynet_ethercat.frames` packing it; nothing is typed in as an expected byte.
"""

from __future__ import annotations

import json

from skynet_ethercat import frames


def main() -> None:
    cases = []

    def add(name: str, datagrams: list) -> None:
        packed = frames.Frame(datagrams).pack()
        back = frames.Frame.unpack(packed)
        cases.append(
            {
                "name": name,
                "raw": packed.hex(),
                "datagrams": [
                    {
                        "command": d.command,
                        "index": d.index,
                        "address": list(d.address),
                        "data": d.data.hex(),
                        "more": d.more,
                        "working_counter": d.working_counter,
                    }
                    for d in back.datagrams
                ],
            }
        )

    add("brd_type", [frames.Datagram(frames.BRD, 1, (0, 0x0000), bytes(1))])
    add("apwr_station", [frames.Datagram(frames.APWR, 2, (0xFFFF, 0x0010), bytes([0x02, 0x10]))])
    add("fprd_al_status", [frames.Datagram(frames.FPRD, 3, (0x1001, 0x0130), bytes(2))])
    add(
        "lrw_image",
        [frames.Datagram(frames.LRW, 200, (0, 0), bytes(range(36)), working_counter=9)],
    )
    add(
        "two_datagrams",
        [
            frames.Datagram(frames.FPWR, 4, (0x1002, 0x0800), bytes(8)),
            frames.Datagram(frames.BWR, 5, (0, 0x0120), bytes([0x08, 0x00])),
        ],
    )
    note = "Golden frames from skynet_ethercat.frames; regenerate with tests/gen_vectors.py."
    print(json.dumps({"note": note, "frames": cases}, indent=2))


if __name__ == "__main__":
    main()
