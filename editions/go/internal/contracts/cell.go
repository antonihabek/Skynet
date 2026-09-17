package contracts

import (
	"sort"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The cell descriptor (§55.2) — the physical counterpart of an environment. A profile document, not
// a repo kind: site-specific, changed when somebody moves a table, nobody's intellectual property. It
// does two jobs — it supplies `env.*` to the precondition context (§54.2), and one layer of allowed
// volumes to the `workspace` guard (§7.1's fourth) — and this file is those two projections plus the
// check that makes them trustworthy.

// CellFault is one thing wrong with a cell descriptor, naming what it is about.
type CellFault struct {
	Code     string
	Subjects []string
	Detail   string
}

// usableKey is a key a dot path can address: non-empty, no dot, and no leading or trailing
// whitespace as ECMAScript trims it. `env.door.closed` is the path to `closed` inside `door`, so a
// fact keyed "door.closed" is one no precondition can ever name.
func usableKey(key string) bool {
	return key != "" && !containsByte(key, '.') && js.Trim(key) == key
}

func containsByte(text string, character byte) bool {
	for at := 0; at < len(text); at++ {
		if text[at] == character {
			return true
		}
	}
	return false
}

// finiteNumberValue is `Number.isFinite(value)` over a parsed value: no coercion, so the string "1"
// is not a finite number and neither is null.
func finiteNumberValue(value canonical.Value) bool {
	number, isNumber := value.Number()
	return isNumber && canonical.Finite(number)
}

func everyFinite(array canonical.Value) bool {
	for _, element := range array.Items() {
		if !finiteNumberValue(element) {
			return false
		}
	}
	return true
}

// shapeIsUsable is whether a volume has geometry the workspace guard can evaluate. A box whose
// minimum exceeds its maximum on any axis is empty, and an empty allowed volume is a configuration
// mistake rather than a lock-out to enforce silently.
func shapeIsUsable(volume canonical.Value) bool {
	shape := volume.Field("shape")
	if shape.Kind() != canonical.KindObject && shape.Kind() != canonical.KindArray {
		return false
	}
	kind, _ := shape.Field("kind").Str()
	switch kind {
	case "sphere":
		radius, _ := shape.Field("radius_m").Number()
		centre := shape.Field("centre")
		return finiteNumberValue(shape.Field("radius_m")) && radius > 0 &&
			centre.Kind() == canonical.KindArray && everyFinite(centre)
	case "box":
		low, high := shape.Field("min"), shape.Field("max")
		if low.Kind() != canonical.KindArray || high.Kind() != canonical.KindArray {
			return false
		}
		if !everyFinite(low) || !everyFinite(high) {
			return false
		}
		for axis, element := range low.Items() {
			value, _ := element.Number()
			ceiling, isNumber := high.At(axis).Number()
			// `value <= shape.max[axis]`: an axis the maximum does not have is `undefined`, and
			// nothing is less than or equal to that.
			if !isNumber || !(value <= ceiling) {
				return false
			}
		}
		return true
	}
	return false
}

// namedString is the reference's test for a name: a string, and not blank once trimmed.
func namedString(value canonical.Value) (string, bool) {
	text, isString := value.Str()
	return text, isString && js.Trim(text) != ""
}

// ── §87.1's intersection test ────────────────────────────────────────────────

// placedShape is a shape moved into the cell's frame by its robot's placement.
//
// Translation only. §44.4's transforms carry a rotation and this deliberately does not read one: a
// rotated box is not a box, so the test would need an oriented-box routine whose disagreements
// between eight editions would be in the last bits of a square root. A cell whose robots are mounted
// at an angle is one whose operator draws axis-aligned volumes large enough to hold the rotation,
// which over-reports sharing and never under-reports it.
type placedShape struct {
	Kind   string
	Frame  string
	Centre [3]float64
	Radius float64
	Min    [3]float64
	Max    [3]float64
}

func triple(value canonical.Value) ([3]float64, bool) {
	out := [3]float64{}
	items := value.Items()
	if value.Kind() != canonical.KindArray || len(items) != 3 {
		return out, false
	}
	for axis, element := range items {
		number, isNumber := element.Number()
		if !isNumber || !canonical.Finite(number) {
			return out, false
		}
		out[axis] = number
	}
	return out, true
}

// readPlacement is the placement, and not ok when the operator wrote none the arithmetic can use.
func readPlacement(robot canonical.Value) ([3]float64, bool) {
	return triple(robot.Field("at"))
}

// placeVolume is one usable volume of a placed robot, in the cell's frame.
func placeVolume(volume canonical.Value, at [3]float64) (placedShape, bool) {
	shape := volume.Field("shape")
	kind, _ := shape.Field("kind").Str()
	frame, _ := volume.Field("frame").Str()
	if kind == "sphere" {
		centre, ok := triple(shape.Field("centre"))
		radius, isNumber := shape.Field("radius_m").Number()
		if !ok || !isNumber {
			return placedShape{}, false
		}
		for axis := 0; axis < 3; axis++ {
			centre[axis] += at[axis]
		}
		return placedShape{Kind: "sphere", Frame: frame, Centre: centre, Radius: radius}, true
	}
	low, lowOk := triple(shape.Field("min"))
	high, highOk := triple(shape.Field("max"))
	if !lowOk || !highOk {
		return placedShape{}, false
	}
	for axis := 0; axis < 3; axis++ {
		low[axis] += at[axis]
		high[axis] += at[axis]
	}
	return placedShape{Kind: "box", Frame: frame, Min: low, Max: high}, true
}

// boxPointDistanceSquared is the squared distance from a point to a box, zero inside.
func boxPointDistanceSquared(low, high, point [3]float64) float64 {
	total := 0.0
	for axis := 0; axis < 3; axis++ {
		outside := 0.0
		if point[axis] < low[axis] {
			outside = low[axis] - point[axis]
		} else if point[axis] > high[axis] {
			outside = point[axis] - high[axis]
		}
		// `float64(…)` around the product, and the self-check is what asks for it: Go's optimiser
		// folds a multiply into the following add on six of the architectures it targets, and a
		// fused multiply-add rounds once where the reference rounds twice. The difference is in the
		// last bit of a distance, and the comparison it feeds is an equality against a sum of radii.
		total += float64(outside * outside)
	}
	return total
}

// intersects is whether two placed shapes share a point.
//
// **Inclusive, and that is the decision rather than an off-by-one.** `margin` counts zero as inside
// — a workspace trip is `margin < 0` — so two robots whose boxes meet on a face can each legally be
// at the point where they meet. A strict test would call that configuration disjoint, and the
// platform would have certified the one arrangement where both arms are allowed into the same
// millimetre.
func intersects(a, b placedShape) bool {
	if a.Kind == "box" && b.Kind == "box" {
		for axis := 0; axis < 3; axis++ {
			if a.Min[axis] > b.Max[axis] || b.Min[axis] > a.Max[axis] {
				return false
			}
		}
		return true
	}
	if a.Kind == "sphere" && b.Kind == "sphere" {
		reach := a.Radius + b.Radius
		squared := 0.0
		for axis := 0; axis < 3; axis++ {
			difference := a.Centre[axis] - b.Centre[axis]
			// Rounded for the reason above: two spheres exactly touching is a case in this corpus,
			// and it is decided by the last bit of this sum.
			squared += float64(difference * difference)
		}
		return squared <= reach*reach
	}
	box, sphere := a, b
	if a.Kind == "sphere" {
		box, sphere = b, a
	}
	return boxPointDistanceSquared(box.Min, box.Max, sphere.Centre) <= sphere.Radius*sphere.Radius
}

// placedVolumes is a robot's usable volumes, placed. A volume with no usable shape is not compared.
func placedVolumes(robot canonical.Value) []placedShape {
	at, ok := readPlacement(robot)
	if !ok {
		return nil
	}
	placed := []placedShape{}
	for _, volume := range robot.Field("allowed").Items() {
		if !shapeIsUsable(volume) {
			continue
		}
		if shape, ok := placeVolume(volume, at); ok {
			placed = append(placed, shape)
		}
	}
	return placed
}

// robotFaults is §87.1's five faults, or none.
//
// Split out rather than inlined because all five are conditional on the same thing — **two or more
// robots** — and a reader has to be able to see that condition once. One robot is §55.2's cell and
// has no pair to be disjoint from; the placement rule would otherwise refuse every cell anybody has
// ever written.
//
// Reported in declaration order, then the pairs in index order, so every edition names the same
// fault first. The disjointness walk skips a robot that already has a fault of its own, for
// ValidateCell's duplicate-fixture reason.
func robotFaults(cell canonical.Value) []CellFault {
	faults := []CellFault{}
	fault := func(code string, subjects []string, detail string) {
		faults = append(faults, CellFault{Code: code, Subjects: subjects, Detail: detail})
	}

	robots := cell.Field("robots").Items()
	several := len(robots) >= 2
	seen := map[string]bool{}
	type comparableRobot struct {
		Slug    string
		Volumes []placedShape
	}
	comparable := []comparableRobot{}

	for index, robot := range robots {
		slug, named := namedString(robot.Field("slug"))
		if !named {
			fault("robot_unnamed", []string{"robots[" + js.NumberToString(float64(index)) + "]"},
				"A robot a precondition cannot name is a robot no skill can ask about. "+
					"`env.robot` is its slug.")
			continue
		}
		if seen[slug] {
			fault("duplicate_robot", []string{slug},
				"Two entries with one slug: a precondition asking about it would be asking about "+
					"whichever the reader happened to reach.")
			continue
		}
		seen[slug] = true

		// Everything below is about the pair, so a cell with one robot stops here.
		if !several {
			continue
		}

		if _, placed := readPlacement(robot); !placed {
			fault("robot_unplaced", []string{slug},
				"This cell holds more than one robot, and this one carries no `at`. Its volumes "+
					"are in its own root frame (§44.2), so nothing can say whether they overlap "+
					"another robot's until somebody says where it stands.")
			continue
		}
		volumes := placedVolumes(robot)
		if len(volumes) == 0 {
			fault("robot_unbounded", []string{slug},
				"This cell holds more than one robot, and this one draws no volume the workspace "+
					"guard can evaluate. A robot that may be anywhere is not disjoint from "+
					"anything.")
			continue
		}
		comparable = append(comparable, comparableRobot{Slug: slug, Volumes: volumes})
	}

	for a := 0; a < len(comparable); a++ {
		for b := a + 1; b < len(comparable); b++ {
			first, second := comparable[a], comparable[b]
			for _, one := range first.Volumes {
				for _, other := range second.Volumes {
					if !intersects(one, other) {
						continue
					}
					fault("shared_volume",
						[]string{first.Slug, second.Slug, one.Frame, other.Frame},
						first.Slug+"'s "+one.Frame+" and "+second.Slug+"'s "+other.Frame+
							" share a point once both robots are placed. §87.1 gives each robot "+
							"its own kernel, so nothing on this machine evaluates the pair — the "+
							"overlap is a claim the platform cannot enforce rather than a hazard "+
							"it has measured.")
				}
			}
		}
	}

	return faults
}

// ValidateCell is every fault in a cell descriptor — a list rather than the first one, because a
// cell's problems are independent and an operator is editing it by hand.
func ValidateCell(cell canonical.Value) []CellFault {
	faults := []CellFault{}
	fault := func(code string, subjects []string, detail string) {
		faults = append(faults, CellFault{Code: code, Subjects: subjects, Detail: detail})
	}

	if _, named := namedString(cell.Field("name")); !named {
		fault("unnamed", []string{}, "A cell has no identity but its name, and this one has none.")
	}

	// Fact keys in sorted order, by UTF-16 code unit as `.sort()` compares them. A compatibility
	// decision rather than a tidy one: the Rust edition holds the facts in a BTreeMap with no insertion
	// order to offer, and a canonical document's key order is sorted anyway.
	keys := []string{}
	for _, member := range cell.Field("facts").Members() {
		keys = append(keys, member.Key)
	}
	sort.SliceStable(keys, func(left, right int) bool { return utf16Less(keys[left], keys[right]) })
	for _, key := range keys {
		if !usableKey(key) {
			fault("fact_key_unusable", []string{key},
				"A precondition addresses this as env."+key+", and a dot path cannot name it. "+
					"Nest the object instead of putting a dot in the key.")
			continue
		}
		if vocabulary.Contains(vocabulary.DerivedFacts, key) {
			fault("fact_key_reserved", []string{key},
				"env."+key+" is derived from the cell itself, so a fact of that name would be "+
					"read by nobody and would make a precondition true for the wrong reason.")
		}
	}

	for index, volume := range cell.Field("allowed").Items() {
		subject := "allowed[" + js.NumberToString(float64(index)) + "]"
		if _, named := namedString(volume.Field("frame")); !named {
			fault("volume_frame_missing", []string{subject},
				"A volume constrains a frame, and this one names none. The workspace guard would "+
					"have nothing to check it against.")
		}
		if !shapeIsUsable(volume) {
			fault("volume_shape_invalid", []string{subject},
				"This volume has no usable geometry — a box whose minimum exceeds its maximum, a "+
					"sphere with no radius, or a shape this edition does not know.")
		}
	}

	fixtures := map[string]bool{}
	for index, fixture := range cell.Field("fixtures").Items() {
		name, named := namedString(fixture.Field("name"))
		if !named {
			fault("fixture_unnamed", []string{"fixtures[" + js.NumberToString(float64(index)) + "]"},
				"A fixture a precondition cannot name is a fixture the OS cannot check for.")
			continue
		}
		if fixtures[name] {
			fault("duplicate_fixture", []string{name},
				"Two fixtures with one name: a precondition asking for it would be asking about "+
					"whichever the reader happened to reach.")
			continue
		}
		fixtures[name] = true
		if volume := fixture.Field("volume"); truthyValue(volume) && !shapeIsUsable(volume) {
			fault("volume_shape_invalid", []string{name},
				"This fixture has been given a volume with no usable geometry. A fixture may be "+
					"named without being measured; it may not be measured wrongly.")
		}
	}

	channels := map[string]bool{}
	for index, entry := range cell.Field("parking_pose").Items() {
		channel, named := namedString(entry.Field("channel"))
		if !named {
			fault("parking_channel_unnamed", []string{"parking_pose[" + js.NumberToString(float64(index)) + "]"},
				"A parking pose is a command, and a command names its channel.")
			continue
		}
		if channels[channel] {
			fault("duplicate_parking_channel", []string{channel},
				"One channel, two parking positions. Which one the robot drives to would depend on "+
					"the order a reader happened to walk the list.")
			continue
		}
		channels[channel] = true
	}

	faults = append(faults, robotFaults(cell)...)

	return faults
}

// cellName is the cell's name, or the empty string when it has none that is a string.
func cellName(cell canonical.Value) string {
	name, _ := cell.Field("name").Str()
	return name
}

// CellEnvironment is the `env.*` branch of a precondition context: the operator's facts, then two
// derived ones they may not shadow. A cell with faults still produces a context, because an
// operator's typo in a fixture name must not take away the battery check as well.
func CellEnvironment(cell canonical.Value, subject string, hasSubject bool) canonical.Value {
	environment := canonical.Object()
	for _, member := range cell.Field("facts").Members() {
		if usableKey(member.Key) && !vocabulary.Contains(vocabulary.DerivedFacts, member.Key) {
			environment = environment.Set(member.Key, member.Value)
		}
	}
	names := []string{}
	for _, fixture := range cell.Field("fixtures").Items() {
		if name, named := namedString(fixture.Field("name")); named {
			names = append(names, name)
		}
	}
	// §87.1. `env.robot` answers *which robot does this cell say I am*, so a slug the cell has not
	// been told about is reported as the empty string rather than echoed back: an edition that
	// echoed it would let a precondition assert membership of a cell by asserting it.
	declared := CellRobotSlugs(cell)
	mine := ""
	if hasSubject && vocabulary.Contains(declared, subject) {
		mine = subject
	}
	return environment.
		Set("cell", canonical.String(cellName(cell))).
		Set("fixtures", canonical.Strings(names)).
		Set("robot", canonical.String(mine)).
		Set("robots", canonical.Strings(declared))
}

// CellRobotSlugs is every slug the cell names, sorted and without the ones it got wrong (§87.1).
//
// Sorted for ValidateCell's reason one field along: a precondition asks `includes`, so order is not
// semantic — but two editions emitting one list in two orders is a corpus failure, and sorted is the
// order every edition can produce. A duplicate appears once, because `env.robots` answers *which
// robots*, and `duplicate_robot` is where the document's mistake is reported.
func CellRobotSlugs(cell canonical.Value) []string {
	slugs := []string{}
	for _, robot := range cell.Field("robots").Items() {
		slug, named := namedString(robot.Field("slug"))
		if !named || vocabulary.Contains(slugs, slug) {
			continue
		}
		slugs = append(slugs, slug)
	}
	sort.SliceStable(slugs, func(left, right int) bool { return utf16Less(slugs[left], slugs[right]) })
	return slugs
}

// CellRobotLayer is the layer one robot of a shared cell contributes, and not ok when it draws
// nothing (§87.1).
//
// A **second** layer rather than a replacement for CellWorkspaceLayer, because §38.2's layers
// compose by conjunction and two layers is how a cell-wide volume and a per-robot one both hold. The
// volumes are **unplaced**, in the robot's own root frame, because that is the frame the `workspace`
// guard resolves poses in (§44.2).
func CellRobotLayer(cell canonical.Value, slug string) (canonical.Value, bool) {
	if js.Trim(slug) == "" {
		return canonical.Null(), false
	}
	for _, robot := range cell.Field("robots").Items() {
		name, named := namedString(robot.Field("slug"))
		if !named || name != slug {
			continue
		}
		volumes := canonical.Array()
		for _, volume := range robot.Field("allowed").Items() {
			if _, named := namedString(volume.Field("frame")); !named || !shapeIsUsable(volume) {
				continue
			}
			volumes = volumes.Push(shapeVolume(volume))
		}
		if volumes.Len() == 0 {
			return canonical.Null(), false
		}
		return canonical.Object().
			Set("name", canonical.String("cell:"+cellName(cell)+":"+slug)).
			Set("volumes", volumes), true
	}
	return canonical.Null(), false
}

// CellWorkspaceLayer is the cell's contribution to the `workspace` guard, and not ok when it draws
// no usable volume. A cell that draws nothing adds no layer — which is not a cell that allows
// nothing: an empty layer would forbid every frame it named.
func CellWorkspaceLayer(cell canonical.Value) (canonical.Value, bool) {
	volumes := canonical.Array()
	for _, volume := range cell.Field("allowed").Items() {
		if _, named := namedString(volume.Field("frame")); !named || !shapeIsUsable(volume) {
			continue
		}
		volumes = volumes.Push(shapeVolume(volume))
	}
	if volumes.Len() == 0 {
		return canonical.Null(), false
	}
	return canonical.Object().
		Set("name", canonical.String("cell:"+cellName(cell))).
		Set("volumes", volumes), true
}

// shapeVolume is one allowed volume on the wire: its frame, and its shape with only the fields that
// shape has.
func shapeVolume(volume canonical.Value) canonical.Value {
	shape := volume.Field("shape")
	kind, _ := shape.Field("kind").Str()
	shaped := canonical.Object().Set("kind", canonical.String(kind))
	if kind == "sphere" {
		shaped = shaped.
			Set("centre", canonical.Array(shape.Field("centre").Items()...)).
			Set("radius_m", shape.Field("radius_m"))
	} else {
		shaped = shaped.
			Set("min", canonical.Array(shape.Field("min").Items()...)).
			Set("max", canonical.Array(shape.Field("max").Items()...))
	}
	return canonical.Object().Set("frame", volume.Field("frame")).Set("shape", shaped)
}

func shapeCellFaults(faults []CellFault) canonical.Value {
	out := canonical.Array()
	for _, entry := range faults {
		out = out.Push(canonical.Object().
			Set("code", canonical.String(entry.Code)).
			Set("subjects", canonical.Strings(entry.Subjects)).
			Set("detail", canonical.String(entry.Detail)))
	}
	return canonical.Object().Set("faults", out)
}

func cellValidate(input canonical.Value) (canonical.Value, error) {
	return shapeCellFaults(ValidateCell(input.Field("cell"))), nil
}

func cellContext(input canonical.Value) (canonical.Value, error) {
	cell := input.Field("cell")
	layer, _ := CellWorkspaceLayer(cell)
	// No subject slug: these cases predate §87.1 and are about a cell with one robot, so `env.robot`
	// is the empty string and `env.robots` is empty — which is what every cell written before §87
	// answers, and the property that makes the field additive.
	return canonical.Object().
		Set("env", CellEnvironment(cell, "", false)).
		Set("layer", layer), nil
}

// cellRobots is §87.1's three projections of one document.
//
// One document rather than three, because they are one question — *what does this cell say about its
// robots* — and an edition could agree about the disjointness arithmetic while disagreeing about
// which layer a robot contributes, which is the half a workspace guard reads.
func cellRobots(input canonical.Value) (canonical.Value, error) {
	cell := input.Field("cell")
	subject, hasSubject := input.Field("robot").Str()

	layers := canonical.Array()
	for _, robot := range cell.Field("robots").Items() {
		slug, _ := robot.Field("slug").Str()
		layer, _ := CellRobotLayer(cell, slug)
		layers = layers.Push(canonical.Object().
			Set("slug", canonical.String(slug)).
			Set("layer", layer))
	}

	return canonical.Object().
		Set("env", CellEnvironment(cell, subject, hasSubject)).
		Set("slugs", canonical.Strings(CellRobotSlugs(cell))).
		Set("layers", layers).
		Set("faults", shapeCellFaults(ValidateCell(cell)).Field("faults")), nil
}
