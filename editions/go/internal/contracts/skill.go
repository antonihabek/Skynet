package contracts

import (
	"fmt"
	"math"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// Skills — preconditions, success, timeout, composition and the lifecycle (§54).
//
// The substantive decision in this file is an asymmetry. A precondition and a success criterion are
// the same shape, and the OS treats not knowing about them in opposite ways: **a precondition nobody
// can evaluate refuses the skill** (§54.2), and **a criterion nobody can evaluate makes the outcome
// `unlabeled`, not `failure`** (§54.3). Before a skill runs, ignorance is a reason not to act; after
// it runs, ignorance is a reason not to claim.

// Clause is one precondition or success criterion. The statement is never dropped — an operator
// staring at a refused skill needs the sentence its author wrote, not the path the OS evaluated.
type Clause struct {
	Statement    string
	Predicate    canonical.Value
	HasPredicate bool
	Hold         bool
	// Origin is `skill` or `operator`: the operator may add preconditions, the skill may not remove
	// them.
	Origin string
}

// ReadClauses reads a manifest's clauses in either spelling. A bare string is prose with no
// predicate — what every skill published before §54.2 carries — and reading one is not an error.
func ReadClauses(input canonical.Value) []Clause {
	clauses := []Clause{}
	for _, entry := range input.Items() {
		if text, isString := entry.Str(); isString {
			clauses = append(clauses, Clause{Statement: text})
			continue
		}
		// `!entry || typeof entry !== 'object'`: arrays are objects to that test and pass it.
		if entry.Kind() != canonical.KindObject && entry.Kind() != canonical.KindArray {
			continue
		}
		statement, _ := entry.Field("statement").Str()
		clause := Clause{Statement: statement, Hold: isTrue(entry.Field("hold"))}
		predicate := entry.Field("predicate")
		if truthyValue(predicate) && (predicate.Kind() == canonical.KindObject || predicate.Kind() == canonical.KindArray) {
			clause.Predicate, clause.HasPredicate = predicate, true
		}
		clauses = append(clauses, clause)
	}
	return clauses
}

// ResolveClauses is the skill's clauses, then the site's. There is no argument by which the
// operator's list can shorten the result, because it is only ever appended — and the publisher's
// come first, so a refusal names the skill's own requirement before the site's.
func ResolveClauses(published, operator []Clause) []Clause {
	out := make([]Clause, 0, len(published)+len(operator))
	for _, clause := range published {
		clause.Origin = "skill"
		out = append(out, clause)
	}
	for _, clause := range operator {
		clause.Origin = "operator"
		out = append(out, clause)
	}
	return out
}

// ClauseResult is what happened to one clause. `unenforceable` is the fourth outcome the predicate
// engine does not have: the case where there is no predicate to ask about.
type ClauseResult struct {
	Index     int
	Statement string
	Origin    string
	Hold      bool
	Outcome   PredicateOutcome
}

func judgeClause(clause Clause, index int, context maybe) ClauseResult {
	result := ClauseResult{Index: index, Statement: clause.Statement, Origin: clause.Origin, Hold: clause.Hold}
	if !clause.HasPredicate {
		result.Outcome = PredicateOutcome{Outcome: "unenforceable"}
		return result
	}
	result.Outcome = EvaluateOutcome(defined(clause.Predicate), context)
	return result
}

// PreconditionReport is §54.2's answer for one phase.
type PreconditionReport struct {
	Phase     string
	Permitted bool
	// Refused is the first clause that did not hold — what a refusal names.
	Refused    *ClauseResult
	Results    []ClauseResult
	Unenforced int
}

// EvaluatePreconditions is whether a skill may start (`entry`, every clause) or continue (`hold`,
// only the clauses marked so). Every clause is evaluated even after one has failed, because an
// operator fixing four problems one attempt at a time is the failure admission already refuses.
func EvaluatePreconditions(clauses []Clause, context maybe, phase string) PreconditionReport {
	report := PreconditionReport{Phase: phase, Results: []ClauseResult{}}
	for index, clause := range clauses {
		if phase == "hold" && !clause.Hold {
			continue
		}
		report.Results = append(report.Results, judgeClause(clause, index, context))
	}
	for at := range report.Results {
		result := report.Results[at]
		if result.Outcome.Outcome != "satisfied" && report.Refused == nil {
			report.Refused = &report.Results[at]
		}
		if result.Outcome.Outcome == "unenforceable" {
			report.Unenforced++
		}
	}
	report.Permitted = report.Refused == nil
	return report
}

// SuccessJudgement is §54.3's answer over post-state.
type SuccessJudgement struct {
	Outcome  string
	Deciding *ClauseResult
	Results  []ClauseResult
}

// JudgeSuccess is whether a skill succeeded, judged by the OS rather than by the skill. The order is
// the content: anything definitely false is a failure, otherwise anything unread is unlabeled, and
// only every criterion holding is a success. No criteria at all is unlabeled — a skill that scores
// itself by declaring nothing is the gaming §54.3 exists to stop.
func JudgeSuccess(clauses []Clause, postState maybe) SuccessJudgement {
	judgement := SuccessJudgement{Results: []ClauseResult{}}
	for index, clause := range clauses {
		judgement.Results = append(judgement.Results, judgeClause(clause, index, postState))
	}
	for at := range judgement.Results {
		if judgement.Results[at].Outcome.Outcome == "unsatisfied" {
			judgement.Outcome, judgement.Deciding = "failure", &judgement.Results[at]
			return judgement
		}
	}
	for at := range judgement.Results {
		if judgement.Results[at].Outcome.Outcome != "satisfied" {
			judgement.Outcome, judgement.Deciding = "unlabeled", &judgement.Results[at]
			return judgement
		}
	}
	if len(judgement.Results) == 0 {
		judgement.Outcome = "unlabeled"
		return judgement
	}
	judgement.Outcome = "success"
	return judgement
}

// ── §54.4 timeout ───────────────────────────────────────────────────────────

// DefaultOverrunFactor is how far past its own estimate a skill may run. An operator setting with a
// default, not a manifest field: a publisher choosing their own overrun is a publisher with no
// timeout.
const DefaultOverrunFactor = 2

// Deadline is §54.4's timeout for one skill.
type Deadline struct {
	DeadlineMS  float64
	HasDeadline bool
	Enforceable bool
	Factor      float64
}

// SkillDeadline is when a running skill is stopped and recorded `aborted`. A skill that declares no
// duration is reported unenforceable rather than given a guessed ceiling: refusing an unbounded skill
// and permitting one are both defensible, and a pure function is the wrong place to decide which.
func SkillDeadline(expected, factor canonical.Value) Deadline {
	chosen := float64(DefaultOverrunFactor)
	if value, isNumber := factor.Number(); isNumber && canonical.Finite(value) && value >= 1 {
		chosen = value
	}
	seconds, isNumber := expected.Number()
	if !isNumber || !canonical.Finite(seconds) || seconds <= 0 {
		return Deadline{Factor: chosen}
	}
	// `Math.round(expectedDurationSeconds * 1000 * factor)`: two products, left to right, each
	// rounded before the next — and the second rounded before Math.round's own subtraction, which
	// the compiler would otherwise fuse it into after inlining.
	return Deadline{DeadlineMS: js.Round(float64(float64(seconds*1000) * chosen)), HasDeadline: true, Enforceable: true, Factor: chosen}
}

// BoundedDeadline is §54.5's third limit: the child's own deadline, or what is left of the root's,
// whichever is sooner. A child declaring no duration is still bounded by its parent, and a root
// with nothing left bounds its child at zero rather than at a number already in the past.
func BoundedDeadline(child Deadline, rootRemaining maybeNumber) Deadline {
	if !rootRemaining.present || !canonical.Finite(rootRemaining.value) {
		return child
	}
	remaining := math.Max(0, rootRemaining.value)
	if !child.Enforceable || !child.HasDeadline {
		return Deadline{DeadlineMS: remaining, HasDeadline: true, Enforceable: true, Factor: child.Factor}
	}
	return Deadline{DeadlineMS: math.Min(child.DeadlineMS, remaining), HasDeadline: true, Enforceable: true, Factor: child.Factor}
}

// ── §54.5 composition ───────────────────────────────────────────────────────

// maxSkillDepth is §54.5's default: the root counts as depth 1, so 4 permits three levels beneath.
const maxSkillDepth = 4

// SkillCall is one declared skill-to-skill call.
type SkillCall struct {
	From string
	To   string
}

// ReadCalls reads declared calls, skipping anything that is not one — at this boundary, so every
// edition discards the same malformed edges in the same place.
func ReadCalls(input canonical.Value) []SkillCall {
	calls := []SkillCall{}
	for _, entry := range input.Items() {
		from, fromIsString := entry.Field("from").Str()
		to, toIsString := entry.Field("to").Str()
		if entry.Kind() != canonical.KindObject || !fromIsString || !toIsString {
			continue
		}
		calls = append(calls, SkillCall{From: from, To: to})
	}
	return calls
}

// CompositionFault is why a call graph cannot be installed, with the chain that reached it.
type CompositionFault struct {
	Code   string
	Path   []string
	Detail string
}

// ValidateComposition is whether a call graph is installable: acyclic and depth-limited, refused at
// install by walking the declared graph rather than discovered by a robot that has already taken a
// lease. Depth-first in declaration order, so the chain a fault names is the same on every edition.
func ValidateComposition(root string, calls []SkillCall, maxDepth canonical.Value) (CompositionFault, bool) {
	limit := float64(maxSkillDepth)
	if value, isNumber := maxDepth.Number(); isNumber && canonical.Finite(value) && value >= 1 {
		limit = math.Floor(value)
	}

	// Declaration order, kept: a map of slices, with the keys' order irrelevant because the walk only
	// ever looks a node's edges up.
	edges := map[string][]string{}
	for _, call := range calls {
		edges[call.From] = append(edges[call.From], call.To)
	}

	var fault *CompositionFault
	var walk func(node string, path []string)
	walk = func(node string, path []string) {
		if fault != nil {
			return
		}
		if float64(len(path)) > limit {
			fault = &CompositionFault{
				Code: "too_deep", Path: append([]string{}, path...),
				Detail: "This call chain is " + js.NumberToString(float64(len(path))) + " skills deep; the limit is " +
					js.NumberToString(limit) + ".",
			}
			return
		}
		for _, next := range edges[node] {
			if fault != nil {
				return
			}
			if containsString(path, next) {
				fault = &CompositionFault{
					Code: "cycle", Path: append(append([]string{}, path...), next),
					Detail: next + " is already in this call chain, so it would call itself.",
				}
				return
			}
			walk(next, append(append([]string{}, path...), next))
		}
	}
	walk(root, []string{root})
	if fault == nil {
		return CompositionFault{}, true
	}
	return *fault, false
}

// ── §54.4 the lifecycle ─────────────────────────────────────────────────────

// SkillTransition is the lifecycle advanced by one evaluation.
type SkillTransition struct {
	From    canonical.Value
	To      string
	Reason  string
	Outcome canonical.Value
	Detail  string
	// ReleaseLeases is true only on the way into `terminal`, on every path there — a release emitted
	// every cycle after a skill ended would race the next invocation's acquisition.
	ReleaseLeases bool
	Changed       bool
}

// robotStoppedCommanding is whether §27's machine has stopped a running skill. `hold` is not one:
// it is where the machine sits between a lease being granted and a policy commanding, and aborting
// on it would kill every skill in the moment it started.
func robotStoppedCommanding(state string) bool {
	return state == "stopping" || state == "safe" || state == "post" || state == "unhomed"
}

// overran is whether the deadline has passed. An unenforceable deadline never has.
func overran(progress canonical.Value) bool {
	deadline := progress.Field("deadline")
	if !truthyValue(deadline) || !truthyValue(deadline.Field("enforceable")) {
		return false
	}
	limit, isNumber := deadline.Field("deadline_ms").Number()
	if !isNumber {
		// Null is "no deadline", and an absent one compares false against anything.
		return false
	}
	elapsed, isNumber := progress.Field("elapsed_ms").Number()
	if !isNumber || !canonical.Finite(elapsed) {
		return false
	}
	return elapsed >= limit
}

// abortReason is why a running skill is over. Causes before effects: a timeout and a guard trip both
// produce a robot that has stopped commanding, so the specific reasons are tried first and the robot's
// state is the fallback that catches whatever this machine could not name.
func abortReason(progress canonical.Value) (string, string, bool) {
	if overran(progress) {
		deadline := progress.Field("deadline")
		limit, hasLimit := deadline.Get("deadline_ms")
		factor, hasFactor := deadline.Get("factor")
		return "timeout", "past " + js.ToString(limit, hasLimit) + " ms, which is the declared estimate times " +
			js.ToString(factor, hasFactor), true
	}
	if isTrue(progress.Field("hold_lost")) {
		return "hold_lost", "a precondition marked `hold` stopped being true, and §54.2 re-checks those every " +
			"cycle rather than only at entry", true
	}
	if isTrue(progress.Field("guard_tripped")) {
		return "guard_tripped", "a guard tripped, and a skill is not an exemption", true
	}
	robot := "post"
	if value, present := presentNonNull(progress, "robot"); present {
		robot = js.ToString(value, true)
	}
	if robotStoppedCommanding(robot) {
		return "robot_stopped", "§27 says the robot is `" + robot + "`, which is not a state a policy commands in", true
	}
	return "", "", false
}

// leaseDeniedDetail is what a denied acquisition tells the operator, which is two different errands:
// a holder named is somebody else using it, and no holder is a grant nobody gave.
func leaseDeniedDetail(progress canonical.Value) string {
	token, _ := progress.Field("lease_token").Str()
	if !js.Truthy(token) {
		return "a required capability could not be leased, and nothing named which"
	}
	holder, _ := progress.Field("lease_holder").Str()
	if !js.Truthy(holder) {
		return "`" + token + "` was never granted to this package, so §25.2 will not let " +
			"it take the lease — nobody is holding it. Grant it, or take it out of " +
			"`required_capabilities`."
	}
	return "`" + token + "` is held by " + holder
}

// Advance is §54.4's lifecycle, advanced by one evaluation: pure, clockless and total over the six
// phases. A phase advances on evidence, and absence of evidence is not advancement — so calling it
// every cycle while the work is still running returns the phase it was given.
func Advance(phase canonical.Value, progress canonical.Value) (SkillTransition, error) {
	from, _ := phase.Str()
	if !vocabulary.Contains(vocabulary.SkillPhases, from) {
		// The reference's switch has no default and returns nothing for a phase outside the six; an
		// edition answering one would be inventing a transition.
		return SkillTransition{}, fmt.Errorf("%q is not one of the skill phases %v", from, vocabulary.SkillPhases)
	}
	done := func(to, reason string, outcome canonical.Value, detail string) (SkillTransition, error) {
		return SkillTransition{
			From: phase, To: to, Reason: reason, Outcome: outcome, Detail: detail,
			ReleaseLeases: to == "terminal" && from != "terminal",
			Changed:       to != from,
		}, nil
	}
	none := canonical.Null()
	aborted := canonical.String("aborted")

	switch from {
	case "idle":
		if isTrue(progress.Field("invoked")) {
			return done("preconditions", "invoked", none, "an invocation arrived")
		}
		return done("idle", "not_invoked", none, "nothing has asked for this skill")

	case "preconditions":
		// Absent is not permitted: a runtime that had not evaluated §54.2 must not be able to start a
		// skill by failing to ask.
		permitted, isBool := progress.Field("entry_permitted").Bool()
		if !isBool {
			return done("preconditions", "preconditions_pending", none,
				"the entry predicates have not been evaluated, and an unevaluated precondition is not a "+
					"satisfied one")
		}
		if !permitted {
			detail := "refused by a precondition"
			if clause := progress.Field("refused_clause"); truthyValue(clause) {
				detail = "refused by: " + js.ToString(clause, true)
			}
			return done("terminal", "preconditions_refused", aborted, detail)
		}
		// §54.5: a nested invocation does not acquire; the root holds the leases for the whole tree.
		if root, isBool := progress.Field("root").Bool(); isBool && !root {
			return done("running", "leases_held_by_root", none, "a nested call runs under the root invocation's leases (§54.5)")
		}
		return done("acquiring", "preconditions_met", none, "every evaluated precondition held")

	case "acquiring":
		// §36.1's `low` row before the table: a refusal for want of energy is a fact about the robot,
		// and a blank sentence is not a refusal (§64).
		withheld, _ := progress.Field("power_withheld").Str()
		if withheld = js.Trim(withheld); withheld != "" {
			return done("terminal", "power_withheld", aborted, withheld)
		}
		leases := "pending"
		if value, present := presentNonNull(progress, "leases"); present {
			leases = js.ToString(value, true)
		}
		if leases == "denied" {
			return done("terminal", "lease_denied", aborted, leaseDeniedDetail(progress))
		}
		if leases != "held" {
			return done("acquiring", "lease_pending", none, "the leases for `required_capabilities` have not been taken")
		}
		return done("running", "leases_acquired", none, "every required capability is leased")

	case "running":
		if reason, detail, over := abortReason(progress); over {
			return done("terminal", reason, aborted, detail)
		}
		if isTrue(progress.Field("finished")) {
			return done("verifying", "verifying", none, "the skill returned; §54.3 has not judged it yet")
		}
		return done("running", "running", none, "the skill is running")

	case "verifying":
		// The abort outranks the verdict: a skill whose robot fell over during verification did not
		// succeed at anything.
		if reason, detail, over := abortReason(progress); over {
			return done("terminal", reason, aborted, detail)
		}
		outcome, present := presentNonNull(progress, "outcome")
		if !present {
			return done("verifying", "verdict_pending", none,
				"the success criteria have not been judged, and §54.3 will not let the skill judge itself")
		}
		return done("terminal", "judged", outcome, "§54.3 judged this "+js.ToString(outcome, true))
	}

	// terminal
	outcome, present := presentNonNull(progress, "outcome")
	if !present {
		outcome = none
	}
	return done("terminal", "terminal", outcome, "this invocation is over")
}

// ── the wire ────────────────────────────────────────────────────────────────

func shapeClauseResult(result ClauseResult) canonical.Value {
	return canonical.Object().
		Set("index", canonical.Number(float64(result.Index))).
		Set("statement", canonical.String(result.Statement)).
		Set("origin", canonical.String(result.Origin)).
		Set("hold", canonical.Bool(result.Hold)).
		Set("outcome", canonical.String(result.Outcome.Outcome)).
		Set("reason", optionalString(result.Outcome.Reason, result.Outcome.HasReason)).
		Set("path", optionalString(result.Outcome.Path, result.Outcome.HasPath)).
		Set("op", optionalString(result.Outcome.Op, result.Outcome.HasOp))
}

func shapeClauseResults(results []ClauseResult) canonical.Value {
	out := canonical.Array()
	for _, result := range results {
		out = out.Push(shapeClauseResult(result))
	}
	return out
}

func shapeOptionalClause(result *ClauseResult) canonical.Value {
	if result == nil {
		return canonical.Null()
	}
	return shapeClauseResult(*result)
}

func shapeDeadline(deadline Deadline) canonical.Value {
	return canonical.Object().
		Set("deadline_ms", optionalNumber(deadline.DeadlineMS, deadline.HasDeadline)).
		Set("enforceable", canonical.Bool(deadline.Enforceable)).
		Set("factor", canonical.Number(deadline.Factor))
}

// ── the corpus ──────────────────────────────────────────────────────────────

func skillPreconditions(input canonical.Value) (canonical.Value, error) {
	clauses := ResolveClauses(ReadClauses(input.Field("published")), ReadClauses(input.Field("operator")))
	phase := "entry"
	if value, present := presentNonNull(input, "phase"); present {
		phase = js.ToString(value, true)
	}
	report := EvaluatePreconditions(clauses, field(input, "context"), phase)
	return canonical.Object().
		Set("phase", canonical.String(report.Phase)).
		Set("permitted", canonical.Bool(report.Permitted)).
		Set("refused", shapeOptionalClause(report.Refused)).
		Set("results", shapeClauseResults(report.Results)).
		Set("unenforceable", canonical.Number(float64(report.Unenforced))), nil
}

func skillSuccess(input canonical.Value) (canonical.Value, error) {
	judgement := JudgeSuccess(ResolveClauses(ReadClauses(input.Field("criteria")), nil), field(input, "post_state"))
	return canonical.Object().
		Set("outcome", canonical.String(judgement.Outcome)).
		Set("deciding", shapeOptionalClause(judgement.Deciding)).
		Set("results", shapeClauseResults(judgement.Results)), nil
}

// skillTimeout reads `overrun_factor: null` as the generator's spelling of "no factor given", which
// is the default — and SkillDeadline reaches the default for a null by its own test anyway.
func skillTimeout(input canonical.Value) (canonical.Value, error) {
	return shapeDeadline(SkillDeadline(input.Field("expected_duration_seconds"), input.Field("overrun_factor"))), nil
}

func skillBoundedDeadline(input canonical.Value) (canonical.Value, error) {
	child := SkillDeadline(input.Field("expected_duration_seconds"), canonical.Number(DefaultOverrunFactor))
	remaining := maybeNumber{}
	if value, isNumber := input.Field("root_remaining_ms").Number(); isNumber {
		remaining = maybeNumber{value: value, present: true}
	}
	return shapeDeadline(BoundedDeadline(child, remaining)), nil
}

func skillComposition(input canonical.Value) (canonical.Value, error) {
	root, _ := input.Field("root").Str()
	fault, installable := ValidateComposition(root, ReadCalls(input.Field("calls")), input.Field("max_depth"))
	if installable {
		return canonical.Object().Set("fault", canonical.Null()), nil
	}
	return canonical.Object().Set("fault", canonical.Object().
		Set("code", canonical.String(fault.Code)).
		Set("path", canonical.Strings(fault.Path)).
		Set("detail", canonical.String(fault.Detail))), nil
}

func skillLifecycle(input canonical.Value) (canonical.Value, error) {
	transition, err := Advance(input.Field("phase"), input.Field("progress"))
	if err != nil {
		return canonical.Null(), err
	}
	return canonical.Object().
		Set("from", transition.From).
		Set("to", canonical.String(transition.To)).
		Set("reason", canonical.String(transition.Reason)).
		Set("outcome", transition.Outcome).
		Set("detail", canonical.String(transition.Detail)).
		Set("release_leases", canonical.Bool(transition.ReleaseLeases)).
		Set("changed", canonical.Bool(transition.Changed)), nil
}
