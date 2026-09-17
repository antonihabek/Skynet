package contracts

import (
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// §88 — what admits a motion request on a program-level body.
//
// §54.1 ended with two sentences that stood from the moment Part VI was written: *"`execute({ skill,
// arguments, timeout_s })` already existed on the Hub side as the invocation point. Nothing routed
// it."* For a joint-commanded body that stopped being true once §54.4's lifecycle was driven; for
// §59.2's machines it was still true, and §84 removed the first of Phase 4's two blockers.
//
// # The second blocker dissolved rather than being bridged
//
// §33.1 keeps `skynetd` off the path that holds a driver, so a routed skill's destination is
// `skynetk`'s and there was no channel from the supervisor to it. Both obvious remedies spend
// §33.1's licence. §88.1's answer is that the route already exists: a skill over a program-level
// body is a policy process emitting motion requests into the ring it would post joint commands
// into, and what was missing was not a channel but the **admission rule** for a request on one.
//
// # It is a gate, and calling it a guard would be a claim
//
// §7's guards clamp a *value* against a resolved ceiling. There is no ceiling for `move_l`. What is
// checkable is **authority, state, and the body's own declared table** — the last being the half
// §84.1 created and nothing consumed.

// MotionVerdict is what AdmitMotion concluded.
type MotionVerdict struct {
	Accepted bool
	Refusal  string
	Detail   string
	// Arguments is the list the driver is handed, in the motion's **declaration order** (§88.2).
	Arguments []MotionArgument
	// Unbounded are parameters whose value nothing checked, because no bound is declared (§5.2).
	Unbounded []string
}

// MotionArgument is one argument of an accepted request.
type MotionArgument struct {
	Name  string
	Value float64
}

func refuseMotion(refusal, detail string) MotionVerdict {
	return MotionVerdict{
		Refusal:   refusal,
		Detail:    detail,
		Arguments: []MotionArgument{},
		Unbounded: []string{},
	}
}

// AdmitMotion is whether this request may be handed to the driver, and with what.
//
// Authority before state, state before content. A request from a holder that does not hold the body
// is refused **without the platform reading what it asked for**, because reading it would mean
// answering a question nobody was entitled to ask.
//
// **`parameter_unknown` is a refusal here and a warning at plan time, and the asymmetry is the
// point.** §84.3 raises `motion_parameter_unused` on a transport plan and does not refuse, because
// the controller does take the motion. At command time the same fact is a value about to be handed
// to a controller that will either interpret it or reject it on a moving robot, and §25.2's argument
// applies unchanged.
func AdmitMotion(gate, request canonical.Value) MotionVerdict {
	held, hasHeld := gate.Field("lease_held_by").Str()
	holder, hasHolder := request.Field("holder").Str()

	if !hasHeld {
		return refuseMotion("no_lease",
			"Nothing holds this body, so there is nobody whose motion this would be (§25.2).")
	}
	if !hasHolder || held != holder {
		named := "nobody"
		if hasHolder {
			named = holder
		}
		return refuseMotion("no_lease",
			"This body is held by `"+held+"` and the request came from `"+named+"` (§25.2).")
	}

	if estop, isBool := gate.Field("estop").Bool(); isBool && estop {
		return refuseMotion("estop",
			"The emergency-stop chain is open (§24). What opened it is not something the body can "+
				"say, so the only correct response is to command nothing.")
	}

	state, _ := gate.Field("state").Str()
	if !containsString(vocabulary.MotionReadyStates, state) {
		return refuseMotion("not_active",
			"§27 has this body in `"+state+"`, which is not a state in which it will run a "+
				"program. Accepting the request would be queueing work behind whatever the body "+
				"is doing instead — a controlled stop, a self-test, or waiting to be homed.")
	}

	if executing, isString := gate.Field("executing").Str(); isString && executing != "" {
		return refuseMotion("busy",
			"The controller is executing `"+executing+"`. A program-level controller owns the "+
				"servo loop (§59.2), so it finishes or it is aborted; there is no second motion "+
				"to interleave with the first.")
	}

	name, _ := request.Field("motion").Str()
	var motion canonical.Value
	found := false
	for _, candidate := range gate.Field("motions").Items() {
		if candidateName, isString := candidate.Field("name").Str(); isString && candidateName == name {
			motion, found = candidate, true
			break
		}
	}
	if !found {
		return refuseMotion("unknown_motion",
			"This body declares no motion called `"+name+"`. The name a request carries is the "+
				"**body's** spelling, which §84.3's plan resolved at install — a policy's own name "+
				"reaching here unresolved is the defect this refusal usually means.")
	}
	motionName, _ := motion.Field("name").Str()

	deadline, hasDeadline := finiteNumber(request.Field("deadline_ms"))
	if !hasDeadline || deadline <= 0 {
		return refuseMotion("deadline_undeclared",
			"A motion takes seconds, during which §7.1's watchdog guard has nothing to watch: a "+
				"correct policy produces no command at all. The deadline is what replaces it "+
				"(§88.3), so a request without one is a program nothing would ever conclude had "+
				"failed.")
	}

	// First wins, as §84.1's reader does with a repeated name: `duplicate_parameter` is the
	// descriptor's fault. A gate that preferred the later one would command against a bound the
	// check never looked at.
	declared := []canonical.Value{}
	declaredNames := []string{}
	for _, parameter := range motion.Field("parameters").Items() {
		parameterName, isString := parameter.Field("name").Str()
		if !isString || containsString(declaredNames, parameterName) {
			continue
		}
		declared = append(declared, parameter)
		declaredNames = append(declaredNames, parameterName)
	}

	parameters := request.Field("parameters")

	// The request's own names first: a parameter the motion has no slot for means the policy and
	// the body disagree about the motion's *shape*, which subsumes arguing about its values.
	supplied := []string{}
	for _, member := range parameters.Members() {
		supplied = append(supplied, member.Key)
	}
	sort.SliceStable(supplied, func(left, right int) bool {
		return utf16Less(supplied[left], supplied[right])
	})
	for _, name := range supplied {
		if containsString(declaredNames, name) {
			continue
		}
		return refuseMotion("parameter_unknown",
			"`"+motionName+"` does not take `"+name+"`. §84.3 warned about this at install — "+
				"`motion_parameter_unused` — and declined to refuse a plan over it, because the "+
				"controller does take the motion. Here it is a value about to be sent to a "+
				"controller that has no slot for it, and §25.2 is clear that commanding with the "+
				"parts of a command a machine happened to understand is worse than refusing.")
	}

	arguments := []MotionArgument{}
	unbounded := []string{}

	for _, parameter := range declared {
		parameterName, _ := parameter.Field("name").Str()
		// `Has` rather than a null test, and it is the reference's own question: a parameter
		// supplied as `null` is *supplied*, and is refused as not finite rather than treated as
		// omitted — the difference between a policy that said nothing and one that said nothing
		// useful.
		if !parameters.Has(parameterName) {
			// §84.1's fail-closed reading: anything but an explicit `false` is required.
			if required, isBool := parameter.Field("required").Bool(); !isBool || required {
				return refuseMotion("parameter_missing",
					"`"+motionName+"` requires `"+parameterName+"` and the request does not carry "+
						"it. §84.1 reads a requirement fail-closed, so a parameter whose "+
						"`required` the descriptor never declared is required here too.")
			}
			// Absent rather than zero. §7.2's rule, and the controller owns its own defaults.
			continue
		}

		number, isFinite := finiteNumber(parameters.Field(parameterName))
		if !isFinite {
			return refuseMotion("parameter_not_finite",
				"`"+parameterName+"` is not a finite number. A controller handed one would either "+
					"fault or move somewhere nobody chose.")
		}

		minimum, hasMin := finiteNumber(parameter.Field("min"))
		maximum, hasMax := finiteNumber(parameter.Field("max"))
		if hasMin && number < minimum {
			return refuseMotion("parameter_out_of_bounds",
				"`"+parameterName+"` is "+js.NumberToString(number)+" and `"+motionName+
					"` declares a minimum of "+js.NumberToString(minimum)+".")
		}
		if hasMax && number > maximum {
			return refuseMotion("parameter_out_of_bounds",
				"`"+parameterName+"` is "+js.NumberToString(number)+" and `"+motionName+
					"` declares a maximum of "+js.NumberToString(maximum)+".")
		}
		if !hasMin && !hasMax {
			unbounded = append(unbounded, parameterName)
		}

		arguments = append(arguments, MotionArgument{Name: parameterName, Value: number})
	}

	detail := "`" + motionName + "` accepted, with every supplied parameter inside a bound this " +
		"body declared."
	if len(unbounded) > 0 {
		counted := "Some parameters were"
		if len(unbounded) == 1 {
			counted = "One parameter was"
		}
		detail = "`" + motionName + "` accepted. " + counted + " not checked against anything, " +
			"because this body declares no bound for " + strings.Join(unbounded, ", ") +
			" — which §5.2 reads as *nobody declared one*, never as unbounded."
	}

	return MotionVerdict{
		Accepted:  true,
		Detail:    detail,
		Arguments: arguments,
		Unbounded: unbounded,
	}
}

// MotionProgressReport is what one cycle concluded about a motion already in flight.
type MotionProgressReport struct {
	Progress    string
	OverdueByMs float64
	Detail      string
}

// MotionProgressOf is whether a motion in flight has run past its deadline (§88.3).
//
// Separate from AdmitMotion because one is asked **once per request** and this is asked **every
// cycle** until the controller answers.
func MotionProgressOf(deadlineMs, startedAtMs, nowMs float64) MotionProgressReport {
	elapsed := nowMs - startedAtMs
	over := elapsed - deadlineMs
	// `>` rather than `>=`, as §87.4's lease is: the instant the deadline falls is still inside it.
	if over > 0 {
		return MotionProgressReport{
			Progress:    "overdue",
			OverdueByMs: over,
			Detail: "The controller has held this motion for " + js.NumberToString(elapsed) +
				" ms against a " + js.NumberToString(deadlineMs) + " ms deadline. §7.1's watchdog " +
				"has nothing to watch on this path (§88.3), so this is the bound that fires: " +
				"abort the motion, enter §27's fault path, and record it.",
		}
	}
	return MotionProgressReport{
		Progress:    "running",
		OverdueByMs: over,
		Detail:      "Inside its deadline, with " + js.NumberToString(-over) + " ms left.",
	}
}

func executeMotion(input canonical.Value) (canonical.Value, error) {
	verdict := AdmitMotion(input.Field("gate"), input.Field("request"))
	arguments := canonical.Array()
	for _, entry := range verdict.Arguments {
		arguments = arguments.Push(canonical.Object().
			Set("name", canonical.String(entry.Name)).
			Set("value", canonical.Number(entry.Value)))
	}
	return canonical.Object().
		Set("accepted", canonical.Bool(verdict.Accepted)).
		Set("refusal", optionalString(verdict.Refusal, verdict.Refusal != "")).
		Set("detail", canonical.String(verdict.Detail)).
		Set("arguments", arguments).
		Set("unbounded", canonical.Strings(verdict.Unbounded)), nil
}

func executeProgress(input canonical.Value) (canonical.Value, error) {
	deadline, _ := finiteNumber(input.Field("deadline_ms"))
	started, _ := finiteNumber(input.Field("started_at_ms"))
	now, _ := finiteNumber(input.Field("now_ms"))
	report := MotionProgressOf(deadline, started, now)
	return canonical.Object().
		Set("progress", canonical.String(report.Progress)).
		Set("overdue_by_ms", canonical.Number(report.OverdueByMs)).
		Set("detail", canonical.String(report.Detail)), nil
}
