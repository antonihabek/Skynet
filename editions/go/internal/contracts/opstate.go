package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The operating state machine (§27, extended by §48.1).
//
// # It is a function of what is observed, not a table of edges
//
// §33.3's rule, applied to state rather than to packages: *an edge-triggered supervisor acts on
// transitions … miss one edge and the system stays wrong forever. A level-triggered supervisor asks
// "what is true, what should be true, what is the difference" and converges from any starting
// state.* §27 gives a table of **entry conditions**, so `Evaluate` is a pure function of the
// conditions observed right now, in a fixed priority order. There is no edge to miss and recovery
// is not a special case.
//
// # Every default is the least capable reading
//
// A caller who has not observed something has not established it. Absent `post` is `incomplete`,
// absent `kernel` is `down`, absent `rhal` is `lost` — a state machine that read "nobody told me
// about the kernel" as "the kernel is live" would be the single most dangerous line here.
//
// # The rules are tried least-capable-first
//
// §33.4: *anything that reduces what the robot may do applies immediately … making a robot less
// capable is never the dangerous direction.* Where two conditions hold, the one that stops the
// robot wins, and the reason reported is that one — which is what an operator needs, because the
// condition holding the robot is the one they have to clear.

// Conditions is what was observed. Every field carries its own presence, because absent is a
// different fact from any value it could take and each absence has a stated default above.
type Conditions struct {
	Post              string
	Homing            string
	Kernel            string
	RHAL              string
	Stop              string
	HasStop           bool
	StopSettled       bool
	GuardHold         bool
	GuardsUnavailable bool
	Policy            string
	ClearedBy         string
}

// Transition is one evaluation of the machine.
type Transition struct {
	From    string
	To      string
	Reason  string
	Changed bool
	Warning bool
}

// IsSafeState reports whether a state is one §33.4's barrier may open at.
//
// `stopping` is not one, and it is the interesting entry: a robot mid-ramp is the state that most
// looks like it is about to be safe and most certainly is not. `safe`, `post` and `unhomed` are,
// because nothing in any of them is moving or holding a task — which is what makes an OS update
// apply to a robot that failed its self-test, the behaviour to want, since the remedy for a robot
// in SAFE is often the update.
func IsSafeState(state string) bool {
	return state == "post" || state == "unhomed" || state == "hold" || state == "safe"
}

// CommandsAutonomously reports whether a policy is commanding the body in this state.
//
// `running` and `degraded`, and the second is the one that had been getting this wrong. §27's
// DEGRADED row says the robot *runs*, reports, and refuses any package whose grants depend on the
// failed guard — it is the impaired *running* state, not a third resting one.
func CommandsAutonomously(state string) bool {
	return state == "running" || state == "degraded"
}

// IsSticky reports whether a state requires an explicit human action to leave (§27). Exactly one
// does: a robot that recovers itself from a safety stop is a robot that resumes moving while
// somebody is standing next to it working out what happened.
func IsSticky(state string) bool { return state == "safe" }

// WarningRequired reports whether §49.1's tone must sound on this transition.
//
// Phrased over CommandsAutonomously rather than compared to the literal `running`, and the reason
// is a defect the reference records: `hold → degraded` is a policy beginning to command a body and
// is not a transition into `running`, so the tone did not sound. A robot whose guard set is
// imperfect — every robot with a lapsed calibration, §45.5's ordinary case — would have begun
// autonomous motion silently. "Resumption after a stop" falls out: `stopping` and `safe` command
// nothing, so leaving either for a commanding state sounds.
func WarningRequired(from, to string) bool {
	return CommandsAutonomously(to) && !CommandsAutonomously(from)
}

// cleared reports whether a clearance is a named human action. An empty name is not one — a caller
// that had nobody to name should not have a clearance.
func cleared(conditions Conditions) bool { return len(conditions.ClearedBy) > 0 }

// stoppingReason is the reason a stop is in progress, by cause.
//
// A switch rather than a template literal, so an added StopCause is a case the corpus rejects
// instead of a reason string nothing declares.
func stoppingReason(cause string) (string, bool) {
	switch cause {
	case "watchdog":
		return "stopping_watchdog", true
	case "proximity":
		return "stopping_proximity", true
	case "balance":
		return "stopping_balance", true
	case "operator":
		return "stopping_operator", true
	case "estop":
		return "stopping_estop", true
	}
	return "", false
}

// derive is the state and reason the conditions imply, before stickiness.
func derive(conditions Conditions) (string, string) {
	rhal := conditions.RHAL
	if rhal == "" {
		rhal = "lost"
	}

	// 1. The kernel lost the body. §27 puts this in the SAFE row beside a completed stop, and it is
	//    first because it is the condition under which nothing else can be known: a robot whose
	//    RHAL is gone has no observed joint state, no estop reading and no guard that can evaluate.
	if rhal == "lost" {
		return "safe", "rhal_lost"
	}

	// 2. A failed self-test. §48.2: SAFE, not DEGRADED, "because a robot that failed its own
	//    self-test should not be talked into running by a reconcile loop".
	if conditions.Post == "failed" {
		return "safe", "post_failed"
	}

	// 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever
	//    the robot is doing, including booting.
	if conditions.HasStop {
		if conditions.StopSettled {
			return "safe", "stop_complete"
		}
		reason, known := stoppingReason(conditions.Stop)
		if !known {
			// The reference's switch is exhaustive over StopCause and has no default; a cause
			// outside the vocabulary cannot be spelt in its type. Here it can be spelt, so it is
			// refused rather than given a reason nothing declares.
			return "stopping", ""
		}
		return "stopping", reason
	}

	// 4. The self-test has not established that this robot may run. Not a fault: §48.4's
	//    post_incomplete, which is a robot waiting on the rest of the operating system.
	post := conditions.Post
	if post == "" {
		post = "incomplete"
	}
	if post != "passed" {
		return "post", "post_incomplete"
	}

	// 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every
	//    pose-dependent guard is unavailable — so a policy must not run, and homing is the only
	//    motion permitted (§48.3).
	homing := conditions.Homing
	if homing == "" {
		homing = "pending"
	}
	if homing == "pending" {
		return "unhomed", "homing_pending"
	}

	// 6. §27: "a robot with a policy and no kernel must not exist". Refusing to start is HOLD — the
	//    safe resting state — rather than SAFE: nothing has gone wrong with the robot, and a sticky
	//    state needing a human would be the wrong answer for a kernel that is merely still starting.
	kernel := conditions.Kernel
	if kernel == "" {
		kernel = "down"
	}
	if kernel != "live" {
		return "hold", "kernel_down"
	}

	// 7. §27's HOLD row, first half. A guard asking for a hold outranks a policy that would like to
	//    run, which is the whole point of a guard.
	if conditions.GuardHold {
		return "hold", "guard_hold"
	}

	// 8. §27's HOLD row, second half: "no policy". Split in two, because "there is nothing to run"
	//    and "there is something and it has not got a lease" are different sentences and the second
	//    is the one with a remedy.
	policy := conditions.Policy
	if policy == "" {
		policy = "none"
	}
	if policy == "none" {
		return "hold", "no_policy"
	}
	if policy != "ready" {
		return "hold", "policy_not_ready"
	}

	// 9. §27's DEGRADED row, below the policy rules deliberately — see CommandsAutonomously.
	//    Reaching here means a policy is ready to command, so `degraded` is the impaired running
	//    state rather than a resting one.
	if conditions.GuardsUnavailable {
		return "degraded", "guards_unavailable"
	}
	if rhal == "degraded" {
		return "degraded", "rhal_degraded"
	}

	return "running", "nominal"
}

// Evaluate is the state this robot is in, given where it was and what is true now.
//
// Pure, clockless and total. Every combination of conditions produces a state, including
// combinations nobody anticipated, which is the property §33.3 asks of anything that converges.
func Evaluate(previous string, conditions Conditions) Transition {
	state, reason := derive(conditions)

	// §27's stickiness, applied after the derivation rather than instead of it. A robot in SAFE
	// whose conditions would put it back in SAFE reports the *condition's* reason rather than
	// `safe_sticky`, because "the estop is still pressed" is what an operator needs and "you have
	// not cleared it yet" is not.
	if previous == "safe" && !cleared(conditions) && state != "safe" {
		state, reason = "safe", "safe_sticky"
	}

	return Transition{
		From:    previous,
		To:      state,
		Reason:  reason,
		Changed: state != previous,
		Warning: WarningRequired(previous, state),
	}
}

// ── the corpus ──────────────────────────────────────────────────────────────

func opstateEvaluate(input canonical.Value) (canonical.Value, error) {
	previous, _ := input.Field("previous").Str()
	block := input.Field("conditions")

	conditions := Conditions{}
	conditions.Post, _ = block.Field("post").Str()
	conditions.Homing, _ = block.Field("homing").Str()
	conditions.Kernel, _ = block.Field("kernel").Str()
	conditions.RHAL, _ = block.Field("rhal").Str()
	// `stop?: StopCause | null` — the type admits null, so absent and null are the same fact here
	// and both mean "no stop is commanded". Reading the string is what distinguishes them from a
	// cause.
	conditions.Stop, conditions.HasStop = block.Field("stop").Str()
	conditions.StopSettled = isTrue(block.Field("stop_settled"))
	conditions.GuardHold = isTrue(block.Field("guard_hold"))
	conditions.GuardsUnavailable = isTrue(block.Field("guards_unavailable"))
	conditions.Policy, _ = block.Field("policy").Str()
	conditions.ClearedBy, _ = block.Field("cleared_by").Str()

	transition := Evaluate(previous, conditions)
	return canonical.Object().
		Set("from", canonical.String(transition.From)).
		Set("to", canonical.String(transition.To)).
		Set("reason", canonical.String(transition.Reason)).
		Set("changed", canonical.Bool(transition.Changed)).
		Set("warning", canonical.Bool(transition.Warning)), nil
}

// isTrue is the reference's `x === true`: a field that is absent, null, or any value other than the
// boolean true is false. Deliberately not truthiness — the reference compares to `true` at every one
// of these sites, so a `1` would be false here and truthy there.
func isTrue(value canonical.Value) bool {
	flag, ok := value.Bool()
	return ok && flag
}

func opstateProperties(canonical.Value) (canonical.Value, error) {
	out := canonical.Array()
	for _, state := range vocabulary.OperatingStates {
		out = out.Push(canonical.Object().
			Set("state", canonical.String(state)).
			Set("safe_state", canonical.Bool(IsSafeState(state))).
			Set("commands", canonical.Bool(CommandsAutonomously(state))).
			Set("sticky", canonical.Bool(IsSticky(state))))
	}
	return out, nil
}

func opstateWarning(canonical.Value) (canonical.Value, error) {
	out := canonical.Array()
	for _, from := range vocabulary.OperatingStates {
		for _, to := range vocabulary.OperatingStates {
			out = out.Push(canonical.Object().
				Set("from", canonical.String(from)).
				Set("to", canonical.String(to)).
				Set("warning", canonical.Bool(WarningRequired(from, to))))
		}
	}
	return out, nil
}
