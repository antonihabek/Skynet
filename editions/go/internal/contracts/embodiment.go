package contracts

import (
	"fmt"
	"math"
	"regexp"
	"strconv"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// ParsedRole is a canonical role, taken apart (§5.1).
type ParsedRole struct {
	Chain string
	Link  string
	// Axis is empty when the role names none. Distinguished from a declared axis by HasAxis
	// rather than by the empty string, because the wire spells the absence `null`.
	Axis    string
	HasAxis bool
	// Index is the ordinal for repeated links — `base.wheel#2`, `right_hand.finger#1`.
	Index    float64
	HasIndex bool
}

// rolePattern is the reference's, with one deliberate respelling.
//
// The reference writes `(?:#(\d+))?`. In JavaScript, without the `u` flag, `\d` is exactly [0-9] —
// and in a great many other regex dialects it is not. The Python edition's `\d` matched
// Arabic-Indic digits, so `base.wheel#١` parsed as ordinal 1 where the reference refuses it; the
// corpus now holds that exact string. Go's `\d` *is* [0-9], so this would be correct either way,
// and it is spelt out anyway: the next reader should be able to see which of the two was meant
// without knowing Go's answer, and `cmd/selfcheck` refuses the shorthand in this package.
//
// `$` is the other half of the same lesson. Python's `$` matches before a final newline, so
// `list.0\n` indexed element 0. Go's `$` is end-of-text unless `(?m)` is set, which matches
// JavaScript's without the `m` flag.
var rolePattern = regexp.MustCompile(`^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#([0-9]+))?$`)

// ParseRole parses a canonical role, reporting whether it is one.
//
// The reference returns null rather than throwing, and the caller treats null as "this joint can
// only be matched by exact name". That is the extensibility rule that keeps the vocabulary honest:
// an unrecognised role never *silently* maps to something plausible. A body the vocabulary does not
// describe is still usable — it just cannot participate in cross-body remapping, which is the
// truthful outcome, because nobody has established what its joints mean.
func ParseRole(role string) (ParsedRole, bool) {
	// The reference's `if (!role) return null` — falsy, which for a string is exactly the empty
	// one. A role of whitespace is not caught here; it fails the pattern after trimming.
	if !js.Truthy(role) {
		return ParsedRole{}, false
	}
	match := rolePattern.FindStringSubmatch(js.ToLower(js.Trim(role)))
	if match == nil {
		return ParsedRole{}, false
	}
	chain, link, axis, index := match[1], match[2], match[3], match[4]
	if !vocabulary.Contains(vocabulary.CanonicalChains, chain) {
		return ParsedRole{}, false
	}
	// `if (axis && ...)`: an axis that did not participate is the empty string, which is falsy, and
	// the check is skipped rather than failing on a chain that has no axis.
	if js.Truthy(axis) && !vocabulary.Contains(vocabulary.CanonicalAxes, axis) {
		return ParsedRole{}, false
	}
	parsed := ParsedRole{Chain: chain, Link: link, Axis: axis, HasAxis: js.Truthy(axis)}
	if js.Truthy(index) {
		// `[0-9]+` cannot fail to parse, and a value too large for a float64 loses precision
		// exactly as Number() does. The error is checked rather than dropped because R-15 does not
		// have an exception for calls that cannot fail today.
		value, err := strconv.ParseFloat(index, 64)
		if err != nil {
			return ParsedRole{}, false
		}
		parsed.Index, parsed.HasIndex = value, true
	}
	return parsed, true
}

// RoleKey is the canonical string form, so two parsed roles can be compared as keys.
func RoleKey(parsed ParsedRole) string {
	key := parsed.Chain + "." + parsed.Link
	if parsed.HasAxis {
		key += "." + parsed.Axis
	}
	if parsed.HasIndex {
		// String(number), not the canonical writer: the reference interpolates the ordinal into
		// a template literal, and the two spell a large one differently. See js.NumberToString.
		key += "#" + js.NumberToString(parsed.Index)
	}
	return key
}

// rolesVocabulary answers `roles/vocabulary`: a role string, parsed, or null.
func rolesVocabulary(input canonical.Value) (canonical.Value, error) {
	// A role that is absent or null is not a string, and `!role` in the reference is true for
	// both — one of the corpus's cases supplies `null` against a `string | null | undefined`
	// parameter, which is corpus/README.md's first kind of null: the type admits it, so it is the
	// value, and it means "no role".
	role, _ := input.Field("role").Str()
	parsed, ok := ParseRole(role)
	if !ok {
		return canonical.Null(), nil
	}
	return canonical.Object().
		Set("chain", canonical.String(parsed.Chain)).
		Set("link", canonical.String(parsed.Link)).
		Set("axis", optionalString(parsed.Axis, parsed.HasAxis)).
		Set("index", optionalNumber(parsed.Index, parsed.HasIndex)).
		Set("key", canonical.String(RoleKey(parsed))), nil
}

// ── the frame graph, §44.3 ──────────────────────────────────────────────────

// Vec3 is a point or a direction, in metres or as a unit vector.
type Vec3 [3]float64

// Quat is a rotation, ordered `xyzw` (§44.4). Not `wxyz`: a reader holding the other convention
// rotates every frame below the first static rotation by the wrong amount, and `a-rotated-static-
// frame` is the case that would show it.
type Quat [4]float64

// CollisionShape is a sphere-swept segment (§44.3): every point within RadiusM of the segment from
// A to B.
type CollisionShape struct {
	// Kind is kept as the document wrote it, because a kind this edition cannot evaluate is named in
	// the fault rather than discarded.
	Kind    canonical.Value
	A, B    Vec3
	RadiusM float64
}

// FrameNode is one node of a §44.3 frame graph, as the descriptor states it.
//
// Several fields carry a Has flag rather than a zero value, and each is a distinction the reference
// reads: a translation absent is the identity and a translation present is the declared one; a
// mimic coefficient present-and-null is a coefficient that is not a finite number (corpus/README.md's
// bare-number rule) and a coefficient absent is no coefficient at all.
type FrameNode struct {
	Name string
	// Parent is the parent's name. IsRoot is true when the document wrote `null`.
	Parent string
	IsRoot bool
	// Kind is kept raw for the same reason CollisionShape.Kind is.
	Kind canonical.Value
	// KindPresent distinguishes an absent kind, which the reference renders as "undefined".
	KindPresent    bool
	Translation    Vec3
	HasTranslation bool
	Rotation       Quat
	HasRotation    bool
	// Joint is present and not null exactly when HasJoint is.
	Joint    string
	HasJoint bool
	// JointType is kept raw, because an unknown one is named in the fault.
	JointType    canonical.Value
	HasJointType bool
	Axis         Vec3
	HasAxis      bool
	Mimics       string
	HasMimics    bool
	// MimicMultiplier and MimicOffset are NaN when the document wrote `null`, and present either way.
	MimicMultiplier    float64
	HasMimicMultiplier bool
	MimicOffset        float64
	HasMimicOffset     bool
	// Collision is `collision === true`: a string "true" or a 1 is not a declaration of geometry.
	Collision bool
	// Shape is nil where the node declares none, or declares `null`.
	Shape           *CollisionShape
	CollisionIgnore []string
}

// FrameGraph is §44.3's tree.
type FrameGraph struct {
	Root  string
	Nodes []FrameNode
}

// FrameFault is why a frame graph cannot be used, naming the frames it is about.
//
// Returned rather than thrown, because §44.5 requires the pose guards to report unavailable **with
// the missing frame named**. "The frame graph is malformed" is not a sentence an operator can act on.
type FrameFault struct {
	Code     string
	Subjects []string
	Detail   string
}

// vec3Of reads a three-element array of bare numbers. A missing element or a `null` is NaN, which is
// what arithmetic on `undefined` produces in the reference and what corpus/README.md's rule makes a
// null against a bare `number` type.
func vec3Of(value canonical.Value) Vec3 {
	var out Vec3
	for at := range out {
		element := value.At(at)
		number, isNumber := element.Number()
		out[at] = js.BareNumber(at < value.Len(), number, isNumber)
	}
	return out
}

func quatOf(value canonical.Value) Quat {
	var out Quat
	for at := range out {
		element := value.At(at)
		number, isNumber := element.Number()
		out[at] = js.BareNumber(at < value.Len(), number, isNumber)
	}
	return out
}

// presentNonNull is the reference's `x !== undefined && x !== null`.
func presentNonNull(object canonical.Value, key string) (canonical.Value, bool) {
	value, present := object.Get(key)
	return value, present && !value.IsNull()
}

// ReadFrameGraph parses a §44.3 graph out of a document.
//
// A node whose name or parent is not a string (or, for the parent, `null`) is refused with an error
// rather than read as something plausible. Those are outside the type the reference declares, and
// what its code would do with them — compare a number against every frame name and find none — is
// behaviour nobody specified; an edition guessing at it would be agreeing with nothing.
func ReadFrameGraph(value canonical.Value) (FrameGraph, error) {
	root, _ := value.Field("root").Str()
	graph := FrameGraph{Root: root, Nodes: []FrameNode{}}
	for at, entry := range value.Field("nodes").Items() {
		node, err := readFrameNode(entry)
		if err != nil {
			return FrameGraph{}, fmt.Errorf("frame %d: %w", at, err)
		}
		graph.Nodes = append(graph.Nodes, node)
	}
	return graph, nil
}

func readFrameNode(entry canonical.Value) (FrameNode, error) {
	name, ok := entry.Field("name").Str()
	if !ok {
		return FrameNode{}, fmt.Errorf("the name is not a string")
	}
	node := FrameNode{Name: name}

	parent := entry.Field("parent")
	switch {
	case parent.IsNull():
		node.IsRoot = true
	case parent.Kind() == canonical.KindString:
		node.Parent, _ = parent.Str()
	default:
		return FrameNode{}, fmt.Errorf("frame %s has a parent that is neither a name nor null", name)
	}

	node.Kind, node.KindPresent = entry.Get("kind")
	if translation, ok := presentNonNull(entry, "translation"); ok {
		node.Translation, node.HasTranslation = vec3Of(translation), true
	}
	if rotation, ok := presentNonNull(entry, "rotation"); ok {
		node.Rotation, node.HasRotation = quatOf(rotation), true
	}
	if joint, ok := presentNonNull(entry, "joint"); ok {
		node.Joint, _ = joint.Str()
		node.HasJoint = true
	}
	node.JointType, node.HasJointType = presentNonNull(entry, "joint_type")
	if axis, ok := presentNonNull(entry, "axis"); ok {
		node.Axis, node.HasAxis = vec3Of(axis), true
	}
	if mimics, ok := presentNonNull(entry, "mimics"); ok {
		node.Mimics, _ = mimics.Str()
		node.HasMimics = true
	}
	if multiplier, present := entry.Get("mimic_multiplier"); present {
		number, isNumber := multiplier.Number()
		node.MimicMultiplier, node.HasMimicMultiplier = js.BareNumber(true, number, isNumber), true
	}
	if offset, present := entry.Get("mimic_offset"); present {
		number, isNumber := offset.Number()
		node.MimicOffset, node.HasMimicOffset = js.BareNumber(true, number, isNumber), true
	}
	node.Collision = isTrue(entry.Field("collision"))
	if shape, ok := presentNonNull(entry, "collision_shape"); ok {
		radius, isNumber := shape.Field("radius_m").Number()
		node.Shape = &CollisionShape{
			Kind:    shape.Field("kind"),
			A:       vec3Of(shape.Field("a")),
			B:       vec3Of(shape.Field("b")),
			RadiusM: js.BareNumber(shape.Has("radius_m"), radius, isNumber),
		}
	}
	for _, ignored := range entry.Field("collision_ignore").Items() {
		text, _ := ignored.Str()
		node.CollisionIgnore = append(node.CollisionIgnore, text)
	}
	return node, nil
}

// kindIs is `node.kind === kind`: strict, so only a string can match.
func (node FrameNode) kindIs(kind string) bool {
	text, isString := node.Kind.Str()
	return node.KindPresent && isString && text == kind
}

// ValidateFrames checks a frame graph against the rules §44.3 pins, reporting whether it is usable.
//
// Every fault here would otherwise present far from its cause: a missing parent as a frame that
// silently resolves to identity, a cycle as a hang in the middle of a control cycle, a duplicate
// name as a limb that moves when a different limb is commanded.
//
// It does **not** check that the graph reaches any particular frame — that is a question about a
// specific guard's needs, and §44.5 has that guard report it with the frame it wanted named.
func ValidateFrames(graph FrameGraph) (FrameFault, bool) {
	if len(graph.Nodes) == 0 {
		return FrameFault{Code: "empty", Subjects: []string{}, Detail: "The graph declares no frames."}, false
	}

	seen := map[string]bool{}
	for _, node := range graph.Nodes {
		if seen[node.Name] {
			return FrameFault{
				Code: "duplicate_name", Subjects: []string{node.Name},
				Detail: "Two frames are named " + node.Name + ", so a parent reference is ambiguous.",
			}, false
		}
		seen[node.Name] = true
	}

	roots := []string{}
	for _, node := range graph.Nodes {
		if node.IsRoot {
			roots = append(roots, node.Name)
		}
	}
	if len(roots) > 1 {
		return FrameFault{
			Code: "several_roots", Subjects: roots,
			Detail: js.NumberToString(float64(len(roots))) + " frames have no parent, so this is a forest and not a tree.",
		}, false
	}
	if !seen[graph.Root] {
		return FrameFault{
			Code: "root_missing", Subjects: []string{graph.Root},
			Detail: "The declared root " + graph.Root + " is not one of the frames.",
		}, false
	}

	// Three checks per node, in the reference's order within each node — a node's kind is judged
	// before its joint and its joint before its parent, and the first node with any fault is the one
	// reported.
	for _, node := range graph.Nodes {
		if !node.kindIs("root") && !node.kindIs("static") && !node.kindIs("dynamic") {
			return FrameFault{
				Code: "unknown_kind", Subjects: []string{node.Name},
				Detail: "Frame " + node.Name + " has kind " + js.ToString(node.Kind, node.KindPresent) +
					", which is not root, static or dynamic.",
			}, false
		}
		if node.kindIs("dynamic") != node.HasJoint {
			detail := "Frame " + node.Name + " is " + js.ToString(node.Kind, node.KindPresent) + " and names a joint."
			if node.kindIs("dynamic") {
				detail = "Frame " + node.Name + " is dynamic and names no joint."
			}
			return FrameFault{Code: "joint_mismatch", Subjects: []string{node.Name}, Detail: detail}, false
		}
		if !node.IsRoot && !seen[node.Parent] {
			return FrameFault{
				Code: "parent_missing", Subjects: []string{node.Name, node.Parent},
				Detail: "Frame " + node.Name + " names the parent " + node.Parent + ", which is not in the graph.",
			}, false
		}
	}

	// Every frame reaches the root by following parents. The walk is bounded by the node count, so a
	// cycle is a walk that ran out of frames rather than one that ran forever.
	byName := map[string]FrameNode{}
	for _, node := range graph.Nodes {
		byName[node.Name] = node
	}
	for _, node := range graph.Nodes {
		here := node
		for step := 0; step <= len(graph.Nodes); step++ {
			if here.IsRoot {
				break
			}
			here = byName[here.Parent]
		}
		if !here.IsRoot {
			return FrameFault{
				Code: "cycle", Subjects: []string{node.Name},
				Detail: "Following parents from " + node.Name + " never reaches the root.",
			}, false
		}
	}

	// The collision geometry, checked here rather than by the guard that consumes it: a shape nobody
	// can read presents as a limb quietly missing from the collision test, and an ignore list naming
	// a frame that does not exist presents as a pair that trips when the operator believed they had
	// excluded it.
	for _, node := range graph.Nodes {
		if shape := node.Shape; shape != nil {
			kind, isString := shape.Kind.Str()
			if !isString || !vocabulary.Contains(vocabulary.CollisionShapes, kind) {
				return FrameFault{
					Code: "unknown_shape", Subjects: []string{node.Name},
					Detail: "Frame " + node.Name + " declares collision geometry of kind " +
						js.ToString(shape.Kind, true) + ", which this edition cannot evaluate.",
				}, false
			}
			if !(canonical.Finite(shape.RadiusM) && shape.RadiusM > 0) {
				return FrameFault{
					Code: "unknown_shape", Subjects: []string{node.Name},
					Detail: "Frame " + node.Name + "'s collision geometry has radius " +
						js.NumberToString(shape.RadiusM) + ". A capsule of no radius bounds nothing, and a " +
						"guard built on it would report clearance it has not established.",
				}, false
			}
			if !node.Collision {
				return FrameFault{
					Code: "collision_mismatch", Subjects: []string{node.Name},
					Detail: "Frame " + node.Name + " declares a collision shape and says it carries no " +
						"collision geometry. One of the two is wrong and nothing here can tell which.",
				}, false
			}
		}
		for _, ignored := range node.CollisionIgnore {
			if !seen[ignored] {
				return FrameFault{
					Code: "ignore_missing", Subjects: []string{node.Name, ignored},
					Detail: "Frame " + node.Name + " is permitted to touch " + ignored + ", which is not one " +
						"of the frames. An exception naming nothing excludes nothing.",
				}, false
			}
		}
	}

	return FrameFault{}, true
}

// ValidateFrameBindings checks a frame graph against the joint list of the body it describes — the
// half of §44.3 ValidateFrames cannot see, because the two most important names in a dynamic frame
// refer to the joint list rather than to the graph.
//
// A name that does not resolve there presents far from its cause: the kernel binds nothing to that
// frame, `solve` marks it unresolved, and both pose guards trip on it every cycle for as long as the
// descriptor says what it says. This turns that permanent in-cycle trip into a refusal while somebody
// is still looking at the file.
//
// It does not require every declared joint to have a frame. A partial tree is legitimate, and
// "complete" means every name the graph uses resolves, not that the graph covers the body.
func ValidateFrameBindings(graph FrameGraph, joints []string) (FrameFault, bool) {
	declared := map[string]bool{}
	for _, joint := range joints {
		declared[joint] = true
	}

	for _, node := range graph.Nodes {
		if node.kindIs("dynamic") && node.HasJoint && !declared[node.Joint] {
			return FrameFault{
				Code: "dangling_joint", Subjects: []string{node.Name, node.Joint},
				Detail: "Frame " + node.Name + " moves with " + node.Joint + ", which the body does not " +
					"declare. Nothing binds it, so its pose never resolves and both pose guards trip on " +
					"it every cycle.",
			}, false
		}

		hasCoefficient := node.HasMimicMultiplier || node.HasMimicOffset
		if !node.HasMimics {
			if hasCoefficient {
				return FrameFault{
					Code: "mimic_coefficient", Subjects: []string{node.Name},
					Detail: "Frame " + node.Name + " declares a mimic multiplier or offset and no `mimics`. " +
						"A coefficient with nothing to multiply describes a linkage that does not exist.",
				}, false
			}
			continue
		}

		if !declared[node.Mimics] {
			return FrameFault{
				Code: "mimic_missing", Subjects: []string{node.Name, node.Mimics},
				Detail: "Frame " + node.Name + " follows " + node.Mimics + ", which the body does not " +
					"declare. A linkage read from a joint nobody drives never resolves, and takes every " +
					"frame below it with it.",
			}, false
		}

		// `node.joint === mimics`: an absent joint is undefined, which equals no string.
		if node.HasJoint && node.Joint == node.Mimics {
			return FrameFault{
				Code: "mimic_self", Subjects: []string{node.Name, node.Mimics},
				Detail: "Frame " + node.Name + " follows the joint it is driven by. " +
					"`value = multiplier × driver + offset` over one value is a fixed point, not a linkage.",
			}, false
		}

		for _, coefficient := range []struct {
			label   string
			value   float64
			present bool
		}{
			{"mimic_multiplier", node.MimicMultiplier, node.HasMimicMultiplier},
			{"mimic_offset", node.MimicOffset, node.HasMimicOffset},
		} {
			if coefficient.present && !canonical.Finite(coefficient.value) {
				return FrameFault{
					Code: "mimic_coefficient", Subjects: []string{node.Name},
					Detail: "Frame " + node.Name + " declares " + coefficient.label + " as a value that is " +
						"not a finite number. A linkage computed from it produces a pose that is not one.",
				}, false
			}
		}
	}

	return FrameFault{}, true
}

// HasPeopleDetection is whether any sensor on the body declares it reports person distance (§5,
// §7.1's `proximity`).
//
// `detects_people === true`, and nothing else: the string "yes" and the number 1 are refused, which
// is the fail-closed direction on a guard input — a malformed declaration leaves the guard
// unavailable and naming what it wants, rather than active on a body nobody has established can see
// a person. A declaration is honoured whatever the sensor's modality, because only the vendor knows
// what their pipeline measures.
func HasPeopleDetection(descriptor canonical.Value) bool {
	for _, sensor := range descriptor.Field("sensors").Items() {
		if isTrue(sensor.Field("detects_people")) {
			return true
		}
	}
	return false
}

// shapeFrameFault is `{ fault: null }` when the graph is usable, and the fault otherwise — nullable
// rather than optional, because "this graph is usable" is a fact worth stating.
func shapeFrameFault(fault FrameFault, usable bool) canonical.Value {
	if usable {
		return canonical.Object().Set("fault", canonical.Null())
	}
	return canonical.Object().Set("fault", canonical.Object().
		Set("code", canonical.String(fault.Code)).
		Set("subjects", canonical.Strings(fault.Subjects)).
		Set("detail", canonical.String(fault.Detail)))
}

func framesValidate(input canonical.Value) (canonical.Value, error) {
	graph, err := ReadFrameGraph(input.Field("graph"))
	if err != nil {
		return canonical.Null(), err
	}
	return shapeFrameFault(ValidateFrames(graph)), nil
}

func framesBindings(input canonical.Value) (canonical.Value, error) {
	graph, err := ReadFrameGraph(input.Field("graph"))
	if err != nil {
		return canonical.Null(), err
	}
	return shapeFrameFault(ValidateFrameBindings(graph, stringsOf(input.Field("joints")))), nil
}

func embodimentDetection(input canonical.Value) (canonical.Value, error) {
	return canonical.Object().Set("people_detection", canonical.Bool(HasPeopleDetection(input))), nil
}

// ── transport, §6 ───────────────────────────────────────────────────────────
//
// How a policy's action space reaches a body. Pure — no database, no filesystem, no clock — which is
// what lets the Hub answer "does this run on my G1?" before a download and a disconnected robot reach
// the same answer with no network. It decides what is possible and under what ceilings, and never
// invents a number: a body with no declared envelope produces a plan that says so.

// jointActionKinds are the channel kinds bound to a joint rather than to a facility of the body.
var jointActionKinds = []string{"joint_position", "joint_velocity", "joint_torque"}

// TransportWarning is one thing an operator is told about a plan.
type TransportWarning struct {
	Code        string
	Message     string
	Subjects    []string
	HasSubjects bool
}

// ChannelBinding is how one action channel reaches this body.
type ChannelBinding struct {
	// SourceIndex and SourceRole are the channel's own values, carried through as written.
	SourceIndex canonical.Value
	SourceRole  canonical.Value
	Method      string
	Target      string
	HasTarget   bool
	// Scale multiplies the source value before it reaches the target; 1 is no scaling.
	Scale float64
	// Limit is the hard ceiling in the target's units, handed to the Safety Kernel.
	Limit maybeNumber
	// Mode is the control mode this binding commands its target joint in (§86.2), empty for a
	// binding that commands no joint — a gripper, a base, a retarget, a drop. Present on a blocked
	// binding too: which mode was asked for is what a reader needs in order to fix it.
	Mode   string
	Reason string
}

// ObservationBinding is how one stream the policy reads is supplied.
type ObservationBinding struct {
	Required  canonical.Value
	Method    string
	Target    canonical.Value
	HasTarget bool
	Reason    string
}

// TransportPlan is the whole answer.
type TransportPlan struct {
	Verdict      string
	RobotSlug    canonical.Value
	HasRobotSlug bool
	SourceDigest canonical.Value
	TargetDigest canonical.Value
	ControlMode  canonical.Value
	HasMode      bool
	ControlHz    maybeNumber
	Bindings     []ChannelBinding
	// Motions is present exactly when the policy declares motions (§84.3), and absent from a plan
	// over joint channels so every plan written before §84 is unchanged.
	Motions      []MotionBinding
	HasMotions   bool
	Observations []ObservationBinding
	// Clamp is nil where the body declares no envelope and no binding carries a limit.
	Clamp    canonical.Value
	HasClamp bool
	Warnings []TransportWarning
}

// transportJoint is one joint of the target body.
type transportJoint struct {
	name         string
	role         string
	limits       canonical.Value
	controlModes []string
}

// targetIndex is the body, indexed the three ways binding needs it.
type targetIndex struct {
	// byName is last-declaration-wins, as a Map set in order is.
	byName map[string]transportJoint
	// byRole is first-declaration-wins: a body declaring a role twice is a descriptor bug, and
	// preferring the later one would make the mapping depend on array order.
	byRole map[string]transportJoint
	chains map[string]bool
	// roles is every role key on the body, in first-declaration order, for the unmapped warning.
	roles []string
}

func readTransportJoints(target canonical.Value) []transportJoint {
	joints := []transportJoint{}
	for _, entry := range target.Field("joints").Items() {
		name, _ := entry.Field("name").Str()
		role, _ := entry.Field("role").Str()
		joints = append(joints, transportJoint{
			name: name, role: role, limits: entry.Field("limits"),
			controlModes: stringsOf(entry.Field("control_modes")),
		})
	}
	return joints
}

func indexTarget(joints []transportJoint) targetIndex {
	index := targetIndex{
		byName: map[string]transportJoint{}, byRole: map[string]transportJoint{}, chains: map[string]bool{},
	}
	for _, joint := range joints {
		index.byName[joint.name] = joint
		parsed, ok := ParseRole(joint.role)
		if !ok {
			continue
		}
		key := RoleKey(parsed)
		if _, taken := index.byRole[key]; !taken {
			index.byRole[key] = joint
		}
		index.chains[parsed.Chain] = true
		if !containsString(index.roles, key) {
			index.roles = append(index.roles, key)
		}
	}
	return index
}

// actionChannel is one channel of the policy's action space.
type actionChannel struct {
	index    canonical.Value
	role     canonical.Value
	roleText string
	kind     string
	joint    string
	peak     maybeNumber
	optional bool
}

func readActionChannels(source canonical.Value) []actionChannel {
	channels := []actionChannel{}
	for _, entry := range source.Field("action_channels").Items() {
		role := entry.Field("role")
		roleText, _ := role.Str()
		kind, _ := entry.Field("kind").Str()
		joint, _ := entry.Field("joint").Str()
		channels = append(channels, actionChannel{
			index: entry.Field("index"), role: role, roleText: roleText, kind: kind, joint: joint,
			peak: numberField(entry, "peak"), optional: truthyValue(entry.Field("optional")),
		})
	}
	return channels
}

// ResolveTransport resolves how a policy's action space reaches a body.
func ResolveTransport(source, target, options canonical.Value) TransportPlan {
	warnings := []TransportWarning{}
	warn := func(code, message string) {
		warnings = append(warnings, TransportWarning{Code: code, Message: message})
	}
	warnAbout := func(code, message string, subjects []string) {
		warnings = append(warnings, TransportWarning{Code: code, Message: message, Subjects: subjects, HasSubjects: true})
	}

	joints := readTransportJoints(target)
	index := indexTarget(joints)
	channels := readActionChannels(source)
	slug, hasSlug := target.Get("robot_slug")
	slugText := js.ToString(slug, hasSlug)
	control := target.Field("control")
	envelope := objectField(target, "envelope")
	space := actionSpaceOf(source, target)
	sourceMotions := source.Field("motions").Items()

	if !truthyValue(target.Field("verified")) {
		warnAbout("unverified_embodiment",
			"This body description is not signed by its vendor. Its joint ratings are unattested.", []string{slugText})
	}
	if len(channels) == 0 && len(sourceMotions) == 0 {
		warn("no_source_embodiment",
			"The policy declares no action space, so nothing about its fit to this body can be checked.")
	}
	// `target.envelope === null`, strictly: an absent envelope is not this warning, though it is no
	// envelope to buildClamp.
	if value, present := target.Get("envelope"); present && value.IsNull() {
		warnAbout("no_target_envelope",
			"No safety envelope has been declared for this robot. Per-joint ratings are the only ceilings available.",
			[]string{slugText})
	}
	if sourceDof, present := source.Get("dof"); present && !strictEqual(sourceDof, target.Field("dof")) {
		targetDof, hasTargetDof := target.Get("dof")
		warn("dof_mismatch",
			"The policy was trained on a "+js.ToString(sourceDof, true)+" degree-of-freedom body; this one has "+
				js.ToString(targetDof, hasTargetDof)+".")
	}
	mode, hasMode := source.Get("control_mode")
	modeText := js.ToString(mode, hasMode)
	// A control mode is how a *joint* is commanded, so only a joint plan is asked about one.
	if space == spaceJoints && !containsValue(control.Field("modes"), mode, hasMode) {
		warnAbout("control_mode_unsupported", "This body does not accept "+modeText+" commands.", []string{modeText})
	}
	if !control.Has("command_latency_ms") {
		warn("latency_undeclared",
			"This body declares no command latency, so loop stability at the policy rate cannot be predicted.")
	}

	sourceDigest := canonical.Null()
	if digest, present := presentNonNull(source, "digest"); present {
		sourceDigest = digest
	}
	targetDigest := canonical.Null()
	if digest, present := presentNonNull(target, "digest"); present {
		targetDigest = digest
	}

	// The two refusals that happen before anything is bound (§84.2, §84.3), and the motion plan.
	// Observations first and the rate second on both, as on the joint path, so their warnings
	// arrive in one order whatever the plan concluded.
	if space.refusesBeforeBinding() || space == spaceMotions {
		verdict := "refused"
		var motions []MotionBinding
		if space.refusesBeforeBinding() {
			warnings = append(warnings, actionSpaceWarning(space, slugText))
			motions = []MotionBinding{}
		} else {
			for _, motion := range sourceMotions {
				motions = append(motions, bindMotion(motion, target, warnAbout))
			}
			verdict = deriveMotionVerdict(motions)
		}
		observations := bindObservations(source.Field("observations"), target, warnAbout)
		controlHz := resolveControlHz(source, target, options, envelope, warn)
		clamp, hasClamp := buildClamp(envelope, nil, target)
		// No `unmapped_roles` on either: its sentence is that the controller *holds* the joints the
		// policy does not drive, and on a program-level body the controller is running the motion.
		return TransportPlan{
			Verdict: verdict, RobotSlug: slug, HasRobotSlug: hasSlug,
			SourceDigest: sourceDigest, TargetDigest: targetDigest,
			ControlMode: mode, HasMode: hasMode, ControlHz: controlHz,
			Bindings: []ChannelBinding{}, Motions: motions, HasMotions: len(sourceMotions) > 0,
			Observations: observations, Clamp: clamp, HasClamp: hasClamp, Warnings: warnings,
		}
	}

	// Retargeting is a property of a whole kinematic chain, never of one joint: an elbow cannot go
	// through task space while its shoulder goes direct. So the chains that need it are decided first.
	retargetChains := decideRetargetChains(channels, index, target, options, warnAbout)

	bindings := []ChannelBinding{}
	bound := &boundModes{}
	// §86.1. `=== true`, not truthiness: the reference compares to `true`, so a `1` is false here
	// and would be true under a looser reading — the trap §5 records for `detects_people`.
	perJointModes := isTrue(target.Field("control").Field("per_joint_modes"))
	for _, channel := range channels {
		bindings = append(bindings, bindChannel(channel, target, index, retargetChains, warnAbout, bound, perJointModes))
	}

	observations := bindObservations(source.Field("observations"), target, warnAbout)
	controlHz := resolveControlHz(source, target, options, envelope, warn)
	clamp, hasClamp := buildClamp(envelope, bindings, target)
	verdict := deriveVerdict(bindings)

	unmapped := []string{}
	for _, role := range index.roles {
		mapped := false
		for _, binding := range bindings {
			text, isString := binding.SourceRole.Str()
			if !isString || !js.Truthy(text) {
				continue
			}
			if parsed, ok := ParseRole(text); ok && RoleKey(parsed) == role {
				mapped = true
				break
			}
		}
		if !mapped {
			unmapped = append(unmapped, role)
		}
	}
	if len(unmapped) > 0 {
		// "1 joint … are" was the plural on the noun and not the verb, in both editions identically —
		// which is why 17 corpus cases carried it and none failed. A corpus detects editions
		// disagreeing; it cannot detect them agreeing about something wrong.
		phrase := "joints on this body are"
		if len(unmapped) == 1 {
			phrase = "joint on this body is"
		}
		warnAbout("unmapped_roles",
			js.NumberToString(float64(len(unmapped)))+" "+phrase+" not driven by the policy and will be held by the controller.",
			unmapped)
	}

	return TransportPlan{
		Verdict: verdict, RobotSlug: slug, HasRobotSlug: hasSlug,
		SourceDigest: sourceDigest, TargetDigest: targetDigest,
		ControlMode: mode, HasMode: hasMode, ControlHz: controlHz,
		Bindings: bindings, Observations: observations,
		Clamp: clamp, HasClamp: hasClamp, Warnings: warnings,
	}
}

// strictEqual is `===` between two parsed values of the kinds a descriptor's scalar fields hold.
func strictEqual(left, right canonical.Value) bool {
	if left.Kind() != right.Kind() {
		return false
	}
	switch left.Kind() {
	case canonical.KindNull:
		return true
	case canonical.KindBool:
		a, _ := left.Bool()
		b, _ := right.Bool()
		return a == b
	case canonical.KindNumber:
		a, _ := left.Number()
		b, _ := right.Number()
		return a == b
	case canonical.KindString:
		a, _ := left.Str()
		b, _ := right.Str()
		return a == b
	}
	// Two arrays or two objects are never `===` unless they are the same object, and two parsed
	// values never are.
	return false
}

// containsValue is `array.includes(value)` over a parsed array: SameValueZero, which for the strings
// a mode list holds is equality.
func containsValue(array, value canonical.Value, present bool) bool {
	for _, element := range array.Items() {
		if !present {
			continue
		}
		if strictEqual(element, value) {
			return true
		}
	}
	return false
}

// decideRetargetChains is which chains must go through task space, and whether this body can.
//
// A chain qualifies when one of its joint channels has no direct or role match. It is only granted
// when the caller asked for retargeting and the body has a solver — otherwise its channels fall
// through to `blocked`, which is the honest outcome.
func decideRetargetChains(
	channels []actionChannel, index targetIndex, target, options canonical.Value,
	warnAbout func(string, string, []string),
) map[string]bool {
	needs := []string{}
	for _, channel := range channels {
		if !containsString(jointActionKinds, channel.kind) {
			continue
		}
		if _, exact := index.byName[channel.joint]; js.Truthy(channel.joint) && exact {
			continue
		}
		parsed, ok := ParseRole(channel.roleText)
		if !ok {
			continue
		}
		if _, byRole := index.byRole[RoleKey(parsed)]; byRole {
			continue
		}
		// The chain exists here and this joint does not — a six-joint arm asked to run a seven-joint
		// policy, which is the case task space can rescue.
		if index.chains[parsed.Chain] && !containsString(needs, parsed.Chain) {
			needs = append(needs, parsed.Chain)
		}
	}
	if len(needs) == 0 {
		return map[string]bool{}
	}

	requested := options.Field("allowRetarget")
	if !isTrue(requested) || target.Field("solvers").Len() == 0 {
		// `options.allowRetarget` is strict for the grant and truthy for the sentence, as the
		// reference writes it.
		message := "Task-space retargeting would be required, and was not requested."
		if truthyValue(requested) {
			message = "Task-space retargeting is required but this body has no inverse-kinematics solver installed."
		}
		warnAbout("retarget_without_solver", message, needs)
		return map[string]bool{}
	}
	granted := map[string]bool{}
	for _, chain := range needs {
		granted[chain] = true
	}
	return granted
}

func binding(channel actionChannel, method string, target string, hasTarget bool, scale float64, limit maybeNumber, reason string) ChannelBinding {
	return ChannelBinding{
		SourceIndex: channel.index, SourceRole: channel.role, Method: method,
		Target: target, HasTarget: hasTarget, Scale: scale, Limit: limit, Reason: reason,
	}
}

// jointBinding is `binding` with §86.2's mode. Separate rather than a longer parameter list on
// `binding`, because a non-joint channel has no mode and the eleven callers that produce one should
// not have to write an empty string to say so.
func jointBinding(channel actionChannel, method string, target string, scale float64, limit maybeNumber, mode, reason string) ChannelBinding {
	out := binding(channel, method, target, true, scale, limit, reason)
	out.Mode = mode
	return out
}

// dropOrBlock is the two ends of a channel with no equivalent: dropped at scale 0 when the policy
// declares it optional, refused otherwise. Named separately because dropping an optional channel is a
// degradation an operator may accept and blocking a required one is a refusal.
func dropOrBlock(channel actionChannel, dropped, blocked string) ChannelBinding {
	if channel.optional {
		return binding(channel, "drop", "", false, 0, maybeNumber{}, dropped)
	}
	return binding(channel, "blocked", "", false, 1, maybeNumber{}, blocked)
}

func bindChannel(
	channel actionChannel, target canonical.Value, index targetIndex, retargetChains map[string]bool,
	warnAbout func(string, string, []string),
	bound *boundModes, perJointModes bool,
) ChannelBinding {
	if !containsString(jointActionKinds, channel.kind) {
		return bindNonJointChannel(channel, target)
	}

	// 1. Exact wire name. Two vendors agreeing on a joint name is rare and unambiguous.
	if js.Truthy(channel.joint) {
		if exact, found := index.byName[channel.joint]; found {
			return finishJointBinding(channel, exact, "direct", "Joint name matches exactly.", warnAbout, bound, perJointModes)
		}
	}

	// 2. Canonical role — the mechanism that makes bodies interoperable at all.
	parsed, hasRole := ParseRole(channel.roleText)
	if hasRole {
		key := RoleKey(parsed)
		if joint, found := index.byRole[key]; found {
			return finishJointBinding(channel, joint, "remap", "Mapped by role "+key+" to \""+joint.name+"\".", warnAbout, bound, perJointModes)
		}
		// 3. Task space, when the chain was granted it.
		if retargetChains[parsed.Chain] {
			return binding(channel, "retarget", "", false, 1, maybeNumber{},
				"Driven through task space on the "+parsed.Chain+" chain; this body has no "+key+".")
		}
	}

	// 4. Optional channels may be dropped; required ones are a refusal.
	blocked := "The channel declares no canonical role, so it can only match a joint of the same name, and none exists."
	if hasRole {
		blocked = "This body has no " + RoleKey(parsed) + " and the policy requires it."
	}
	return dropOrBlock(channel, "No equivalent on this body, and the policy declares the channel optional.", blocked)
}

func bindNonJointChannel(channel actionChannel, target canonical.Value) ChannelBinding {
	effectors := target.Field("end_effectors").Items()
	switch channel.kind {
	case "gripper":
		for _, effector := range effectors {
			kind, _ := effector.Field("kind").Str()
			if kind != "gripper" && kind != "dexterous_hand" {
				continue
			}
			name, _ := effector.Field("name").Str()
			limit := maybeNumber{}
			if _, present := presentNonNull(effector, "max_grip_force_n"); present {
				limit = numberField(effector, "max_grip_force_n")
			}
			return binding(channel, "remap", name, true, 1, limit, "Driven through the "+kind+" \""+name+"\".")
		}
		return dropOrBlock(channel,
			"This body has no gripper; the channel is optional.",
			"This body has no gripper and the policy requires one.")

	case "base_velocity":
		base, hasBase := target.Get("base")
		baseText, _ := base.Str()
		if baseText == "fixed" {
			return dropOrBlock(channel,
				"This body has a fixed base; the channel is optional.",
				"The policy commands base motion and this body has a fixed base.")
		}
		limit := maybeNumber{}
		if envelope := objectField(target, "envelope"); envelope.present {
			if _, present := presentNonNull(envelope.value, "base_velocity_max_m_s"); present {
				limit = envelope.number("base_velocity_max_m_s")
			}
		}
		// A wheeled base driven by a policy trained on a legged one is a synthesis, not a remap: the
		// command means the same thing, and nothing below the base interface is comparable.
		method := "synthesize"
		if baseText == "legged" {
			method = "remap"
		}
		return binding(channel, method, "base", true, scaleFor(channel.peak, limit), limit,
			"Base velocity applied to this "+js.ToString(base, hasBase)+" base.")

	case "cartesian":
		solvers := target.Field("solvers")
		for _, effector := range effectors {
			if kind, _ := effector.Field("kind").Str(); kind == "none" {
				continue
			}
			if solvers.Len() == 0 {
				break
			}
			name, _ := effector.Field("name").Str()
			return binding(channel, "retarget", name, true, 1, maybeNumber{},
				"Solved onto \""+name+"\" using "+js.ToString(solvers.At(0), true)+".")
		}
		return dropOrBlock(channel,
			"End-effector pose control is unavailable on this body; the channel is optional.",
			"End-effector pose control needs an end effector and a solver; this body has neither.")
	}

	// `discrete` — a mode switch or a named action. No physical units, so nothing here can bound it;
	// the package's capability grants are what gate it.
	return binding(channel, "direct", "", false, 1, maybeNumber{},
		"Discrete command passed through; it commands no actuator directly.")
}

// ratingFor is which joint rating bounds a channel of this kind.
func ratingFor(kind string, limits canonical.Value) maybeNumber {
	present := func(key string) maybeNumber {
		if _, found := presentNonNull(limits, key); !found {
			return maybeNumber{}
		}
		return numberField(limits, key)
	}
	switch kind {
	case "joint_velocity":
		return present("velocity_max_rad_s")
	case "joint_torque":
		return present("effort_max_nm")
	case "joint_position":
		// Bounded by travel rather than by one magnitude. This is the larger absolute bound, which
		// scaling and the rate limiter need; both ends travel beside it in the clamp's `travel`, which
		// is what the envelope guard holds (F159). `=== undefined`, so a declared null is a bound.
		if !limits.Has("position_max_rad") || !limits.Has("position_min_rad") {
			return maybeNumber{}
		}
		return maybeNumber{value: math.Max(
			math.Abs(numberField(limits, "position_max_rad").value),
			math.Abs(numberField(limits, "position_min_rad").value),
		), present: true}
	}
	return maybeNumber{}
}

func controlModeFor(kind string) (string, bool) {
	switch kind {
	case "joint_position":
		return "position", true
	case "joint_velocity":
		return "velocity", true
	case "joint_torque":
		return "torque", true
	case "cartesian":
		return "cartesian", true
	}
	return "", false
}

func finishJointBinding(
	channel actionChannel, joint transportJoint, method, reason string,
	warnAbout func(string, string, []string),
	bound *boundModes, perJointModes bool,
) ChannelBinding {
	rating := ratingFor(channel.kind, joint.limits)
	scale := scaleFor(channel.peak, rating)
	mode, hasMode := controlModeFor(channel.kind)

	if hasMode && !containsString(joint.controlModes, mode) {
		return jointBinding(channel, "blocked", joint.name, 1, rating, mode,
			"\""+joint.name+"\" does not accept "+mode+" commands.")
	}

	if hasMode {
		// §86.2. One joint, one mode, per cycle. A second channel onto a joint already bound in
		// another mode would meet it as one entry in `values` and one in `modes`, and the command
		// would carry whichever was written last. First-wins, as §84.3's binder does with a motion.
		already, hasAlready := bound.get(joint.name)
		if hasAlready && already != mode {
			warnAbout("joint_mode_conflict",
				"\""+joint.name+"\" is already bound in "+already+
					"; a joint carries one mode per cycle, so this "+mode+" channel is blocked.",
				[]string{joint.name})
			return jointBinding(channel, "blocked", joint.name, 1, rating, mode,
				"\""+joint.name+"\" is commanded in "+already+
					" by an earlier channel, and a joint takes one mode per cycle.")
		}

		// §86.1's other half. A driver that does not read the per-joint array takes one mode for
		// the whole command, so a channel in a second mode cannot be delivered to it at all.
		if elsewhere, found := bound.other(mode); found && !perJointModes {
			warnAbout("mixed_modes_unsupported",
				"This body is already commanded in "+elsewhere+
					" and its driver reads one mode per command, so the "+mode+" channel on \""+
					joint.name+"\" cannot be delivered.",
				[]string{joint.name})
			return jointBinding(channel, "blocked", joint.name, 1, rating, mode,
				"This driver does not declare per-joint modes (\u00a786.1), and the body is already commanded in "+
					elsewhere+".")
		}

		if !hasAlready {
			bound.set(joint.name, mode)
		}
	}

	if scale < 1 {
		warnAbout("scaled_down",
			"Commands to \""+joint.name+"\" are scaled to "+js.ToFixed(float64(scale*100), 0)+
				"% because the policy was trained on a stronger joint.", []string{joint.name})
	} else if channel.peak.present && !rating.present {
		warnAbout("unscalable_channel",
			"\""+joint.name+"\" declares no rating for this control mode, so the policy's commands cannot be scaled to it.",
			[]string{joint.name})
	}
	// §7.1, F159. A position channel is held to the joint's travel, and a travel needs both ends. A
	// joint that declares one has neither a travel nor a magnitude, so the kernel bounds its position
	// by nothing — said here, per channel. `=== undefined`, so a declared null is an end.
	if hasMode && mode == "position" && joint.limits.Has("position_min_rad") != joint.limits.Has("position_max_rad") {
		warnAbout("travel_one_end",
			"\""+joint.name+"\" declares one end of its travel and not the other, so the Safety Kernel bounds its position by nothing.",
			[]string{joint.name})
	}
	return jointBinding(channel, method, joint.name, scale, rating, mode, reason)
}

// boundModes is §86.2's record of which joint is already commanded in which mode, in binding order.
// A slice rather than a map because the order decides which channel survives a conflict, and Go's
// map iteration is deliberately random.
type boundModes struct {
	entries []struct {
		joint string
		mode  string
	}
}

func (b *boundModes) get(joint string) (string, bool) {
	for _, entry := range b.entries {
		if entry.joint == joint {
			return entry.mode, true
		}
	}
	return "", false
}

// other is the first mode bound on any joint that is not `mode`.
func (b *boundModes) other(mode string) (string, bool) {
	for _, entry := range b.entries {
		if entry.mode != mode {
			return entry.mode, true
		}
	}
	return "", false
}

func (b *boundModes) set(joint, mode string) {
	b.entries = append(b.entries, struct {
		joint string
		mode  string
	}{joint, mode})
}

// scaleFor is how far a channel must be scaled to stay inside a rating. 1 when either number is
// missing: guessing a ratio from one known side would be inventing the other.
func scaleFor(peak, rating maybeNumber) float64 {
	if !peak.present || !rating.present || peak.value <= 0 {
		return 1
	}
	return math.Min(1, rating.value/peak.value)
}

func bindObservations(required, target canonical.Value, warnAbout func(string, string, []string)) []ObservationBinding {
	sensors := target.Field("sensors").Items()
	out := []ObservationBinding{}
	for _, requirement := range required.Items() {
		name, hasName := requirement.Get("name")
		modality, hasModality := requirement.Get("modality")
		optional := truthyValue(requirement.Field("optional"))

		var candidate canonical.Value
		found, exact := false, false
		for _, sensor := range sensors {
			sensorName, sensorHasName := sensor.Get("name")
			sensorModality, sensorHasModality := sensor.Get("modality")
			if sameField(sensorName, sensorHasName, name, hasName) && sameField(sensorModality, sensorHasModality, modality, hasModality) {
				candidate, found, exact = sensor, true, true
				break
			}
		}
		if !found {
			candidate, found = matchSensorByRole(requirement, sensors)
		}

		if !found {
			if !optional {
				warnAbout("observation_missing",
					"The policy reads a "+js.ToString(modality, hasModality)+" stream called \""+
						js.ToString(name, hasName)+"\" and this body has none.",
					[]string{js.ToString(name, hasName)})
			}
			reason := "Not present on this body."
			if optional {
				reason = "Not present on this body; the policy declares it optional."
			}
			out = append(out, ObservationBinding{Required: name, Method: "missing", Reason: reason})
			continue
		}

		candidateName, candidateHasName := candidate.Get("name")
		degraded := false
		for _, dimension := range []string{"width", "height", "fps"} {
			wanted := numberField(requirement, dimension)
			if !wanted.present {
				continue
			}
			// `(candidate.width ?? 0) < requirement.width`.
			have := 0.0
			if _, present := presentNonNull(candidate, dimension); present {
				have = numberField(candidate, dimension).value
			}
			if have < wanted.value {
				degraded = true
				break
			}
		}
		if degraded {
			warnAbout("observation_degraded",
				"\""+js.ToString(candidateName, candidateHasName)+
					"\" is lower resolution or slower than the stream the policy was trained on.",
				[]string{js.ToString(candidateName, candidateHasName)})
			out = append(out, ObservationBinding{
				Required: name, Method: "downscale", Target: candidateName, HasTarget: true,
				Reason: "Present but below the trained resolution or frame rate; frames are upscaled.",
			})
			continue
		}
		method, reason := "remap", "Matched by role to \""+js.ToString(candidateName, candidateHasName)+"\"."
		if exact {
			method, reason = "direct", "Sensor name and modality match."
		}
		out = append(out, ObservationBinding{Required: name, Method: method, Target: candidateName, HasTarget: true, Reason: reason})
	}
	return out
}

// sameField is `a === b` where either side may be absent: undefined equals only undefined.
func sameField(left canonical.Value, leftPresent bool, right canonical.Value, rightPresent bool) bool {
	if !leftPresent || !rightPresent {
		return leftPresent == rightPresent
	}
	return strictEqual(left, right)
}

func matchSensorByRole(requirement canonical.Value, sensors []canonical.Value) (canonical.Value, bool) {
	roleText, _ := requirement.Field("role").Str()
	parsed, ok := ParseRole(roleText)
	if !ok {
		return canonical.Null(), false
	}
	key := RoleKey(parsed)
	modality, hasModality := requirement.Get("modality")
	for _, sensor := range sensors {
		sensorModality, sensorHasModality := sensor.Get("modality")
		if !sameField(sensorModality, sensorHasModality, modality, hasModality) {
			continue
		}
		sensorRole, _ := sensor.Field("role").Str()
		if sensorParsed, ok := ParseRole(sensorRole); ok && RoleKey(sensorParsed) == key {
			return sensor, true
		}
	}
	return canonical.Null(), false
}

// resolveControlHz is the rate the loop will actually run at: clamped down to what the body
// accepts, never up. The watchdog check is the one that matters most — a loop slower than the
// controller's timeout faults while the robot is moving.
func resolveControlHz(source, target, options canonical.Value, envelope optionalObject, warn func(string, string)) maybeNumber {
	// `options.requestedHz ?? source.control_hz`: the caller's rate when it gave one, the policy's
	// otherwise, and no rate at all when neither says.
	requested := maybeNumber{}
	if _, present := presentNonNull(options, "requestedHz"); present {
		requested = numberField(options, "requestedHz")
	} else if _, present := presentNonNull(source, "control_hz"); present {
		requested = numberField(source, "control_hz")
	}
	if !requested.present {
		return maybeNumber{}
	}

	resolved := requested.value
	if maximum := numberField(target.Field("control"), "rate_hz_max"); maximum.present && requested.value > maximum.value {
		resolved = maximum.value
		warn("rate_unsupported",
			"The policy runs at "+js.NumberToString(requested.value)+" Hz; this body accepts at most "+
				js.NumberToString(maximum.value)+" Hz, so the loop is clamped.")
	}

	if watchdog := envelope.number("watchdog_ms"); watchdog.present && watchdog.value > 0 {
		if resolved < 1000/watchdog.value {
			warn("rate_below_watchdog",
				"At "+js.NumberToString(resolved)+" Hz the controller's "+js.NumberToString(watchdog.value)+
					" ms watchdog fires between commands.")
		}
	}
	return maybeNumber{value: resolved, present: true}
}

// buildClamp is the ceilings the Safety Kernel enforces, from the envelope and the bindings. Null
// where there is neither an envelope nor a single bound channel.
func buildClamp(envelope optionalObject, bindings []ChannelBinding, target canonical.Value) (canonical.Value, bool) {
	perChannel := canonical.Object()
	jointModes := canonical.Object()
	travel := canonical.Object()
	for _, entry := range bindings {
		// A blocked binding commands nothing, so it contributes neither a ceiling nor a mode. It
		// carries `Limit` — the rating of the joint it could not command — so a reader can see what
		// was on offer, and that is the only thing it is for.
		if entry.Method == "blocked" {
			continue
		}
		if entry.HasTarget && js.Truthy(entry.Target) && entry.Limit.present {
			perChannel = perChannel.Set(entry.Target, canonical.Number(entry.Limit.value))
		}
		// §86.3. Without this the kernel cannot read a ceiling in the right units.
		if entry.HasTarget && js.Truthy(entry.Target) && entry.Mode != "" {
			jointModes = jointModes.Set(entry.Target, canonical.String(entry.Mode))
		}
		// §7.1, F159. Both ends of a position joint's travel. `find`, so the first joint of the name.
		if entry.HasTarget && js.Truthy(entry.Target) && entry.Mode == "position" {
			for _, joint := range target.Field("joints").Items() {
				if name, _ := joint.Field("name").Str(); name != entry.Target {
					continue
				}
				low, hasLow := joint.Field("limits").Get("position_min_rad")
				high, hasHigh := joint.Field("limits").Get("position_max_rad")
				if hasLow && hasHigh {
					travel = travel.Set(entry.Target, canonical.Array(low, high))
				}
				break
			}
		}
	}
	if !envelope.present && perChannel.Len() == 0 && jointModes.Len() == 0 && travel.Len() == 0 {
		return canonical.Null(), false
	}

	// `...(x !== undefined && { k: x })`, then the shape's `optional`, which drops a null as well:
	// the net rule is present-and-not-null.
	clamp := canonical.Object()
	if envelope.present {
		for _, key := range []string{"joint_velocity_max_rad_s", "joint_torque_max_nm", "base_velocity_max_m_s"} {
			if value, present := presentNonNull(envelope.value, key); present {
				clamp = clamp.Set(key, value)
			}
		}
		if watchdog := envelope.number("watchdog_ms"); watchdog.present && watchdog.value > 0 {
			clamp = clamp.Set("min_control_hz", canonical.Number(1000/watchdog.value))
		}
	}
	clamp = clamp.Set("per_channel", perChannel)
	// Absent — not empty — on a plan that binds no joint, so a motion plan and a plan over a gripper
	// and a base are the same bytes they were before §86.
	if jointModes.Len() > 0 {
		clamp = clamp.Set("joint_modes", jointModes)
	}
	// Absent from a plan that binds no joint in position, for the reason `joint_modes` is.
	if travel.Len() > 0 {
		clamp = clamp.Set("travel", travel)
	}
	if envelope.present {
		if value, present := presentNonNull(envelope.value, "source"); present {
			clamp = clamp.Set("source", value)
		}
	}
	return clamp, true
}

// deriveVerdict is the worst binding in the plan. One blocked required channel refuses the whole
// plan, because a policy missing an actuator it depends on does something nobody has characterised.
func deriveVerdict(bindings []ChannelBinding) string {
	has := func(methods ...string) bool {
		for _, entry := range bindings {
			if containsString(methods, entry.Method) {
				return true
			}
		}
		return false
	}
	switch {
	case has("blocked"):
		return "refused"
	case has("drop"):
		return "degraded"
	case has("retarget", "synthesize"):
		return "retargeted"
	}
	return "direct"
}

// shapeTransportPlan is the plan on the wire. Nullable fields are present and null; optional ones are absent.
func shapeTransportPlan(plan TransportPlan) canonical.Value {
	bindings := canonical.Array()
	for _, entry := range plan.Bindings {
		role := entry.SourceRole
		bindings = bindings.Push(canonical.Object().
			Set("source_index", entry.SourceIndex).
			Set("source_role", role).
			Set("method", canonical.String(entry.Method)).
			Set("target", optionalString(entry.Target, entry.HasTarget)).
			Set("scale", canonical.Number(entry.Scale)).
			Set("limit", entry.Limit.nullable()).
			Set("reason", canonical.String(entry.Reason)))
	}
	observations := canonical.Array()
	for _, entry := range plan.Observations {
		target := canonical.Null()
		if entry.HasTarget {
			target = entry.Target
		}
		observations = observations.Push(canonical.Object().
			Set("required", entry.Required).
			Set("method", canonical.String(entry.Method)).
			Set("target", target).
			Set("reason", canonical.String(entry.Reason)))
	}
	warnings := canonical.Array()
	for _, entry := range plan.Warnings {
		warnings = warnings.Push(canonical.Object().
			Set("code", canonical.String(entry.Code)).
			Set("message", canonical.String(entry.Message)).
			SetIf("subjects", canonical.Strings(entry.Subjects), entry.HasSubjects))
	}
	clamp := canonical.Null()
	if plan.HasClamp {
		clamp = plan.Clamp
	}
	return canonical.Object().
		Set("verdict", canonical.String(plan.Verdict)).
		SetIf("robot_slug", plan.RobotSlug, plan.HasRobotSlug).
		Set("source_digest", plan.SourceDigest).
		Set("target_digest", plan.TargetDigest).
		SetIf("control_mode", plan.ControlMode, plan.HasMode).
		Set("control_hz", plan.ControlHz.nullable()).
		Set("bindings", bindings).
		SetIf("motions", shapeMotionBindings(plan.Motions), plan.HasMotions).
		Set("observations", observations).
		Set("clamp", clamp).
		Set("warnings", warnings)
}

func transportResolve(input canonical.Value) (canonical.Value, error) {
	return shapeTransportPlan(ResolveTransport(input.Field("source"), input.Field("target"), input.Field("options"))), nil
}

// optionalString is a value the reference writes as `x ?? null`.
func optionalString(value string, present bool) canonical.Value {
	if !present {
		return canonical.Null()
	}
	return canonical.String(value)
}

// optionalNumber is the same for a number.
func optionalNumber(value float64, present bool) canonical.Value {
	if !present {
		return canonical.Null()
	}
	return canonical.Number(value)
}
