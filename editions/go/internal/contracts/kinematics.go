package contracts

import (
	"fmt"
	"math"
	"sort"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// Forward kinematics, the arithmetic underneath it, and the geometry the two pose guards use (§44).
//
// # Why the trigonometry is written out
//
// §31.2 makes the kernel's sin, cos and sqrt single-source: the specification defines them as an
// explicit sequence of IEEE-754 binary64 operations, because addition, subtraction, multiplication
// and division are correctly rounded everywhere and so the same sequence produces the same bits
// everywhere. Go's math.Sin is a different sequence, and would part company with the reference on
// the first revolute joint.
//
// # Why every product that meets a sum is wrapped in float64(…)
//
// The same argument has a Go-specific half, and it is the reason this file looks the way it does.
// The Go specification permits the compiler to fuse `x*y + z` into one instruction that does not
// round `x*y` — "possibly across statements" — and on arm64, riscv64, ppc64, s390x, loong64 and
// amd64 at GOAMD64=v3 it does. A fused operation is a different sequence of roundings, so a pose
// computed on a Jetson would differ in its last bits from the same pose computed on the machine that
// ran the corpus, and the corpus pins every bit. The specification's own remedy is an explicit
// conversion, which "rounds to the precision of the target type, preventing fusion".
//
// So every product whose result is added or subtracted is written float64(a*b). A product consumed
// only by another product, a division or a comparison is left bare, because there is no instruction
// that fuses those. `cmd/selfcheck` reads the compiler's assembly for all six fusing targets and fails
// on any fused instruction in this edition, which is what makes the rule a gate rather than a habit
// — and it is a stronger check than reading this file, because the fusion that matters most happens
// after inlining, where no source scan can follow it.

// ── square root ─────────────────────────────────────────────────────────────

// kernelSqrt is Newton's iteration from a bit-pattern seed, a fixed five times.
//
// Not math.Sqrt, although a correctly rounded root would agree with it on most inputs: the Rust
// edition has no square root under no_std and computes this one, and an edition that took a
// shortcut the reference cannot take would be checking a different function. The iteration count
// is fixed rather than run to convergence, because a convergence test lets two editions take a
// different number of steps without noticing.
func kernelSqrt(x float64) float64 {
	if math.IsNaN(x) || x < 0 {
		return math.NaN()
	}
	if x == 0 {
		return x
	}
	if math.IsInf(x, 1) {
		return x
	}

	y := math.Float64frombits((math.Float64bits(x) >> 1) + (1023 << 51))
	if y == 0 || !canonical.Finite(y) {
		y = x
	}
	for step := 0; step < 5; step++ {
		y = float64(0.5 * (y + x/y))
	}
	return y
}

// ── range reduction ─────────────────────────────────────────────────────────

const (
	// twoOverPi is 2/π to the nearest double.
	twoOverPi = 6.36619772367581382433e-1
	// pio2First is π/2 with its low 33 bits cleared, so k × pio2First is exact for every admitted k.
	pio2First = 1.57079632673412561417e0
	// pio2Second is π/2 − pio2First, so the second step recovers the bits the first dropped.
	pio2Second = 6.0771005063039659766e-11
	// pio2SecondTail is the remainder of that split.
	pio2SecondTail = 2.02226624879595063154e-21
	// maxQuadrants is the largest quadrant count the reduction is exact for: k × pio2First is exact
	// while k fits in 20 bits, about 1.65 × 10⁶ radians. Past it the angle is refused rather than
	// reduced with its low bits gone.
	maxQuadrants = 1048576
)

// quadrantRound is the reference's `Math.trunc(v >= 0 ? v + 0.5 : v - 0.5)`.
//
// **Not observation.go's roundHalfAway**, and the two must never be merged although both are
// called "round half away from zero" in their own modules. That one truncates and then tests the
// fraction; this one adds a half and then truncates, and they differ at 0.49999999999999994 — the
// largest double below a half — where `v + 0.5` rounds up to exactly 1.0 and this returns 1 while
// that returns 0. Each reproduces its own module of the reference, and the reference has two.
func quadrantRound(value float64) float64 {
	value = float64(value)
	if value >= 0 {
		return math.Trunc(value + 0.5)
	}
	return math.Trunc(value - 0.5)
}

type reduced struct {
	r        float64
	quadrant int
	exact    bool
}

// reduce is a two-step Cody–Waite reduction. The second step is unconditional: a branch that
// skipped it when it did not matter would make the number of operations depend on the input.
func reduce(x float64) reduced {
	k := quadrantRound(float64(x * twoOverPi))
	if math.IsNaN(k) || math.Abs(k) >= maxQuadrants {
		return reduced{exact: false}
	}

	t := x - float64(k*pio2First)
	w0 := float64(k * pio2Second)
	r0 := t - w0
	w1 := float64(k*pio2SecondTail) - (t - r0 - w0)
	r := r0 - w1

	// `((k % 4) + 4) % 4`: ECMAScript's % keeps the dividend's sign, as math.Mod does.
	quadrant := math.Mod(math.Mod(k, 4)+4, 4)
	return reduced{r: r, quadrant: int(quadrant), exact: true}
}

// fdlibm's minimax coefficients, with fdlibm's own digits, so a reviewer can diff them against the
// published table without converting anything.
const (
	sinS1 = -1.66666666666666324348e-1
	sinS2 = 8.33333333332248946124e-3
	sinS3 = -1.98412698298579493134e-4
	sinS4 = 2.75573137070700676789e-6
	sinS5 = -2.50507602534068634195e-8
	sinS6 = 1.58969099521155010221e-10

	cosC1 = 4.16666666666666019037e-2
	cosC2 = -1.38888888888741095749e-3
	cosC3 = 2.48015872894767294178e-5
	cosC4 = -2.75573143513906633035e-7
	cosC5 = 2.0875723212981748279e-9
	cosC6 = -1.13596475577881948265e-11
)

func kernelSin(x float64) float64 {
	z := float64(x * x)
	poly := sinS1 + float64(z*(sinS2+float64(z*(sinS3+float64(z*(sinS4+float64(z*(sinS5+float64(z*sinS6)))))))))
	return x + float64(x*z*poly)
}

func kernelCos(x float64) float64 {
	z := float64(x * x)
	poly := cosC1 + float64(z*(cosC2+float64(z*(cosC3+float64(z*(cosC4+float64(z*(cosC5+float64(z*cosC6)))))))))
	return 1 - float64(0.5*z) + float64(z*z*poly)
}

// kernelSine is the specification's sine: NaN for a non-finite angle and for one outside the range
// the reduction is exact over. The NaN propagates into the pose, and solve reads a non-finite pose as
// an unresolved frame and names it — §7.2's rule applied to arithmetic.
func kernelSine(x float64) float64 {
	if !canonical.Finite(x) {
		return math.NaN()
	}
	reduction := reduce(x)
	if !reduction.exact {
		return math.NaN()
	}
	switch reduction.quadrant {
	case 0:
		return kernelSin(reduction.r)
	case 1:
		return kernelCos(reduction.r)
	case 2:
		return -kernelSin(reduction.r)
	}
	return -kernelCos(reduction.r)
}

// kernelCosine is the specification's cosine, with kernelSine's domain rule.
func kernelCosine(x float64) float64 {
	if !canonical.Finite(x) {
		return math.NaN()
	}
	reduction := reduce(x)
	if !reduction.exact {
		return math.NaN()
	}
	switch reduction.quadrant {
	case 0:
		return kernelCos(reduction.r)
	case 1:
		return -kernelSin(reduction.r)
	case 2:
		return -kernelCos(reduction.r)
	}
	return kernelSin(reduction.r)
}

// ── transforms ──────────────────────────────────────────────────────────────

// Transform is a rigid transform: metres, and a quaternion ordered `xyzw` (§44.4).
type Transform struct {
	Translation Vec3
	Rotation    Quat
}

var identityTransform = Transform{Rotation: Quat{0, 0, 0, 1}}

// quaternionProduct is the Hamilton product, `xyzw` in and `xyzw` out. Each sum is evaluated left to
// right, as the reference's is: floating-point addition is not associative, and the order of the
// four terms is part of the specification.
func quaternionProduct(a, b Quat) Quat {
	ax, ay, az, aw := a[0], a[1], a[2], a[3]
	bx, by, bz, bw := b[0], b[1], b[2], b[3]
	return Quat{
		float64(aw*bx) + float64(ax*bw) + float64(ay*bz) - float64(az*by),
		float64(aw*by) - float64(ax*bz) + float64(ay*bw) + float64(az*bx),
		float64(aw*bz) + float64(ax*by) - float64(ay*bx) + float64(az*bw),
		float64(aw*bw) - float64(ax*bx) - float64(ay*by) - float64(az*bz),
	}
}

// rotate applies a quaternion to a point. Active rotation, per §44.4.
func rotate(q Quat, v Vec3) Vec3 {
	qx, qy, qz, qw := q[0], q[1], q[2], q[3]
	tx := float64(2 * (float64(qy*v[2]) - float64(qz*v[1])))
	ty := float64(2 * (float64(qz*v[0]) - float64(qx*v[2])))
	tz := float64(2 * (float64(qx*v[1]) - float64(qy*v[0])))
	return Vec3{
		v[0] + float64(qw*tx) + (float64(qy*tz) - float64(qz*ty)),
		v[1] + float64(qw*ty) + (float64(qz*tx) - float64(qx*tz)),
		v[2] + float64(qw*tz) + (float64(qx*ty) - float64(qy*tx)),
	}
}

// compose is `parent` followed by `local`: the child's transform in the parent's parent.
func compose(parent, local Transform) Transform {
	rotated := rotate(parent.Rotation, local.Translation)
	return Transform{
		Translation: Vec3{
			parent.Translation[0] + rotated[0],
			parent.Translation[1] + rotated[1],
			parent.Translation[2] + rotated[2],
		},
		Rotation: quaternionProduct(parent.Rotation, local.Rotation),
	}
}

func finiteTransform(transform Transform) bool {
	for _, value := range transform.Translation {
		if !canonical.Finite(value) {
			return false
		}
	}
	for _, value := range transform.Rotation {
		if !canonical.Finite(value) {
			return false
		}
	}
	return true
}

// ── the flattened frame set ─────────────────────────────────────────────────

// scalarJoints are the joint kinds one number can describe.
var scalarJoints = []string{"revolute", "continuous", "prismatic"}

// maxFrames is the frame count the kernel is built for. A larger graph is refused rather than
// truncated: half a frame graph is a robot with a limb missing from its own collision model.
const maxFrames = 128

// Mimic is a linkage: `value = Multiplier × driver + Offset`, followed exactly one step.
type Mimic struct {
	Joint      string
	Multiplier float64
	Offset     float64
}

// KinematicJoint is a frame's joint, once flattening has checked its kind.
type KinematicJoint struct {
	Name  string
	Kind  string
	Axis  Vec3
	Mimic *Mimic
}

// Capsule is a sphere-swept segment in its frame's own coordinates.
type Capsule struct {
	A, B    Vec3
	RadiusM float64
}

// Frame is one frame, flattened: its parent is an index strictly smaller than its own, or -1 on the
// root. That ordering is what makes forward kinematics one forward pass and a cycle in the graph
// unrepresentable rather than a hang in a control loop.
type Frame struct {
	Name      string
	Parent    int
	Local     Transform
	Joint     *KinematicJoint
	Collision *Capsule
	// Ignore is the frames this one may touch, by index, ascending.
	Ignore []int
}

func frameFault(code string, subjects []string, detail string) FrameFault {
	return FrameFault{Code: code, Subjects: subjects, Detail: detail}
}

// Flatten orders a §44.3 frame graph the way the kernel reads it.
//
// Breadth-first from the root, so a parent is always emitted before its children by construction
// rather than by a sort that has to be checked afterwards. The order is a function of the tree and
// of the descriptor's own node order among siblings, which makes it the same on every edition — a
// requirement, because verdicts name frames and the corpus compares those names in order.
func Flatten(graph FrameGraph) ([]Frame, FrameFault, bool) {
	if fault, usable := ValidateFrames(graph); !usable {
		return nil, fault, false
	}
	if len(graph.Nodes) > maxFrames {
		count := js.NumberToString(float64(len(graph.Nodes)))
		return nil, frameFault("too_large", []string{},
			"The body declares "+count+" frames and this kernel is built for "+
				js.NumberToString(maxFrames)+". Half a frame graph is a robot with a limb missing from its own "+
				"collision model, so the graph is refused rather than truncated."), false
	}

	indexOf := func(name string) int {
		for at, node := range graph.Nodes {
			if node.Name == name {
				return at
			}
		}
		return -1
	}

	rootIndex := indexOf(graph.Root)
	if rootIndex < 0 {
		return nil, frameFault("root_missing", []string{graph.Root},
			"The declared root "+graph.Root+" is not one of the frames."), false
	}

	order := []int{rootIndex}
	for cursor := 0; cursor < len(order); cursor++ {
		parent := graph.Nodes[order[cursor]]
		for at, node := range graph.Nodes {
			if !node.IsRoot && node.Parent == parent.Name {
				order = append(order, at)
			}
		}
	}

	if len(order) != len(graph.Nodes) {
		ordered := map[int]bool{}
		for _, at := range order {
			ordered[at] = true
		}
		orphan := ""
		for at, node := range graph.Nodes {
			if !ordered[at] {
				orphan = node.Name
				break
			}
		}
		return nil, frameFault("unreachable", []string{orphan},
			"Frame "+orphan+" is not reachable from the root "+graph.Root+". Its parent chain is valid "+
				"and leads somewhere else, which is a second tree in one document."), false
	}

	position := make([]int, len(graph.Nodes))
	for slot, at := range order {
		position[at] = slot
	}

	frames := make([]Frame, 0, len(order))
	for _, at := range order {
		node := graph.Nodes[at]
		parent := -1
		if !node.IsRoot {
			parent = position[indexOf(node.Parent)]
		}

		var joint *KinematicJoint
		if node.HasJoint {
			if !node.HasJointType {
				return nil, frameFault("unknown_joint_type", []string{node.Name},
					"Frame "+node.Name+" names a joint and no joint type, so nothing can tell whether it "+
						"turns or slides."), false
			}
			kind, isString := node.JointType.Str()
			admitted := isString && (containsString(scalarJoints, kind) || kind == "ball" || kind == "floating")
			if !admitted {
				return nil, frameFault("unknown_joint_type", []string{node.Name},
					"Frame "+node.Name+" names joint type "+js.ToString(node.JointType, true)+", which is not "+
						"one of the five §44.3 admits. Reading it as revolute is how a prismatic axis becomes "+
						"a rotation."), false
			}
			// URDF's default, stated: the axis is `1 0 0` when the file is silent.
			axis := Vec3{1, 0, 0}
			if node.HasAxis {
				axis = node.Axis
			}
			joint = &KinematicJoint{Name: node.Joint, Kind: kind, Axis: axis}
			// `node.mimics ? … : {}` — truthy, so an empty name is no linkage.
			if node.HasMimics && js.Truthy(node.Mimics) {
				multiplier, offset := 1.0, 0.0
				if node.HasMimicMultiplier {
					multiplier = node.MimicMultiplier
				}
				if node.HasMimicOffset {
					offset = node.MimicOffset
				}
				joint.Mimic = &Mimic{Joint: node.Mimics, Multiplier: multiplier, Offset: offset}
			}
		}

		ignore := []int{}
		for _, name := range node.CollisionIgnore {
			if found := indexOf(name); found >= 0 {
				ignore = append(ignore, position[found])
			}
		}
		sort.Ints(ignore)

		local := Transform{Rotation: Quat{0, 0, 0, 1}}
		if node.HasTranslation {
			local.Translation = node.Translation
		}
		// Absent means no rotation, and the identity quaternion is `xyzw = 0 0 0 1` — not `0 0 0 0`,
		// which is not a rotation at all and would collapse every pose below it.
		if node.HasRotation {
			local.Rotation = node.Rotation
		}

		frame := Frame{Name: node.Name, Parent: parent, Local: local, Joint: joint, Ignore: ignore}
		if node.Shape != nil {
			frame.Collision = &Capsule{A: node.Shape.A, B: node.Shape.B, RadiusM: node.Shape.RadiusM}
		}
		frames = append(frames, frame)
	}
	return frames, FrameFault{}, true
}

// staticallyResolvable is whether a frame's pose is computable whatever the joint values are: false
// when the walk to the root passes through a joint one scalar cannot describe. Knowing it statically
// turns "the guard holds the robot every cycle" into "the guard reported itself unavailable when the
// lease was granted, naming the frame" (§44.5).
func staticallyResolvable(frames []Frame, index int) bool {
	here := index
	for {
		if here < 0 || here >= len(frames) {
			return false
		}
		frame := frames[here]
		if frame.Joint != nil && !containsString(scalarJoints, frame.Joint.Kind) {
			return false
		}
		if frame.Parent < 0 {
			return true
		}
		here = frame.Parent
	}
}

// collisionBodies is how many frames carry geometry this kernel can both read and place.
func collisionBodies(frames []Frame) int {
	count := 0
	for at, frame := range frames {
		if frame.Collision != nil && staticallyResolvable(frames, at) {
			count++
		}
	}
	return count
}

// collisionPairs is how many pairs the self-collision guard tests each cycle — reported rather than
// assumed small, because the cost is quadratic.
func collisionPairs(frames []Frame) int {
	bodies := collisionBodies(frames)
	return bodies * (bodies - 1) / 2
}

// ── solving ─────────────────────────────────────────────────────────────────

// JointBinding is how a joint is driven this cycle, and where it is now.
type JointBinding struct {
	Joint string
	// Channel is the command channel driving it, when the transport plan bound one to a position.
	Channel    string
	HasChannel bool
	Measured   float64
}

// Solved is every frame's pose in the root frame.
type Solved struct {
	// Poses holds nil for an unresolved frame.
	Poses      []*Transform
	Unresolved []int
	// Lookahead is whether every joint value came from this cycle's command.
	Lookahead bool
}

// Solve computes every frame's pose in one forward pass.
//
// A frame is unresolved when its parent is, when its joint has no value, when its joint kind needs
// more than a scalar, or when the arithmetic produced something that is not finite — and unresolved
// propagates down the chain, because a pose relative to an unknown pose is unknown.
func Solve(frames []Frame, bindings []JointBinding, command canonical.Value) Solved {
	solved := Solved{Poses: make([]*Transform, 0, len(frames)), Unresolved: []int{}, Lookahead: true}

	valueOf := func(joint string) (float64, bool) {
		for _, binding := range bindings {
			if binding.Joint != joint {
				continue
			}
			if !binding.HasChannel {
				solved.Lookahead = false
				return binding.Measured, true
			}
			value, present := command.Get(binding.Channel)
			if !present {
				solved.Lookahead = false
				return binding.Measured, true
			}
			number, isNumber := value.Number()
			return js.BareNumber(true, number, isNumber), true
		}
		return 0, false
	}

	unresolved := func(index int) {
		solved.Poses = append(solved.Poses, nil)
		solved.Unresolved = append(solved.Unresolved, index)
	}

	for index, frame := range frames {
		parentPose := identityTransform
		if frame.Parent >= 0 {
			resolved := solved.Poses[frame.Parent]
			if resolved == nil {
				unresolved(index)
				continue
			}
			parentPose = *resolved
		}

		local := frame.Local
		if frame.Joint != nil {
			// A mimic is followed exactly one step. URDF permits no more, and a chain of them would
			// need a traversal with its own cycle check inside the control loop.
			var value float64
			var found bool
			if driver := frame.Joint.Mimic; driver != nil {
				value, found = valueOf(driver.Joint)
				if found {
					value = float64(driver.Multiplier*value) + driver.Offset
				}
			} else {
				value, found = valueOf(frame.Joint.Name)
			}
			if !found {
				unresolved(index)
				continue
			}
			moved, ok := motion(*frame.Joint, value)
			if !ok {
				unresolved(index)
				continue
			}
			local = compose(local, moved)
		}

		pose := compose(parentPose, local)
		if !finiteTransform(pose) {
			unresolved(index)
			continue
		}
		solved.Poses = append(solved.Poses, &pose)
	}
	return solved
}

// motion is the transform a joint value produces, in the joint's own frame.
//
// Not ok where a scalar cannot describe the motion, or where the axis is degenerate. A zero-length
// axis is a body-description defect and not a fixed joint: reading it as one would weld a limb in
// the guard's model of the robot while it moved in the world.
func motion(joint KinematicJoint, value float64) (Transform, bool) {
	if !canonical.Finite(value) || !containsString(scalarJoints, joint.Kind) {
		return Transform{}, false
	}
	axis, ok := unit(joint.Axis)
	if !ok {
		return Transform{}, false
	}
	if joint.Kind == "prismatic" {
		return Transform{
			Translation: Vec3{float64(axis[0] * value), float64(axis[1] * value), float64(axis[2] * value)},
			Rotation:    Quat{0, 0, 0, 1},
		}, true
	}
	half := float64(0.5 * value)
	s := kernelSine(half)
	c := kernelCosine(half)
	if !canonical.Finite(s) || !canonical.Finite(c) {
		return Transform{}, false
	}
	return Transform{Rotation: Quat{float64(axis[0] * s), float64(axis[1] * s), float64(axis[2] * s), c}}, true
}

func unit(v Vec3) (Vec3, bool) {
	squared := float64(v[0]*v[0]) + float64(v[1]*v[1]) + float64(v[2]*v[2])
	if !canonical.Finite(squared) || squared <= 0 {
		return Vec3{}, false
	}
	length := kernelSqrt(squared)
	if !canonical.Finite(length) || length <= 0 {
		return Vec3{}, false
	}
	return Vec3{v[0] / length, v[1] / length, v[2] / length}, true
}

// ── volumes ─────────────────────────────────────────────────────────────────

// VolumeShape is a box or a sphere, in the frame the volume names.
type VolumeShape struct {
	Kind    string
	Min     Vec3
	Max     Vec3
	Centre  Vec3
	RadiusM float64
}

// Volume is one allowed region for one frame.
type Volume struct {
	Frame string
	Shape VolumeShape
}

// WorkspaceLayer is one layer of an operator's workspace (§38.2). Layers compose by conjunction: a
// frame must be inside at least one volume of every layer that names it, which is "each layer may
// only narrow" as a predicate rather than as geometry.
type WorkspaceLayer struct {
	Name    string
	Volumes []Volume
}

// margin is the signed distance from a point to a volume's boundary: positive inside.
func margin(shape VolumeShape, point Vec3) float64 {
	if shape.Kind == "sphere" {
		dx := point[0] - shape.Centre[0]
		dy := point[1] - shape.Centre[1]
		dz := point[2] - shape.Centre[2]
		return shape.RadiusM - kernelSqrt(float64(dx*dx)+float64(dy*dy)+float64(dz*dz))
	}

	var outside Vec3
	anyOutside := false
	insideMargin := math.Inf(1)
	for axis := 0; axis < 3; axis++ {
		below := shape.Min[axis] - point[axis]
		above := point[axis] - shape.Max[axis]
		if below > 0 {
			outside[axis] = below
			anyOutside = true
		} else if above > 0 {
			outside[axis] = above
			anyOutside = true
		}
		toLow := point[axis] - shape.Min[axis]
		toHigh := shape.Max[axis] - point[axis]
		nearest := toHigh
		if toLow < toHigh {
			nearest = toLow
		}
		if nearest < insideMargin {
			insideMargin = nearest
		}
	}
	if anyOutside {
		return -kernelSqrt(float64(outside[0]*outside[0]) + float64(outside[1]*outside[1]) + float64(outside[2]*outside[2]))
	}
	return insideMargin
}

// ── distance between two capsules ───────────────────────────────────────────

func sub(a, b Vec3) Vec3 { return Vec3{a[0] - b[0], a[1] - b[1], a[2] - b[2]} }

func dot(a, b Vec3) float64 {
	return float64(a[0]*b[0]) + float64(a[1]*b[1]) + float64(a[2]*b[2])
}

func clamp01(value float64) float64 {
	if value > 1 {
		return 1
	}
	if value > 0 {
		return value
	}
	return 0
}

// segmentDistanceSquared is the closest distance between two segments, squared — squared because
// the self-collision guard's decision compares it against a squared threshold, so no square root can
// move a verdict.
func segmentDistanceSquared(p1, q1, p2, q2 Vec3) float64 {
	d1 := sub(q1, p1)
	d2 := sub(q2, p2)
	r := sub(p1, p2)
	a := dot(d1, d1)
	e := dot(d2, d2)
	f := dot(d2, r)

	const epsilon = 1e-18
	var s, t float64
	if a <= epsilon && e <= epsilon {
		return dot(r, r)
	}
	if a <= epsilon {
		s = 0
		t = clamp01(f / e)
	} else {
		c := dot(d1, r)
		if e <= epsilon {
			t = 0
			s = clamp01(-c / a)
		} else {
			b := dot(d1, d2)
			denominator := float64(a*e) - float64(b*b)
			if denominator > epsilon {
				s = clamp01((float64(b*f) - float64(c*e)) / denominator)
			} else {
				s = 0
			}
			t = (float64(b*s) + f) / e
			if t < 0 {
				t = 0
				s = clamp01(-c / a)
			} else if t > 1 {
				t = 1
				s = clamp01((b - c) / a)
			}
		}
	}

	c1 := Vec3{p1[0] + float64(d1[0]*s), p1[1] + float64(d1[1]*s), p1[2] + float64(d1[2]*s)}
	c2 := Vec3{p2[0] + float64(d2[0]*t), p2[1] + float64(d2[1]*t), p2[2] + float64(d2[2]*t)}
	difference := sub(c1, c2)
	return dot(difference, difference)
}

// capsuleInWorld is a capsule's two endpoints, in the root frame.
func capsuleInWorld(pose Transform, capsule Capsule) (Vec3, Vec3) {
	a := rotate(pose.Rotation, capsule.A)
	b := rotate(pose.Rotation, capsule.B)
	return Vec3{pose.Translation[0] + a[0], pose.Translation[1] + a[1], pose.Translation[2] + a[2]},
		Vec3{pose.Translation[0] + b[0], pose.Translation[1] + b[1], pose.Translation[2] + b[2]}
}

// ignores is whether either frame lists the other. Symmetric on purpose.
func ignores(frames []Frame, a, b int) bool {
	return containsInt(frames[a].Ignore, b) || containsInt(frames[b].Ignore, a)
}

// adjacent is whether one frame is the other's parent — excluded from the collision test
// unconditionally.
func adjacent(frames []Frame, a, b int) bool {
	return frames[a].Parent == b || frames[b].Parent == a
}

// distance is the specified square root, for the number a report carries.
func distance(squared float64) float64 { return kernelSqrt(math.Abs(squared)) }

func containsInt(values []int, value int) bool {
	for _, candidate := range values {
		if candidate == value {
			return true
		}
	}
	return false
}

// ── reading them out of a document ──────────────────────────────────────────

// ReadJointBindings reads the bindings a case supplies. `measured` is a bare number, so a null there
// is NaN, which motion refuses and the frame reports unresolved.
func ReadJointBindings(value canonical.Value) []JointBinding {
	bindings := []JointBinding{}
	for _, entry := range value.Items() {
		joint, _ := entry.Field("joint").Str()
		binding := JointBinding{Joint: joint}
		if channel, ok := entry.Field("channel").Str(); ok {
			binding.Channel, binding.HasChannel = channel, true
		}
		measured, present := entry.Get("measured")
		number, isNumber := measured.Number()
		binding.Measured = js.BareNumber(present, number, isNumber)
		bindings = append(bindings, binding)
	}
	return bindings
}

// FlattenDocument reads and flattens a graph a case supplies, where the corpus generator guarantees
// the graph is valid. A fault here is therefore a disagreement with the generator's own check, and
// is reported as an error rather than shaped into an answer the reference never gave.
func FlattenDocument(value canonical.Value) ([]Frame, error) {
	graph, err := ReadFrameGraph(value)
	if err != nil {
		return nil, err
	}
	frames, fault, ok := Flatten(graph)
	if !ok {
		return nil, fmt.Errorf("the frame graph does not flatten: %s — %s", fault.Code, fault.Detail)
	}
	return frames, nil
}

func vector(values []float64) canonical.Value { return canonical.Numbers(values) }

// shapeSolved is the solved pose set, every frame present and null where it has no pose — "this frame
// has no pose" is the fact the guards act on, so it is rendered rather than omitted.
func shapeSolved(frames []Frame, solved Solved) canonical.Value {
	rows := canonical.Array()
	for at, frame := range frames {
		pose := solved.Poses[at]
		row := canonical.Object().
			Set("name", canonical.String(frame.Name)).
			Set("resolved", canonical.Bool(pose != nil))
		if pose == nil {
			row = row.Set("translation", canonical.Null()).Set("rotation", canonical.Null())
		} else {
			row = row.Set("translation", vector(pose.Translation[:])).Set("rotation", vector(pose.Rotation[:]))
		}
		rows = rows.Push(row)
	}
	unresolved := make([]string, 0, len(solved.Unresolved))
	for _, at := range solved.Unresolved {
		unresolved = append(unresolved, frames[at].Name)
	}
	return canonical.Object().
		Set("frames", rows).
		Set("lookahead", canonical.Bool(solved.Lookahead)).
		Set("unresolved", canonical.Strings(unresolved)).
		Set("collision_pairs", canonical.Number(float64(collisionPairs(frames))))
}

func kinematicsSolve(input canonical.Value) (canonical.Value, error) {
	frames, err := FlattenDocument(input.Field("frames"))
	if err != nil {
		return canonical.Null(), err
	}
	solved := Solve(frames, ReadJointBindings(input.Field("joints")), input.Field("command"))
	return shapeSolved(frames, solved), nil
}
