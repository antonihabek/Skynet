#!/usr/bin/env python3
"""Configure Lely's CiA 402 fake slave as a drive this repository's masters can speak to.

`ros2_canopen`'s `canopen_fake_slaves` ships one device description, `cia402_slave.eds`, written for
its own master (`canopen_402_driver`), and that master reconfigures the slave's PDOs over SDO when it
boots it. The description as shipped is a valid CiA 402 device that these masters correctly refuse:

* it maps TPDO2 to the statusword and the *mode display*, and RPDO2 to the controlword and the *mode
  of operation* — where the CiA 402 default, which `drivers/canopen` and `drivers/canopen-rs` speak
  and check for at `open()`, is the statusword and the position actual value, and the controlword
  and the target position;
* it has no `0x6091` gear ratio and no `0x6080` max motor speed, and gives `0x608F:1` no value, so
  the unit a position is in is undeclared — the objects §5.2's rule ("a limit with no provenance is
  a rumour") makes the masters refuse a body over.

An integrator commissioning such a drive edits its device description, and that is all this does:
the EDS is read out of the image, the seven things below are changed, and the result is written as
the device configuration the slave is started with. Nothing about the slave's behaviour — the CAN
stack, the SDO server, the NMT state machine, the PDO engine, the CiA 402 state machine and the
motion it simulates — is touched, because none of it is in the description. That is Lely's and
`ros2_canopen`'s, and it is the point of running it.

    python3 make_dcf.py [--keep-mapping] <cia402_slave.eds> <out.dcf>

`--keep-mapping` leaves both PDOs as Lely ships them and changes everything else: the drive a
master must refuse for its mapping alone, with its units declared so nothing earlier refuses it.

What is changed, and why each figure is the one it is (the masters' own simulated drives use the
same, `skynet_canopen.slave.SimulatedDrive`):

| Object | Value | Why |
|---|---|---|
| `0x1A01:2` | `0x60640020` | TPDO2's second entry becomes the position actual value |
| `0x1601:2` | `0x607A0020` | RPDO2's second entry becomes the target position |
| `0x608F:1` | `131072` | encoder increments per motor revolution (2^17) |
| `0x6091:1`, `0x6091:2` | `50`, `1` | motor revolutions per shaft revolution: a 50:1 gearbox |
| `0x6080` | `3000` | max motor speed, rpm |
| `0x607D:1`, `0x607D:2` | `-4194304`, `4194304` | software position limits, ±2^22 increments |

Objects added because the shipped description lacks them: `0x6080` and `0x6091`.
"""

from __future__ import annotations

import configparser
import sys

#: The two PDO entries changed to the CiA 402 default this repository's masters speak.
MAPPING: dict[str, dict[str, str]] = {
    "1A01sub2": {"DefaultValue": "0x60640020"},
    "1601sub2": {"DefaultValue": "0x607A0020"},
}

#: `[section]` → `{key: value}` — the other changes to sections the description already has.
CHANGES: dict[str, dict[str, str]] = {
    "608Fsub1": {"DefaultValue": "131072"},
    "607Dsub1": {"DefaultValue": "-4194304"},
    "607Dsub2": {"DefaultValue": "4194304"},
}

#: Objects the description lacks, as `(index, section)` — index for the object list.
ADDITIONS: tuple[tuple[int, dict[str, dict[str, str]]], ...] = (
    (
        0x6080,
        {
            "6080": {
                "ParameterName": "Max Motor Speed",
                "ObjectType": "0x07",
                "DataType": "0x0007",
                "AccessType": "rw",
                "DefaultValue": "3000",
                "PDOMapping": "0",
            }
        },
    ),
    (
        0x6091,
        {
            "6091": {"SubNumber": "3", "ParameterName": "Gear Ratio", "ObjectType": "0x08"},
            "6091sub0": {
                "ParameterName": "Highest sub-index supported",
                "ObjectType": "0x07",
                "DataType": "0x0005",
                "AccessType": "const",
                "DefaultValue": "0x02",
                "PDOMapping": "0",
            },
            "6091sub1": {
                "ParameterName": "Motor Revolutions",
                "ObjectType": "0x07",
                "DataType": "0x0007",
                "AccessType": "rw",
                "DefaultValue": "50",
                "PDOMapping": "0",
            },
            "6091sub2": {
                "ParameterName": "Shaft Revolutions",
                "ObjectType": "0x07",
                "DataType": "0x0007",
                "AccessType": "rw",
                "DefaultValue": "1",
                "PDOMapping": "0",
            },
        },
    ),
)


def build(eds_text: str, *, keep_mapping: bool = False) -> str:
    """The device configuration, as text, from the shipped description's text."""
    parser = configparser.ConfigParser(interpolation=None, strict=True)
    parser.optionxform = str  # keys are case-sensitive in an EDS
    parser.read_string(eds_text)

    changes_to_make = dict(CHANGES) if keep_mapping else {**MAPPING, **CHANGES}
    for section, changes in changes_to_make.items():
        if section not in parser:
            raise SystemExit(
                f"make_dcf: the description has no [{section}]. `ros2_canopen` changed "
                "cia402_slave.eds, and this script's edits were written against the one it "
                "shipped at 0.3.4; read the new one before trusting the drive it would produce."
            )
        for key, value in changes.items():
            parser[section][key] = value

    listed = parser["OptionalObjects"]
    count = int(listed["SupportedObjects"], 0)
    present = {int(listed[str(i)], 0) for i in range(1, count + 1)}
    for index, sections in ADDITIONS:
        if index in present:
            raise SystemExit(
                f"make_dcf: the description already carries {index:#06x}. `ros2_canopen` added "
                "it since 0.3.4; drop the addition here rather than declaring it twice."
            )
        for name, keys in sections.items():
            parser[name] = keys
        count += 1
        listed[str(count)] = f"{index:#06x}"
        listed["SupportedObjects"] = str(count)

    out = []
    for name in parser.sections():
        out.append(f"[{name}]")
        out.extend(f"{key}={value}" for key, value in parser[name].items())
        out.append("")
    return "\n".join(out)


def main(argv: list[str]) -> int:
    keep_mapping = "--keep-mapping" in argv
    paths = [word for word in argv if word != "--keep-mapping"]
    if len(paths) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    with open(paths[0], encoding="utf-8") as source:
        text = build(source.read(), keep_mapping=keep_mapping)
    with open(paths[1], "w", encoding="utf-8", newline="\n") as sink:
        sink.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
