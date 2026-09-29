# skynet-ur — a Universal Robots arm over RTDE

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as *"URSim (UR's own
controller, containerised) … Almost everything, against real vendor software"*, and that is what it
was built against. **Every byte this driver sends was measured against URSim**, not transcribed from
memory of a document — which is the difference between this and the thing F15 was right to refuse: a
hand-written protocol tested against a fake written from the same memory proves that two copies of
one misunderstanding agree.

```bash
make selftest                          # the wire format. Needs nothing
make check-ursim-docker                # URSim at the pin in ursim.mk, as a UR5, everything against it, URSim removed
make check-ursim-models                # the same for every model the driver names — what the nightly runs

# or against a URSim you started yourself:
SKYNET_UR_HOST=127.0.0.1 make check
```

## Three interfaces, because one is not enough

| Port | What only it can do |
|---|---|
| 30004 RTDE | the body at 125 Hz, and the setpoint registers |
| 29999 dashboard | power on, release the brakes, read the safety status |
| 30002 secondary | upload the URScript program that reads the registers |

A UR owns its servo loop. The only way in at rate is `servoj`, which is a URScript statement, so
something has to be *running on the controller*. [`control.py`](skynet_ur/control.py) is that loop:
it reads six float registers and an integer gate every pass and calls `servoj`, and the driver
writes the registers over RTDE. The alternative — one `servoj` statement per cycle over the
secondary interface — works and **replaces the running program fifty times a second**, which is not
what a production integration does.

The gate is what makes §24's **hold** expressible. A loop that simply stopped writing would leave
`servoj` tracking its last setpoint; `cmd == 0` makes the program call `sync()` instead, which
yields a control cycle and commands nothing.

## Two things the measurement taught

**`TEXT_MESSAGE` arrives unsolicited, interleaved with replies.** The first thing a fresh URSim says
is *"Safety Setup has not been confirmed yet"*, and it says it in the middle of the handshake. A
client that takes the next packet as its reply is then off by one for the rest of the session — it
reads the version reply as the outputs reply, and every one of them parses. The first probe against
URSim did exactly that, and the symptom was a recipe id of zero and a types list of spaces.

**The output stream is a firehose, and a client that does not drain it reads the past.** Packets
arrive at 125 Hz whether or not anybody reads them, and a socket buffer holds seconds of them. The
second probe commanded a motion, sampled every fortieth cycle, and concluded the robot had not
moved. It had; the packets being read were a second old. `Rtde.newest()` drains to the last complete
packet, which is §47.6's latest-wins rule arriving from a different direction: for a control loop,
an old sample is not data.

There is a third, found by the suite rather than the probes: **`IN_USE` does not only mean "no such
variable"**. RTDE's general-purpose registers are allocated to one session at a time, so a second
client — or the same one reconnecting before the controller has released them — is told `IN_USE`
about a recipe that worked moments earlier. The refusal used to name the wrong cause; it names both
now, commonest first.

## What it refuses

* **A configuration with no `host`.** There is no default: a driver that guessed `192.168.1.100`
  would eventually connect to somebody else's robot.
* **A robot that is not in `NORMAL` safety.** `unlock protective stop` exists on the dashboard and
  this driver does not send it. A protective stop is the machine saying a person or a collision
  happened, and a driver that cleared it on the way up would erase the one signal that says so.
* **A recipe the controller answered `IN_USE` for**, per above.
* **Any mode but position.** Velocity is `speedj` and torque is not reachable from URScript at all;
  wiring either against a controller nobody has run them on is the shape F15 refused.

## What it does not claim

**Joint torque.** RTDE publishes `actual_current` in amps, and turning that into newton-metres needs
a torque constant and a gear ratio the controller does not publish. §5.2 forbids inventing one, so
`effort_nm` is `NaN` on every cycle — §24's value for a quantity the driver does not measure.

**A safety envelope.** Per-model, from the tech sheets: with a declared model the envelope
carries that model's axis velocity and torque ratings with the citation (§5.3 `basis: declared`)
and an 80 ms watchdog; with no model declared the envelope is null and the guards that need one
report unavailable — a rating without a model is still a rumour.

**A rated stop.** `emergency_stop()` clears the gate, tells the dashboard to stop the program, and
latches a flag that refuses every later command. A rated stop is the emergency circuit, and no frame
on any of these three sockets reaches it — the descriptor declares no `estop` rather than `true`, and
§15's `safety.estop.independent` is a bench test this body cannot pass.

**Servo timing.** The plan's table puts that in the *not dry-verifiable* column and it stays there.
URSim is the controller's software on a developer's machine, not a controller: what it proves is the
protocol and the program, not that a setpoint reaches a joint inside 8 ms.

## No fake

There is no simulated UR in this package, deliberately. A UR driver with a built-in simulator is a
driver that comes up when the arm is unplugged, and the first thing anybody would learn from it is
that the robot is fine.
