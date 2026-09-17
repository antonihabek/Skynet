package contracts

import (
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// The watchdog chain (§46).
//
// Four layers, each catching a failure the one above it cannot see. Layer 3 is petted by the
// **Safety Kernel**, not by `skynetd`, and that is what makes it mean anything: the WDT is
// satisfied only while the thing that actually enforces the envelope is running its cycle. A
// watchdog petted by a supervisor tells you the supervisor is alive, which is not the question
// anybody is asking.
//
// # A reset is not a stop
//
// §46.2's uncomfortable part. When the SoC resets, the drives lose their command source, and what
// they then do is a property of the **hardware**. Some hold. Some engage brakes. Some fault and
// disable. **Some coast** — and on such a body, layer 3's own recovery path is a fall. So the host
// tier is refused for a coasting body, and the shape of that refusal is unusual and correct: **the
// middle rung is the unsafe one.** Guest is fine because enforcement is advisory; certified is fine
// because layer 4 cuts drive enable independently of the processor being reset.

// CommandLossBehaviours is §46.2's vocabulary, in the order the specification lists it.
var CommandLossBehaviours = []string{"hold", "brake", "fault", "coast"}

// WatchdogLayer is one rung of the chain.
type WatchdogLayer struct {
	Layer        float64
	Name         string
	Catches      string
	Response     string
	RequiredFrom string
	PettedBy     string
}

// WatchdogChain is the chain, as data (§46.1).
var WatchdogChain = []WatchdogLayer{
	{
		Layer: 1, Name: "kernel_guard",
		Catches:  "the policy stopped producing commands",
		Response: "stop_cat2", RequiredFrom: "guest",
		PettedBy: "the policy runtime, every cycle",
	},
	{
		Layer: 2, Name: "supervisor",
		Catches:  "the supervisor's reconcile loop hung",
		Response: "restart skynetd", RequiredFrom: "guest",
		PettedBy: "skynetd, from inside the reconcile loop",
	},
	{
		Layer: 3, Name: "soc_wdt",
		Catches:  "the application processor wedged entirely",
		Response: "reset the SoC", RequiredFrom: "host",
		// §46.1's sentence, and the reason layer 3 means anything.
		PettedBy: "the Safety Kernel, not skynetd",
	},
	{
		Layer: 4, Name: "safety_mcu",
		Catches:  "the AP wedged and the reset did not recover it",
		Response: "cut drive enable", RequiredFrom: "certified",
		PettedBy: "an independent timer on the safety MCU",
	},
}

// HostBlockedByCommandLoss is why a body may not attempt the host tier, or "" when it may (§46.2).
//
// Two refusals, and they are the same refusal: **the recovery path has not been established to be
// survivable.** A body that coasts has one that demonstrably is not; a body that has declared
// nothing has one nobody has characterised, and treating silence as `hold` would be inventing the
// most convenient answer to the only question layer 3 turns on.
func HostBlockedByCommandLoss(onCommandLoss string, declared bool) string {
	if declared && onCommandLoss == "coast" {
		return "this body's drives coast when commands stop, and layer 3 of the watchdog chain " +
			"recovers by resetting the processor — which on this machine is a fall. The sound " +
			"configuration is the certified tier, where layer 4 cuts drive enable independently of " +
			"the processor (§46.2)"
	}
	if !declared {
		return "this body does not declare what its drives do when commands stop, and layer 3 of the " +
			"watchdog chain recovers by resetting the processor. Silence is not `hold`: it is a " +
			"recovery path nobody has characterised (§46.2)"
	}
	if !containsString(CommandLossBehaviours, onCommandLoss) {
		return "this body declares `on_command_loss: " + onCommandLoss + "`, which is not one of hold, " +
			"brake, fault or coast — so what it does while its processor resets is not something " +
			"anybody can read off the descriptor (§46.2)"
	}
	return ""
}

// ── Per chain (§87.3) ───────────────────────────────────────────────────────

// ChainBehaviour is one chain this body has, and what its drives do when commands stop.
type ChainBehaviour struct {
	Chain string
	// Behaviour is empty and Declared false when the declaration does not cover this chain.
	Behaviour string
	Declared  bool
}

// CommandLossReport is §87.3's resolution over one body.
type CommandLossReport struct {
	// PerChain is whether a map was declared at all — not whether it covers anything.
	PerChain bool
	// Chains is each chain this body has, in the order `chains` was given.
	Chains []ChainBehaviour
	// UnknownChains are chains the declaration names and this body does not have.
	UnknownChains []string
	// HostBlocked is why host is refused, or empty. One answer, whichever way the body declared.
	HostBlocked string
}

// ResolveCommandLoss is §87.3's resolution: what each chain does, and whether host is available.
//
// **One answer about host, whichever way the body declared**, which is why this exists rather than a
// second HostBlockedBy…: §78 is the record of what two spellings of one fact cost. The body-wide
// rules stay in HostBlockedByCommandLoss and this delegates to them.
//
// The refusal order is the substance, and two of its steps were found by building it:
//
//  1. **Both declared** — §84.2's rule about two action spaces, for the same reason.
//  2. **A map on a body with no chains** — a body whose joints carry no parseable role is on *no*
//     chain, so a complete-looking map covers nothing.
//  3. **A chain the map does not cover** — refused, not inherited.
//  4. **An unrecognised behaviour, before a `coast`.** `coast` is characterised; an unrecognised
//     word is not, which is strictly worse and does not rule out coasting either.
//  5. **`coast` on any chain** — with every coasting chain named.
func ResolveCommandLoss(
	body string, hasBody bool, byChain canonical.Value, chains []string,
) CommandLossReport {
	declared := byChain.Kind() == canonical.KindObject

	report := CommandLossReport{PerChain: declared, Chains: []ChainBehaviour{}, UnknownChains: []string{}}

	if declared {
		for _, member := range byChain.Members() {
			if !containsString(chains, member.Key) {
				report.UnknownChains = append(report.UnknownChains, member.Key)
			}
		}
		sort.SliceStable(report.UnknownChains, func(left, right int) bool {
			return utf16Less(report.UnknownChains[left], report.UnknownChains[right])
		})
	}

	for _, chain := range chains {
		entry := ChainBehaviour{Chain: chain}
		if declared {
			entry.Behaviour, entry.Declared = byChain.Field(chain).Str()
		} else if hasBody {
			entry.Behaviour, entry.Declared = body, true
		}
		report.Chains = append(report.Chains, entry)
	}

	if declared && hasBody {
		report.HostBlocked = "this body declares both `on_command_loss` and " +
			"`on_command_loss_by_chain`, which are two descriptions of one fact — redundant where " +
			"they agree and a contradiction where they do not. Delete the one this body does not " +
			"mean (§87.3)"
		return report
	}

	if !declared {
		report.HostBlocked = HostBlockedByCommandLoss(body, hasBody)
		return report
	}

	if len(chains) == 0 {
		report.HostBlocked = "this body declares what its drives do per chain, and none of its " +
			"joints carries a role this vocabulary parses — so the declaration covers no joint at " +
			"all. §5.1 permits a body the vocabulary does not describe; what it cannot do is " +
			"declare per chain. Give the joints roles, or declare `on_command_loss` for the body " +
			"(§87.3)"
		return report
	}

	for _, entry := range report.Chains {
		if !entry.Declared {
			report.HostBlocked = "this body has a " + entry.Chain + " chain and its per-chain " +
				"declaration does not cover it. A missing chain is refused rather than inheriting " +
				"a body-wide value, because inheriting makes a partially declared body look fully " +
				"declared (§87.3)"
			return report
		}
	}

	for _, entry := range report.Chains {
		if !containsString(CommandLossBehaviours, entry.Behaviour) {
			report.HostBlocked = "this body declares `" + entry.Chain + ": " + entry.Behaviour +
				"`, which is not one of hold, brake, fault or coast — so what that chain does " +
				"while its processor resets is not something anybody can read off the descriptor. " +
				"An unrecognised behaviour is refused before a `coast` is, because `coast` is " +
				"characterised and this is not (§46.2, §87.3)"
			return report
		}
	}

	coasting := []string{}
	for _, entry := range report.Chains {
		if entry.Behaviour == "coast" {
			coasting = append(coasting, entry.Chain)
		}
	}
	if len(coasting) > 0 {
		report.HostBlocked = "this body's " + strings.Join(coasting, ", ") + " drives coast when " +
			"commands stop, and layer 3 of the watchdog chain recovers by resetting the processor " +
			"— which for those chains is a fall. The sound configuration is the certified tier, " +
			"where layer 4 cuts drive enable independently of the processor (§46.2)"
	}

	return report
}

// Windows is the four windows §46.3 nests, in order, innermost first.
var Windows = []string{
	"kernel_cycle_worst_case_ms", "envelope_watchdog_ms", "soc_wdt_ms", "drive_command_timeout_ms",
}

// NestingFault is one inversion of §46.3's order.
type NestingFault struct {
	Code   string
	Inner  string
	Outer  string
	Detail string
}

// NestingReport is what checking the nesting found.
type NestingReport struct {
	Faults  []NestingFault
	Missing []string
	// Ordered means every pair that could be compared nests. **Not** the same as "the chain is
	// sound": a chain with three of its four windows undeclared is ordered and says nothing.
	Ordered bool
	// Complete means every window is declared *and* every pair nests.
	Complete bool
}

func nestingCode(inner, outer string) string {
	switch {
	case inner == "kernel_cycle_worst_case_ms" && outer == "envelope_watchdog_ms":
		return "cycle_exceeds_watchdog"
	case inner == "envelope_watchdog_ms" && outer == "soc_wdt_ms":
		return "wdt_pre_empts_kernel"
	case inner == "soc_wdt_ms" && outer == "drive_command_timeout_ms":
		return "drive_faults_first"
	}
	return "windows_not_nested"
}

func nestingConsequence(inner, outer string) string {
	switch {
	case inner == "kernel_cycle_worst_case_ms" && outer == "envelope_watchdog_ms":
		return "The kernel cannot complete a cycle inside its own command deadline, so the watchdog " +
			"guard trips on a robot that is working correctly."
	case inner == "envelope_watchdog_ms" && outer == "soc_wdt_ms":
		return "The processor resets before the kernel executes its controlled stop, so the harsher " +
			"recovery pre-empts the gentler one and the robot never performs its best available " +
			"behaviour (§46.3)."
	case inner == "soc_wdt_ms" && outer == "drive_command_timeout_ms":
		return "The drive faults on its own before the hardware watchdog fires, so layer 3 is present, " +
			"configured, and never reached (§46.3)."
	}
	return "The windows must nest innermost-first for every layer above to be reachable."
}

// CheckNesting checks §46.3's nesting.
//
// Adjacent pairs only: comparing every pair would report the same inversion up to three times and
// bury the one an operator has to fix.
//
// **Strictly less than.** Two watchdogs with the same window is a race whose winner is decided by
// interrupt latency, and a chain whose order depends on which timer fires first is not a chain.
//
// A gap in the middle does not stop the walk: with no `envelope_watchdog_ms` declared, the kernel
// cycle and the SoC window are still worth comparing, because that inversion is real whatever sits
// between them.
func CheckNesting(values []float64, present []bool) NestingReport {
	missing := []string{}
	declaredNames := []string{}
	declaredValues := []float64{}
	for at, name := range Windows {
		if !present[at] {
			missing = append(missing, name)
			continue
		}
		declaredNames = append(declaredNames, name)
		declaredValues = append(declaredValues, values[at])
	}

	faults := []NestingFault{}
	for at := 0; at+1 < len(declaredNames); at++ {
		innerName, inner := declaredNames[at], declaredValues[at]
		outerName, outer := declaredNames[at+1], declaredValues[at+1]
		if inner < outer {
			continue
		}
		faults = append(faults, NestingFault{
			Code:  nestingCode(innerName, outerName),
			Inner: innerName,
			Outer: outerName,
			Detail: innerName + " is " + js.NumberToString(inner) + " ms and " + outerName + " is " +
				js.NumberToString(outer) + " ms, so the outer window is not outside the inner one. " +
				nestingConsequence(innerName, outerName),
		})
	}

	return NestingReport{
		Faults:   faults,
		Missing:  missing,
		Ordered:  len(faults) == 0,
		Complete: len(faults) == 0 && len(missing) == 0,
	}
}

func containsString(values []string, value string) bool {
	for _, entry := range values {
		if entry == value {
			return true
		}
	}
	return false
}

// ── the corpus ──────────────────────────────────────────────────────────────

func watchdogCase(input canonical.Value) (canonical.Value, error) {
	onCommandLoss, declared := input.Field("on_command_loss").Str()

	block := input.Field("windows")
	values := make([]float64, len(Windows))
	present := make([]bool, len(Windows))
	for at, name := range Windows {
		value, isNumber := block.Field(name).Number()
		// The reference's `present()` also refuses a non-finite value, which JSON cannot spell —
		// so a number here is always finite and the check is the type test alone.
		values[at], present[at] = value, isNumber && canonical.Finite(value)
	}
	report := CheckNesting(values, present)

	chain := canonical.Array()
	for _, layer := range WatchdogChain {
		chain = chain.Push(canonical.Object().
			Set("layer", canonical.Number(layer.Layer)).
			Set("name", canonical.String(layer.Name)).
			Set("catches", canonical.String(layer.Catches)).
			Set("response", canonical.String(layer.Response)).
			Set("required_from", canonical.String(layer.RequiredFrom)).
			Set("petted_by", canonical.String(layer.PettedBy)))
	}

	// An array, not an object: canonical JSON sorts an object's keys, and these four windows mean
	// nothing except in order. The object form renders them alphabetically, which on a panel about
	// nesting is worse than rendering nothing.
	windows := canonical.Array()
	for at, name := range Windows {
		windows = windows.Push(canonical.Object().
			Set("window", canonical.String(name)).
			Set("ms", optionalNumber(values[at], present[at])))
	}

	faults := canonical.Array()
	for _, fault := range report.Faults {
		faults = faults.Push(canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("inner", canonical.String(fault.Inner)).
			Set("outer", canonical.String(fault.Outer)).
			Set("detail", canonical.String(fault.Detail)))
	}

	// §87.3. `chains` is the body's own chain list rather than the map's keys — coverage is a
	// property of the body, and a map that named chains a body does not have would otherwise
	// measure itself.
	chains := []string{}
	for _, entry := range input.Field("chains").Items() {
		if name, isString := entry.Str(); isString {
			chains = append(chains, name)
		}
	}
	loss := ResolveCommandLoss(onCommandLoss, declared, input.Field("on_command_loss_by_chain"), chains)

	perChainRows := canonical.Array()
	for _, entry := range loss.Chains {
		perChainRows = perChainRows.Push(canonical.Object().
			Set("chain", canonical.String(entry.Chain)).
			Set("behaviour", optionalString(entry.Behaviour, entry.Declared)))
	}

	return canonical.Object().
		Set("chain", chain).
		Set("on_command_loss", optionalString(onCommandLoss, declared)).
		// §87.3. Present on every document, because *"this body declares one behaviour"* and *"this
		// body declares one behaviour per chain"* are two states a reader has to be able to tell
		// apart without knowing which fields the descriptor happened to carry.
		Set("per_chain", canonical.Object().
			Set("declared", canonical.Bool(loss.PerChain)).
			Set("chains", perChainRows).
			Set("unknown_chains", canonical.Strings(loss.UnknownChains))).
		Set("host_blocked", optionalString(loss.HostBlocked, loss.HostBlocked != "")).
		Set("nesting", canonical.Object().
			Set("declared", windows).
			Set("faults", faults).
			Set("missing", canonical.Strings(report.Missing)).
			Set("ordered", canonical.Bool(report.Ordered)).
			Set("complete", canonical.Bool(report.Complete))), nil
}
