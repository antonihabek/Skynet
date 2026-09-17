"""The URScript program that turns RTDE registers into joint motion.

# Why there is a program at all

A UR controller owns its servo loop, and the only way into it at rate is `servoj`, which is a
URScript statement. So something has to be *running on the controller* reading setpoints, and the
two ways to feed it are:

* **Send a `servoj` statement per cycle** over the secondary interface. It works, and every statement
  **replaces the running program**: the controller stops what it was doing and starts a one-line
  program fifty times a second. It is the approach that looks simplest and it is not what a
  production integration does.
* **Upload a loop once** and hand it setpoints through RTDE's general-purpose registers. One program,
  started at open, reading six doubles and an integer every pass.

This driver takes the second. `SERVO_PROGRAM` is the loop.

# The program, line by line

```
def skynet_control():
  while (True):
    cmd = read_input_integer_register(0)
    if cmd == 1:
      q = [read_input_float_register(0), ... read_input_float_register(5)]
      servoj(q, 0, 0, 0.008, 0.1, 300)
    else:
      sync()
    end
  end
end
```

`cmd` is the gate, and it is the field that makes a **hold** expressible. §24's command mask says
*hold this joint*, and a control loop that stopped writing registers would leave `servoj` with a
setpoint it goes on tracking; `cmd == 0` means the program calls `sync()` instead, which yields one
control cycle and commands nothing. A driver with no gate can say "go here" and cannot say "stop
asking".

`servoj(q, a, v, t, lookahead_time, gain)` — `a` and `v` are ignored by `servoj` and are written as
zero rather than omitted, because the positional form is what every UR example uses and a reader
comparing this against one should find the same shape. `t = 0.008` is the control period the
statement covers: the driver must produce a setpoint at least that often or the arm decelerates,
which is the honest behaviour and is why `write_command` is the thing that must not be late.

# What this file deliberately does not do

It does not template the program from a configuration. The lookahead and the gain are the two
numbers that decide whether this arm is smooth or unstable, they are properties of a machine nobody
here has, and a driver that let them be set from a JSON field would be offering a tuning knob whose
safe range it cannot state. They are constants, they are cited, and changing them is editing this
file — which is a review rather than a deployment.
"""

from __future__ import annotations

__all__ = ["SERVO_PROGRAM", "COMMAND_REGISTER", "FIRST_JOINT_REGISTER", "CMD_HOLD", "CMD_SERVO"]

#: The integer register the program reads as its gate.
COMMAND_REGISTER = 0
#: The first of six consecutive float registers carrying the setpoint.
FIRST_JOINT_REGISTER = 0

CMD_HOLD = 0
CMD_SERVO = 1

#: Seconds one `servoj` statement covers. The driver must write a setpoint at least this often.
SERVO_PERIOD_S = 0.008
#: `servoj`'s lookahead and gain. Constants on purpose — see the module header.
SERVO_LOOKAHEAD_S = 0.1
SERVO_GAIN = 300

SERVO_PROGRAM = f"""def skynet_control():
  while (True):
    cmd = read_input_integer_register({COMMAND_REGISTER})
    if cmd == {CMD_SERVO}:
      q = [read_input_float_register({FIRST_JOINT_REGISTER + 0}),
           read_input_float_register({FIRST_JOINT_REGISTER + 1}),
           read_input_float_register({FIRST_JOINT_REGISTER + 2}),
           read_input_float_register({FIRST_JOINT_REGISTER + 3}),
           read_input_float_register({FIRST_JOINT_REGISTER + 4}),
           read_input_float_register({FIRST_JOINT_REGISTER + 5})]
      servoj(q, 0, 0, {SERVO_PERIOD_S}, {SERVO_LOOKAHEAD_S}, {SERVO_GAIN})
    else:
      sync()
    end
  end
end
"""
