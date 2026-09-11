package contracts

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"hash/crc32"
	"math"
	"strconv"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The black box (§13, §26): the ring's record layout, the ring header, the six MCAP channel
// schemas and the sealed file's framing — the formats §31.2 keeps single-source because *a black box
// sealed by one edition must be readable by every other*.
//
// # 64-bit values are uint64, and arrive as decimal strings
//
// A wall-clock epoch in nanoseconds is about 1.7e18, past 2^53, so a float64 cannot hold one and
// adding a few microseconds to one silently does nothing. The reference uses bigint; the corpus
// writes every 64-bit field as a decimal string so that no edition receives a value that went
// through a JSON number; this edition parses them straight into uint64.
//
// # NaN has one bit pattern here
//
// §24 writes an unmeasured reading as NaN, and a NaN has 2^52 encodings. The reference's DataView
// wrote the quiet NaN with an empty payload — 0x7FC00000 as f32, 0x7FF8000000000000 as f64 — and
// Go's own math.NaN() is 0x7FF8000000000001, whose conversion to f32 happens to reach the same bits
// on amd64 because the hardware drops the low payload. "Happens to, on this machine" is not a format,
// so the encoder writes the canonical pattern explicitly.

const (
	recordPrefix   = 24
	recordSchema   = 2
	cycleFixed     = 176
	maxTripped     = 8
	maxStreams     = 6
	streamName     = 16
	eventSubject   = 64
	eventDetail    = 96
	frameBody      = 52 + maxStreams*(streamName+8)
	eventBody      = 4 + eventSubject + eventDetail
	gapBody        = 24
	headerFixed    = 184
	headerAlign    = 4096
	robotIDWidth   = 64
	tokenName      = 48
	channelName    = 48
	formatVersion  = 2
	skewUnmeasured = 4294967295

	quietNaN32 = 0x7FC00000
	quietNaN64 = 0x7FF8000000000000
)

var recordKinds = map[string]byte{"cycle": 1, "frame": 2, "event": 3, "gap": 4}

var eventCodes = []string{
	"lease_granted", "lease_released", "lease_preempted", "estop", "estop_cleared",
	"package_started", "package_stopped", "grant_changed", "ota", "clock_step",
	"recording_started", "sealed", "config_changed", "admission", "skill_transition",
}

var gapReasons = []string{"writer_behind", "encoder_behind", "not_recording", "write_failed"}

// wireCode is a vocabulary member's one-based position, 0 for one the table does not hold — the
// reference's `indexOf(x) + 1`, reproduced rather than refused, because a name the table lacks is
// written as the code that means "none".
func wireCode(table []string, name string) int {
	for at, member := range table {
		if member == name {
			return at + 1
		}
	}
	return 0
}

// RingShape is what a ring's records are sized for.
type RingShape struct {
	Joints   int
	Channels int
}

func divCeil(value, by int) int { return (value + by - 1) / by }

// recordSize is one slot: the largest of the four record kinds, rounded up to eight bytes.
func recordSize(shape RingShape) int {
	cycle := cycleFixed + shape.Channels*13 + divCeil(shape.Channels, 8) + shape.Joints*12
	largest := cycle
	for _, candidate := range []int{recordPrefix + frameBody, recordPrefix + eventBody, recordPrefix + gapBody} {
		if candidate > largest {
			largest = candidate
		}
	}
	return divCeil(largest, 8) * 8
}

// ── ECMAScript's integer conversions, which DataView applies before it writes ──

// toUint is ToUint16 / ToUint32: truncate toward zero, then wrap modulo 2^bits; NaN and the
// infinities are 0.
func toUint(value float64, bits uint) uint64 {
	if !canonical.Finite(value) {
		return 0
	}
	modulus := math.Ldexp(1, int(bits))
	wrapped := math.Mod(math.Trunc(value), modulus)
	if wrapped < 0 {
		wrapped += modulus
	}
	return uint64(wrapped)
}

// slot is a little-endian writer over a fixed buffer, so every offset below reads like the
// reference's Cursor.
type slot []byte

func (s slot) u8(at int, value float64) { s[at] = byte(toUint(value, 8)) }
func (s slot) u16(at int, value float64) {
	binary.LittleEndian.PutUint16(s[at:], uint16(toUint(value, 16)))
}
func (s slot) u32(at int, value float64) {
	binary.LittleEndian.PutUint32(s[at:], uint32(toUint(value, 32)))
}

// i32 is ToInt32: the same wrap, read back as two's complement.
func (s slot) i32(at int, value float64) { s.u32(at, value) }

func (s slot) u64(at int, value uint64) { binary.LittleEndian.PutUint64(s[at:], value) }

func (s slot) f32(at int, value float64) {
	bits := uint32(quietNaN32)
	if !math.IsNaN(value) {
		bits = math.Float32bits(float32(value))
	}
	binary.LittleEndian.PutUint32(s[at:], bits)
}

func (s slot) f64(at int, value float64) {
	bits := uint64(quietNaN64)
	if !math.IsNaN(value) {
		bits = math.Float64bits(value)
	}
	binary.LittleEndian.PutUint64(s[at:], bits)
}

// text writes a string into width bytes, NUL-padded and cut on a character boundary: a cut inside a
// multi-byte sequence would put invalid UTF-8 in the ring, and every reader would have to guess. A
// continuation byte is 10xxxxxx, so the cut walks back off any it lands on.
func (s slot) text(at, width int, value string) {
	encoded := []byte(value)
	end := len(encoded)
	if end > width {
		end = width
	}
	for end > 0 && end < len(encoded) && encoded[end]&0xC0 == 0x80 {
		end--
	}
	for index := at; index < at+width; index++ {
		s[index] = 0
	}
	copy(s[at:], encoded[:end])
}

// ── reading a case's input ──────────────────────────────────────────────────

// uint64Field reads a 64-bit field the corpus writes as a decimal string.
func uint64Field(object canonical.Value, key string) (uint64, error) {
	text, ok := object.Field(key).Str()
	if !ok {
		return 0, fmt.Errorf("%s is not a decimal string", key)
	}
	value, err := strconv.ParseUint(text, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("%s: %w", key, err)
	}
	return value, nil
}

func hexField(object canonical.Value, key string, length int) ([]byte, error) {
	text, _ := object.Field(key).Str()
	decoded, err := hex.DecodeString(text)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", key, err)
	}
	if len(decoded) != length {
		return nil, fmt.Errorf("%s holds %d bytes and the format has room for %d", key, len(decoded), length)
	}
	return decoded, nil
}

// bareNumbers reads an array whose type is `number[]`, a null in it being NaN.
func bareNumbers(array canonical.Value) []float64 {
	out := make([]float64, 0, array.Len())
	for _, element := range array.Items() {
		number, isNumber := element.Number()
		out = append(out, js.BareNumber(true, number, isNumber))
	}
	return out
}

// ── the record ──────────────────────────────────────────────────────────────

// EncodeRecord encodes one record into a fresh slot of exactly recordSize(shape) bytes.
func EncodeRecord(shape RingShape, sequence, monoNS uint64, entry canonical.Value) ([]byte, error) {
	kind, _ := entry.Field("kind").Str()
	code, known := recordKinds[kind]
	if !known {
		return nil, fmt.Errorf("%q is not a record kind", kind)
	}
	record := slot(make([]byte, recordSize(shape)))
	record[4] = code
	record[5] = recordSchema
	record.u64(8, sequence)
	record.u64(16, monoNS)

	var err error
	switch kind {
	case "cycle":
		err = encodeCycle(shape, record, entry)
	case "frame":
		err = encodeFrame(record, entry)
	case "event":
		codeName, _ := entry.Field("code").Str()
		subject, _ := entry.Field("subject").Str()
		detail, _ := entry.Field("detail").Str()
		record.u16(recordPrefix, float64(wireCode(eventCodes, codeName)))
		record.text(recordPrefix+4, eventSubject, subject)
		record.text(recordPrefix+4+eventSubject, eventDetail, detail)
	case "gap":
		var dropped, firstTick uint64
		if dropped, err = uint64Field(entry, "dropped"); err != nil {
			return nil, err
		}
		if firstTick, err = uint64Field(entry, "first_tick"); err != nil {
			return nil, err
		}
		reason, _ := entry.Field("reason").Str()
		record.u64(recordPrefix, dropped)
		record.u64(recordPrefix+8, firstTick)
		record.u16(recordPrefix+16, float64(wireCode(gapReasons, reason)))
	}
	if err != nil {
		return nil, err
	}
	binary.LittleEndian.PutUint32(record[0:], crc32.ChecksumIEEE(record[4:]))
	return record, nil
}

func encodeCycle(shape RingShape, record slot, entry canonical.Value) error {
	arrays := map[string]canonical.Value{}
	for _, check := range []struct {
		field string
		want  int
	}{
		{"desired", shape.Channels}, {"command", shape.Channels}, {"mask", shape.Channels},
		{"clamped", shape.Channels}, {"position", shape.Joints}, {"velocity", shape.Joints},
		{"effort", shape.Joints},
	} {
		array := entry.Field(check.field)
		if array.Len() != check.want {
			return fmt.Errorf("black box: %s holds %d values, and the ring's shape says %d",
				check.field, array.Len(), check.want)
		}
		arrays[check.field] = array
	}

	tick, err := uint64Field(entry, "tick")
	if err != nil {
		return err
	}
	stateStamp, err := uint64Field(entry, "state_stamp_ns")
	if err != nil {
		return err
	}
	commandStamp, err := uint64Field(entry, "command_stamp_ns")
	if err != nil {
		return err
	}
	observation, err := hexField(entry, "observation", 32)
	if err != nil {
		return err
	}

	record.u64(24, tick)
	copy(record[32:], observation)
	record.u16(64, bareNumber(entry, "policy"))
	record.u16(66, bareNumber(entry, "state_flags"))
	record.u32(68, bareNumber(entry, "fault_code"))
	record.i32(72, bareNumber(entry, "rhal_result"))
	record.u32(76, bareNumber(entry, "state_seq"))
	record.u64(80, stateStamp)
	record.u64(88, commandStamp)
	record.u32(96, bareNumber(entry, "command_seq"))
	record.u8(100, bareNumber(entry, "mode"))
	if isTrue(entry.Field("advisory")) {
		record[101] = 1
	}
	if response, isString := entry.Field("response").Str(); isString {
		record.u8(102, float64(wireCode(vocabulary.FaultResponses, response)))
	}
	tripped := stringsOf(entry.Field("tripped"))
	if len(tripped) > maxTripped {
		tripped = tripped[:maxTripped]
	}
	record.u8(103, float64(len(tripped)))
	for index, guard := range tripped {
		record.u8(104+index, float64(wireCode(vocabulary.Guards, guard)))
	}
	record.u16(112, bareNumber(entry, "capability"))
	record.f32(116, bareNumber(entry, "nearest_person_m"))
	for index, value := range bareNumbers(entry.Field("base_pose")) {
		record.f32(120+index*4, value)
	}
	for index, value := range bareNumbers(entry.Field("base_twist")) {
		record.f32(148+index*4, value)
	}
	// 172..176 was reserved padding, which is why this landed without moving a byte or bumping the
	// ring's format. The schema byte separates "reserved zero" from "the read succeeded".
	record.i32(172, bareNumber(entry, "read_result"))

	desired := cycleFixed
	command := desired + shape.Channels*4
	mask := command + shape.Channels*8
	clamped := mask + shape.Channels
	position := clamped + divCeil(shape.Channels, 8)
	velocity := position + shape.Joints*4
	effort := velocity + shape.Joints*4

	for index, value := range bareNumbers(arrays["desired"]) {
		record.f32(desired+index*4, value)
	}
	// f64, because the kernel computed it: rounding it would make every clamped cycle report a false
	// divergence on replay.
	for index, value := range bareNumbers(arrays["command"]) {
		record.f64(command+index*8, value)
	}
	for index, value := range bareNumbers(arrays["mask"]) {
		record.u8(mask+index, value)
	}
	for index, was := range arrays["clamped"].Items() {
		if isTrue(was) {
			record[clamped+index/8] |= 1 << (index % 8)
		}
	}
	for index, value := range bareNumbers(arrays["position"]) {
		record.f32(position+index*4, value)
	}
	for index, value := range bareNumbers(arrays["velocity"]) {
		record.f32(velocity+index*4, value)
	}
	for index, value := range bareNumbers(arrays["effort"]) {
		record.f32(effort+index*4, value)
	}
	return nil
}

func encodeFrame(record slot, entry canonical.Value) error {
	observation, err := hexField(entry, "observation", 32)
	if err != nil {
		return err
	}
	target, err := uint64Field(entry, "target_ns")
	if err != nil {
		return err
	}
	copy(record[recordPrefix:], observation)
	record.u64(recordPrefix+32, target)
	// `worstSkewUs ?? SKEW_UNMEASURABLE`: null is fewer than two contributors, whose spread is
	// undefined rather than zero.
	skew := float64(skewUnmeasured)
	if value, isNumber := entry.Field("worst_skew_us").Number(); isNumber {
		skew = value
	}
	record.u32(recordPrefix+40, skew)

	streams := entry.Field("streams").Items()
	written := len(streams)
	if written > maxStreams {
		written = maxStreams
	}
	record.u16(recordPrefix+44, float64(written))
	// Streams the record could not carry are counted rather than dropped: a limit of the format.
	record.u16(recordPrefix+46, bareNumber(entry, "streams_omitted")+float64(len(streams)-written))
	record.u16(recordPrefix+48, bareNumber(entry, "streams_absent"))
	for index, stream := range streams[:written] {
		at := recordPrefix + 52 + index*(streamName+8)
		name, _ := stream.Field("name").Str()
		record.text(at, streamName, name)
		record.u32(at+streamName, bareNumber(stream, "age_us"))
		if isTrue(stream.Field("stale")) {
			record[at+streamName+4] = 1
		}
	}
	return nil
}

// ── the ring header ─────────────────────────────────────────────────────────

// headerLen is the header with both tables, rounded up to a page.
func headerLen(shape RingShape, digests, capabilities int) int {
	raw := headerFixed + digests*32 + capabilities*tokenName + shape.Channels*channelName
	return divCeil(raw, headerAlign) * headerAlign
}

// EncodeHeader is the header a ring is created with, including the digest and capability tables.
func EncodeHeader(input canonical.Value) ([]byte, RingShape, error) {
	geometry, meta := input.Field("geometry"), input.Field("meta")
	shape := readShape(geometry.Field("shape"))
	digestCapacity := int(bareNumber(geometry, "digests"))
	capabilityCapacity := int(bareNumber(geometry, "capabilities"))

	length := headerLen(shape, digestCapacity, capabilityCapacity)
	header := slot(make([]byte, length))
	copy(header[0:], "SKYNETBB")
	header.u16(8, formatVersion)
	header.u32(12, float64(length))
	header.u32(16, float64(recordSize(shape)))
	header.u32(20, bareNumber(geometry, "capacity"))
	header.u16(24, float64(shape.Joints))
	header.u16(26, float64(shape.Channels))
	header.u8(28, bareNumber(meta, "tier"))
	header.u8(29, bareNumber(meta, "enforcement"))
	if isTrue(meta.Field("simulator")) {
		header[30] = 1
	}
	embodiment, err := hexField(meta, "embodiment", 32)
	if err != nil {
		return nil, shape, err
	}
	osImage, err := hexField(meta, "os_image", 32)
	if err != nil {
		return nil, shape, err
	}
	copy(header[32:], embodiment)
	copy(header[64:], osImage)
	robotID, _ := meta.Field("robot_id").Str()
	header.text(96, robotIDWidth, robotID)
	header.u16(160, float64(digestCapacity))
	header.u16(162, float64(capabilityCapacity))
	wallEpoch, err := uint64Field(meta, "wall_epoch_ns")
	if err != nil {
		return nil, shape, err
	}
	mono, err := uint64Field(meta, "mono_ns")
	if err != nil {
		return nil, shape, err
	}
	header.u64(164, wallEpoch)
	header.u64(172, mono)

	digestBase := headerFixed
	for index, entry := range input.Field("digests").Items() {
		text, _ := entry.Str()
		digest, err := hex.DecodeString(text)
		if err != nil || len(digest) != 32 {
			return nil, shape, fmt.Errorf("digest %d is not 32 bytes of hex", index)
		}
		copy(header[digestBase+index*32:], digest)
	}
	tokenBase := digestBase + digestCapacity*32
	for index, token := range stringsOf(input.Field("capabilities")) {
		header.text(tokenBase+index*tokenName, tokenName, token)
	}
	channelBase := tokenBase + capabilityCapacity*tokenName
	channels := stringsOf(meta.Field("channels"))
	if len(channels) > shape.Channels {
		channels = channels[:shape.Channels]
	}
	for index, name := range channels {
		header.text(channelBase+index*channelName, channelName, name)
	}

	binary.LittleEndian.PutUint32(header[180:], crc32.ChecksumIEEE(header))
	return header, shape, nil
}

func readShape(value canonical.Value) RingShape {
	return RingShape{Joints: int(bareNumber(value, "joints")), Channels: int(bareNumber(value, "channels"))}
}

// ── the channel schemas (§26, §31.2) ────────────────────────────────────────

// WireChannel is one of the six channels, with its embedded schema.
type WireChannel struct {
	ID         int
	Topic      string
	Name       string
	Definition string
}

func schemaField(kind, description string) canonical.Value {
	return canonical.Object().Set("type", canonical.String(kind)).Set("description", canonical.String(description))
}

func schemaMaybe(kind, description string) canonical.Value {
	return canonical.Object().
		Set("type", canonical.Strings([]string{kind, "null"})).
		Set("description", canonical.String(description))
}

func schemaArray(items canonical.Value, description string) canonical.Value {
	return canonical.Object().
		Set("type", canonical.String("array")).
		Set("items", items).
		Set("description", canonical.String(description))
}

func schemaNumbers(description string) canonical.Value {
	return schemaArray(canonical.Object().Set("type", canonical.Strings([]string{"number", "null"})), description)
}

// property is one named member of a schema's `properties`, kept in order so the Go reads like the
// reference; the writer sorts either way.
type property struct {
	name  string
	value canonical.Value
}

func schemaObject(properties []property, required []string, description string) canonical.Value {
	members := canonical.Object()
	for _, entry := range properties {
		members = members.Set(entry.name, entry.value)
	}
	return canonical.Object().
		Set("type", canonical.String("object")).
		Set("description", canonical.String(description)).
		Set("properties", members).
		Set("required", canonical.Strings(required))
}

func schemaMono() canonical.Value {
	return schemaField("integer",
		"CLOCK_MONOTONIC_RAW at the instant this record was produced. The time base every control "+
			"decision was made against (§35.1); MCAP's log_time is the wall-clock rendering of it.")
}

func schemaChannelValues(description string) canonical.Value {
	return schemaArray(schemaObject([]property{
		{"channel", schemaField("string", "The action channel's name.")},
		{"value", schemaMaybe("number", "The value on that channel.")},
		{"hold", schemaField("boolean",
			"True where the mask says hold this joint at its current setpoint — the OS saying nothing "+
				"rather than saying zero (§24).")},
	}, []string{"channel", "value", "hold"}, "One channel's value."), description)
}

// WireChannels is the six channels in id order. Ids are fixed for the life of the format, and the
// definitions are the canonical bytes of each schema document — the artifact §31.2 keeps single-source.
func WireChannels() ([]WireChannel, error) {
	documents := []struct {
		id          int
		topic, name string
		document    canonical.Value
	}{
		{1, "/sk/state", "sk_state", schemaObject([]property{
			{"mono_ns", schemaMono()},
			{"stamp_ns", schemaField("integer", "rhal_state.stamp_ns: CLOCK_MONOTONIC_RAW at acquisition, never at return (§24).")},
			{"seq", schemaField("integer", "rhal_state.seq.")},
			{"flags", schemaField("integer", "Bitwise-or of the RHAL flags: estop, fault, degraded, braked.")},
			{"fault_code", schemaField("integer", "Vendor-specific, surfaced verbatim (§24).")},
			{"read_result", schemaMaybe("integer",
				"What read_state returned this cycle: 0, or a negative errno (§24). Null in a recording "+
					"sealed before the field existed, which is not the same as a read that succeeded (§26.4).")},
			{"nearest_person_m", schemaMaybe("number",
				"Distance to the nearest detected person, in metres; null when nothing was measuring it. "+
					"An input to the proximity guard rather than a reading the RHAL supplies (§7.1).")},
			{"position_rad", schemaNumbers("Measured joint positions.")},
			{"velocity_rad_s", schemaNumbers("Measured velocities; null where the joint does not measure it.")},
			{"effort_nm", schemaNumbers("Measured efforts; null where unmeasured.")},
			{"base_pose", schemaNumbers("x y z qx qy qz qw; all null when the base is not localised.")},
			{"base_twist", schemaNumbers("vx vy vz wx wy wz.")},
		}, []string{
			"mono_ns", "stamp_ns", "seq", "flags", "fault_code", "read_result", "nearest_person_m",
			"position_rad", "velocity_rad_s", "effort_nm", "base_pose", "base_twist",
		}, "One sample of the robot's state: what the RHAL reported (§24), whether the read succeeded, "+
			"and the proximity reading the guards saw.")},

		{2, "/sk/desired", "sk_desired", schemaObject([]property{
			{"mono_ns", schemaMono()},
			{"tick", schemaField("integer", "The control tick.")},
			{"stamp_ns", schemaField("integer", "When the policy produced this command, not when it was sent.")},
			{"seq", schemaField("integer", "rhal_command.seq; must advance (§24).")},
			{"mode", schemaField("string", "position, velocity, torque, or unknown.")},
			{"policy_digest", schemaField("string",
				"Hex SHA-256 of the package version that produced this, or empty when the recording"+
					"'s digest table did not hold it.")},
			{"capability", schemaMaybe("string",
				"The capability token this command exercised, or null when the runtime did not say — which "+
					"is not the same as no capability, and the mode gate treats the two differently (§7.2).")},
			{"observation", schemaField("string",
				"Hex SHA-256 of the observation frame the policy was given. A hash, never the bytes: an "+
					"hour of three-camera video is not something the robot can keep (§26).")},
			{"values", schemaChannelValues("What the policy asked for.")},
		}, []string{"mono_ns", "tick", "stamp_ns", "seq", "mode", "policy_digest", "capability", "observation", "values"},
			"What the policy asked for, before the Safety Kernel saw it.")},

		{3, "/sk/verdict", "sk_verdict", schemaObject([]property{
			{"mono_ns", schemaMono()},
			{"tick", schemaField("integer", "The control tick.")},
			{"response", schemaMaybe("string",
				"clamp, hold, stop_cat2, stop_cat1 or stop_cat0 — the strongest response any tripped guard "+
					"demanded. Null when nothing tripped, which is a fact rather than an absence and so is "+
					"present-and-null.")},
			{"tripped", schemaArray(schemaField("string",
				"A guard name, in evaluation order: watchdog, envelope, rate_limit, workspace, "+
					"self_collision, balance, proximity, mode_gate (§7.1)."), "Guards that tripped.")},
			{"clamped", schemaArray(schemaObject([]property{
				{"channel", schemaField("string", "Which channel was reduced.")},
				{"from", schemaMaybe("number", "What the policy asked for.")},
				{"to", schemaMaybe("number", "What the kernel sent instead.")},
			}, []string{"channel", "from", "to"}, "One clamped channel, and by how much."),
				"Channels whose values were reduced.")},
			{"advisory", schemaField("boolean",
				"True when the tier could only advise: the command was forwarded unchanged despite a trip. "+
					"Always false at host and certified (§42).")},
		}, []string{"mono_ns", "tick", "response", "tripped", "clamped", "advisory"},
			"What the Safety Kernel decided this cycle.")},

		{4, "/sk/command", "sk_command", schemaObject([]property{
			{"mono_ns", schemaMono()},
			{"tick", schemaField("integer", "The control tick.")},
			{"values", schemaChannelValues("What actually went to write_command.")},
			{"rhal_result", schemaField("integer",
				"What write_command returned: 0, or a negative errno. -1 EPERM because the drive is faulted "+
					"is a different fact from -110 ETIMEDOUT (§24).")},
		}, []string{"mono_ns", "tick", "values", "rhal_result"},
			"What was sent to the hardware, and what the hardware said about it.")},

		{5, "/sk/frame", "sk_frame", schemaObject([]property{
			{"mono_ns", schemaMono()},
			{"observation", schemaField("string", "Hex SHA-256 of the assembled frame.")},
			{"target_ns", schemaField("integer", "The instant the assembler targeted.")},
			{"worst_skew_us", schemaMaybe("integer",
				"The worst skew between any stream and that instant. Recorded so that 'the policy behaved "+
					"oddly' can be checked against 'the frame it was given was 80 ms skewed' months later "+
					"(§25.1). Null when fewer than two streams contributed: the spread of one instant is "+
					"undefined rather than zero, and zero would make a frame with three dead capture paths "+
					"read as a perfectly synchronised one.")},
			{"streams_omitted", schemaField("integer",
				"Streams the record could not carry, counted rather than dropped. A limit of the format.")},
			{"streams_absent", schemaField("integer",
				"Streams the policy declared that produced no sample at all. A limit of the robot, and "+
					"deliberately not the field above: an investigation reading one as the other goes looking "+
					"for a bigger record instead of a dead camera.")},
			{"streams", schemaArray(schemaObject([]property{
				{"name", schemaField("string", "The stream's name.")},
				{"age_us", schemaField("integer",
					"How far back this stream's sample was, relative to the instant the assembler targeted.")},
				{"stale", schemaField("boolean",
					"Whether that exceeded the policy's declared staleness budget, which is a hold rather "+
						"than a silently older frame (§25.1).")},
			}, []string{"name", "age_us", "stale"}, "One stream's contribution to the frame."),
				"Per-stream ages, for the streams that had one.")},
		}, []string{
			"mono_ns", "observation", "target_ns", "worst_skew_us", "streams_omitted", "streams_absent",
			"streams",
		}, "One assembled observation frame, and how well aligned it was.")},

		{6, "/sk/event", "sk_event", schemaObject([]property{
			{"mono_ns", schemaMono()},
			{"event", schemaField("string",
				"lease_granted, lease_released, lease_preempted, estop, estop_cleared, package_started, "+
					"package_stopped, grant_changed, ota, clock_step, recording_started, sealed, "+
					"config_changed, admission, skill_transition, or gap.")},
			{"subject", schemaField("string", "What it happened to: a package id, a capability token, a slot.")},
			{"detail", schemaField("string", "A sentence about it.")},
			{"dropped", schemaField("integer",
				"Gap events only: how many records were not written. A gap annotated as a gap is honest; a "+
					"gap that looks like a quiet period is not (§47.4).")},
			{"first_tick", schemaField("integer", "Gap events only: the first tick that went unrecorded.")},
			{"reason", schemaField("string",
				"Gap events only: writer_behind, encoder_behind, not_recording or write_failed.")},
		}, []string{"mono_ns", "event", "subject", "detail"},
			"Something that changed, and every gap in the recording.")},
	}

	channels := make([]WireChannel, 0, len(documents))
	for _, entry := range documents {
		definition, err := canonical.Encode(entry.document)
		if err != nil {
			return nil, fmt.Errorf("schema %s: %w", entry.name, err)
		}
		channels = append(channels, WireChannel{ID: entry.id, Topic: entry.topic, Name: entry.name, Definition: definition})
	}
	return channels, nil
}

// ── the MCAP writer ─────────────────────────────────────────────────────────

var mcapMagic = []byte{0x89, 0x4D, 0x43, 0x41, 0x50, 0x30, 0x0D, 0x0A}

const (
	opHeader        = 0x01
	opFooter        = 0x02
	opSchema        = 0x03
	opChannel       = 0x04
	opMessage       = 0x05
	opStatistics    = 0x0B
	opMetadata      = 0x0C
	opSummaryOffset = 0x0E
	opDataEnd       = 0x0F
)

// mcapBytes is an append-only little-endian buffer.
type mcapBytes struct{ data []byte }

func (b *mcapBytes) byte1(value byte)  { b.data = append(b.data, value) }
func (b *mcapBytes) u16(value uint16)  { b.data = binary.LittleEndian.AppendUint16(b.data, value) }
func (b *mcapBytes) u32(value uint32)  { b.data = binary.LittleEndian.AppendUint32(b.data, value) }
func (b *mcapBytes) u64(value uint64)  { b.data = binary.LittleEndian.AppendUint64(b.data, value) }
func (b *mcapBytes) raw(source []byte) { b.data = append(b.data, source...) }
func (b *mcapBytes) str(value string)  { b.u32(uint32(len(value))); b.data = append(b.data, value...) }
func (b *mcapBytes) record(opcode byte, body *mcapBytes) {
	b.byte1(opcode)
	b.u64(uint64(len(body.data)))
	b.raw(body.data)
}

// McapMetadata is one metadata record: a name, and its key/value pairs in order.
type McapMetadata struct {
	Name  string
	Pairs [][2]string
}

// McapMessage is one message a sealed file carries.
type McapMessage struct {
	ChannelID uint16
	LogTime   uint64
	Payload   string
}

// WriteMcap writes one MCAP file: magic, header, schemas, channels, metadata, messages, data end,
// the summary (schemas and channels again, then statistics), summary offsets, footer, magic.
// Unchunked and uncompressed, exactly as the reference writes it.
func WriteMcap(library string, channels []WireChannel, metadata []McapMetadata, messages []McapMessage) []byte {
	out := &mcapBytes{}
	schemaBody := func(channel WireChannel) *mcapBytes {
		body := &mcapBytes{}
		body.u16(uint16(channel.ID))
		body.str(channel.Name)
		body.str("jsonschema")
		body.str(channel.Definition)
		return body
	}
	channelBody := func(channel WireChannel) *mcapBytes {
		body := &mcapBytes{}
		body.u16(uint16(channel.ID))
		body.u16(uint16(channel.ID))
		body.str(channel.Topic)
		body.str("json")
		body.u32(0)
		return body
	}

	out.raw(mcapMagic)
	header := &mcapBytes{}
	header.str("")
	header.str(library)
	out.record(opHeader, header)
	for _, channel := range channels {
		out.record(opSchema, schemaBody(channel))
	}
	for _, channel := range channels {
		out.record(opChannel, channelBody(channel))
	}

	for _, entry := range metadata {
		pairs := &mcapBytes{}
		for _, pair := range entry.Pairs {
			pairs.str(pair[0])
			pairs.str(pair[1])
		}
		body := &mcapBytes{}
		body.str(entry.Name)
		body.u32(uint32(len(pairs.data)))
		body.raw(pairs.data)
		out.record(opMetadata, body)
	}

	// Per-channel sequences and counts, in channel order. The reference holds them in a Map built
	// from the channel list and iterated in insertion order — a message on a channel the list does not
	// declare adds a key at the end — so an ordered slice here rather than a Go map, whose iteration
	// order would change the statistics record on every run.
	ids := make([]uint16, 0, len(channels))
	for _, channel := range channels {
		ids = append(ids, uint16(channel.ID))
	}
	sequences := make([]uint32, len(ids))
	counts := make([]uint64, len(ids))
	slotFor := func(id uint16) int {
		for at, candidate := range ids {
			if candidate == id {
				return at
			}
		}
		ids = append(ids, id)
		sequences = append(sequences, 0)
		counts = append(counts, 0)
		return len(ids) - 1
	}
	var first, last uint64
	haveFirst := false
	for _, message := range messages {
		at := slotFor(message.ChannelID)
		sequences[at]++
		counts[at]++
		sequence := sequences[at]
		// The minimum, not the first one seen: a robot writing from a ring is not writing in log-time
		// order.
		if !haveFirst || message.LogTime < first {
			first, haveFirst = message.LogTime, true
		}
		if message.LogTime > last {
			last = message.LogTime
		}
		body := &mcapBytes{}
		body.u16(message.ChannelID)
		body.u32(sequence)
		body.u64(message.LogTime)
		body.u64(message.LogTime)
		body.raw([]byte(message.Payload))
		out.record(opMessage, body)
	}

	dataEnd := &mcapBytes{}
	dataEnd.u32(crc32.ChecksumIEEE(out.data))
	out.record(opDataEnd, dataEnd)

	summaryStart := uint64(len(out.data))
	schemaStart := uint64(len(out.data))
	for _, channel := range channels {
		out.record(opSchema, schemaBody(channel))
	}
	schemaLen := uint64(len(out.data)) - schemaStart

	channelStart := uint64(len(out.data))
	for _, channel := range channels {
		out.record(opChannel, channelBody(channel))
	}
	channelLen := uint64(len(out.data)) - channelStart

	statisticsStart := uint64(len(out.data))
	statistics := &mcapBytes{}
	var total uint64
	for _, count := range counts {
		total += count
	}
	statistics.u64(total)
	statistics.u16(uint16(len(channels))) // schemas
	statistics.u32(uint32(len(channels))) // channels
	statistics.u32(0)                     // attachments
	statistics.u32(uint32(len(metadata))) // metadata
	statistics.u32(0)                     // chunks
	statistics.u64(first)
	statistics.u64(last)
	perChannel := &mcapBytes{}
	for at, id := range ids {
		perChannel.u16(id)
		perChannel.u64(counts[at])
	}
	statistics.u32(uint32(len(perChannel.data)))
	statistics.raw(perChannel.data)
	out.record(opStatistics, statistics)
	statisticsLen := uint64(len(out.data)) - statisticsStart

	summaryOffsetStart := uint64(len(out.data))
	for _, offset := range []struct {
		opcode        byte
		start, length uint64
	}{
		{opSchema, schemaStart, schemaLen},
		{opChannel, channelStart, channelLen},
		{opStatistics, statisticsStart, statisticsLen},
	} {
		body := &mcapBytes{}
		body.byte1(offset.opcode)
		body.u64(offset.start)
		body.u64(offset.length)
		out.record(opSummaryOffset, body)
	}

	footer := &mcapBytes{}
	footer.byte1(opFooter)
	footer.u64(20)
	footer.u64(summaryStart)
	footer.u64(summaryOffsetStart)
	// The running CRC continues from the summary section into the footer's own leading bytes, which is
	// the CRC of the two concatenated.
	summaryCrc := crc32.Update(crc32.ChecksumIEEE(out.data[summaryStart:]), crc32.IEEETable, footer.data)
	footer.u32(summaryCrc)
	out.raw(footer.data)
	out.raw(mcapMagic)
	return out.data
}

// mcapOpcodes is the file's record opcodes in order, as two-digit hex — pinned beside the digest
// because a digest says *that* two editions disagree and this says *where*. It also checks that the
// framing walks: a file whose record lengths do not land exactly on the trailing magic opens nowhere.
func mcapOpcodes(bytes []byte) (string, error) {
	end := len(bytes) - 8
	found := []string{}
	at := 8
	for at < end {
		if at+9 > end {
			return "", fmt.Errorf("mcap: a record header runs past the end of the file at %d", at)
		}
		found = append(found, fmt.Sprintf("%02x", bytes[at]))
		at += 9 + int(binary.LittleEndian.Uint64(bytes[at+1:]))
	}
	if at != end {
		return "", fmt.Errorf("mcap: the framing ended at %d, the trailing magic at %d", at, end)
	}
	return strings.Join(found, ","), nil
}

// ── the corpus ──────────────────────────────────────────────────────────────

func blackboxSchemas(input canonical.Value) (canonical.Value, error) {
	topic, _ := input.Field("channel").Str()
	channels, err := WireChannels()
	if err != nil {
		return canonical.Null(), err
	}
	for _, channel := range channels {
		if channel.Topic == topic {
			return canonical.Object().
				Set("id", canonical.Number(float64(channel.ID))).
				Set("topic", canonical.String(channel.Topic)).
				Set("name", canonical.String(channel.Name)).
				Set("definition", canonical.String(channel.Definition)), nil
		}
	}
	return canonical.Null(), fmt.Errorf("no channel has the topic %q", topic)
}

func blackboxRecords(input canonical.Value) (canonical.Value, error) {
	sequence, err := uint64Field(input, "sequence")
	if err != nil {
		return canonical.Null(), err
	}
	mono, err := uint64Field(input, "mono_ns")
	if err != nil {
		return canonical.Null(), err
	}
	bytes, err := EncodeRecord(readShape(input.Field("shape")), sequence, mono, input.Field("entry"))
	if err != nil {
		return canonical.Null(), err
	}
	return canonical.Object().
		Set("size", canonical.Number(float64(len(bytes)))).
		Set("hex", canonical.String(hex.EncodeToString(bytes))), nil
}

func blackboxHeader(input canonical.Value) (canonical.Value, error) {
	bytes, shape, err := EncodeHeader(input)
	if err != nil {
		return canonical.Null(), err
	}
	return canonical.Object().
		Set("size", canonical.Number(float64(len(bytes)))).
		Set("record_size", canonical.Number(float64(recordSize(shape)))).
		Set("hex", canonical.String(hex.EncodeToString(bytes))), nil
}

func blackboxSeal(input canonical.Value) (canonical.Value, error) {
	channels, err := WireChannels()
	if err != nil {
		return canonical.Null(), err
	}
	library, _ := input.Field("library").Str()
	// One metadata record per case: the generator writes the `metadata` field as the record itself.
	record := input.Field("metadata")
	name, _ := record.Field("name").Str()
	pairs := [][2]string{}
	for _, pair := range record.Field("pairs").Items() {
		key, _ := pair.At(0).Str()
		value, _ := pair.At(1).Str()
		pairs = append(pairs, [2]string{key, value})
	}
	messages := []McapMessage{}
	for _, entry := range input.Field("messages").Items() {
		logTime, err := uint64Field(entry, "log_time")
		if err != nil {
			return canonical.Null(), err
		}
		payload, _ := entry.Field("payload").Str()
		messages = append(messages, McapMessage{
			ChannelID: uint16(toUint(bareNumber(entry, "channel_id"), 16)), LogTime: logTime, Payload: payload,
		})
	}
	bytes := WriteMcap(library, channels, []McapMetadata{{Name: name, Pairs: pairs}}, messages)
	opcodes, err := mcapOpcodes(bytes)
	if err != nil {
		return canonical.Null(), err
	}
	digest := sha256.Sum256(bytes)
	return canonical.Object().
		Set("size", canonical.Number(float64(len(bytes)))).
		Set("sha256", canonical.String(hex.EncodeToString(digest[:]))).
		Set("opcodes", canonical.String(opcodes)), nil
}
