# skynet-apollo — an Apptronik Apollo humanoid in MuJoCo

Phase 7 named this row unreachable: no sales, no interface, no route. The challenge held
for the machine and fell for the model — DeepMind's menagerie ships an Apollo MJCF under
Apache-2.0, maintained (a keyframe fix landed 2026-09-19), and engaged with by the vendor
itself (`CHANGELOG.md`: *"IMU position correction based on Apptronik request"*). So a sim
driver on a pinned substrate is honest work, and a hardware claim would not be. No
Apptronik SDK exists publicly, no unit is for sale, and nothing here speaks to either.

```bash
make check MUJOCO_GL=egl            # from drivers/apollo (osmesa in CI)
```

The substrate is fetched at check time, never vendored: a sparse checkout of
`google-deepmind/mujoco_menagerie` at commit `822c2d8`, verified by commit and then by
the sha256 of both XML files. `MENAGERIE_DIR` overrides the fetch when the model is
already on disk; the verification does not relax.

## Three facts shape the driver, and all three are the file's

**1. Thirty-two position servos, each named for its hinge.** Actuator order is the RHAL
joint order — torso, neck, arms, legs — with a free `floating_base` the RHAL never
carries as a joint. The suite asserts the names in order; the cross-check asserts the
served descriptor against the file joint for joint.

**2. The forceranges are servo tuning, not torque ratings.** Every actuator carries one
(±10.6 N·m at the neck, ±494 at the hip), and the served per-joint `effort_max_nm`
repeats them — that is what the sim servo applies, true of the model. The *envelope*
stays null anyway: a safety figure needs a datasheet and a risk assessment, and a policy
validated against a modeller's gains would carry that validation to hardware, which is
the transfer §70 exists to prevent. The cross-check asserts both halves together so the
row can never learn to cite the tuning as a rating.

**3. The base and the roles are the operator's words.** A free root says the machine
moves and not how, so `open()` refuses until `base` is declared (`legged`); the 32
roles in `roles.json` are an anatomical assertion in `chain.link.axis` form, because
`describe` assigns none and a second implementation of "which joint is this" is the
failure mode the corpus exists to prevent.

## What `make check` runs

The suite (substrate pin, bring-up refusals, the 32-joint descriptor, the clock, masked
holds, three seconds of the kernel's hold frame on the standing body — which put Apollo on
the floor until the MuJoCo driver latched its hold, F121 — the stand, a 0.3 rad elbow motion
proof, the stop latch, the pipe half), the layout cross-check above, then `skynet driver check` through the real ABI with the
scene, base, keyframe and roles in `--config`. Guest ceiling, twice over — Python
(§31.3) and a simulator (§35.3) — with `simulator: true` on every result.

## What it is not

Not the hardware: no torque ratings, no vendor motion data, no balance controller (the
PD servos hold the stand; nothing keeps it there). Not the Isaac USD (`digit_v4`-class
assets need the Isaac Sim driver §70 names and this tree does not have). The three
torso/track cameras render when a GL context exists and are absent with a note when one
does not — a declared stream nothing can read would bind a policy's observation to
nothing.
