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
func CellEnvironment(cell canonical.Value) canonical.Value {
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
	return environment.
		Set("cell", canonical.String(cellName(cell))).
		Set("fixtures", canonical.Strings(names))
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
	return canonical.Object().
		Set("env", CellEnvironment(cell)).
		Set("layer", layer), nil
}
