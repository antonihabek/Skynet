/**
 * The black box, in the reference edition — §13 and §26.
 *
 * This is the differential-test oracle for `crates/skynet-blackbox`: the ring's record layout, the
 * ring header, the six MCAP channel schemas, and the sealed file's framing, written a second time
 * from the specification so that `tools/corpus-gen/generate.ts` can freeze the bytes and every
 * edition can be held to them.
 *
 * # Why this one is not a copy of a Hub module
 *
 * The other seven files here are the Hub's `lib/skynet/os/*.ts` with their import paths changed,
 * because the Hub genuinely runs those functions. The Hub does not write black boxes — a robot
 * does — so there is nothing to vendor, and this file is reference-only. It is still worth having
 * twice: §31.2 puts the MCAP channel schemas in the short list of artifacts that stay single-source
 * across editions, on the grounds that *a black box sealed by one edition must be readable by every
 * other*, and a format with one implementation has not been specified, only written.
 *
 * # 64-bit values are `bigint`
 *
 * Deliberately, and it is the thing most likely to be got wrong by a third implementation. A
 * recording's timestamps are `uint64` nanoseconds; a wall-clock epoch in nanoseconds is about
 * 1.7e18, which is past 2^53, so a `number` cannot hold one and adding a few microseconds to one
 * silently does nothing. Every field the format declares as 64-bit is a `bigint` here.
 */

import { canonical } from './canonical.ts'

// ── CRC-32 ───────────────────────────────────────────────────────────────────

const CRC_TABLE = (() => {
  const table = new Uint32Array(256)
  for (let i = 0; i < 256; i += 1) {
    let crc = i
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? 0xedb88320 ^ (crc >>> 1) : crc >>> 1
    }
    table[i] = crc >>> 0
  }
  return table
})()

export function crc32(bytes: Uint8Array, seed = 0xffffffff): number {
  let state = seed
  for (const byte of bytes) {
    state = (CRC_TABLE[(state ^ byte) & 0xff] ^ (state >>> 8)) >>> 0
  }
  return state >>> 0
}

/** The finished value: the running state, inverted. */
export function crc32Final(state: number): number {
  return (state ^ 0xffffffff) >>> 0
}

/** The CRC-32 of one buffer, start to finish. */
export function crc32Of(bytes: Uint8Array): number {
  return crc32Final(crc32(bytes))
}

// ── The record ───────────────────────────────────────────────────────────────

export const PREFIX = 24
export const CYCLE_FIXED = 176
export const MAX_TRIPPED = 8
export const MAX_STREAMS = 6
export const STREAM_NAME = 16
export const EVENT_SUBJECT = 64
export const EVENT_DETAIL = 96
export const NO_CAPABILITY = 0xffff

const FRAME_BODY = 48 + MAX_STREAMS * (STREAM_NAME + 8)
const EVENT_BODY = 4 + EVENT_SUBJECT + EVENT_DETAIL
const GAP_BODY = 24

/** Guard names in evaluation order (§7.1). A guard's wire code is its one-based position here. */
export const GUARDS = [
  'watchdog', 'envelope', 'rate_limit', 'workspace',
  'self_collision', 'balance', 'proximity', 'mode_gate',
] as const

/** Responses weakest to strongest; the wire code is the one-based position, 0 meaning none. */
export const RESPONSES = ['clamp', 'hold', 'stop_cat2', 'stop_cat1', 'stop_cat0'] as const

export const KINDS = { cycle: 1, frame: 2, event: 3, gap: 4 } as const

export const EVENT_CODES = [
  'lease_granted', 'lease_released', 'lease_preempted', 'estop', 'estop_cleared',
  'package_started', 'package_stopped', 'grant_changed', 'ota', 'clock_step',
  'recording_started', 'sealed',
] as const

export const GAP_REASONS = ['writer_behind', 'encoder_behind', 'not_recording', 'write_failed'] as const

export interface Shape {
  joints: number
  channels: number
}

function divCeil(value: number, by: number): number {
  return Math.ceil(value / by)
}

export function recordSize(shape: Shape): number {
  const cycle = CYCLE_FIXED + shape.channels * 13 + divCeil(shape.channels, 8) + shape.joints * 12
  const largest = Math.max(cycle, PREFIX + FRAME_BODY, PREFIX + EVENT_BODY, PREFIX + GAP_BODY)
  return divCeil(largest, 8) * 8
}

function cycleOffsets(shape: Shape) {
  const desired = CYCLE_FIXED
  const command = desired + shape.channels * 4
  const mask = command + shape.channels * 8
  const clamped = mask + shape.channels
  const position = clamped + divCeil(shape.channels, 8)
  const velocity = position + shape.joints * 4
  const effort = velocity + shape.joints * 4
  return { desired, command, mask, clamped, position, velocity, effort }
}

/** A little-endian writer over a fixed buffer, so every offset in this file reads like the Rust. */
class Cursor {
  readonly bytes: Uint8Array
  private readonly view: DataView

  constructor(size: number) {
    this.bytes = new Uint8Array(size)
    this.view = new DataView(this.bytes.buffer)
  }

  static over(bytes: Uint8Array): Cursor {
    const cursor = Object.create(Cursor.prototype) as Cursor
    Object.assign(cursor, { bytes, view: new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength) })
    return cursor
  }

  u8(at: number, value: number): void { this.view.setUint8(at, value) }
  u16(at: number, value: number): void { this.view.setUint16(at, value, true) }
  u32(at: number, value: number): void { this.view.setUint32(at, value >>> 0, true) }
  i32(at: number, value: number): void { this.view.setInt32(at, value, true) }
  u64(at: number, value: bigint): void { this.view.setBigUint64(at, value, true) }
  f32(at: number, value: number): void { this.view.setFloat32(at, value, true) }
  f64(at: number, value: number): void { this.view.setFloat64(at, value, true) }

  raw(at: number, source: Uint8Array): void { this.bytes.set(source, at) }

  /**
   * Write `text` into `width` bytes, NUL-padded, truncated on a character boundary.
   *
   * The boundary matters: a cut inside a multi-byte sequence puts invalid UTF-8 in the ring, and
   * every reader then has to decide what to do about it. A continuation byte is `10xxxxxx`, so the
   * cut walks back off any it lands on.
   */
  text(at: number, width: number, value: string): void {
    const encoded = new TextEncoder().encode(value)
    let end = Math.min(encoded.length, width)
    while (end > 0 && (encoded[end] & 0xc0) === 0x80) end -= 1
    this.bytes.fill(0, at, at + width)
    this.bytes.set(encoded.subarray(0, end), at)
  }
}

export interface Cycle {
  tick: bigint
  observation: Uint8Array
  policy: number
  stateFlags: number
  faultCode: number
  rhalResult: number
  stateSeq: number
  stateStampNs: bigint
  commandStampNs: bigint
  commandSeq: number
  mode: number
  advisory: boolean
  response: string | null
  tripped: string[]
  capability: number
  nearestPersonM: number
  basePose: number[]
  baseTwist: number[]
  desired: number[]
  command: number[]
  mask: number[]
  clamped: boolean[]
  position: number[]
  velocity: number[]
  effort: number[]
}

export interface Frame {
  observation: Uint8Array
  targetNs: bigint
  worstSkewUs: number
  streamsOmitted: number
  streams: { name: string; ageUs: number; stale: boolean }[]
}

export interface EventRecord {
  code: string
  subject: string
  detail: string
}

export interface GapRecord {
  dropped: bigint
  firstTick: bigint
  reason: string
}

export type Entry =
  | { kind: 'cycle'; cycle: Cycle }
  | { kind: 'frame'; frame: Frame }
  | { kind: 'event'; event: EventRecord }
  | { kind: 'gap'; gap: GapRecord }

/** Encode one record into a fresh slot of exactly `recordSize(shape)` bytes. */
export function encodeRecord(
  shape: Shape, sequence: bigint, monoNs: bigint, entry: Entry,
): Uint8Array {
  const slot = new Cursor(recordSize(shape))
  slot.u8(4, KINDS[entry.kind])
  slot.u8(5, 1)
  slot.u64(8, sequence)
  slot.u64(16, monoNs)

  if (entry.kind === 'cycle') encodeCycle(shape, slot, entry.cycle)
  else if (entry.kind === 'frame') encodeFrame(slot, entry.frame)
  else if (entry.kind === 'event') encodeEvent(slot, entry.event)
  else {
    slot.u64(PREFIX, entry.gap.dropped)
    slot.u64(PREFIX + 8, entry.gap.firstTick)
    slot.u16(PREFIX + 16, GAP_REASONS.indexOf(entry.gap.reason as never) + 1)
  }

  slot.u32(0, crc32Of(slot.bytes.subarray(4)))
  return slot.bytes
}

function encodeCycle(shape: Shape, slot: Cursor, cycle: Cycle): void {
  const check = (field: string, found: number, expected: number) => {
    if (found !== expected) {
      throw new Error(`black box: ${field} holds ${found} values, and the ring's shape says ${expected}`)
    }
  }
  check('desired', cycle.desired.length, shape.channels)
  check('command', cycle.command.length, shape.channels)
  check('mask', cycle.mask.length, shape.channels)
  check('clamped', cycle.clamped.length, shape.channels)
  check('position', cycle.position.length, shape.joints)
  check('velocity', cycle.velocity.length, shape.joints)
  check('effort', cycle.effort.length, shape.joints)

  slot.u64(24, cycle.tick)
  slot.raw(32, cycle.observation)
  slot.u16(64, cycle.policy)
  slot.u16(66, cycle.stateFlags)
  slot.u32(68, cycle.faultCode)
  slot.i32(72, cycle.rhalResult)
  slot.u32(76, cycle.stateSeq)
  slot.u64(80, cycle.stateStampNs)
  slot.u64(88, cycle.commandStampNs)
  slot.u32(96, cycle.commandSeq)
  slot.u8(100, cycle.mode)
  slot.u8(101, cycle.advisory ? 1 : 0)
  slot.u8(102, cycle.response === null ? 0 : RESPONSES.indexOf(cycle.response as never) + 1)
  slot.u8(103, Math.min(cycle.tripped.length, MAX_TRIPPED))
  cycle.tripped.slice(0, MAX_TRIPPED).forEach((guard, index) => {
    slot.u8(104 + index, GUARDS.indexOf(guard as never) + 1)
  })
  slot.u16(112, cycle.capability)
  slot.f32(116, cycle.nearestPersonM)
  cycle.basePose.forEach((value, index) => slot.f32(120 + index * 4, value))
  cycle.baseTwist.forEach((value, index) => slot.f32(148 + index * 4, value))

  const at = cycleOffsets(shape)
  cycle.desired.forEach((value, index) => slot.f32(at.desired + index * 4, value))
  cycle.command.forEach((value, index) => slot.f64(at.command + index * 8, value))
  cycle.mask.forEach((value, index) => slot.u8(at.mask + index, value))
  cycle.clamped.forEach((was, index) => {
    if (was) slot.bytes[at.clamped + Math.floor(index / 8)] |= 1 << (index % 8)
  })
  cycle.position.forEach((value, index) => slot.f32(at.position + index * 4, value))
  cycle.velocity.forEach((value, index) => slot.f32(at.velocity + index * 4, value))
  cycle.effort.forEach((value, index) => slot.f32(at.effort + index * 4, value))
}

function encodeFrame(slot: Cursor, frame: Frame): void {
  slot.raw(PREFIX, frame.observation)
  slot.u64(PREFIX + 32, frame.targetNs)
  slot.u32(PREFIX + 40, frame.worstSkewUs)
  const written = Math.min(frame.streams.length, MAX_STREAMS)
  slot.u16(PREFIX + 44, written)
  slot.u16(PREFIX + 46, frame.streamsOmitted + Math.max(0, frame.streams.length - MAX_STREAMS))
  frame.streams.slice(0, MAX_STREAMS).forEach((stream, index) => {
    const at = PREFIX + 48 + index * (STREAM_NAME + 8)
    slot.text(at, STREAM_NAME, stream.name)
    slot.u32(at + STREAM_NAME, stream.ageUs)
    slot.u8(at + STREAM_NAME + 4, stream.stale ? 1 : 0)
  })
}

function encodeEvent(slot: Cursor, event: EventRecord): void {
  slot.u16(PREFIX, EVENT_CODES.indexOf(event.code as never) + 1)
  slot.text(PREFIX + 4, EVENT_SUBJECT, event.subject)
  slot.text(PREFIX + 4 + EVENT_SUBJECT, EVENT_DETAIL, event.detail)
}

// ── The ring header ──────────────────────────────────────────────────────────

export const MAGIC = new TextEncoder().encode('SKYNETBB')
export const HEADER_FIXED = 184
export const HEADER_ALIGN = 4096
export const ROBOT_ID = 64
export const TOKEN_NAME = 48
export const CHANNEL_NAME = 48
export const FORMAT_VERSION = 1

const AT = {
  MAGIC: 0, VERSION: 8, HEADER_LEN: 12, RECORD_SIZE: 16, CAPACITY: 20,
  JOINTS: 24, CHANNELS: 26, TIER: 28, ENFORCEMENT: 29, SIMULATOR: 30,
  EMBODIMENT: 32, OS_IMAGE: 64, ROBOT_ID: 96, DIGEST_CAPACITY: 160,
  CAPABILITY_CAPACITY: 162, WALL_EPOCH_NS: 164, MONO_NS: 172, CRC: 180,
} as const

export interface Geometry {
  shape: Shape
  capacity: number
  digests: number
  capabilities: number
}

export function headerLen(geometry: Geometry): number {
  const raw = HEADER_FIXED
    + geometry.digests * 32
    + geometry.capabilities * TOKEN_NAME
    + geometry.shape.channels * CHANNEL_NAME
  return divCeil(raw, HEADER_ALIGN) * HEADER_ALIGN
}

export interface Meta {
  robotId: string
  embodiment: Uint8Array
  osImage: Uint8Array
  tier: number
  enforcement: number
  simulator: boolean
  wallEpochNs: bigint
  monoNs: bigint
  channels: string[]
}

/** The header a ring is created with, including the two tables. */
export function encodeHeader(
  geometry: Geometry, meta: Meta, digests: Uint8Array[] = [], capabilities: string[] = [],
): Uint8Array {
  const length = headerLen(geometry)
  const header = new Cursor(length)
  header.raw(AT.MAGIC, MAGIC)
  header.u16(AT.VERSION, FORMAT_VERSION)
  header.u32(AT.HEADER_LEN, length)
  header.u32(AT.RECORD_SIZE, recordSize(geometry.shape))
  header.u32(AT.CAPACITY, geometry.capacity)
  header.u16(AT.JOINTS, geometry.shape.joints)
  header.u16(AT.CHANNELS, geometry.shape.channels)
  header.u8(AT.TIER, meta.tier)
  header.u8(AT.ENFORCEMENT, meta.enforcement)
  header.u8(AT.SIMULATOR, meta.simulator ? 1 : 0)
  header.raw(AT.EMBODIMENT, meta.embodiment)
  header.raw(AT.OS_IMAGE, meta.osImage)
  header.text(AT.ROBOT_ID, ROBOT_ID, meta.robotId)
  header.u16(AT.DIGEST_CAPACITY, geometry.digests)
  header.u16(AT.CAPABILITY_CAPACITY, geometry.capabilities)
  header.u64(AT.WALL_EPOCH_NS, meta.wallEpochNs)
  header.u64(AT.MONO_NS, meta.monoNs)

  const digestBase = HEADER_FIXED
  digests.forEach((digest, index) => header.raw(digestBase + index * 32, digest))
  const tokenBase = digestBase + geometry.digests * 32
  capabilities.forEach((token, index) => header.text(tokenBase + index * TOKEN_NAME, TOKEN_NAME, token))
  const channelBase = tokenBase + geometry.capabilities * TOKEN_NAME
  meta.channels.slice(0, geometry.shape.channels).forEach((name, index) => {
    header.text(channelBase + index * CHANNEL_NAME, CHANNEL_NAME, name)
  })

  header.u32(AT.CRC, crc32Of(header.bytes))
  return header.bytes
}

// ── The channel schemas (§26, §31.2) ─────────────────────────────────────────

const field = (type: string, description: string) => ({ type, description })
const maybe = (type: string, description: string) => ({ type: [type, 'null'], description })
const arrayOf = (items: unknown, description: string) => ({ type: 'array', items, description })
const numberArray = (description: string) =>
  arrayOf({ type: ['number', 'null'] }, description)
const object = (properties: Record<string, unknown>, required: string[], description: string) =>
  ({ type: 'object', description, properties, required })

const mono = () => field(
  'integer',
  'CLOCK_MONOTONIC_RAW at the instant this record was produced. The time base every control '
  + "decision was made against (§35.1); MCAP's log_time is the wall-clock rendering of it.",
)

const channelValues = (description: string) => arrayOf(
  object({
    channel: field('string', "The action channel's name."),
    value: maybe('number', 'The value on that channel.'),
    hold: field(
      'boolean',
      'True where the mask says hold this joint at its current setpoint — the OS saying nothing '
      + 'rather than saying zero (§24).',
    ),
  }, ['channel', 'value', 'hold'], "One channel's value."),
  description,
)

/** The six channels, in id order. Ids are fixed for the life of the format. */
export function wire(): { id: number; topic: string; name: string; definition: string }[] {
  const documents: [number, string, string, unknown][] = [
    [1, '/sk/state', 'sk_state', object({
      mono_ns: mono(),
      stamp_ns: field('integer', 'rhal_state.stamp_ns: CLOCK_MONOTONIC_RAW at acquisition, never at return (§24).'),
      seq: field('integer', 'rhal_state.seq.'),
      flags: field('integer', 'Bitwise-or of the RHAL flags: estop, fault, degraded, braked.'),
      fault_code: field('integer', 'Vendor-specific, surfaced verbatim (§24).'),
      position_rad: numberArray('Measured joint positions.'),
      velocity_rad_s: numberArray('Measured velocities; null where the joint does not measure it.'),
      effort_nm: numberArray('Measured efforts; null where unmeasured.'),
      base_pose: numberArray('x y z qx qy qz qw; all null when the base is not localised.'),
      base_twist: numberArray('vx vy vz wx wy wz.'),
    }, [
      'mono_ns', 'stamp_ns', 'seq', 'flags', 'fault_code',
      'position_rad', 'velocity_rad_s', 'effort_nm', 'base_pose', 'base_twist',
    ], "One sample of the robot's state, as the RHAL reported it (§24).")],

    [2, '/sk/desired', 'sk_desired', object({
      mono_ns: mono(),
      tick: field('integer', 'The control tick.'),
      stamp_ns: field('integer', 'When the policy produced this command, not when it was sent.'),
      seq: field('integer', 'rhal_command.seq; must advance (§24).'),
      mode: field('string', 'position, velocity, torque, or unknown.'),
      policy_digest: field(
        'string',
        'Hex SHA-256 of the package version that produced this, or empty when the recording'
        + "'s digest table did not hold it.",
      ),
      observation: field(
        'string',
        'Hex SHA-256 of the observation frame the policy was given. A hash, never the bytes: an '
        + 'hour of three-camera video is not something the robot can keep (§26).',
      ),
      values: channelValues('What the policy asked for.'),
    }, ['mono_ns', 'tick', 'stamp_ns', 'seq', 'mode', 'policy_digest', 'observation', 'values'],
      'What the policy asked for, before the Safety Kernel saw it.')],

    [3, '/sk/verdict', 'sk_verdict', object({
      mono_ns: mono(),
      tick: field('integer', 'The control tick.'),
      response: maybe(
        'string',
        'clamp, hold, stop_cat2, stop_cat1 or stop_cat0 — the strongest response any tripped guard '
        + 'demanded. Null when nothing tripped, which is a fact rather than an absence and so is '
        + 'present-and-null.',
      ),
      tripped: arrayOf(field(
        'string',
        'A guard name, in evaluation order: watchdog, envelope, rate_limit, workspace, '
        + 'self_collision, balance, proximity, mode_gate (§7.1).',
      ), 'Guards that tripped.'),
      clamped: arrayOf(object({
        channel: field('string', 'Which channel was reduced.'),
        from: maybe('number', 'What the policy asked for.'),
        to: maybe('number', 'What the kernel sent instead.'),
      }, ['channel', 'from', 'to'], 'One clamped channel, and by how much.'),
        'Channels whose values were reduced.'),
      advisory: field(
        'boolean',
        'True when the tier could only advise: the command was forwarded unchanged despite a trip. '
        + 'Always false at host and certified (§42).',
      ),
    }, ['mono_ns', 'tick', 'response', 'tripped', 'clamped', 'advisory'],
      'What the Safety Kernel decided this cycle.')],

    [4, '/sk/command', 'sk_command', object({
      mono_ns: mono(),
      tick: field('integer', 'The control tick.'),
      values: channelValues('What actually went to write_command.'),
      rhal_result: field(
        'integer',
        'What write_command returned: 0, or a negative errno. -1 EPERM because the drive is faulted '
        + 'is a different fact from -110 ETIMEDOUT (§24).',
      ),
    }, ['mono_ns', 'tick', 'values', 'rhal_result'],
      'What was sent to the hardware, and what the hardware said about it.')],

    [5, '/sk/frame', 'sk_frame', object({
      mono_ns: mono(),
      observation: field('string', 'Hex SHA-256 of the assembled frame.'),
      target_ns: field('integer', 'The instant the assembler targeted.'),
      worst_skew_us: field(
        'integer',
        "The worst skew between any stream and that instant. Recorded so that 'the policy behaved "
        + "oddly' can be checked against 'the frame it was given was 80 ms skewed' months later "
        + '(§25.1).',
      ),
      streams_omitted: field('integer', 'Streams the record could not carry, counted rather than dropped.'),
      streams: arrayOf(object({
        name: field('string', "The stream's name."),
        age_us: field(
          'integer',
          "How far back this stream's sample was, relative to the instant the assembler targeted.",
        ),
        stale: field(
          'boolean',
          "Whether that exceeded the policy's declared staleness budget, which is a hold rather "
          + 'than a silently older frame (§25.1).',
        ),
      }, ['name', 'age_us', 'stale'], "One stream's contribution to the frame."), 'Per-stream ages.'),
    }, ['mono_ns', 'observation', 'target_ns', 'worst_skew_us', 'streams_omitted', 'streams'],
      'One assembled observation frame, and how well aligned it was.')],

    [6, '/sk/event', 'sk_event', object({
      mono_ns: mono(),
      event: field(
        'string',
        'lease_granted, lease_released, lease_preempted, estop, estop_cleared, package_started, '
        + 'package_stopped, grant_changed, ota, clock_step, recording_started, sealed, or gap.',
      ),
      subject: field('string', 'What it happened to: a package id, a capability token, a slot.'),
      detail: field('string', 'A sentence about it.'),
      dropped: field(
        'integer',
        'Gap events only: how many records were not written. A gap annotated as a gap is honest; a '
        + 'gap that looks like a quiet period is not (§47.4).',
      ),
      first_tick: field('integer', 'Gap events only: the first tick that went unrecorded.'),
      reason: field(
        'string',
        'Gap events only: writer_behind, encoder_behind, not_recording or write_failed.',
      ),
    }, ['mono_ns', 'event', 'subject', 'detail'],
      'Something that changed, and every gap in the recording.')],
  ]

  return documents.map(([id, topic, name, document]) => ({
    id, topic, name, definition: canonical(document),
  }))
}

// ── The MCAP writer ──────────────────────────────────────────────────────────

export const MCAP_MAGIC = new Uint8Array([0x89, 0x4d, 0x43, 0x41, 0x50, 0x30, 0x0d, 0x0a])

export const OP = {
  HEADER: 0x01, FOOTER: 0x02, SCHEMA: 0x03, CHANNEL: 0x04, MESSAGE: 0x05,
  STATISTICS: 0x0b, METADATA: 0x0c, SUMMARY_OFFSET: 0x0e, DATA_END: 0x0f,
} as const

class Bytes {
  private parts: number[] = []

  push(...values: number[]): void { this.parts.push(...values) }
  u16(value: number): void { this.push(value & 0xff, (value >>> 8) & 0xff) }
  u32(value: number): void {
    this.push(value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff)
  }

  u64(value: bigint): void {
    for (let i = 0n; i < 8n; i += 1n) this.push(Number((value >> (i * 8n)) & 0xffn))
  }

  raw(source: Uint8Array): void { for (const byte of source) this.push(byte) }
  string(value: string): void {
    const encoded = new TextEncoder().encode(value)
    this.u32(encoded.length)
    this.raw(encoded)
  }

  get bytes(): Uint8Array { return new Uint8Array(this.parts) }
  get length(): number { return this.parts.length }
}

export interface Message {
  channelId: number
  logTime: bigint
  payload: string
}

/**
 * Write one MCAP file: magic, header, schemas, channels, metadata, messages, data end, summary,
 * summary offsets, footer, magic. Unchunked and uncompressed, exactly as the Rust edition writes it.
 */
export function writeMcap(
  library: string,
  schemas: { id: number; name: string; definition: string }[],
  channels: { id: number; schemaId: number; topic: string }[],
  metadata: { name: string; pairs: [string, string][] }[],
  messages: Message[],
): Uint8Array {
  const out = new Bytes()
  const record = (opcode: number, body: Bytes) => {
    out.push(opcode)
    out.u64(BigInt(body.length))
    out.raw(body.bytes)
  }
  const schemaBody = (schema: { id: number; name: string; definition: string }) => {
    const body = new Bytes()
    body.u16(schema.id)
    body.string(schema.name)
    body.string('jsonschema')
    const encoded = new TextEncoder().encode(schema.definition)
    body.u32(encoded.length)
    body.raw(encoded)
    return body
  }
  const channelBody = (channel: { id: number; schemaId: number; topic: string }) => {
    const body = new Bytes()
    body.u16(channel.id)
    body.u16(channel.schemaId)
    body.string(channel.topic)
    body.string('json')
    body.u32(0)
    return body
  }

  out.raw(MCAP_MAGIC)
  const header = new Bytes()
  header.string('')
  header.string(library)
  record(OP.HEADER, header)

  for (const schema of schemas) record(OP.SCHEMA, schemaBody(schema))
  for (const channel of channels) record(OP.CHANNEL, channelBody(channel))

  for (const entry of metadata) {
    const map = new Bytes()
    for (const [key, value] of entry.pairs) {
      map.string(key)
      map.string(value)
    }
    const body = new Bytes()
    body.string(entry.name)
    body.u32(map.length)
    body.raw(map.bytes)
    record(OP.METADATA, body)
  }

  const sequences = new Map<number, number>(channels.map(channel => [channel.id, 0]))
  const counts = new Map<number, bigint>(channels.map(channel => [channel.id, 0n]))
  let first: bigint | null = null
  let last = 0n
  for (const message of messages) {
    const sequence = (sequences.get(message.channelId) ?? 0) + 1
    sequences.set(message.channelId, sequence)
    counts.set(message.channelId, (counts.get(message.channelId) ?? 0n) + 1n)
    first = first === null || message.logTime < first ? message.logTime : first
    last = message.logTime > last ? message.logTime : last

    const body = new Bytes()
    body.u16(message.channelId)
    body.u32(sequence)
    body.u64(message.logTime)
    body.u64(message.logTime)
    body.raw(new TextEncoder().encode(message.payload))
    record(OP.MESSAGE, body)
  }

  const dataEnd = new Bytes()
  dataEnd.u32(crc32Of(out.bytes))
  record(OP.DATA_END, dataEnd)

  const summaryStart = BigInt(out.length)
  const schemaStart = BigInt(out.length)
  for (const schema of schemas) record(OP.SCHEMA, schemaBody(schema))
  const schemaLen = BigInt(out.length) - schemaStart

  const channelStart = BigInt(out.length)
  for (const channel of channels) record(OP.CHANNEL, channelBody(channel))
  const channelLen = BigInt(out.length) - channelStart

  const statisticsStart = BigInt(out.length)
  const statistics = new Bytes()
  statistics.u64([...counts.values()].reduce((sum, count) => sum + count, 0n))
  statistics.u16(schemas.length)
  statistics.u32(channels.length)
  statistics.u32(0)
  statistics.u32(metadata.length)
  statistics.u32(0)
  statistics.u64(first ?? 0n)
  statistics.u64(last)
  const perChannel = new Bytes()
  for (const [id, count] of counts) {
    perChannel.u16(id)
    perChannel.u64(count)
  }
  statistics.u32(perChannel.length)
  statistics.raw(perChannel.bytes)
  record(OP.STATISTICS, statistics)
  const statisticsLen = BigInt(out.length) - statisticsStart

  const summaryOffsetStart = BigInt(out.length)
  for (const [opcode, start, length] of [
    [OP.SCHEMA, schemaStart, schemaLen],
    [OP.CHANNEL, channelStart, channelLen],
    [OP.STATISTICS, statisticsStart, statisticsLen],
  ] as [number, bigint, bigint][]) {
    const body = new Bytes()
    body.push(opcode)
    body.u64(start)
    body.u64(length)
    record(OP.SUMMARY_OFFSET, body)
  }

  const footer = new Bytes()
  footer.push(OP.FOOTER)
  footer.u64(20n)
  footer.u64(summaryStart)
  footer.u64(summaryOffsetStart)
  const summarySection = out.bytes.subarray(Number(summaryStart))
  const summaryCrc = crc32Final(crc32(footer.bytes, crc32(summarySection)))
  footer.u32(summaryCrc)
  out.raw(footer.bytes)

  out.raw(MCAP_MAGIC)
  return out.bytes
}

/**
 * The file's record opcodes, in order, as two-digit hex.
 *
 * Pinned beside the digest in the corpus because a digest says *that* two editions disagree and this
 * says *where*. It also checks that the framing walks: a file whose record lengths do not land
 * exactly on the trailing magic is a file no reader will open.
 */
export function opcodes(bytes: Uint8Array): string {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const end = bytes.length - 8
  const found: string[] = []
  let at = 8
  while (at < end) {
    if (at + 9 > end) throw new Error(`mcap: a record header runs past the end of the file at ${at}`)
    found.push(bytes[at].toString(16).padStart(2, '0'))
    at += 9 + Number(view.getBigUint64(at + 1, true))
  }
  if (at !== end) throw new Error(`mcap: the framing ended at ${at}, the trailing magic at ${end}`)
  return found.join(',')
}

/** Lower-case hex, as the sealed file's metadata spells a digest. */
export function hex(bytes: Uint8Array): string {
  return [...bytes].map(byte => byte.toString(16).padStart(2, '0')).join('')
}
