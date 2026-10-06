"""Generate the Piper entry of `drivers/fault-vocabulary.json` from AgileX's own SDK, and splice it in.

    PYTHONPATH=bindings/python:drivers/canopen:drivers/piper python3 drivers/piper/vocab_gen.py <piper_sdk checkout> [--write]

The table is *generated* from the maker's own source and not retyped, the way ODrive's was from its DBC: the names of the sixteen arm
statuses, the twelve flags of the error word, and the eight flags of a joint driver's and of the gripper's status bytes are the
maker's own attribute and enumerator names, read out of `piper_sdk` at the pin `layout_check.py` holds. What the *class* of each code is
(`ok`, `fault`, `estop`) is this repository's statement about what the driver must do with it, and is written here.

Run without `--write` it prints the entry; with it, it adds or replaces `"piper"` in the file as text, leaving every other byte alone.
"""

from __future__ import annotations

import json
import os
import sys

from skynet_piper import protocol as p

PIN = "c9e8a28174e71eeaac448593cb65f8ab258a92fe"


def load(sdk: str):
    sys.path.insert(0, sdk)
    from piper_sdk.piper_msgs.msg_v2 import ArmMsgFeedbackLowSpd, ArmMsgFeedbackStatus, ArmMsgFeedbackStatusEnum, ArmMsgFeedBackGripper

    return ArmMsgFeedbackLowSpd, ArmMsgFeedbackStatus, ArmMsgFeedbackStatusEnum, ArmMsgFeedBackGripper


def entry(sdk: str) -> dict:
    low_spd, status_message, enums, gripper_message = load(sdk)
    source = (
        f"AgileX's own SDK, `agilexrobotics/piper_sdk` at {PIN[:8]} (2026-07-31, the pin `drivers/piper/layout_check.py` holds), "
        "names generated from its enumerations and flag classes and not retyped"
    )
    read_only = "Read, not measured: the maker's firmware raises these on events this repository cannot cause, and AgileX publishes no simulator."

    # arm status: the maker's sixteen values.
    arm_codes = {}
    for member in enums.ArmStatus:
        if member.name == "UNKNOWN":
            continue
        cls = "ok" if member == 0 else ("estop" if member == p.ARM_STATUS_EMERGENCY_STOP else "fault")
        arm_codes[str(int(member))] = {"name": f"0x{int(member):02X} {member.name}", "class": cls}

    # the error word: the maker's twelve flags, by the bit its own setter reads.
    probe = status_message()
    word_codes = {"0": {"name": "0x0000 NONE", "class": "ok"}}
    for bit in range(16):
        probe.err_code = 1 << bit
        flags = vars(probe.err_status)
        named = [name for name, value in flags.items() if value]
        if named:
            word_codes[str(1 << bit)] = {"name": f"0x{1 << bit:04X} {named[0].upper()}", "class": "fault"}

    def byte_codes(message_class, ok_name: str, skip: tuple[str, ...]) -> dict:
        probe_message = message_class()
        codes = {"0": {"name": f"0x00 {ok_name}", "class": "ok"}}
        for bit in range(8):
            probe_message.status_code = 1 << bit
            named = [name for name, value in vars(probe_message.foc_status).items() if value]
            if named and named[0] not in skip:
                codes[str(1 << bit)] = {"name": f"0x{1 << bit:02X} {named[0].upper()}", "class": "fault"}
        return codes

    # a joint driver's byte: the maker's class has a setter on `foc_status_code`, the gripper's on `status_code`.
    joint = low_spd()
    joint_codes = {"0": {"name": "0x00 NONE", "class": "ok"}}
    for bit in range(8):
        joint.foc_status_code = 1 << bit
        named = [name for name, value in vars(joint.foc_status).items() if value]
        if named and named[0] != "driver_enable_status":
            joint_codes[str(1 << bit)] = {"name": f"0x{1 << bit:02X} {named[0].upper()}", "class": "fault"}
    gripper_codes = byte_codes(gripper_message, "NONE", skip=("driver_enable_status", "homing_status"))

    modes = {}
    for member in enums.CtrlMode:
        if member.name == "UNKNOWN":
            continue
        modes[str(int(member))] = {"name": f"0x{int(member):02X} {member.name}", "class": "ok" if member == p.CTRL_CAN else "fault"}

    return {
        "subject": "drivers/piper/faultvocab.py",
        "vocabularies": {
            "arm_status": {
                "channel": "the arm status byte of every status frame (0x2A1 byte 1), read every cycle",
                "source": f"{source}: ArmMsgFeedbackStatusEnum.ArmStatus, sixteen values. {read_only}",
                "kind": "alarm",
                "why": "every nonzero status is the arm saying it is not doing what it is told (a stop, no solution, a limit, a joint that does not answer, a brake that did not release, a collision, a temperature) or that it is in a teaching state in which it takes no CAN command. The driver faults on all of them and refuses commands whole; it sends no frame that clears one, and the maker's resume byte is a reset that drops the arm. 1 is the arm's own fast stop and reads as estop: it is the stop chain this driver itself sends into, and another master's.",
                "codes": arm_codes,
                "unmapped": "fault",
                "unmapped_because": "the status is a value, not a bitset, and the driver reads it whole (`status.arm_status != NORMAL`): a value the table does not define is an arm newer than this table, and reading it as normal would clear a fault nobody read. The probes are values past the sixteen the maker defines, one in every power of two and the top of the byte.",
                "probes": [16, 17, 32, 64, 128, 255],
                "fault_code": "derived",
                "fault_code_is": "the arm status in bits 16 to 23 of the fault word: `code << 16`",
            },
            "error_word": {
                "channel": "the error word of every status frame (0x2A1 bytes 6 and 7, one big-endian uint16), read every cycle",
                "source": f"{source}: the twelve flags of ArmMsgFeedbackStatus.ErrStatus, by the bit its own setter reads (communication in bits 0 to 5, angle limit in bits 8 to 13). {read_only}",
                "kind": "alarm",
                "why": "every nonzero word is a joint that does not answer or a joint past its angle limit: the arm refuses the target or goes on without that joint, and the driver refuses commands whole until a person clears it at the arm. The word is carried verbatim in the low sixteen bits of the fault word.",
                "codes": word_codes,
                "unmapped": "fault",
                "unmapped_because": "the word is a bitset the driver reads whole (`if status.err_code`, carried verbatim): a bit the maker lists as reserved (6, 7, 14, 15) is a flag from a firmware newer than this table, and masking it to healthy would clear a fault nobody read.",
                "probes": [64, 128, 16384, 32768],
                "fault_code": "verbatim",
            },
            "driver_status": {
                "channel": "a joint driver's status byte (0x261 to 0x266 byte 5), read every cycle for each of the six joints",
                "source": f"{source}: the flags of ArmMsgFeedbackLowSpd.FOC_Status. {read_only}",
                "kind": "alarm",
                "why": "each fault bit (supply voltage, motor over temperature, over current, driver over temperature, collision protection, driver error, stall protection) is a joint that is protecting itself or has failed; the arm may ignore its target or stop, and the driver refuses commands whole. The union of the six drivers' fault bits is the top byte of the fault word, so what a driver said is not thrown away.",
                "codes": joint_codes,
                "unmapped": "fault",
                "unmapped_because": "every bit of the byte is defined: seven fault bits and the enable bit, which is its own vocabulary (`driver_enable`).",
                "probes": [],
                "no_probes": "a byte with every bit defined has no value outside its table",
                "fault_code": "derived",
                "fault_code_is": "the injected driver's fault bits in the top byte: `code << 24`",
            },
            "driver_enable": {
                "channel": "a joint driver's enable bit (0x261 to 0x266 byte 5, bit 6), read every cycle for each of the six joints",
                "source": f"{source}: FOC_Status.driver_enable_status. A disabled Piper joint is a limp one. {read_only}",
                "kind": "alarm",
                "why": "a driver that reports itself disabled while the session expects it enabled is a joint that will not follow its target and, on a Piper, that gives way: the driver faults and refuses commands, and never re-enables on its own after open (a disable is the person's, or the arm's own protection).",
                "codes": {"1": {"name": "0x40 DRIVER_ENABLED", "class": "ok"}, "0": {"name": "0x00 DRIVER_DISABLED", "class": "fault"}},
                "unmapped": "fault",
                "unmapped_because": "a bit has two values and both are listed.",
                "probes": [],
                "no_probes": "a single bit has no value outside its table",
                "fault_code": "derived",
                "fault_code_is": "0x40 in the top byte for a disabled driver, and zero for an enabled one",
            },
            "gripper_status": {
                "channel": "the gripper's status byte (0x2A8 byte 6), read every cycle when the configuration declares a gripper",
                "source": f"{source}: the flags of ArmMsgFeedBackGripper.FOC_Status. Bit 4 is the gripper's *sensor* where a joint driver's is collision protection, and bit 7 its zeroing state where a joint driver's is stall protection. {read_only}",
                "kind": "alarm",
                "why": "each fault bit (supply voltage, motor over temperature, over current, driver over temperature, sensor, driver error) is a hand that is protecting itself or has failed. The body is one driver, so the driver faults and refuses commands whole, joints included; the gripper's faults are bit 14 of the fault word, not mixed into the six joint drivers' top byte, whose bits mean something else.",
                "codes": gripper_codes,
                "unmapped": "fault",
                "unmapped_because": "bits 6 and 7 (enabled, zeroed) are not faults and the six below them are all listed.",
                "probes": [],
                "no_probes": "every fault bit of the byte is defined, and the two above them are state, not faults",
                "fault_code": "derived",
                "fault_code_is": "bit 14 of the fault word (FAULT_GRIPPER) for any gripper fault bit, and zero for none",
            },
            "gripper_enable": {
                "channel": "the gripper's enable bit (0x2A8 byte 6, bit 6), read every cycle when the configuration declares a gripper",
                "source": f"{source}: ArmMsgFeedBackGripper.FOC_Status.driver_enable_status. {read_only}",
                "kind": "alarm",
                "why": "a hand that has disabled itself is not taking commands; the driver faults, as for a joint driver, with the same 0x40 in the top byte.",
                "codes": {"1": {"name": "0x40 DRIVER_ENABLED", "class": "ok"}, "0": {"name": "0x00 DRIVER_DISABLED", "class": "fault"}},
                "unmapped": "fault",
                "unmapped_because": "a bit has two values and both are listed.",
                "probes": [],
                "no_probes": "a single bit has no value outside its table",
                "fault_code": "derived",
                "fault_code_is": "0x40 in the top byte for a disabled gripper, and zero for an enabled one",
            },
            "ctrl_mode": {
                "channel": "the control mode byte of every status frame (0x2A1 byte 0), read every cycle",
                "source": f"{source}: ArmMsgFeedbackStatusEnum.CtrlMode, eight values. {read_only}",
                "kind": "alarm",
                "why": "CAN control is the one mode in which the arm takes this master's joint targets. Any other (standby, teaching, Ethernet, Wi-Fi, remote controller, linkage teaching, offline trajectory) is the arm listening to something else: the driver faults and refuses commands whole, and sets bit 15 of the fault word, the one bit of it that is this driver's, so the flag has a word beside it (§24).",
                "codes": modes,
                "unmapped": "fault",
                "unmapped_because": "the mode is a value the driver compares with CAN control: a value the table does not define is not CAN control.",
                "probes": [8, 16, 128, 255],
                "fault_code": "derived",
                "fault_code_is": "bit 15 of the fault word (FAULT_NOT_CAN_CONTROL) for any mode but CAN control, and zero for CAN control",
            },
        },
        "not_covered": [
            {
                "what": "the drivers' voltage, temperature and current numbers (0x251 to 0x266)",
                "why": "the driver reads each driver's status byte and never the analogue values: the maker's own thresholds raise the over-temperature, over-current and low-voltage flags, so a number the driver compared with a limit of its own would be a second, unsourced threshold beside the maker's.",
            },
            {
                "what": "the move mode, teach status, motion status and trajectory number of the status frame",
                "why": "none is a fault: the move mode is what this driver set, motion status says whether a target was reached (the positions say the same), and the teach and trajectory fields belong to modes this driver does not speak. A teaching state is in `arm_status` and in `ctrl_mode`.",
            },
            {
                "what": "the gripper's zeroing bit (0x2A8 byte 6, bit 7)",
                "why": "a gripper that has not been zeroed reports a stroke from an arbitrary origin, which is a calibration state and not a fault; `homing` is reported as unreported for the whole body (the arm's joints report no zeroing state at all), and the zeroing command is never sent.",
            },
        ],
    }


def _end_of_object(text: str, start: int) -> int:
    """The index just past the JSON object that begins at ``text[start]`` (a ``{``), reading strings so a brace inside one does not count."""
    assert text[start] == "{"
    depth = 0
    in_string = False
    index = start
    while index < len(text):
        char = text[index]
        if in_string:
            if char == "\\":
                index += 1
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    raise ValueError("the object does not end")


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    sdk = argv[0]
    result = entry(sdk)
    body = json.dumps(result, indent=2, ensure_ascii=False)
    body = "\n".join(("    " + line) if index else line for index, line in enumerate(body.split("\n")))
    if "--write" not in argv[1:]:
        print(body)
        return 0
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fault-vocabulary.json")
    text = open(path, encoding="utf-8", newline="").read()
    assert "\r\n" not in text
    tail = "\n  }\n}\n"
    assert text.endswith(tail), repr(text[-20:])
    marker = '\n    "piper": '
    if marker in text:
        start = text.index(marker) + len(marker)
        end = _end_of_object(text, start)
        text = text[:start] + body + text[end:]
    else:
        text = text[: -len(tail)] + ",\n    \"piper\": " + body + tail
    json.loads(text)
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)
    print(f"wrote the piper entry to {os.path.normpath(path)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
