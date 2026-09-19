"""Golden CANopen vectors for `drivers/canopen-rs`, generated from the Python driver.

The Python driver is the executable specification (DRY-COVERAGE-PLAN.md 6.6), so the Rust port's
framing and profile are held to *its* outputs, not to a second reading of CiA 301 and CiA 402. This
file is committed beside `vectors.json` so the vectors can be regenerated rather than trusted:

    cd drivers/canopen-rs/tests
    PYTHONPATH=../../canopen:../../../bindings/python python3 gen_vectors.py > vectors.json

Every value here is computed by `skynet_canopen` itself. Nothing is typed in as an expected byte.
"""

from __future__ import annotations

import json
import math

from skynet_canopen import cia402, frames
from skynet_canopen.cia402 import Command, State


def frame(f: frames.Frame) -> dict:
    return {"id": f.arbitration_id, "data": f.data.hex()}


def main() -> None:
    out: dict = {
        "note": (
            "Golden vectors from skynet_canopen (frames.py, cia402.py). Regenerate with "
            "tests/gen_vectors.py; never edit by hand."
        ),
        "cob_ids": {
            "node": 5,
            "emcy": frames.emcy_id(5),
            "tpdo2": frames.tpdo2_id(5),
            "rpdo2": frames.rpdo2_id(5),
            "sdo_request": frames.sdo_request_id(5),
            "sdo_response": frames.sdo_response_id(5),
            "heartbeat": frames.heartbeat_id(5),
            "sync": frames.SYNC_ID,
            "nmt": frames.NMT_ID,
        },
        # Master-side requests, which the port must pack byte for byte.
        "requests": {
            "upload_statusword": frame(frames.sdo_upload_request(1, 0x6041, 0)),
            "upload_encoder_revolutions": frame(frames.sdo_upload_request(3, 0x608F, 2)),
            "download_controlword": frame(frames.sdo_download_request(2, 0x6040, 0, 0x000F, 2)),
            "download_mode_csp": frame(frames.sdo_download_request(4, 0x6060, 0, 8, 1)),
            "download_mode_negative": frame(frames.sdo_download_request(4, 0x6060, 0, -3, 1)),
            "download_target_negative": frame(
                frames.sdo_download_request(7, 0x607A, 0, -123456, 4)
            ),
            "rpdo2_enable": frame(frames.rpdo2(1, 0x000F, -4194304)),
            "rpdo2_quick_stop": frame(frames.rpdo2(3, 0x0002, 65536)),
            "nmt_start_node": frame(frames.Frame(frames.NMT_ID, bytes([frames.NMT_START, 2]))),
            "nmt_pre_operational_all": frame(
                frames.Frame(frames.NMT_ID, bytes([frames.NMT_PRE_OPERATIONAL, 0]))
            ),
        },
        # Drive-side replies, which the port must parse to the values the Python driver parses.
        "replies": [],
        "tpdo2": [],
        "states": [],
        "controlwords": {},
        "units": [],
    }

    replies = [
        ("upload_u32", frames.sdo_upload_response(1, 0x608F, 1, 131072, 4), 0x608F, 1),
        ("upload_u16", frames.sdo_upload_response(1, 0x6041, 0, 0x0637, 2), 0x6041, 0),
        ("upload_u8", frames.sdo_upload_response(2, 0x6061, 0, 8, 1), 0x6061, 0),
        ("upload_i32_raw", frames.sdo_upload_response(2, 0x607D, 1, -(1 << 22), 4), 0x607D, 1),
        ("download_ack", frames.sdo_download_response(3, 0x6040, 0), 0x6040, 0),
    ]
    for name, reply, index, subindex in replies:
        out["replies"].append(
            {
                "name": name,
                "frame": frame(reply),
                "index": index,
                "subindex": subindex,
                "value": frames.parse_sdo_response(reply.data, index, subindex),
            }
        )
    abort = frames.sdo_abort_response(4, 0x608F, 1, frames.ABORT_NO_SUCH_OBJECT)
    try:
        frames.parse_sdo_response(abort.data, 0x608F, 1)
        raise SystemExit("an abort parsed as a value")
    except frames.SdoAbort as error:
        out["abort"] = {
            "frame": frame(abort),
            "index": error.index,
            "subindex": error.subindex,
            "code": error.code,
            "explain": error.explain(),
        }

    for node, statusword, position in ((1, 0x0637, 123456), (2, 0x0250, -4194304)):
        packed = frames.tpdo2(node, statusword, position)
        out["tpdo2"].append(
            {"frame": frame(packed), "parsed": list(frames.parse_tpdo2(packed.data))}
        )

    for state in State:
        for voltage in (True, False):
            word = cia402.statusword_for(state, voltage=voltage)
            out["states"].append(
                {
                    "state": state.value,
                    "voltage": voltage,
                    "statusword": word,
                    "decoded": cia402.state_of(word).value,
                }
            )
    for command in Command:
        out["controlwords"][command.value] = cia402.controlword_for(command)

    # The simulated drive's defaults (131072 increments per motor turn, 50:1), and an odd ratio,
    # across rounding boundaries in both directions.
    for increments, revolutions, gear_motor, gear_shaft in ((131072, 1, 50, 1), (4096, 1, 101, 3)):
        for counts in (0, 1, -1, 4194304, -4194304, 123457):
            radians = cia402.increments_to_rad(
                counts, increments, revolutions, gear_motor, gear_shaft
            )
            out["units"].append(
                {
                    "ratio": [increments, revolutions, gear_motor, gear_shaft],
                    "counts": counts,
                    "radians": radians,
                    "back": cia402.rad_to_increments(
                        radians, increments, revolutions, gear_motor, gear_shaft
                    ),
                }
            )
        for radians in (0.5, -0.5, 1e-9, -1e-9, 3.14159):
            out["units"].append(
                {
                    "ratio": [increments, revolutions, gear_motor, gear_shaft],
                    "radians_in": radians,
                    "counts_out": cia402.rad_to_increments(
                        radians, increments, revolutions, gear_motor, gear_shaft
                    ),
                }
            )

    # How the descriptor renders numbers and strings. The bridge's descriptor is `json.dumps` with
    # compact separators, and the port's is held to it byte for byte — so the port's renderer is
    # held here first, to Python's own output, across the cases where Python's float repr and a
    # naive formatter part ways: integral floats, the positional/exponential switch at 1e-4 and
    # 1e16, subnormals, and non-ASCII text under `ensure_ascii`.
    numbers = (
        0.0, -0.0, 1.0, 3.0, 0.5, 1000.0, 1e-05, 0.0001, 0.00012345, 1e15, 1e16, 1.5e16,
        123456789012345.67, 2.5e-07, 1.7976931348623157e308, 5e-324, 0.1 + 0.2,
        cia402.increments_to_rad(1 << 22, 131072, 1, 50, 1),
        cia402.increments_to_rad(-(1 << 22), 131072, 1, 50, 1),
        3000 / 60.0 * 2.0 * math.pi * (1 / 50),
    )
    out["json_numbers"] = [{"value": value, "text": json.dumps(value)} for value in numbers]
    strings = ("J1", "right_arm.shoulder.pitch", 'a"b\\c', "tab\there\nline\r\x08\x0c",
               "\x01\x1f\x7f", "café", "☃", "\U0001f600")
    out["json_strings"] = [{"value": value, "text": json.dumps(value)} for value in strings]

    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
