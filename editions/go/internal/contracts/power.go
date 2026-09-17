package contracts

import (
	"math"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// Power and thermal (§36).
//
// Every absence produces a named unavailability rather than an assumption: `brownout_fault_codes`
// empty means *this vendor has not said which of its codes mean sag*, not *this body cannot brown
// out*.

// ReserveBand is where the `reserve` band ends, as a multiple of the reserve.
//
// A quarter of a reserve wide. §36.1 puts `reserve` "at the reserve budget" and `critical` "below
// reserve", which as written are a single point and everything under it — and a gauge reporting
// within a percent of the boundary would then chatter between *begin a controlled stop* and *stop
// immediately*, on a robot that is mid-ramp.
const ReserveBand = 1.25

// LowBand is where the `low` band ends, as a multiple of the reserve.
//
// **One whole reserve above the reserve band**, and that width is the argument: `low`'s only
// behaviour is that a running task may finish and no new one may start, so the warning has to
// arrive with at least one further safe stop's worth of energy in hand.
const LowBand = 2.25

// SustainedThrottleMS is how long above the throttle threshold counts as "sustained" (§36.3).
//
// Thirty seconds. §36.3 raises DEGRADED on *sustained* throttling and does not say how long, and
// both mistakes are real: one cycle makes every heavy motion a degraded robot, and ten minutes
// makes DEGRADED a state a thermally limited robot never reaches.
const SustainedThrottleMS = 30000

// ThrottleDepth is how many rungs are given up while throttling but not critical.
//
// The first three, and the line between three and four is the meaningful one: telemetry, OTA and
// black-box compression are housekeeping, and giving them up costs the robot nothing it was hired
// to do. Rungs four and five change what it can *do*.
const ThrottleDepth = 3

// PowerFault is a fault in the power subsystem itself, rather than a level of charge.
//
// Separate from the state because they answer different questions and an operator needs both: the
// state says how much energy is left, and a fault says whether that number can be believed.
type PowerFault struct {
	Code   string
	Detail string
}

// FaultDegrades reports whether a fault raises §27's DEGRADED.
//
// The two thermal ones do, because §36.3 says so in as many words. The declaration gaps do not: a
// descriptor missing a field is a descriptor to fix, and a robot that reported itself degraded for
// it would be reporting a paperwork problem in the same column as a hot motor.
func FaultDegrades(code string) bool {
	return code == "thermal_sustained" || code == "thermal_critical"
}

// FaultWithholdsLeases reports whether a fault withholds new task leases.
//
// A robot that cannot tell whether it is at reserve should not be handed *new* work, and should not
// have taken from it the work it is already doing — which is exactly §36.1's `low` behaviour,
// arrived at from the other direction.
func FaultWithholdsLeases(code string) bool {
	return code == "gauge_silent" || code == "capacity_undeclared" ||
		code == "reserve_undeclared" || code == "charge_out_of_range" || code == "thermal_critical"
}

// StateGrantsNewLeases reports whether §25.2 may grant a new task lease in this state.
//
// `unmetered` grants, and that is the whole reason the state exists: a bench arm on mains must not
// be refused work because it has no battery to report on. A *gauge fault* withholds leases, and
// GrantsNewLeases is where the two are combined.
func StateGrantsNewLeases(state string) bool { return state == "normal" || state == "unmetered" }

// StateRequiresSafeState reports whether §36.1 requires the robot to be taken to a safe state.
func StateRequiresSafeState(state string) bool { return state == "reserve" || state == "critical" }

// StateStopsImmediately reports whether the stop is immediate rather than after the running task.
func StateStopsImmediately(state string) bool { return state == "critical" }

// StateNotifiesOperator reports whether an operator is told (§36.1's low row, and below it).
//
// §36.1 derives three facts from a power state, and this one reached no document in any edition
// until 2026-09-16 — the reference included, which is why the corpus never caught it. A robot that
// has stopped taking work and a robot that has stopped taking work and owes somebody a phone call
// are different situations, and only the first was reportable.
func StateNotifiesOperator(state string) bool {
	return state == "low" || state == "reserve" || state == "critical"
}

// PowerReserve is §36.2's budget, and what it could not account for.
type PowerReserve struct {
	// WH is the reserve, or absent when the body declares nothing to compute one from.
	WH    float64
	HasWH bool
	// Unaccounted are §36.2's terms nothing on this robot supplies. Non-empty means WH is a floor.
	Unaccounted []string
	// SafeStateBasis is the basis `distance_to_safe_state` was accounted on, or empty (§87.2).
	//
	// The difference between a term **nobody supplied** and a term **bounded** over an area
	// somebody walked, and a reader who cannot tell the two apart is reading a floor as a
	// measurement.
	SafeStateBasis string
}

// ReserveComplete reports whether every term §36.2 names was accounted for. False on every robot
// today.
func ReserveComplete(reserve PowerReserve) bool {
	return reserve.HasWH && len(reserve.Unaccounted) == 0
}

// ComputeReserve computes §36.2's reserve budget from what the body declares.
//
// The arithmetic is deliberately the identity: the reserve *is* `safe_stop_energy_wh`, because the
// other two terms §36.2 names have no source. Scaling it by a plausible payload factor would have
// been one line and a number nobody could defend — and this is the number that decides when a
// two-metre humanoid stops walking.
func ComputeReserve(profile canonical.Value, base string, hasBase bool) PowerReserve {
	reserve := PowerReserve{Unaccounted: []string{}}
	declared, has := finiteNumber(profile.Field("safe_stop_energy_wh"))
	if has && declared > 0 {
		reserve.WH, reserve.HasWH = declared, true
	} else {
		reserve.Unaccounted = append(reserve.Unaccounted, vocabulary.ReserveTerms[0])
	}
	// `payload` still has no source on this platform. Appended unconditionally rather than looked
	// up, so the day one arrives this is a visible place to change rather than a silent
	// over-report.
	reserve.Unaccounted = append(reserve.Unaccounted, vocabulary.ReserveTerms[1])

	// §87.2. The third term leaves Unaccounted when the declaration is sound — which is not the
	// same as the figure being measured, and SafeStateBasis is what keeps the two distinguishable.
	// `hasBase` false is *nobody said what this body stands on*, which gets the reading this
	// function had before §87: "nobody told me" and "it is bolted down" are the two things §7.2
	// exists to keep apart.
	if hasBase {
		reserve.SafeStateBasis = SafeStateBasisOf(profile, base)
	}
	if reserve.SafeStateBasis == "" {
		reserve.Unaccounted = append(reserve.Unaccounted, vocabulary.ReserveTerms[2])
	}
	return reserve
}

// ── §87.2, the safe state's provenance ──────────────────────────────────────

// SafeStateFault is one thing wrong with a safe-state declaration (§87.2).
type SafeStateFault struct {
	Code   string
	Detail string
}

// BaseIsMobile reports whether this base kind has a navigation problem between it and a safe state.
func BaseIsMobile(base string, hasBase bool) bool {
	return hasBase && containsString(vocabulary.MobileBases, base)
}

// ValidateSafeState is §87.2's checks over the declaration, and not ok when there is none.
//
// The first fault in declaration order, like ValidateMotions and for the same reason: the basis is
// read before the area, and the area before anything about the figure, because a declaration whose
// basis is wrong is one whose other fields are being read against the wrong rule.
//
// The first rule is the one that keeps every descriptor written before §87 valid: **an absent basis
// on a `fixed` base is `fixed`.** A robot bolted to a bench should not have to declare what it is
// not, which is §35.3's argument about `simulator` applied to the same shape.
func ValidateSafeState(profile canonical.Value, base string, hasBase bool) (SafeStateFault, bool) {
	figure, hasFigure := finiteNumber(profile.Field("time_to_safe_state_s"))
	hasFigure = hasFigure && figure > 0

	declaration := profile.Field("safe_state")
	if declaration.Kind() != canonical.KindObject {
		if !BaseIsMobile(base, hasBase) {
			return SafeStateFault{}, false
		}
		return SafeStateFault{
			Code: "basis_undeclared",
			Detail: "A figure with no basis is a rumour (§5.2), and on a body that travels the " +
				"time to a safe state depends on where it is standing. Declare `fixed` if it " +
				"stops where it is, or `mapped_worst_case` with the area the worst case was taken " +
				"over.",
		}, true
	}

	basis, _ := declaration.Field("basis").Str()
	if basis == "live" {
		return SafeStateFault{
			Code: "basis_refused",
			Detail: "A figure the navigation stack supplies per cycle is the design §87.2 " +
				"rejected: it puts a planner inside §36's reserve arithmetic, so a skill admitted " +
				"at one instant is refused at the next for a reason nobody can audit, and it " +
				"makes navigation a safety-relevant component with no tier. Declare a worst case " +
				"over a mapped area instead.",
		}, true
	}

	if !containsString(vocabulary.SafeStateBases, basis) {
		return SafeStateFault{
			Code: "basis_unknown",
			Detail: "`" + basis + "` is not one of fixed or mapped_worst_case, so nothing can " +
				"read how this figure was arrived at.",
		}, true
	}

	if basis == "mapped_worst_case" {
		if !BaseIsMobile(base, hasBase) {
			return SafeStateFault{
				Code: "mapped_on_a_fixed_base",
				Detail: "This base does not travel, so there is no mapped area to take a worst " +
					"case over. It is a claim about a navigation problem this body does not have, " +
					"and it arrives by copying a descriptor between machines.",
			}, true
		}
		area, _ := declaration.Field("area").Str()
		if js.Trim(area) == "" {
			return SafeStateFault{
				Code: "area_undeclared",
				Detail: "A worst case over nothing is not a worst case. Name the area it was " +
					"taken over, so somebody reading the reserve later knows where the figure " +
					"stops being true.",
			}, true
		}
	}

	if !hasFigure {
		return SafeStateFault{
			Code: "figure_undeclared",
			Detail: "A basis with no `time_to_safe_state_s` beside it describes how a number was " +
				"arrived at without carrying the number.",
		}, true
	}

	return SafeStateFault{}, false
}

// SafeStateBasisOf is the basis the third reserve term may be accounted on, or empty.
//
// `fixed` on a fixed base is the reading a bolted-down arm's silence always had, spelled out; every
// other combination has to be declared and has to pass ValidateSafeState, because accounting a term
// against a declaration with a fault in it would be the invented measurement §5.2 forbids.
func SafeStateBasisOf(profile canonical.Value, base string) string {
	hasBase := base != ""
	if _, faulty := ValidateSafeState(profile, base, hasBase); faulty {
		return ""
	}
	figure, hasFigure := finiteNumber(profile.Field("time_to_safe_state_s"))
	if !hasFigure || figure <= 0 {
		return ""
	}
	declared, _ := profile.Field("safe_state").Field("basis").Str()
	if declared == "mapped_worst_case" {
		return "mapped_worst_case"
	}
	if declared == "fixed" {
		return "fixed"
	}
	if BaseIsMobile(base, hasBase) {
		return ""
	}
	return "fixed"
}

// HostBlockedBySafeState is whether this body may attempt the host tier, given what it says about
// its safe state (§87.2). Empty when it may.
//
// The AMR-shaped sibling of §46.2's HostBlockedByCommandLoss, and it has the same shape for the same
// reason: the tier is refused where the recovery path has not been established, and the refusal
// names the fix. **A `fixed` base is never refused on this ground**, because it never was the
// problem.
func HostBlockedBySafeState(profile canonical.Value, base string, hasBase bool) string {
	if !BaseIsMobile(base, hasBase) {
		return ""
	}
	fault, faulty := ValidateSafeState(profile, base, hasBase)
	if !faulty {
		return ""
	}
	// Two sentences rather than one, and neither is assembled out of the other. §64 makes a message
	// that misdescribes what happened a defect, and a refusal built by splicing a clause in front of
	// a detail that already opens with its own subject is exactly how one gets written.
	return "§36.2 computes this body's reserve budget from the time it takes to reach a safe " +
		"state, and this body travels, so that figure is one nothing can place (" + fault.Code +
		"). " + fault.Detail + " (§87.2)"
}

// safeStateCase is §87.2's declaration, its consequence, and the reserve it moves.
//
// The four together rather than the fault alone, because §87.2's whole argument is that a figure and
// its provenance are one fact: a document carrying `basis_undeclared` and not carrying *"and
// therefore host is refused and the reserve term is unaccounted"* would leave a reader to re-derive
// the consequence, which is where two editions would disagree.
func safeStateCase(input canonical.Value) (canonical.Value, error) {
	profile := input.Field("profile")
	base, hasBase := input.Field("base").Str()

	faultValue := canonical.Null()
	if fault, faulty := ValidateSafeState(profile, base, hasBase); faulty {
		faultValue = canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("detail", canonical.String(fault.Detail))
	}

	basis := ""
	if hasBase {
		basis = SafeStateBasisOf(profile, base)
	}
	blocked := HostBlockedBySafeState(profile, base, hasBase)
	budget := ComputeReserve(profile, base, hasBase)

	return canonical.Object().
		Set("base", optionalString(base, hasBase)).
		Set("fault", faultValue).
		Set("basis", optionalString(basis, basis != "")).
		Set("host_blocked", optionalString(blocked, blocked != "")).
		Set("reserve", canonical.Object().
			Set("wh", optionalNumber(budget.WH, budget.HasWH)).
			Set("unaccounted", canonical.Strings(budget.Unaccounted)).
			Set("safe_state_basis", optionalString(budget.SafeStateBasis, budget.SafeStateBasis != ""))), nil
}

// ClaimsAPack reports whether this body claims a battery at all.
//
// A pack is claimed by declaring either its capacity or the energy a safe stop costs — the second
// because an energy budget in watt-hours is a statement about a pack even when nobody wrote the
// capacity down. A body that declares neither is a mains machine, and `unmetered` is the correct
// reading of its silence.
func ClaimsAPack(profile canonical.Value) bool {
	return profile.Has("pack_capacity_wh") || profile.Has("safe_stop_energy_wh")
}

// ThermalPressure is the pressure a temperature implies, given what the body declares.
//
// A temperature with no declared threshold is `nominal` **and a fault**, not a silent pass: nobody
// has said what hot is on this body. Guessing 85 °C would be right for a CPU package and wrong for
// a harmonic drive by fifty degrees.
func ThermalPressure(profile canonical.Value, temperature float64, hasTemperature bool) string {
	if !hasTemperature {
		return "nominal"
	}
	if critical, has := profile.Field("thermal_critical_c").Number(); has && temperature >= critical {
		return "critical"
	}
	if throttle, has := profile.Field("thermal_throttle_c").Number(); has && temperature >= throttle {
		return "throttling"
	}
	return "nominal"
}

// Shed is what is given up at this pressure, in ShedOrder order (§36.3).
//
// Never returns anything in NeverShed: there is no thermal situation in which giving up the safety
// envelope is the right trade, and inference is throttled before control rather than beside it.
func Shed(level string) []string {
	depth := 0
	switch level {
	case "throttling":
		depth = ThrottleDepth
	case "critical":
		depth = len(vocabulary.ShedOrder)
	}
	return vocabulary.ShedOrder[:depth]
}

// BudgetMissed is whether a package's declared inference budget can still be met (§36.3).
//
// Empty when it can, or when the question cannot be asked — no declaration, no measurement. A
// package is never blocked on an absence.
//
// The comparison is against the **observed** loop time rather than against the thermal state, which
// is the honest coupling: thermal throttling is the usual cause of a missed budget and not the only
// one, and a robot that blocked a package for being warm while its loop was comfortably inside
// budget would be enforcing a proxy instead of the thing itself.
func BudgetMissed(declared float64, hasDeclared bool, observed float64, hasObserved bool) string {
	if !hasDeclared || declared <= 0 || !hasObserved {
		return ""
	}
	if observed <= declared {
		return ""
	}
	return "this package declares an inference budget of " + js.NumberToString(declared) + " ms and the loop " +
		"is taking " + js.NumberToString(observed) + " ms. §36.3 throttles inference before control, so the " +
		"loop is late rather than the robot being unsafe — but a policy quietly missing its own " +
		"declared deadline is what the budget exists to prevent, so it is refused until the " +
		"measurement comes back inside it."
}

// PowerAssessment is §36 for one cycle.
type PowerAssessment struct {
	State        string
	Reason       string
	Reserve      PowerReserve
	RemainingWH  float64
	HasRemaining bool
	Thermal      string
	Shed         []string
	Faults       []PowerFault
}

// GrantsNewLeases is the state's answer, narrowed by any fault.
func GrantsNewLeases(assessment PowerAssessment) bool {
	if !StateGrantsNewLeases(assessment.State) {
		return false
	}
	for _, fault := range assessment.Faults {
		if FaultWithholdsLeases(fault.Code) {
			return false
		}
	}
	return true
}

// PowerDegraded reports whether §27's DEGRADED is raised by power or thermal (§36.3).
func PowerDegraded(assessment PowerAssessment) bool {
	for _, fault := range assessment.Faults {
		if FaultDegrades(fault.Code) {
			return true
		}
	}
	return false
}

// AssessPower assesses §36 for one cycle.
//
// The order of the rules is the design, and the first is what §36.4 is written for.
//
//  1. **Brownout.** The bus said sag; the gauge is not consulted, "because a gauge that disagrees
//     with the bus is a gauge that is wrong". The only rule that ignores a reading it has.
//  2. **No pack declared.** A mains machine, correctly reporting that its energy is not metered.
//  3. **No reading.** A body that claims a pack and got nothing back. Not `critical`, and the fault
//     is what makes it distinguishable from rule 2.
//  4. **No reserve, or no capacity.** A reading arrived and cannot be compared against anything.
//  5. **The bands.** The only rule that looks at a number, and last for that reason.
func AssessPower(profile, observation canonical.Value, base string, hasBase bool) PowerAssessment {
	budget := ComputeReserve(profile, base, hasBase)
	temperature, hasTemperature := finiteNumber(observation.Field("temperature_c"))
	thermal := ThermalPressure(profile, temperature, hasTemperature)
	faults := []PowerFault{}

	remaining, hasRemaining := remainingWattHours(profile, observation)
	claimsPack := ClaimsAPack(profile)
	codes := observation.Field("fault_code")
	declaredCodes := profile.Field("brownout_fault_codes")
	hasCharge := observation.Has("state_of_charge")
	hasEnergy := observation.Has("energy_remaining_wh")
	noReading := !hasCharge && !hasEnergy

	// Rule 1, before the faults are gathered because §36.4's override does not depend on any of
	// them. A brownout with a silent gauge is still a brownout.
	faultCode, hasFaultCode := codes.Number()
	brownout := isTrue(observation.Field("rhal_degraded")) && hasFaultCode &&
		containsNumber(declaredCodes, faultCode)

	state, reason := "", ""
	switch {
	case brownout:
		state, reason = "critical", "brownout"
	case !claimsPack:
		state, reason = "unmetered", "no_pack"
	case noReading:
		state, reason = "unmetered", "no_reading"
	case !hasRemaining:
		// Two ways to arrive with no energy figure, and they send an operator to two different
		// places: a gauge whose reading cannot be believed, and a descriptor with no capacity to
		// scale a believable one by.
		state = "unmetered"
		reason = "no_capacity"
		if chargeUnusable(observation) {
			reason = "reading_unusable"
		}
	case !budget.HasWH:
		state, reason = "unmetered", "no_reserve"
	default:
		state, reason = powerBand(remaining, budget.WH), bandReason(remaining, budget.WH)
	}

	// The faults, in PowerFaultCodes order so a console renders them the same way every time.
	if claimsPack && noReading {
		faults = append(faults, PowerFault{
			Code: "gauge_silent",
			Detail: "this body declares a pack and nothing on this robot read one. The state is " +
				"reported as unmetered rather than critical — §36.4 overrides the gauge on a positive " +
				"reading from the bus, and a missing reading is not one — and no new task lease is " +
				"granted until something answers for the pack (§36.1).",
		})
	}
	if claimsPack && !hasEnergy && hasCharge && !profile.Has("pack_capacity_wh") {
		faults = append(faults, PowerFault{
			Code: "capacity_undeclared",
			Detail: "the gauge reports a fraction and this body declares no `pack_capacity_wh` to turn " +
				"it into watt-hours, so there is nothing to compare against the reserve. Declare the " +
				"capacity, or report `energy_remaining_wh` from the gauge (§36.2).",
		})
	}
	if claimsPack && !budget.HasWH {
		faults = append(faults, PowerFault{
			Code: "reserve_undeclared",
			Detail: "this body declares a pack and no `safe_stop_energy_wh`, so §36.2's reserve cannot " +
				"be computed and the four states have no boundary. It is the figure " +
				"`power.safe_state.reachable` measures, and the one that decides when this robot stops.",
		})
	}
	if isTrue(observation.Field("rhal_degraded")) && declaredCodes.Len() == 0 {
		faults = append(faults, PowerFault{
			Code: "brownout_undeclarable",
			Detail: "the RHAL is reporting `RHAL_DEGRADED` and this body declares no " +
				"`brownout_fault_codes`, so nothing here can tell a voltage sag from a dead encoder. " +
				"§24 makes a fault code vendor-specific and surfaced verbatim, which is precisely why " +
				"the vendor rather than the OS has to name the codes that mean sag (§36.4).",
		})
	}
	if charge, isCharge := observation.Field("state_of_charge").Number(); hasCharge &&
		(!isCharge || !canonical.Finite(charge) || charge < 0 || charge > 1) {
		faults = append(faults, PowerFault{
			Code: "charge_out_of_range",
			Detail: "the gauge reports a state of charge of " + js.NumberToString(charge) + ", and a state of " +
				"charge is a fraction between 0 and 1. The reading is discarded rather than clamped: a " +
				"clamped 1.4 is a full battery, and this one is a gauge nobody should be trusting yet " +
				"(§36.1).",
		})
	}
	throttledFor := numberOr(observation.Field("throttled_for_ms"), 0)
	if thermal == "throttling" && throttledFor >= SustainedThrottleMS {
		faults = append(faults, PowerFault{
			Code: "thermal_sustained",
			Detail: "this body has been throttling for " + js.NumberToString(math.Floor(throttledFor/1000)) + " s. " +
				"§36.3 raises DEGRADED on sustained throttling, and " + js.NumberToString(ThrottleDepth) +
				" of the five rungs are given up — never the RHAL, the kernel or the control loop.",
		})
	}
	if thermal == "critical" {
		faults = append(faults, PowerFault{
			Code: "thermal_critical",
			Detail: "this body is at or above its declared critical temperature. Every rung of §36.3's " +
				"ladder is given up, inference included; the RHAL, the kernel and the control loop are " +
				"not, because there is no thermal situation in which giving up the safety envelope is " +
				"the right trade.",
		})
	}
	if hasTemperature && !profile.Has("thermal_throttle_c") && !profile.Has("thermal_critical_c") {
		faults = append(faults, PowerFault{
			Code: "thermal_undeclared",
			Detail: "something is reading a temperature and this body declares neither " +
				"`thermal_throttle_c` nor `thermal_critical_c`, so nothing here knows what hot is on " +
				"this machine. The reading is reported and nothing is shed: 85 °C is right for a CPU " +
				"package and fifty degrees wrong for a harmonic drive (§36.3).",
		})
	}

	return PowerAssessment{
		State: state, Reason: reason, Reserve: budget,
		RemainingWH: remaining, HasRemaining: hasRemaining,
		Thermal: thermal, Shed: Shed(thermal), Faults: faults,
	}
}

// remainingWattHours is the energy the gauge accounts for.
//
// `energy_remaining_wh` wins over a fraction times a capacity, because a gauge reporting energy has
// been told the pack's capacity by something closer to the pack than a descriptor field. A fraction
// outside 0–1 produces nothing rather than a clamp.
func remainingWattHours(profile, observation canonical.Value) (float64, bool) {
	if energy, has := finiteNumber(observation.Field("energy_remaining_wh")); has && energy >= 0 {
		return energy, true
	}
	charge, hasCharge := finiteNumber(observation.Field("state_of_charge"))
	if !hasCharge || charge < 0 || charge > 1 {
		return 0, false
	}
	capacity, hasCapacity := finiteNumber(profile.Field("pack_capacity_wh"))
	if !hasCapacity || capacity <= 0 {
		return 0, false
	}
	return charge * capacity, true
}

// chargeUnusable reports whether the gauge produced a state of charge that is not one.
//
// Only asked when there is no energy figure, and only about the fraction: an energy_remaining_wh
// that is negative or not finite is already ignored by remainingWattHours.
func chargeUnusable(observation canonical.Value) bool {
	if !observation.Has("state_of_charge") {
		return false
	}
	charge, isNumber := observation.Field("state_of_charge").Number()
	return !isNumber || !canonical.Finite(charge) || charge < 0 || charge > 1
}

// powerBand is §36.1's bands.
func powerBand(remaining, budget float64) string {
	switch {
	case remaining < budget:
		return "critical"
	case remaining < budget*ReserveBand:
		return "reserve"
	case remaining < budget*LowBand:
		return "low"
	}
	return "normal"
}

// bandReason is kept beside powerBand rather than derived from the state, so the distinction
// between *at reserve* and *below it* — the one §36.1 spends two rows on — survives.
func bandReason(remaining, budget float64) string {
	switch powerBand(remaining, budget) {
	case "critical":
		return "below_reserve"
	case "reserve":
		return "at_reserve"
	case "low":
		return "approaching_reserve"
	}
	return "above_reserve"
}

func containsNumber(array canonical.Value, value float64) bool {
	for _, entry := range array.Items() {
		if number, ok := entry.Number(); ok && number == value {
			return true
		}
	}
	return false
}

// ── the corpus ──────────────────────────────────────────────────────────────

func powerCase(input canonical.Value) (canonical.Value, error) {
	profile := input.Field("profile")
	observation := input.Field("observation")
	// §87.2's `hasBase` is *did the caller say what this body stands on*, and these cases do not: a
	// cycle's power document is about a gauge, and a case that named a base would be asserting the
	// safe-state rules from a file that is not about them.
	assessment := AssessPower(profile, observation, "", false)

	declared, hasDeclared := finiteNumber(input.Field("declared_budget_ms"))
	observed, hasObserved := finiteNumber(input.Field("observed_inference_ms"))
	missed := BudgetMissed(declared, hasDeclared, observed, hasObserved)

	faults := canonical.Array()
	for _, fault := range assessment.Faults {
		faults = faults.Push(canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("detail", canonical.String(fault.Detail)))
	}

	return canonical.Object().
		Set("state", canonical.String(assessment.State)).
		Set("reason", canonical.String(assessment.Reason)).
		Set("remaining_wh", optionalNumber(assessment.RemainingWH, assessment.HasRemaining)).
		Set("reserve", canonical.Object().
			Set("wh", optionalNumber(assessment.Reserve.WH, assessment.Reserve.HasWH)).
			Set("unaccounted", canonical.Strings(assessment.Reserve.Unaccounted)).
			Set("complete", canonical.Bool(ReserveComplete(assessment.Reserve))).
			// §87.2. Beside `unaccounted` rather than instead of it: the term leaving the list
			// says the computation happened, and the basis says *how*.
			Set("safe_state_basis", optionalString(assessment.Reserve.SafeStateBasis,
				assessment.Reserve.SafeStateBasis != ""))).
		Set("thermal", canonical.String(assessment.Thermal)).
		Set("shed", canonical.Strings(assessment.Shed)).
		Set("faults", faults).
		Set("degraded", canonical.Bool(PowerDegraded(assessment))).
		Set("grants_new_leases", canonical.Bool(GrantsNewLeases(assessment))).
		Set("requires_safe_state", canonical.Bool(StateRequiresSafeState(assessment.State))).
		Set("stops_immediately", canonical.Bool(StateStopsImmediately(assessment.State))).
		Set("notifies_operator", canonical.Bool(StateNotifiesOperator(assessment.State))).
		Set("budget_missed", optionalString(missed, missed != "")), nil
}
