/*
 * The on-robot console (§32.2).
 *
 * Vanilla, because §32.2 says "no build step, no CDN" and means it: this file is compiled into the
 * daemon binary and served to a browser on a robot that may never have had internet.
 *
 * ── The rule this file exists to enforce ────────────────────────────────────
 *
 * > It must never show a value it did not receive.
 *
 * That is one sentence and three separate mechanisms:
 *
 *   1. `absent()` — a field the daemon returned as null renders as the *reason* it is null. Never
 *      a zero, never a dash, never an empty cell. §33.6 is explicit that zero guard trips and no
 *      measurement of guard trips are opposite facts, and a console that renders them the same way
 *      undoes that distinction at the last possible moment.
 *   2. `Feed` — every panel is drawn from a feed that remembers when its last answer arrived. The
 *      age is printed beside the heading and the panel goes `.stale` past a threshold. A poll that
 *      fails does not clear the panel — losing the last thing the robot said is worse — but it
 *      cannot leave it looking live either.
 *   3. Panels with no data source at all render what is missing and why, rather than a plausible
 *      shape. The guards panel used to be the whole of that case and is now the opposite one: it
 *      draws §7.6's resolved limits, and the part that still has no source — trip counts — is
 *      written under the table rather than printed as a zero.
 *
 * ── Escaping ────────────────────────────────────────────────────────────────
 *
 * Every value that reaches the DOM goes through `textContent`, never `innerHTML`. Package ids,
 * grant tokens and failure reasons all originate outside this file, and the page's CSP forbids
 * inline script — but the cheapest way to keep that true is to never build markup from a string.
 */

'use strict'

// ── Small helpers ────────────────────────────────────────────────────────────

const $ = id => document.getElementById(id)

function el(tag, className, text) {
  const node = document.createElement(tag)
  if (className) node.className = className
  if (text !== undefined && text !== null) node.textContent = String(text)
  return node
}

function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild)
  return node
}

/**
 * A value nobody measured, rendered as the reason rather than as a number.
 *
 * The one place this file refuses to be terse. `—` would be shorter and would read as zero.
 */
function absent(reason) {
  return el('span', 'absent', reason)
}

function pill(text, kind) {
  return el('span', 'pill ' + (kind || 'off'), text)
}

/**
 * How to colour a §27 state, by what it does rather than by what it is called.
 *
 * `safe` is the one worth writing down: it is a reassuring word and, on a robot whose body cannot
 * be reached, it is the state that says so. `running` and `degraded` are the two where a policy is
 * commanding joints, and they get the attention.
 */
function stateTone(state) {
  if (state === 'running' || state === 'degraded') return 'warn'
  if (state === 'stopping' || state === 'safe') return 'bad'
  if (state === 'hold') return 'ok'
  return 'off'
}

function duration(ms) {
  if (!Number.isFinite(ms) || ms < 0) return null
  const s = Math.floor(ms / 1000)
  if (s < 60) return s + 's'
  const m = Math.floor(s / 60)
  if (m < 60) return m + 'm ' + (s % 60) + 's'
  const h = Math.floor(m / 60)
  if (h < 24) return h + 'h ' + (m % 60) + 'm'
  return Math.floor(h / 24) + 'd ' + (h % 24) + 'h'
}

function bytes(n) {
  if (!Number.isFinite(n)) return null
  if (n < 1024) return n + ' B'
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KiB'
  return (n / (1024 * 1024)).toFixed(1) + ' MiB'
}

// ── The token ────────────────────────────────────────────────────────────────
//
// `sessionStorage`, not `localStorage`. A robot console is a shared screen in a workshop, and a
// credential that outlives the tab is a credential the next person inherits. §39.2 issues these at
// pairing; this page only borrows one.

const TOKEN_KEY = 'skynet.console.token'

function readToken() {
  try {
    return sessionStorage.getItem(TOKEN_KEY) || ''
  } catch (_) {
    // A browser with storage disabled is a browser this console still has to work in; the token
    // simply does not survive a reload.
    return memoryToken
  }
}

let memoryToken = ''

function writeToken(value) {
  memoryToken = value
  try {
    if (value) sessionStorage.setItem(TOKEN_KEY, value)
    else sessionStorage.removeItem(TOKEN_KEY)
  } catch (_) {
    /* held in memory only */
  }
}

// ── Talking to the daemon ────────────────────────────────────────────────────

async function call(method, path, body) {
  const headers = {}
  const token = readToken()
  if (token) headers['authorization'] = 'Bearer ' + token
  if (body !== undefined) headers['content-type'] = 'application/json'

  const response = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    cache: 'no-store',
  })

  let payload = null
  const text = await response.text()
  if (text) {
    try {
      payload = JSON.parse(text)
    } catch (_) {
      payload = null
    }
  }
  return { status: response.status, ok: response.ok, body: payload, text }
}

/**
 * One panel's source of truth, and its age.
 *
 * The age is the whole point. A feed holds the last answer *and* when it arrived, so a panel can
 * always say how old what it is showing is — which is what turns "never show a value it did not
 * receive" from an intention into something the page structurally cannot violate.
 */
class Feed {
  constructor(name, path, render) {
    this.name = name
    this.path = path
    this.render = render
    this.value = null
    // The last answer, whatever its status. `value` is only ever a *successful* one, because a
    // panel that rendered a refusal as data would draw a robot's state out of an error page — but
    // a refusal from this daemon is a document with fields in it, and throwing it away is how a
    // panel ends up printing two words where the daemon wrote a paragraph.
    this.body = null
    this.at = 0
    this.status = 'pending'
    this.detail = ''
  }

  async poll() {
    try {
      const result = await call('GET', this.path)
      this.body = result.body
      if (result.ok) {
        this.value = result.body
        this.at = Date.now()
        this.status = 'ok'
        this.detail = ''
      } else if (result.status === 401 || result.status === 403) {
        this.status = 'denied'
        this.detail = reasonOf(result, 'this needs a token')
      } else {
        this.status = 'error'
        this.detail = reasonOf(result, 'HTTP ' + result.status)
      }
    } catch (error) {
      this.body = null
      this.status = 'unreachable'
      this.detail = String(error && error.message ? error.message : error)
    }
  }

  ageMs() {
    return this.at ? Date.now() - this.at : null
  }
}

/**
 * The sentence a refusal carries, not the status line's reason phrase.
 *
 * `api::error` writes both: `error` is "Service Unavailable" and `detail` is why. This console read
 * the first for eight sessions, so every failing panel said two words where the daemon had written
 * a paragraph naming the cause and the remedy — the same class of defect as the guards panel and
 * the attestation sentence, and found the same way, by looking at the page.
 */
function reasonOf(result, fallback) {
  const body = result.body || {}
  return body.detail || body.error || fallback
}

// The threshold past which a panel is stale. Three poll periods: one missed answer is a lost
// packet, three is the daemon not answering, and a console that cried stale on every hiccup would
// train an operator to ignore the word.
const POLL_MS = 2000
const STALE_MS = POLL_MS * 3

// ── Panels ───────────────────────────────────────────────────────────────────

function renderStatus(feed) {
  const fields = clear($('status-fields'))
  const note = $('status-note')

  const add = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }

  if (!feed.value) {
    note.hidden = false
    note.textContent = feed.detail || 'no answer from the daemon yet'
    return
  }
  const s = feed.value

  $('identity').textContent =
    (s.edition || '?') + ' ' + (s.version || '?') + ' · contracts ' + (s.contracts || '?')

  add('Edition', s.edition || absent('not reported'))
  add('Daemon version', s.version || absent('not reported'))
  add('OS version', s.os_version || absent('not reported'))

  // §15 awards a tier from evidence. This robot has none, so the field is what it is enforcing —
  // and the console says which of the two it is, because "host tier" over a machine nobody
  // measured is the precise failure this platform exists to avoid.
  const enforcement = el('span')
  enforcement.appendChild(pill(s.enforcement || 'unknown', 'ok'))
  enforcement.appendChild(document.createTextNode(' '))
  enforcement.appendChild(el('span', 'dim', 'enforced, not awarded — no SCTS evidence on this robot'))
  add('Enforcement', enforcement)

  // §27's state machine, which this panel named as unreportable for six sessions because nothing
  // declared it. It is declared now, and the honest report has two halves: the state, and how much
  // of the machine's input anybody actually observed.
  //
  // The pill is coloured by what the state *does* rather than by its name. `safe` sounds
  // reassuring and is the state a robot is in when its body cannot be reached; `running` sounds
  // fine and is the one state where a policy is commanding joints. A console that coloured by the
  // word would be reassuring about exactly the wrong rows.
  const machine = el('span')
  machine.appendChild(pill(s.state || 'unreported', stateTone(s.state)))
  if (s.state_reason) {
    machine.appendChild(document.createTextNode(' '))
    // The reason, always. `safe` alone reads as a robot somebody stopped; `safe · rhal_lost` reads
    // as a robot nothing has looked at, and those are opposite ends of the building.
    machine.appendChild(el('span', 'dim', s.state_reason.replace(/_/g, ' ')))
  }
  add('State machine', machine)

  add('Uptime', duration(s.uptime_ms) || absent('not reported'))
  add('Packages running', Number.isFinite(s.packages_running) ? s.packages_running : absent('not reported'))
  add('Generation', String(s.generation) + ' of ' + String(s.desired_generation) +
    ' wanted (' + (s.desired_source || 'unknown source') + ')')

  const safe = el('span')
  safe.appendChild(pill(s.safe_state === 'yes' ? 'safe' : 'not safe',
    s.safe_state === 'yes' ? 'ok' : 'off'))
  if (s.safe_state_source) {
    safe.appendChild(document.createTextNode(' '))
    safe.appendChild(el('span', 'dim', s.safe_state_source))
  }
  add('Safe state', safe)

  add('Stop reachable', s.stop_available ? pill('yes', 'ok') : pill('NO', 'bad'))
  add('Provisioned', s.provisioned ? pill('yes', 'ok') : pill('no — status and stop only', 'warn'))

  // The daemon's own caveat, carried through rather than dropped. It is the sentence that stops
  // "safe state: yes" being read as something the robot measured.
  if (s.safe_state_note) {
    note.hidden = false
    note.textContent = s.safe_state_note
  } else {
    note.hidden = true
  }
}

/**
 * §27's operating state machine.
 *
 * The panel this console said for six sessions it could not draw, because §27's machine was
 * described in four places and declared in none. It is declared now, and this renders the two
 * halves an operator standing next to a robot needs: **where the machine says the robot is**, and
 * **how much of the machine's input anybody actually looked at**.
 *
 * The second half is not a caveat somebody was being scrupulous about. §33.1 keeps `skynetd` away
 * from the actuator path, so the kernel, the RHAL, the guard set, the stop and the homing state are
 * all on the far side of a line this process may not cross — and every unobserved condition is read
 * at its least capable value. That is why a robot with nothing wrong with it reports `safe`. A page
 * that showed the word without the reason would be describing a robot somebody stopped.
 */
function renderState(feed) {
  const fields = clear($('state-fields'))
  const note = $('state-note')
  const add = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    if (typeof value === 'string') dd.textContent = value
    else dd.appendChild(value)
    fields.appendChild(dd)
  }

  if (!feed.value) {
    note.hidden = false
    note.textContent = feed.detail || 'no answer from the daemon yet'
    return
  }
  const s = feed.value

  const machine = el('span')
  machine.appendChild(pill(s.state || 'unreported', stateTone(s.state)))
  if (s.reason) {
    machine.appendChild(document.createTextNode(' '))
    machine.appendChild(el('span', 'dim', s.reason.replace(/_/g, ' ')))
  }
  add('State', machine)

  // The three properties other sections read off the state, rendered from the daemon's answer
  // rather than re-derived here. A page that decided for itself whether `stopping` was a safe
  // state would be the second definition of §33.4's barrier, which is the failure §31.2 is about.
  add('Commands the body', s.commands ? pill('yes', 'warn') : pill('no', 'ok'))
  add('§33.4 barrier', s.safe_state ? pill('open', 'ok') : pill('shut', 'off'))
  // Sticky, *and whether the stickiness is what is holding it*. §27 makes `safe` require an
  // explicit human action, and a row saying so on a robot whose RHAL is missing would send an
  // operator to clear a stop nobody set — the condition is what is holding it, and clearing would
  // put it straight back. The daemon already distinguishes the two: `safe_sticky` is the reason
  // reported only when every condition would otherwise have let the robot go.
  const human = el('span')
  if (!s.sticky) {
    human.appendChild(pill('no', 'ok'))
  } else if (s.reason === 'safe_sticky') {
    human.appendChild(pill('yes', 'warn'))
    human.appendChild(document.createTextNode(' '))
    human.appendChild(el('span', 'dim', 'nothing else is holding it — a clearance releases it'))
  } else {
    human.appendChild(pill('yes', 'warn'))
    human.appendChild(document.createTextNode(' '))
    human.appendChild(el('span', 'dim',
      'and clearing it now would not release it: ' + String(s.reason || '').replace(/_/g, ' ') +
      ' would put it straight back'))
  }
  add('Needs a human to leave', human)

  // §49.1's warning channel, on the panel about the transitions that demand it. Three states, and
  // the third is the one a two-state rendering would get wrong: a robot that has never entered a
  // commanding state has never been asked to warn, and drawing that as a success would be the same
  // error as an undated health field (§33.6).
  const warning = s.warning || {}
  const heard = el('span')
  if (!warning.declared) {
    heard.appendChild(pill('none declared', 'warn'))
  } else if (warning.last === true) {
    heard.appendChild(pill('sounded', 'ok'))
  } else if (warning.last === false) {
    heard.appendChild(pill('DID NOT SOUND', 'bad'))
  } else {
    heard.appendChild(pill(warning.label || 'declared', 'ok'))
    heard.appendChild(document.createTextNode(' '))
    heard.appendChild(el('span', 'dim', 'never asked to'))
  }
  if (warning.last_detail) {
    heard.appendChild(document.createTextNode(' '))
    heard.appendChild(el('span', 'dim', warning.last_detail))
  }
  add('Warning tone (§49.1)', heard)

  // Which of §27's conditions anybody looked at. The list, not a count, because "1 of 9" tells an
  // operator how bad it is and this tells them which one they have.
  const observed = s.observed || {}
  const names = Object.keys(observed).sort()
  const seen = names.filter(name => observed[name])
  const unseen = names.filter(name => !observed[name])

  const inputs = el('span')
  if (seen.length) {
    inputs.appendChild(pill(seen.join(', '), 'ok'))
  } else {
    inputs.appendChild(absent('nothing'))
  }
  if (unseen.length) {
    inputs.appendChild(document.createTextNode(' '))
    inputs.appendChild(el('span', 'dim', 'not observed: ' + unseen.join(', ')))
  }
  add('Conditions observed', inputs)

  if (s.partial) {
    note.hidden = false
    note.textContent = s.partial
  } else {
    note.hidden = true
  }
}

/**
 * The guards panel, drawn from §7.6's resolved limits.
 *
 * §32.2 asks for three things: which guards are active, which are unavailable *with the exact input
 * missing*, and trip counts this session. The first two are `GET /limits` — the same
 * `resolve_limits` a control loop would call, over this robot's real descriptor, its signed profile
 * and §55.2's cell — and every "missing input" string here is the kernel's own, for this body.
 *
 * The third has nothing to count. §33.1 keeps `skynetd` off the actuator path, so no cycle is
 * evaluated on this robot and there are no trips to total. That is said under the table rather than
 * rendered as `0`, which is the distinction this whole file exists to preserve.
 *
 * ── What this panel replaced ────────────────────────────────────────────────
 *
 * A hand-written list of eight guards and what each was missing, maintained here, drawn entirely as
 * `unavailable`. It had drifted the way such lists do: it said `balance` was *"implemented by no
 * edition"* about a guard `skynet-kernel` implements and reports `not applicable` for on a fixed
 * base, and it drew `envelope` and `mode_gate` as unavailable when both are active on every robot.
 * The panel was confidently wrong about the machine, in a page whose one rule is never to show a
 * value it did not receive.
 *
 * ── The sentence that has to stay ───────────────────────────────────────────
 *
 * None of this is enforced. The limits are composed and reported; no verdict reaches an actuator,
 * because no policy runtime exists to carry a command as far as a guard. An operator reading
 * "active" as "enforcing" is the misreading this panel is most able to cause, so it says so twice:
 * once at the top, and once for `envelope` specifically, which is active on every robot and has
 * nothing to clamp against until a transport plan is in force (§6.3).
 */
function renderGuards(feed) {
  const body = clear($('guards-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer from the daemon yet'))
    return
  }
  const limits = feed.value

  const heading = el('p')
  heading.appendChild(pill('resolved at ' + (limits.tier || '?'), 'off'))
  heading.appendChild(document.createTextNode(' '))
  heading.appendChild(pill(limits.enforced ? 'enforced' : 'enforced by nothing', 'warn'))
  body.appendChild(heading)

  // ── The ceilings, with provenance ─────────────────────────────────────────
  const number = (value, unit) =>
    Number.isFinite(value) ? String(value) + (unit ? ' ' + unit : '') : null

  const ceilings = el('table')
  const chead = el('tr')
  chead.appendChild(el('th', null, 'Ceiling'))
  chead.appendChild(el('th', null, 'In force'))
  ceilings.appendChild(chead)
  for (const [label, value, unit] of [
    ['joint velocity', limits.joint_velocity_max_rad_s, 'rad/s'],
    ['joint torque', limits.joint_torque_max_nm, 'Nm'],
    ['base velocity', limits.base_velocity_max_m_s, 'm/s'],
    ['jerk, per cycle', limits.max_delta_per_cycle, ''],
    ['minimum control rate', limits.min_control_hz, 'Hz'],
  ]) {
    const row = el('tr')
    row.appendChild(el('td', null, label))
    const cell = el('td')
    const rendered = number(value, unit)
    cell.appendChild(rendered ? document.createTextNode(rendered) : absent('nobody declared one'))
    row.appendChild(cell)
    ceilings.appendChild(row)
  }
  body.appendChild(ceilings)

  const provenance = el('p', 'note')
  provenance.appendChild(document.createTextNode('Source: '))
  provenance.appendChild(limits.source
    ? document.createTextNode(limits.source)
    : absent('none cited — a safety limit without provenance is a rumour'))
  body.appendChild(provenance)

  // ── The eight, as the kernel partitioned them ─────────────────────────────
  const list = el('table')
  const head = el('tr')
  head.appendChild(el('th', null, 'Guard'))
  head.appendChild(el('th', null, 'State'))
  head.appendChild(el('th', null, 'Missing input'))
  list.appendChild(head)

  const row = (name, state, tone, missing) => {
    const tr = el('tr')
    tr.appendChild(el('td', null, name))
    const cell = el('td')
    cell.appendChild(pill(state, tone))
    tr.appendChild(cell)
    tr.appendChild(el('td', 'dim', missing || ''))
    list.appendChild(tr)
  }

  for (const name of limits.active_guards || []) {
    // The one active guard that can be idle, and the console is where that is most likely to be
    // misread. `per_channel_note` is present exactly when there are no per-channel ceilings.
    const idle = name === 'envelope' && limits.per_channel_note
    row(name, idle ? 'active · no ceiling' : 'active', idle ? 'warn' : 'ok',
      idle ? 'a per-channel ceiling — §6.3 hands one to the kernel from a transport plan, and no ' +
             'policy is running to have one' : '')
  }
  for (const entry of limits.unavailable_guards || []) {
    row(entry.guard, 'unavailable', 'off', (entry.missing || []).join(', '))
  }
  body.appendChild(list)

  // ── The notices, which are the kernel's own sentences ─────────────────────
  //
  // `guard_unavailable` is dropped, and only that one. The kernel emits it because a consumer with
  // nothing but the notice list would otherwise never learn a guard is off — but this panel has the
  // table above, which says the same thing with the missing input in its own column. Rendering both
  // puts one fact in two places for a reader to reconcile, five times over, and pushes the notices
  // that carry something new off the bottom of the panel.
  for (const notice of limits.notices || []) {
    if (notice.code === 'guard_unavailable') continue
    const note = el('p', 'note')
    note.appendChild(pill(notice.code, 'warn'))
    note.appendChild(document.createTextNode(' ' + (notice.message || '')))
    body.appendChild(note)
  }

  // ── The two things this panel cannot report ───────────────────────────────
  body.appendChild(el('p', 'note',
    'Trip counts are absent, not zero. §33.1 keeps skynetd off the actuator path, so no cycle is ' +
    'evaluated on this robot and there is nothing to total — and a zero here would read as ' +
    '"nothing tripped", which is a different fact from "nothing was measured".'))
  if (limits.enforced_note) body.appendChild(el('p', 'note', limits.enforced_note))
  if (limits.tier_source) body.appendChild(el('p', 'note', 'Tier: ' + limits.tier_source))
  for (const gap of limits.unmeasured || []) {
    body.appendChild(el('p', 'note', gap))
  }
}

/**
 * What body this robot believes it is (§5).
 *
 * Three states, and the difference between the first two is the point. "Nobody has told this robot
 * what it is" is a provisioning step that did not run; "somebody told it and the answer does not
 * work" is a generator that emitted something wrong, and it carries the field. A panel that showed
 * both as an empty box would be hiding the only thing that decides what to do next.
 */
function renderEmbodiment(feed) {
  const body = clear($('embodiment-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const b = feed.value

  if (b.state !== 'present') {
    const heading = el('p')
    heading.appendChild(pill(b.state === 'absent' ? 'no descriptor' : 'descriptor unusable',
      b.state === 'absent' ? 'off' : 'bad'))
    body.appendChild(heading)
    body.appendChild(el('p', 'note', b.detail || ''))
    return
  }

  const fields = el('dl', 'fields')
  const add = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }

  add('Robot', b.robot_slug)
  add('Base', b.base)
  add('Joints', String(b.joints) + (b.dof === b.joints ? '' : ' (dof ' + b.dof + ')'))
  add('Simulated', b.simulator
    ? pill('yes — a benchmark from this body is not a field result', 'warn')
    : pill('no', 'ok'))

  const digest = el('span')
  digest.textContent = b.digest.slice(0, 16) + '…'
  digest.title = b.digest
  add('Digest', digest)

  // §5. A digest identifies the file; a signature identifies who stands behind it. Kept as its own
  // row rather than folded into the digest, because a reader who sees one must not take it for the
  // other — that is the whole reason the vendor signs anything.
  if (b.signature === 'verified') {
    add('Signed by', pill(b.signer, 'ok'))
  } else if (b.signature) {
    add('Signed by', pill(b.signature === 'absent' ? 'nobody' : b.signature,
      b.signature === 'failed' ? 'bad' : 'warn'))
  }

  if (b.frames && b.frames.present) {
    add('Frame graph', b.frames.nodes + ' frames rooted at ' + b.frames.root +
      ', ' + b.frames.convention + ' · ' + b.frames.dynamic + ' dynamic · ' +
      b.frames.collision_bodies + ' with collision geometry')
  } else {
    add('Frame graph', absent((b.frames && b.frames.detail) || 'not declared'))
  }
  body.appendChild(fields)

  // §5 has the vendor sign the descriptor and SCTS checks it. Nothing here did, and the panel says
  // so rather than letting a digest imply provenance it does not carry.
  if (b.signature_note) body.appendChild(el('p', 'note', b.signature_note))
}

function renderPlan(feed) {
  const body = clear($('plan-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
  } else if (feed.value.pass === null) {
    body.appendChild(el('p', 'note', feed.value.note || 'no reconcile pass has run'))
  } else {
    const pass = feed.value.pass
    const summary = el('p', 'note')
    summary.appendChild(document.createTextNode(
      'converged: ' + (pass.converged ? 'yes' : 'no') + ' · ' +
      (pass.applied || []).length + ' applied · ' +
      (pass.deferred || []).length + ' deferred'))
    body.appendChild(summary)

    const rows = []
    for (const action of pass.applied || []) rows.push([action, action.outcome, action.reason])
    for (const action of pass.deferred || []) rows.push([action, 'deferred', action.reason])

    if (rows.length === 0) {
      body.appendChild(el('p', 'note', 'the pass did nothing, which is what a converged robot looks like'))
    } else {
      const table = el('table')
      const head = el('tr')
      for (const label of ['Stage', 'Action', 'Subject', 'Outcome', 'Reason']) {
        head.appendChild(el('th', null, label))
      }
      table.appendChild(head)
      for (const [action, outcome, reason] of rows) {
        const row = el('tr')
        row.appendChild(el('td', 'num', action.stage))
        row.appendChild(el('td', null, action.action))
        row.appendChild(el('td', null, action.subject))
        const cell = el('td')
        cell.appendChild(pill(outcome, outcome === 'applied' || outcome === 'already_true' ? 'ok'
          : outcome === 'deferred' ? 'warn' : 'bad'))
        row.appendChild(cell)
        row.appendChild(el('td', 'dim', reason || ''))
        table.appendChild(row)
      }
      body.appendChild(table)
    }
  }

  // The other plan §32.2 asks for, under its own name in its own subpanel.
  renderTransport()
}

// ── §32.2's Plan panel: the transport plan, per package ─────────────────────

/** Which package the transport subpanel is showing, or '' for none chosen. */
let transportPackage = ''
/** The last answer for it, so a redraw does not refetch. */
let transportAnswer = null

/**
 * The resolved transport plan and the clamps in force, with provenance for every number (§32.2).
 *
 * For a long time this subpanel said "unavailable on this robot" and explained that `skynet
 * transport` computes the same plan on a workstation. That stopped being true when §37.1's
 * `transport` stage started running at install and `GET /limits?package=` started serving what it
 * produced — so the panel asks which package and shows the answer.
 *
 * **A picker rather than a list**, because a plan is per package and fetching one per installed
 * package would put N requests on a link-local socket every two seconds, on a listener §33.7 gives
 * one request per connection. The operator picks; the panel fetches once.
 *
 * The plan is shown for **staged** packages too, and the panel says which. §6.2's whole argument for
 * a pure planner is that it answers "does this run on my robot, and how well" before anything is
 * committed to — and a panel that only spoke about running packages would answer it only after.
 */
function renderTransport() {
  const transport = clear($('transport-body'))
  transport.appendChild(el('h2', null, 'Transport plan'))

  const installed = (packagesFeed.value && packagesFeed.value.packages) || []
  if (installed.length === 0) {
    transport.appendChild(absent('no package is installed or staged on this robot'))
    transport.appendChild(el('p', 'note',
      '§6.3 hands a per-channel ceiling to the Safety Kernel from a plan, and a plan belongs to a ' +
      'package. A robot with none has no per-channel ceiling in force, which the Guards panel ' +
      'above says in the row for the `envelope` guard.'))
    return
  }

  const picker = el('select')
  picker.appendChild(el('option', null, '— choose a package —'))
  for (const entry of installed) {
    const option = el('option', null, entry.id)
    option.value = entry.id
    if (entry.id === transportPackage) option.selected = true
    picker.appendChild(option)
  }
  picker.addEventListener('change', async () => {
    transportPackage = picker.value
    transportAnswer = null
    if (transportPackage) {
      // `encodeURIComponent` and not interpolation: a package id carries a `/`, which is legal in a
      // query value, and anything else in one would be a different request.
      const result = await call('GET', '/limits?package=' + encodeURIComponent(transportPackage))
      transportAnswer = result.ok ? result.body : { error: (result.body && result.body.error) || ('HTTP ' + result.status) }
    }
    renderTransport()
  })
  transport.appendChild(picker)

  if (!transportPackage) return
  if (!transportAnswer) {
    transport.appendChild(el('p', 'note', 'asking…'))
    return
  }
  if (transportAnswer.error) {
    transport.appendChild(el('p', 'bad', transportAnswer.error))
    return
  }
  if (transportAnswer.package_clamp) {
    // Not an error: a driver has no action space, and "there is nothing to plan" is the answer.
    transport.appendChild(absent(transportAnswer.package_clamp))
    return
  }

  const plan = transportAnswer.plan
  if (!plan) {
    transport.appendChild(absent('this package produced no plan'))
    return
  }

  const head = el('p')
  head.appendChild(pill(plan.verdict || '?', plan.verdict === 'refused' ? 'bad' : plan.verdict === 'direct' ? 'ok' : 'warn'))
  head.appendChild(document.createTextNode(
    ' ' + (plan.bindings || []).length + ' channel(s) onto ' + (plan.robot_slug || '?') +
    (Number.isFinite(plan.control_hz) ? ' at ' + plan.control_hz + ' Hz' : '')))
  transport.appendChild(head)
  if (transportAnswer.package_state) {
    transport.appendChild(el('p', 'note',
      'This package is ' + transportAnswer.package_state + '. ' +
      (transportAnswer.package_state === 'staged'
        ? 'Nothing is running under this plan; it is what the package would be held to.'
        : '')))
  }

  const table = el('table')
  const header = el('tr')
  header.appendChild(el('th', null, 'Channel'))
  header.appendChild(el('th', null, 'Method'))
  header.appendChild(el('th', null, 'Target'))
  header.appendChild(el('th', null, 'Ceiling'))
  table.appendChild(header)
  for (const binding of plan.bindings || []) {
    const row = el('tr')
    row.appendChild(el('td', 'dim', binding.source_role || ('channel ' + binding.source_index)))
    const method = el('td')
    method.appendChild(pill(binding.method || '?',
      binding.method === 'blocked' ? 'bad' : binding.method === 'direct' ? 'ok' : 'warn'))
    row.appendChild(method)
    const target = el('td')
    target.appendChild(binding.target
      ? document.createTextNode(binding.target)
      : absent('nothing on this body'))
    row.appendChild(target)
    const ceiling = el('td')
    // Provenance for every number, which is what §32.2 asks for: this ceiling is the joint's own
    // rating out of the descriptor, not a figure the plan invented.
    ceiling.appendChild(Number.isFinite(binding.limit)
      ? document.createTextNode(String(binding.limit))
      : absent('this joint declares no rating'))
    row.appendChild(ceiling)
    table.appendChild(row)
  }
  transport.appendChild(table)

  for (const warning of plan.warnings || []) {
    const note = el('p', 'note')
    note.appendChild(pill(warning.code || 'warning', 'warn'))
    note.appendChild(document.createTextNode(' ' + (warning.message || '')))
    transport.appendChild(note)
  }
}

/**
 * §12's A/B slots, and whether the wanted image may be applied.
 *
 * The verdict is drawn even when it is a refusal with several findings, and all of them are shown:
 * an OS update is the one case where each attempt round the loop costs a reboot, so a panel showing
 * the first problem would cost an operator a reboot per finding.
 *
 * The gated components are called out **above** the findings, because they are true whether or not
 * the update is refused. "This image touches the safety kernel" is what somebody needs before they
 * go and book bench time; the refusal is only what happens if they have not.
 */
/**
 * §10, on one panel: whose this robot is, what it is, and what it is running.
 *
 * The panel never renders a credential, because the route never carries one — `credential_prefix`
 * is the whole of what any surface has a use for, and a masked credential is a credential whose
 * length is on a LAN listener.
 */
function renderIdentity(feed) {
  const body = clear($('identity-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const v = feed.value

  const fields = el('dl', 'fields')
  const row = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }

  // Paired is a fact and attested is a measurement, and they are two pills rather than one status
  // for that reason. A robot can be perfectly paired and attest nothing about what it is (§50.3),
  // and a single word covering both would have to pick which of those to hide.
  row('Paired', pill(v.paired ? 'yes' : 'no', v.paired ? 'ok' : 'warn'))
  row('Attested', pill(v.attested ? 'yes' : 'no', v.attested ? 'ok' : 'warn'))
  row('Namespace', v.namespace || absent('nobody — this robot is unpaired'))
  row('Device', v.device_id || absent('not issued'))
  row('Reports to', v.endpoint || absent('no Hub'))
  row('Credential', v.credential_prefix ? v.credential_prefix + '\u2026' : absent('none held'))
  body.appendChild(fields)

  if (v.fault) body.appendChild(el('p', 'bad', v.fault))

  if (v.attestation && v.attestation.sentence) {
    const line = el('p', v.attestation.complete ? null : 'note', v.attestation.sentence)
    body.appendChild(line)
  }

  for (const finding of v.findings || []) {
    body.appendChild(el('p', finding.severity === 'error' ? 'bad' : 'note', finding.message))
  }
}

/** §11 and §33.8: where the desired state came from, and what the last pull made of it. */
function renderIntent(feed) {
  const body = clear($('intent-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const v = feed.value

  const fields = el('dl', 'fields')
  const row = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }
  // §33.8's rung, coloured by what it means rather than by whether it is the happy one: a robot on
  // `last_known_good` is running the right software for the wrong reason, and `run_nothing` is the
  // safe default rather than a healthy state.
  const rung = v.source || 'unreported'
  row('Source', pill(rung, rung === 'current' ? 'ok' : rung === 'last_known_good' ? 'warn' : 'bad'))
  body.appendChild(fields)

  if (v.source_because) body.appendChild(el('p', 'note', v.source_because))
  if (v.note) { body.appendChild(el('p', 'dim', v.note)); return }

  const pull = v.pull
  if (!pull) {
    body.appendChild(el('p', 'dim', 'No pull has been attempted.'))
    return
  }
  if (!pull.reached) {
    body.appendChild(el('p', 'note', 'The Hub was not reached: ' + (pull.detail || 'no detail')))
    body.appendChild(el('p', 'dim',
      'Nothing changed. No control decision requires a network round trip (\u00a711), so this robot '
      + 'reconciles against what it has.'))
    return
  }

  const verdict = pull.verdict
  if (!verdict) return
  const line = !verdict.adopt
    ? 'Refused; still on generation ' + verdict.generation
    : verdict.changed
      ? 'Adopted generation ' + verdict.generation
      : 'Generation ' + verdict.generation + ', unchanged'
  body.appendChild(el('p', !verdict.adopt ? 'bad' : verdict.changed ? null : 'dim', line))

  for (const fault of verdict.faults || []) {
    body.appendChild(el('p', fault.severity === 'error' ? 'bad' : 'note', fault.message))
  }
}

/** §11 and §17's check-in: what this robot would say, and whether anybody hears it. */
function renderCheckin(feed) {
  const body = clear($('checkin-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const v = feed.value

  const fields = el('dl', 'fields')
  const row = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }
  // The claim rather than the boolean, because three of the four values of `applied: false` are
  // different problems: working, waiting for a person, and a supervisor disagreeing with itself.
  row('Claim', pill(v.claim || 'unreported', v.applied ? 'ok' : 'warn'))
  row('Generation', v.generation ?? absent('not reported'))
  row('State', v.state || absent('not reported'))
  row('Heard', pill(v.queued ? 'queued' : 'no', v.queued ? 'ok' : 'warn'))
  body.appendChild(fields)

  if (v.detail) body.appendChild(el('p', 'dim', v.detail))
  if (v.attestation && v.attestation.sentence) {
    body.appendChild(el('p', null, v.attestation.sentence))
  }

  const health = v.health || {}

  // The fields that *were* measured, with their ages. This panel listed the missing ones and not
  // the present ones for exactly as long as it took to open it: a health panel that only ever says
  // what it does not know is one nobody reads twice, and §33.6's age is the half of a reading that
  // decides whether to act on it.
  const measured = Object.entries(health.fields || {}).filter(([, held]) => held !== null)
  if (measured.length > 0) {
    const dl = el('dl', 'fields')
    for (const [name, held] of measured) {
      dl.appendChild(el('dt', null, name))
      dl.appendChild(el('dd', null, held.value + ' · ' + (duration(held.age_ms) || '0s') + ' old'))
    }
    body.appendChild(dl)
  }

  if (health.degraded) {
    body.appendChild(el('p', 'bad',
      'Degraded — alive and producing nothing: ' + (health.stalled || []).join(', ')))
  }
  // The two ways a field goes missing, kept apart on the page as §33.6 keeps them apart in the
  // document. A console that showed one list would be reporting a clock problem as an absent sensor.
  if ((health.undated || []).length > 0) {
    body.appendChild(el('p', 'note',
      'Measured and undateable, so not reported: ' + health.undated.join(', ')
      + ' — §33.6 requires an age on every field, and a number nobody can date renders as current.'))
  }
  if ((health.unmeasured || []).length > 0) {
    body.appendChild(el('p', 'dim', 'Nobody measured: ' + health.unmeasured.join(', ')))
  }

  for (const entry of v.pending || []) {
    body.appendChild(el('p', entry.deferred ? 'note' : 'dim',
      entry.stage + ': ' + (entry.reason || '')))
  }

  if (v.queued_note) body.appendChild(el('p', v.queued ? 'dim' : 'note', v.queued_note))
}

/**
 * Where the release manifest came from, or why there is none (§12, §57).
 *
 * The panel above answers "may this image be installed" and needs a manifest to answer it. Before
 * the pull existed, every reason there was no manifest rendered as one sentence — and the four
 * causes want four different people: nobody published this version for this architecture, the Hub
 * was unreachable, the robot is not paired, or the Hub changed a release under its own version.
 *
 * That last one gets the loudest treatment on the page, because it is the only one that is somebody
 * else's mistake and the only one an operator cannot fix from the robot.
 */
function renderReleases(feed) {
  const body = clear($('releases-body'))
  const v = feed.value
  if (!v) return

  if (!v.asked) {
    if (v.note) body.appendChild(el('p', 'dim', v.note))
    return
  }

  const where = (v.version || '') + ' on ' + (v.channel || 'no channel') + ', ' + (v.arch || '?')
  if (!v.reached) {
    body.appendChild(el('p', 'note',
      'The release catalogue was not reached (' + where + '): ' + (v.detail || 'no detail')
      + ' — this robot goes on with the manifests it holds, which is what it does whenever it '
      + 'cannot reach the Hub at all (§34.1).'))
    return
  }

  if (v.fault) {
    body.appendChild(el('p', 'bad', v.fault))
    return
  }

  body.appendChild(el('p', 'dim',
    v.written
      ? 'The manifest for ' + where + ' was fetched and is now held.'
      : 'This robot already holds the manifest for ' + where + '.'))
}

/**
 * §55.2's cell, and the faults that explain a refused precondition.
 *
 * The `env.*` block is the context **a precondition sees**, not the facts as they were written —
 * and the difference between the two is exactly the fault list underneath. An operator whose skill
 * was refused for `env.door closed` has no other way to learn that the key they typed never reached
 * the context; `cell_environment` drops it silently, which is right for a context and useless for a
 * diagnosis.
 *
 * The volumes are drawn with the reason they go no further. §55.2 sends them to the `workspace`
 * guard, which is the kernel's, and §33.1 keeps this daemon off that path — so a panel that showed
 * the facts and omitted the volumes would suggest there were none.
 */
function renderCell(feed) {
  const body = clear($('cell-body'))
  const v = feed.value
  if (!v) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }

  if (!v.held) {
    body.appendChild(el('p', 'note', v.note || 'no cell'))
    return
  }

  const head = el('p')
  head.appendChild(el('strong', null, v.name || '(unnamed)'))
  head.appendChild(el('span', 'dim', '  ' + (v.fixtures || 0) + ' fixture(s)'))
  body.appendChild(head)

  const environment = v.environment || {}
  const keys = Object.keys(environment)
  if (keys.length > 0) {
    const fields = el('dl', 'fields')
    for (const key of keys) {
      fields.appendChild(el('dt', null, 'env.' + key))
      fields.appendChild(el('dd', null, JSON.stringify(environment[key])))
    }
    body.appendChild(fields)
  }

  const volumes = (v.workspace && v.workspace.volumes) || []
  body.appendChild(el('p', 'dim',
    volumes.length + ' allowed volume(s) — ' + (v.workspace_note || '')))

  const faults = v.faults || []
  if (faults.length === 0) return

  const table = el('table', 'subpanel')
  const header = el('tr')
  for (const label of ['Fault', 'What it means']) header.appendChild(el('th', null, label))
  table.appendChild(header)
  for (const fault of faults) {
    const row = el('tr')
    const code = el('td')
    code.appendChild(pill(fault.code, 'bad'))
    row.appendChild(code)
    row.appendChild(el('td', 'dim', fault.message || ''))
    table.appendChild(row)
  }
  body.appendChild(table)
  if (v.note) body.appendChild(el('p', 'note', v.note))
}

function renderOta(feed) {
  const body = clear($('ota-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }

  const fields = el('dl', 'fields')
  const row = (label, value, tone) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    if (tone) dd.appendChild(pill(value, tone))
    else dd.textContent = value
    fields.appendChild(dd)
  }
  row('Active', feed.value.active || 'unknown')
  row('Staged', feed.value.staged || 'nothing')
  row('Channel', feed.value.channel || 'none')
  row('Board', feed.value.board || 'unreported')
  body.appendChild(fields)

  if (feed.value.pending) {
    // The one state on this panel that is not a report. A pending boot is a robot on probation, and
    // it says so rather than being a version string that looks like the others.
    body.appendChild(el('p', 'note',
      `This boot is proving itself: \`${feed.value.pending.version}\` was applied and the `
      + `bootloader returns to the previous slot unless it passes within `
      + `${feed.value.pending.budget_ms} ms (\u00a712).`))
  }

  const assessment = feed.value.assessment
  if (!assessment) {
    body.appendChild(el('p', 'note',
      'The desired state names no OS version, so there is nothing to assess.'))
    return
  }

  const verdict = el('p')
  verdict.appendChild(el('strong', null, assessment.version + ' '))
  verdict.appendChild(pill(assessment.admitted ? 'admitted' : 'refused',
    assessment.admitted ? 'ok' : 'bad'))
  body.appendChild(verdict)

  if (assessment.because) {
    body.appendChild(el('p', 'note', assessment.because))
  }

  const gated = (assessment.verdict && assessment.verdict.gated_components) || []
  if (gated.length > 0) {
    body.appendChild(el('p', 'note',
      `This image changes ${gated.join(', ')}. \u00a712 refuses an update touching the safety `
      + 'kernel or the envelope without a bench pass for these exact bytes \u2014 which is a thing '
      + 'to book, not a thing to retry.'))
  }

  const faults = (assessment.verdict && assessment.verdict.faults) || []
  if (faults.length > 0) {
    const table = el('table', 'subpanel')
    const head = el('tr')
    for (const label of ['', 'Finding', 'What to do']) head.appendChild(el('th', null, label))
    table.appendChild(head)
    for (const fault of faults) {
      const line = el('tr')
      const severity = el('td')
      severity.appendChild(pill(fault.severity, fault.severity === 'error' ? 'bad' : 'warn'))
      line.appendChild(severity)
      line.appendChild(el('td', null, fault.code))
      line.appendChild(el('td', 'dim', fault.message))
      table.appendChild(line)
    }
    body.appendChild(table)
  }

  if (assessment.bench_note) {
    body.appendChild(el('p', 'note', 'Bench pass: ' + assessment.bench_note))
  }
  renderTransfer(body, feed.value.transfer)
  if (feed.value.note) {
    body.appendChild(el('p', 'note', feed.value.note))
  }
}

/**
 * §34.3's wait, drawn (§12).
 *
 * *"Bulk classes wait for an unmetered bearer, and the wait is visible rather than silent."* This
 * is where it is visible. The panel above says whether an image may be installed; this says whether
 * its bytes are here, and those are different questions with different answers for most of a
 * rollout — an admitted release whose transfer is waiting on a dock is the ordinary state of a
 * robot on cellular.
 *
 * Two things it deliberately does not do:
 *
 * * **No progress bar without a total.** A manifest that declares no size gives a count of bytes
 *   and no fraction. A bar drawn against a guessed denominator is a number a person plans around,
 *   and it would be invented.
 * * **It says whose number the total is.** A size in the manifest is a publisher's signed
 *   statement; a size from the server is the party serving the bytes describing its own file.
 */
function renderTransfer(body, transfer) {
  if (!transfer) return

  const tone = {
    have: 'ok',
    wait_for_unmetered: 'warn',
    no_bearer: 'warn',
    refuse: 'bad',
  }[transfer.decision] || 'note'

  const line = el('p')
  line.appendChild(el('strong', null, 'Bundle '))
  line.appendChild(pill(transfer.decision, tone))
  body.appendChild(line)

  if (typeof transfer.total === 'number' && transfer.total > 0) {
    const share = Math.min(100, (transfer.received / transfer.total) * 100)
    body.appendChild(el('p', 'dim',
      bytes(transfer.received) + ' of ' + bytes(transfer.total)
      + ' (' + share.toFixed(0) + '%, as the ' + (transfer.total_declared_by || 'server')
      + ' declares it)'))
  } else if (transfer.received > 0) {
    body.appendChild(el('p', 'dim',
      bytes(transfer.received) + ' received. The manifest declares no size, so there is no '
      + 'fraction to show — the digest is what says when it is complete.'))
  }

  if (transfer.detail) body.appendChild(el('p', 'note', transfer.detail))
  if (transfer.error) body.appendChild(el('p', 'bad', transfer.error))
}

/**
 * A byte count a person reads.
 *
 * Powers of two, because an operating system image is measured against a partition.
 */
function bytes(count) {
  const units = ['B', 'KiB', 'MiB', 'GiB']
  let value = Math.max(0, Number(count) || 0)
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return unit === 0 ? value.toFixed(0) + ' B' : value.toFixed(1) + ' ' + units[unit]
}

/**
 * §54.4's invocations, as a ladder rather than a status word.
 *
 * The phase alone is not enough to act on and the reason alone is not enough to place, so both are
 * shown on every row — an operator looking at `terminal / lease_denied` needs the second word to
 * know whether to grant a token, stop another package, or call the publisher.
 *
 * The outcome is drawn only for a terminal invocation, and `aborted` is *not* coloured as a
 * failure. §54.4 is specific that a skill refused before it started did not fail at its task, and a
 * console that painted the two the same would train an operator to read a cell configuration
 * problem as a broken skill.
 */
function renderSkills(feed) {
  const body = clear($('skills-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const list = feed.value.invocations || []
  if (list.length === 0) {
    // §32.3: an empty state says what is absent. Nothing has been invoked is a fact, and it is not
    // the same fact as a robot that cannot run skills.
    body.appendChild(el('p', 'note',
      'Nothing has been invoked. A skill is installed like any other package and run on demand ' +
      '(§54.1); POST /skills starts one.'))
    return
  }

  const table = el('table')
  const head = el('tr')
  for (const label of ['Invocation', 'Skill', 'Phase', 'Reason', 'Outcome', 'Leases', 'Detail', '']) {
    head.appendChild(el('th', null, label))
  }
  table.appendChild(head)

  for (const entry of list) {
    const row = el('tr')
    row.appendChild(el('td', 'dim', entry.id))

    const skill = el('td')
    skill.textContent = entry.package
    if (entry.parent) skill.title = 'nested under ' + entry.parent
    row.appendChild(skill)

    const phase = el('td')
    phase.appendChild(pill(entry.phase,
      entry.phase === 'running' ? 'ok' : entry.phase === 'terminal' ? 'off' : 'warn'))
    row.appendChild(phase)

    row.appendChild(el('td', 'dim', entry.reason))

    // Three outcomes and three readings. `aborted` is deliberately not `bad`: §54.4 says a skill
    // that was never allowed to start did not fail at its task, and colouring it as a failure is
    // how a success rate becomes a punishment for an operator's cell configuration.
    const outcome = el('td')
    if (entry.outcome === 'success') outcome.appendChild(pill('success', 'ok'))
    else if (entry.outcome === 'failure') outcome.appendChild(pill('failure', 'bad'))
    else if (entry.outcome === 'aborted') outcome.appendChild(pill('aborted', 'warn'))
    else if (entry.outcome === 'unlabeled') outcome.appendChild(pill('unlabeled', 'off'))
    row.appendChild(outcome)

    const leases = el('td')
    leases.appendChild(pill(entry.leases,
      entry.leases === 'held' ? 'ok' : entry.leases === 'denied' ? 'bad' : 'off'))
    if ((entry.capabilities || []).length > 0) {
      leases.title = entry.capabilities.join(', ')
    }
    row.appendChild(leases)

    row.appendChild(el('td', 'dim', entry.detail || ''))

    const actions = el('td', 'actions')
    if (entry.phase !== 'terminal') {
      const stop = el('button', null, 'Stop')
      stop.addEventListener('click', () => stopSkill(entry.id))
      actions.appendChild(stop)
    }
    row.appendChild(actions)

    table.appendChild(row)
  }
  body.appendChild(table)

  // The one thing a reader of this table would otherwise get wrong. It is written under the table
  // rather than in a tooltip because it changes what the *outcome* column means.
  body.appendChild(el('p', 'note',
    'The outcome is the OS\u2019s, not the skill\u2019s (\u00a754.3): a skill that judged its ' +
    'own success would always succeed. A criterion nothing on this robot can evaluate is ' +
    '`unlabeled` rather than a pass.'))
}

async function stopSkill(id) {
  await call('POST', '/skills/' + encodeURIComponent(id) + '/stop')
  refresh()
}

function renderPackages(feed) {
  const body = clear($('packages-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const list = feed.value.packages || []
  if (list.length === 0) {
    // §32.3: an empty state says what is absent. Never a demo fleet.
    body.appendChild(el('p', 'note', 'No package is installed on this robot.'))
    return
  }

  const table = el('table')
  const head = el('tr')
  for (const label of ['Package', 'State', 'Wanted', 'Signed by', 'Digest', 'Grants', '']) {
    head.appendChild(el('th', null, label))
  }
  table.appendChild(head)

  for (const entry of list) {
    const row = el('tr')
    row.appendChild(el('td', null, entry.id))

    const state = el('td')
    state.appendChild(pill(entry.state,
      entry.state === 'running' ? 'ok' : entry.state === 'failed' ? 'bad' : 'off'))
    row.appendChild(state)

    row.appendChild(el('td', 'dim', entry.wanted))

    // §37.1's four outcomes, kept apart. `untrusted` is a valid signature by somebody this robot
    // has not been told to believe — a different job from `absent`, and a very different one from
    // `failed`, so they do not share a colour or a word.
    const signer = el('td')
    if (entry.signature === 'verified') {
      signer.appendChild(pill(entry.signer, 'ok'))
    } else if (entry.signature === 'failed') {
      signer.appendChild(pill('signature failed', 'bad'))
    } else if (entry.signature === 'untrusted') {
      signer.appendChild(pill('untrusted key', 'warn'))
    } else {
      signer.appendChild(pill('nobody', 'warn'))
    }
    row.appendChild(signer)

    const digest = el('td', 'dim')
    digest.textContent = entry.digest ? entry.digest.slice(0, 16) + '…' : ''
    digest.title = entry.digest || ''
    row.appendChild(digest)

    const grants = el('td')
    const held = entry.grants || []
    if (held.length === 0) {
      grants.appendChild(el('span', 'dim', 'none'))
    } else {
      for (const token of held) {
        const chip = el('button', 'act', token + ' ✕')
        chip.title = 'revoke ' + token
        chip.addEventListener('click', () => revoke(entry.id, token))
        grants.appendChild(chip)
      }
    }
    row.appendChild(grants)

    const actions = el('td')
    for (const [label, verb] of [['start', 'start'], ['stop', 'stop'], ['rollback', 'rollback']]) {
      const button = el('button', 'act', label)
      button.addEventListener('click', () => act(entry.id, verb))
      actions.appendChild(button)
    }
    row.appendChild(actions)

    table.appendChild(row)
  }
  body.appendChild(table)

  body.appendChild(el('p', 'note',
    'Revoking a grant takes effect on the next command rather than the next restart (§32.2).'))
}

function renderHealth(feed) {
  const body = clear($('health-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const h = feed.value

  const fields = el('dl', 'fields')
  const add = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }

  add('Degraded', h.degraded ? pill('yes', 'bad') : pill('no', 'ok'))

  // Every one of these arrives as null, and each renders as an absence rather than as a zero.
  // This is the panel §33.6 was written about.
  //
  // The *reason* is one panel note rather than five copies of itself. It was five, and looking at
  // the page settled it: the same thirty-word sentence repeated down the column buried the two
  // fields that carry an answer, and a panel nobody reads reports nothing. §32.2's rule is that a
  // null renders as the reason it is null, and that is satisfied by a marker beside the field and
  // the reason immediately under it — which is exactly the shape the guards panel already uses.
  let anyUnmeasured = false
  const measured = (label, value, unit) => {
    if (value === null || value === undefined) {
      anyUnmeasured = true
      add(label, absent('unmeasured'))
    } else {
      add(label, String(value) + (unit || ''))
    }
  }
  measured('Generation lag', h.generation_lag)
  measured('Storage headroom', h.storage_headroom)
  measured('Guard trip rate', h.guard_trip_rate)
  measured('Battery', h.battery)
  measured('Temperature', h.temperature_c, ' °C')
  body.appendChild(fields)

  if (anyUnmeasured) {
    body.appendChild(el('p', 'note', h.unmeasured || 'not measured on this robot'))
  }

  const packages = h.packages || []
  if (packages.length > 0) {
    const table = el('table')
    const head = el('tr')
    for (const label of ['Package', 'Live', 'Readiness', 'Last sequence', 'Age']) {
      head.appendChild(el('th', null, label))
    }
    table.appendChild(head)
    for (const p of packages) {
      const row = el('tr')
      row.appendChild(el('td', null, p.id))
      const live = el('td')
      live.appendChild(pill(p.live ? 'live' : 'not live', p.live ? 'ok' : 'off'))
      row.appendChild(live)
      const readiness = el('td')
      readiness.appendChild(p.readiness === 'unknown'
        ? absent('nothing reports readiness')
        : pill(p.readiness, p.readiness === 'ready' ? 'ok' : 'warn'))
      row.appendChild(readiness)
      const sequence = el('td', 'num')
      sequence.appendChild(p.last_sequence === null || p.last_sequence === undefined
        ? absent('no command sequence — needs the policy runtime')
        : document.createTextNode(String(p.last_sequence)))
      row.appendChild(sequence)
      row.appendChild(el('td', 'num', duration(p.age_ms) || ''))
      table.appendChild(row)
    }
    body.appendChild(table)
  }
}

function renderBlackbox(feed) {
  const body = clear($('blackbox-body'))

  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    if (feed.status === 'error') {
      body.appendChild(el('p', 'note',
        'A daemon whose black box could not be opened still serves stop and status, which is why ' +
        'this panel can be the only one missing.'))
    }
    // A robot whose ring will not open still has the windows it sealed before it stopped opening,
    // and those are the ones somebody is looking for. The refusal carries them (\u00a779), so they
    // are drawn under it rather than lost with it.
    const held = (feed.body && feed.body.collection) || null
    if (held && (held.windows || []).length > 0) {
      body.appendChild(el('p', 'note',
        'Sealed windows are still on the disk and can still be downloaded:'))
      body.appendChild(sealedTable(held))
    }
    return
  }
  const b = feed.value
  const window_ = b.window || {}

  const fields = el('dl', 'fields')
  const add = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }
  add('Ring', bytes(b.bytes) + ' · ' + b.capacity + ' records of ' + b.record_size + ' B')
  add('Covers', Number.isFinite(b.seconds_covered_at_rate)
    ? b.seconds_covered_at_rate.toFixed(1) + ' s at ' + b.rate_hz + ' Hz'
    : absent('not reported'))
  add('Records held', String(window_.records) + ' (sequences ' + window_.first_sequence +
    '–' + window_.last_sequence + ')')
  add('Continuity', window_.complete
    ? pill('complete', 'ok')
    : pill((window_.holes || []).length + ' hole(s), ' + window_.dropped + ' dropped', 'warn'))
  add('Control cycles', Number.isFinite(window_.cycles) && window_.cycles > 0
    ? String(window_.cycles)
    : absent('none — this recording holds events only'))
  body.appendChild(fields)

  // The daemon's own sentence about what this recording cannot contain. Carried verbatim: it is
  // the difference between a healthy-looking black box and one that has never seen a control cycle.
  if (b.cycles_unavailable) {
    body.appendChild(el('p', 'note', b.cycles_unavailable))
  }

  const seal = el('button', 'act', 'Seal now')
  seal.addEventListener('click', sealNow)
  body.appendChild(seal)

  // §79's collection, off `collection.windows` rather than `sealed`. The two are the same set and
  // only one of them says whether the Hub has a copy — and this panel's whole job is to answer
  // questions an investigator would otherwise have to assume the answer to.
  const collection = b.collection || {}
  const sealed = collection.windows || (b.sealed || []).map(
    file => ({ name: file.name, size_bytes: file.bytes, collected: null }))

  if (sealed.length === 0) {
    body.appendChild(el('p', 'note', 'No sealed recording on this robot yet.'))
  } else {
    body.appendChild(sealedTable({ ...collection, windows: sealed }))

    if (collection.pending > 0) {
      body.appendChild(el('p', 'note',
        collection.pending + ' window(s), ' + bytes(collection.pending_bytes) +
        ', are on this robot and nowhere else. A sealed window is bulk and waits for an unmetered ' +
        'link (\u00a734.3); the incident naming it went out on whatever bearer was up (\u00a779.3).'))
    }
    const attempt = collection.last_attempt || {}
    if (attempt.error) {
      body.appendChild(el('p', 'note', 'Last upload attempt: ' + attempt.error))
    }
  }
}

/** The sealed windows, and which of them the Hub has (\u00a779). */
function sealedTable(collection) {
  const table = el('table')
  const head = el('tr')
  for (const label of ['Sealed file', 'Size', 'On the Hub', '']) {
    head.appendChild(el('th', null, label))
  }
  table.appendChild(head)
  for (const file of collection.windows || []) {
    const row = el('tr')
    row.appendChild(el('td', null, file.name))
    row.appendChild(el('td', 'num', bytes(file.size_bytes)))
    const where = el('td')
    // Three states, not two. `null` is an older daemon whose index carries no collection — and
    // rendering that as "here only" would be this console asserting something it was not told.
    where.appendChild(file.collected === null || file.collected === undefined
      ? absent('not reported by this daemon')
      : pill(file.collected ? 'collected' : 'here only', file.collected ? 'ok' : 'warn'))
    row.appendChild(where)
    const cell = el('td')
    const download = el('button', 'act', 'download')
    download.addEventListener('click', () => downloadSealed(file.name))
    cell.appendChild(download)
    row.appendChild(cell)
    table.appendChild(row)
  }
  return table
}

// ── Actions ──────────────────────────────────────────────────────────────────

function say(message) {
  const node = $('stop-result')
  node.hidden = false
  node.textContent = message
}

/**
 * The stop button.
 *
 * No confirmation, by §32.2. The cost of an accidental stop is a stopped robot; the cost of a
 * dialog is the half-second somebody spends reading it while watching something go wrong.
 */
async function stopAll() {
  say('stopping…')
  try {
    const result = await call('POST', '/stop')
    if (!result.ok) {
      say('stop failed: HTTP ' + result.status + ' — ' + ((result.body && result.body.error) || ''))
      return
    }
    const stopped = (result.body && result.body.stopped) || []
    const failed = (result.body && result.body.failed) || []
    let message = 'Stopped ' + stopped.length + ' package(s).'
    if (failed.length > 0) {
      message += ' FAILED: ' + failed.map(f => f.id + ' (' + f.reason + ')').join(', ')
    }
    if (result.body && result.body.note) message += ' ' + result.body.note
    say(message)
  } catch (error) {
    // The one failure this console must not swallow. If the daemon is unreachable, the person
    // looking at this page needs to know the button did nothing.
    say('STOP DID NOT REACH THE DAEMON: ' + error + '. Use the physical estop.')
  }
  refresh()
}

async function act(id, verb) {
  const result = await call('POST', '/packages/' + encodeURIComponent(id) + '/' + verb)
  say(verb + ' ' + id + ': ' + (result.ok
    ? ((result.body && result.body.outcome) || 'accepted')
    : 'HTTP ' + result.status + ' — ' + ((result.body && result.body.error) || '')))
  refresh()
}

async function revoke(id, token) {
  const result = await call('DELETE',
    '/grants/' + encodeURIComponent(id) + '/' + encodeURIComponent(token))
  say('revoke ' + token + ' from ' + id + ': ' + (result.ok ? 'done' : 'HTTP ' + result.status))
  refresh()
}

async function sealNow() {
  const result = await call('POST', '/blackbox/seal')
  say(result.ok
    ? 'sealed: ' + ((result.body && result.body.name) || 'written')
    : 'seal failed: HTTP ' + result.status + ' — ' + ((result.body && result.body.error) || ''))
  refresh()
}

/**
 * Download a sealed recording.
 *
 * Through `fetch` rather than a link, because the route needs the operator role and a plain
 * navigation cannot carry an `Authorization` header.
 */
async function downloadSealed(name) {
  const headers = {}
  const token = readToken()
  if (token) headers['authorization'] = 'Bearer ' + token
  const response = await fetch('/blackbox/' + encodeURIComponent(name), { headers, cache: 'no-store' })
  if (!response.ok) {
    say('download failed: HTTP ' + response.status)
    return
  }
  const blob = await response.blob()
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
  URL.revokeObjectURL(url)
}

/**
 * The power-on self-test (§48.2).
 *
 * Four outcomes rather than two, and the three that are not `pass` are the reason the panel is
 * worth having. `fail` means something is wrong and §48.2 sends the robot to `SAFE`, which is
 * sticky and needs a human. `unavailable` means the input does not exist — nothing is broken and
 * nothing was proven. `elsewhere` means §33.1 forbids this daemon from running the check at all,
 * which is a different sentence again: it is not that nobody ran it, it is that this process is not
 * who would.
 */
function renderPost(feed) {
  const body = clear($('post-body'))
  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const p = feed.value

  const verdict = el('dl', 'fields')
  verdict.appendChild(el('dt', null, 'Verdict'))
  const dd = el('dd')
  dd.appendChild(pill(p.verdict, p.failed ? 'bad' : p.may_run ? 'ok' : 'warn'))
  // The sentence a badge cannot carry: "incomplete" is not "nearly passed".
  dd.appendChild(el('div', 'dim', p.failed
    ? '§48.2 sends a failed self-test to SAFE, which is sticky and needs a human.'
    : p.may_run
      ? 'Every check passed, so this robot may leave POST.'
      : 'Nothing is wrong and nothing is proven. A check that could not run is not a pass, so this '
        + 'robot may not run a policy.'))
  verdict.appendChild(dd)
  body.appendChild(verdict)

  const table = el('table', 'subpanel')
  const head = el('tr')
  for (const label of ['Check', 'Outcome', 'What it means']) head.appendChild(el('th', null, label))
  table.appendChild(head)
  for (const check of p.checks || []) {
    const row = el('tr')
    row.appendChild(el('td', 'mono', check.check))
    const outcome = el('td')
    outcome.appendChild(pill(check.outcome,
      check.outcome === 'pass' ? 'ok' : check.outcome === 'fail' ? 'bad'
        : check.outcome === 'unavailable' ? 'warn' : 'off'))
    row.appendChild(outcome)
    const what = el('td', 'dim')
    what.appendChild(document.createTextNode(check.detail))
    // What a failure would mean, kept on every row: it is what makes an `elsewhere` row worth
    // showing rather than hiding.
    what.appendChild(el('div', null, 'failure means: ' + check.failure_means))
    row.appendChild(what)
    table.appendChild(row)
  }
  body.appendChild(table)

  if (p.partial) body.appendChild(el('p', 'note', p.partial))
}

/**
 * The watchdog chain (§46), and whether this body's windows nest.
 *
 * Two things on one panel because they are one question. The chain says what catches what; the
 * nesting says whether the layers above are reachable at all — a SoC watchdog shorter than the
 * kernel's own window resets the processor before the kernel executes its controlled stop, and a
 * page showing four healthy layers over that configuration would be the most misleading panel here.
 */
function renderWatchdog(feed) {
  const body = clear($('watchdog-body'))
  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const w = feed.value

  // §46.2 first: it decides which tier this body may attempt, which outranks everything below.
  const loss = el('dl', 'fields')
  const addLoss = (label, value) => {
    loss.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    loss.appendChild(dd)
  }
  addLoss('On command loss', w.on_command_loss
    ? pill(w.on_command_loss, w.host_blocked ? 'warn' : 'ok')
    : absent('not declared'))
  body.appendChild(loss)
  if (w.host_blocked) body.appendChild(el('p', 'bad', w.host_blocked))

  // The nesting, and the declared numbers beside it so a reader can see the order themselves.
  const nesting = w.nesting || {}
  const table = el('table', 'subpanel')
  const head = el('tr')
  for (const label of ['Window', 'Declared']) head.appendChild(el('th', null, label))
  table.appendChild(head)
  // In nesting order, which is why the route sends an array: an object would arrive alphabetical,
  // and a table about nesting that is not in nesting order teaches the reader the wrong nest.
  for (const entry of nesting.declared || []) {
    const row = el('tr')
    row.appendChild(el('td', 'mono', entry.window))
    const cell = el('td')
    if (entry.ms === null || entry.ms === undefined) cell.appendChild(absent('not declared'))
    else cell.appendChild(document.createTextNode(entry.ms + ' ms'))
    row.appendChild(cell)
    table.appendChild(row)
  }
  body.appendChild(table)

  for (const fault of nesting.faults || []) body.appendChild(el('p', 'bad', fault.detail))
  if ((nesting.faults || []).length === 0) {
    // Ordered and complete are two facts, and a panel that showed one would be lying about the
    // other: nothing contradicts, and nothing was established.
    body.appendChild(el('p', 'note', nesting.complete
      ? 'Every window is declared and they nest, so every layer above is reachable (§46.3).'
      : 'Nothing contradicts, and nothing is established: '
        + (nesting.missing || []).length + ' of 4 windows are undeclared.'))
  }

  // And which layers this robot actually has, as against which the chain describes — four rows of
  // vocabulary over a machine with one of them running would read as four running watchdogs.
  const here = el('table', 'subpanel')
  const hhead = el('tr')
  for (const label of ['Layer', 'Here', 'Catches']) hhead.appendChild(el('th', null, label))
  here.appendChild(hhead)
  const states = new Map((w.here || []).map(entry => [entry.name, entry]))
  for (const layer of w.chain || []) {
    const row = el('tr')
    row.appendChild(el('td', 'mono', layer.layer + ' · ' + layer.name))
    const state = states.get(layer.name)
    const cell = el('td')
    cell.appendChild(pill(state ? state.state : 'unknown', state && state.state === 'armed' ? 'ok' : 'off'))
    if (state) cell.appendChild(el('div', 'dim', state.detail))
    row.appendChild(cell)
    const what = el('td', 'dim')
    what.appendChild(document.createTextNode(layer.catches))
    what.appendChild(el('div', null, '→ ' + layer.response + ' · petted by ' + layer.petted_by))
    row.appendChild(what)
    here.appendChild(row)
  }
  body.appendChild(here)
}

/**
 * The outbound queue (§34.4): what this robot is waiting to say, and what it has thrown away.
 *
 * The dropped column is the point. §47.4's rule applied to a queue — a gap annotated as a gap is
 * honest, a gap that looks like a quiet period is not — so a robot that has discarded forty
 * thousand telemetry items says so, rather than presenting a suspiciously tidy series.
 */
function renderOutbound(feed) {
  const body = clear($('outbound-body'))
  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const q = feed.value

  const table = el('table')
  const head = el('tr')
  for (const label of ['Class', 'Waiting', 'Size', 'Dropped']) head.appendChild(el('th', null, label))
  table.appendChild(head)

  for (const row of q.classes || []) {
    const tr = el('tr')
    const name = el('td')
    name.appendChild(document.createTextNode(row.class))
    // The one class that is never discarded, said on the row rather than in a footnote.
    if (!row.droppable) name.appendChild(el('div', 'dim', 'never dropped'))
    tr.appendChild(name)
    tr.appendChild(el('td', 'num', String(row.held)))
    tr.appendChild(el('td', 'num', bytes(row.bytes) || '0 B'))
    const dropped = el('td', 'num')
    if (row.dropped > 0) dropped.appendChild(pill(String(row.dropped), 'warn'))
    else dropped.appendChild(document.createTextNode('0'))
    tr.appendChild(dropped)
    table.appendChild(tr)
  }
  body.appendChild(table)

  const fields = el('dl', 'fields subpanel')
  const add = (label, value) => {
    fields.appendChild(el('dt', null, label))
    const dd = el('dd')
    dd.appendChild(value instanceof Node ? value : document.createTextNode(String(value)))
    fields.appendChild(dd)
  }
  add('Budget', bytes(q.budget) + ' · ' + bytes(q.reserve) + ' reserved for incidents')
  // Collapsed is not a loss and is counted apart from one: the newer check-in carries the same
  // level-triggered state, which is the whole reason the older one can go.
  add('Check-ins superseded', String(q.collapsed))
  add('Delivered', String(q.delivered || 0))
  // Only when it has happened, and never as a synonym for the line above it. An item a local sink
  // accepted left the queue and did not leave the machine, and a panel showing one "sent" total
  // would report a working uplink on a robot running with --outbox.
  if (q.handed_off > 0) {
    const local = el('span')
    local.appendChild(pill(String(q.handed_off), 'warn'))
    local.appendChild(el('div', 'dim', 'accepted by a local sink and still on this machine — '
      + 'nothing was sent'))
    add('Handed off locally', local)
  }
  body.appendChild(fields)

  // §34.3's bearers. The table is here rather than in its own panel because the only question it
  // answers is about this queue: what will the link carry right now.
  const link = q.link
  if (link) {
    if (!link.declared) {
      body.appendChild(el('p', 'note', 'No bearer has been declared, so nothing can be sent. '
        + 'Which interface is the modem is a fact about this machine that the OS cannot infer, so '
        + 'it is read from state/bearers.json (§34.3).'))
    } else if (!link.bearers.length) {
      body.appendChild(el('p', 'note', 'state/bearers.json declares no bearers.'))
    } else {
      const links = el('table')
      const lhead = el('tr')
      for (const label of ['Bearer', 'Kind', 'State', 'Metered']) lhead.appendChild(el('th', null, label))
      links.appendChild(lhead)
      for (const bearer of link.bearers) {
        const tr = el('tr', bearer.name === link.active ? 'active' : null)
        const name = el('td')
        name.appendChild(document.createTextNode(bearer.name))
        if (bearer.name === link.active) name.appendChild(el('div', 'dim', 'in use'))
        tr.appendChild(name)
        tr.appendChild(el('td', null, bearer.kind))
        const state = el('td')
        state.appendChild(document.createTextNode(bearer.up ? 'up' : 'down'))
        // Where that came from, on the row. "down" on a machine that has no /sys/class/net entry
        // for the interface is not a measurement, and a panel that did not say so would be
        // reporting one nobody took.
        if (bearer.liveness !== 'observed') state.appendChild(el('div', 'dim', bearer.liveness))
        tr.appendChild(state)
        const metered = el('td')
        metered.appendChild(document.createTextNode(bearer.metered ? 'yes' : 'no'))
        // Declared or assumed, said on the row. A console that showed only "metered: yes" would
        // present an assumption as a measurement, and the vendor who thinks their Wi-Fi is free
        // needs to know the robot does not and that it is one line of configuration away.
        metered.appendChild(el('div', 'dim', bearer.metering_declared ? 'declared' : 'assumed'))
        tr.appendChild(metered)
        links.appendChild(tr)
      }
      body.appendChild(links)
    }
  }

  // Where items go, and what the head of the queue is waiting for.
  //
  // **Two sentences, never one.** The first version concatenated them, and on an unpaired robot it
  // read `Sends to: none — this robot is not paired with a Hub (§10). the active bearer carries
  // this class` — two true statements rendering as one claim that the queue was fine. §34.3's
  // verdict is about the *link*; the obstacle is about whether there is anywhere to send at all,
  // and a panel that cannot keep those apart will always be reassuring about the wrong one.
  const delivery = q.delivery
  if (delivery) {
    const stuck = delivery.obstacle || delivery.verdict !== 'send'
    const where = el('p', stuck ? 'note warn' : 'note')
    where.appendChild(document.createTextNode('Sends to: ' + delivery.sink))
    body.appendChild(where)
    if (delivery.obstacle) body.appendChild(el('p', 'note warn', delivery.obstacle))
    // §34.3: the wait is visible rather than silent, so the link gets its own sentence whatever it
    // says — including when it says yes and nothing is listening.
    body.appendChild(el('p', delivery.verdict === 'send' ? 'note' : 'note warn',
      'Link: ' + delivery.detail))
  }
}

/**
 * Calibration (§45), and the guards it is starving.
 *
 * The panel leads with the guards rather than the records, because that is the question somebody
 * standing next to a robot has: *what is this machine not checking right now, and why.* §45.5's
 * whole requirement is that an expired parameter set be named — "proximity unavailable" sends
 * nobody anywhere, and "proximity unavailable: the camera_extrinsics calibration expired 40 days
 * ago" sends them to a target board.
 */
function renderCalibration(feed) {
  const body = clear($('calibration-body'))
  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const c = feed.value
  const starved = c.unavailable || []

  // Before the table, because it changes what the table means: none of these guards would run even
  // with a fresh calibration, and a reader who takes this panel for a to-do list would go and
  // re-measure a camera to no effect.
  if (c.precedes) body.appendChild(el('p', 'note', c.precedes))

  if (starved.length === 0) {
    body.appendChild(el('p', 'note', 'No guard is short of a calibration.'))
  } else {
    const table = el('table')
    const head = el('tr')
    for (const label of ['Guard', 'Waiting on', 'Why']) head.appendChild(el('th', null, label))
    table.appendChild(head)
    for (const entry of starved) {
      const row = el('tr')
      row.appendChild(el('td', null, entry.guard))
      const set = el('td')
      set.appendChild(el('span', 'mono', entry.set))
      set.appendChild(document.createTextNode(' '))
      set.appendChild(pill(entry.state, entry.state === 'absent' ? 'off' : 'warn'))
      row.appendChild(set)
      row.appendChild(el('td', 'dim', entry.detail))
      table.appendChild(row)
    }
    body.appendChild(table)
  }

  // Then the records themselves, so a set that *is* valid is visible rather than merely absent
  // from the list above.
  const sets = c.sets || []
  if (sets.length > 0) {
    const table = el('table', 'subpanel')
    const head = el('tr')
    for (const label of ['Parameter set', 'State', 'Method', 'Signed by']) {
      head.appendChild(el('th', null, label))
    }
    table.appendChild(head)
    for (const entry of sets) {
      const row = el('tr')
      row.appendChild(el('td', 'mono', entry.set))

      const state = el('td')
      state.appendChild(pill(entry.state, entry.state === 'valid' ? 'ok'
        : entry.state === 'absent' ? 'off' : entry.state === 'failed' ? 'bad' : 'warn'))
      // What was actually checked, which is how a reader tells "still valid" from "no policy".
      if (entry.state === 'valid') {
        state.appendChild(el('div', 'dim', (entry.checked || []).length > 0
          ? entry.checked.join(' · ')
          : 'no expiry policy — nothing was checked'))
      } else if (entry.detail) {
        state.appendChild(el('div', 'dim', entry.detail))
      }
      row.appendChild(state)

      const method = el('td')
      if (entry.method) {
        method.appendChild(document.createTextNode(entry.method))
        if (entry.residual !== null && entry.residual !== undefined) {
          method.appendChild(el('div', 'dim', 'residual ' + entry.residual))
        }
      } else {
        method.appendChild(absent('no record'))
      }
      row.appendChild(method)

      const signed = el('td')
      if (entry.signer) signed.appendChild(pill(entry.signer, 'ok'))
      else if (entry.state === 'absent') signed.appendChild(absent('—'))
      else signed.appendChild(pill(entry.signature, entry.signature === 'failed' ? 'bad' : 'warn'))
      row.appendChild(signed)
      table.appendChild(row)
    }
    body.appendChild(table)
  }

  // §32.2: never show a value it did not receive. Three of §45.4's four conditions have nothing
  // producing them on this robot, and a panel of expired records with no explanation reads as a
  // robot that has drifted rather than one that cannot measure.
  for (const line of c.unmeasured || []) {
    body.appendChild(el('p', 'note', 'not measured: ' + line))
  }
}

/**
 * The trust store (§19, §38.3) — which keys this robot believes, and who said so.
 *
 * Three states before any key is drawn, because they are three different machines and the
 * remedies have nothing in common: no file at all is a robot nobody provisioned, a file that does
 * not parse is one somebody broke, and a file naming no keys is one somebody emptied. All three
 * make every signature report `untrusted`, which is why the packages panel sends people here.
 */
function renderTrust(feed) {
  const body = clear($('trust-body'))
  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const t = feed.value

  if (t.source === 'missing') {
    body.appendChild(el('p', 'note',
      'No trust store. This robot trusts nobody, so every signature reports untrusted — which is ' +
      'true, and is not the same as a package being wrong. Add a key with `skynetd trust add`.'))
    return
  }
  if (t.source === 'unreadable') {
    body.appendChild(el('p', 'bad', 'The trust store cannot be read: ' + (t.source_detail || '')))
    body.appendChild(el('p', 'note',
      'This robot trusts nobody until it is repaired. A file that broke on its own is worth a ' +
      'second look.'))
    return
  }

  const rows = t.keys || []
  if (rows.length === 0) {
    body.appendChild(el('p', 'note',
      'The trust store is present and names no keys, so every signature reports untrusted.'))
    return
  }

  const table = el('table')
  const head = el('tr')
  for (const label of ['Label', 'Key', 'Added']) head.appendChild(el('th', null, label))
  table.appendChild(head)

  for (const entry of rows) {
    const row = el('tr')
    if (!entry.usable) {
      // An entry the robot could not read is not a key, and drawing it in the same shape as one
      // would suggest this robot is trusting something it is not.
      const cell = el('td', 'bad', entry.at + ' — unusable')
      cell.colSpan = 3
      cell.title = entry.detail || ''
      row.appendChild(cell)
      table.appendChild(row)
      continue
    }

    row.appendChild(el('td', null, entry.label))

    // Truncated, with the whole of it on hover and in the title: 64 hex characters in a table
    // column on a phone is a horizontal scrollbar, and the first eight are what somebody compares
    // against a release page.
    const key = el('td', 'mono', (entry.key || '').slice(0, 16) + '…')
    key.title = entry.key || ''
    row.appendChild(key)

    const when = el('td')
    if (entry.provenance_recorded) {
      when.appendChild(document.createTextNode(entry.provenance))
    } else {
      // §38.3's question with no answer. The key still verifies — this is a gap in the audit, not
      // in the security — so it is drawn as an absence rather than as a fault.
      when.appendChild(absent('unrecorded'))
    }
    if (entry.note) when.appendChild(el('div', 'dim', entry.note))
    row.appendChild(when)
    table.appendChild(row)
  }
  body.appendChild(table)

  for (const notice of t.notices || []) body.appendChild(el('p', 'note', notice))
}

/**
 * The audit log (§38.3), newest first.
 *
 * A separate panel from the black box on purpose: the ring holds the last few minutes and this
 * holds the life of the machine, and the question "when did somebody trust that key" is nearly
 * always older than the ring.
 */
function renderAudit(feed) {
  const body = clear($('audit-body'))
  if (!feed.value) {
    body.appendChild(el('p', 'note', feed.detail || 'no answer yet'))
    return
  }
  const a = feed.value
  const entries = a.entries || []

  if (entries.length === 0) {
    body.appendChild(el('p', 'note',
      'Nothing has been recorded. A robot provisioned by editing files directly has an empty ' +
      'audit log and a full trust store — which is exactly the case the Trust panel marks ' +
      'unrecorded.'))
  } else {
    const table = el('table')
    const head = el('tr')
    for (const label of ['When', 'Who', 'What']) head.appendChild(el('th', null, label))
    table.appendChild(head)

    for (const entry of entries) {
      const row = el('tr')
      row.appendChild(el('td', 'mono', entry.at))
      row.appendChild(el('td', null, entry.actor))
      const what = el('td')
      what.appendChild(document.createTextNode(entry.action + ' ' + entry.subject))
      if (entry.previous) what.appendChild(el('div', 'dim', 'was: ' + entry.previous))
      row.appendChild(what)
      table.appendChild(row)
    }
    body.appendChild(table)

    if (a.total > entries.length) {
      body.appendChild(el('p', 'note',
        'Showing the most recent ' + entries.length + ' of ' + a.total +
        '. The whole record is state/audit.jsonl on the robot.'))
    }
  }

  // Never silent about a line that could not be read: a log with invisible gaps is worse than no
  // log, because somebody is relying on it.
  for (const line of a.broken || []) {
    body.appendChild(el('p', 'bad',
      'Line ' + line.line + ' of the audit log ' + line.detail))
  }
}

// ── The poll loop ────────────────────────────────────────────────────────────

const bodyFeed = new Feed('embodiment', '/embodiment', renderEmbodiment)
// Named, because §32.2's Plan panel lists the packages a transport plan can be asked for.
const packagesFeed = new Feed('packages', '/packages', renderPackages)

const feeds = [
  new Feed('status', '/status', renderStatus),
  new Feed('state', '/state', renderState),
  bodyFeed,
  new Feed('guards', '/limits', renderGuards),
  new Feed('plan', '/plan', renderPlan),
  packagesFeed,
  new Feed('identity', '/identity', renderIdentity),
  new Feed('intent', '/intent', renderIntent),
  new Feed('checkin', '/checkin', renderCheckin),
  new Feed('ota', '/ota', renderOta),
  new Feed('releases', '/releases', renderReleases),
  new Feed('skills', '/skills', renderSkills),
  new Feed('health', '/health', renderHealth),
  new Feed('blackbox', '/blackbox', renderBlackbox),
  new Feed('post', '/post', renderPost),
  new Feed('watchdog', '/watchdog', renderWatchdog),
  new Feed('outbound', '/outbound', renderOutbound),
  new Feed('calibration', '/calibration', renderCalibration),
  new Feed('cell', '/cell', renderCell),
  new Feed('trust', '/trust', renderTrust),
  new Feed('audit', '/audit', renderAudit),
]

function paintAges() {
  for (const feed of feeds) {
    const label = document.querySelector('[data-age="' + feed.name + '"]')
    const panel = $('panel-' + feed.name)
    if (!label || !panel) continue

    const age = feed.ageMs()
    if (feed.status === 'denied') {
      label.textContent = '· needs a token'
      panel.classList.remove('stale')
    } else if (age === null) {
      label.textContent = feed.status === 'pending' ? '· waiting' : '· ' + feed.detail
      panel.classList.remove('stale')
    } else {
      label.textContent = '· as of ' + (duration(age) || '0s') + ' ago' +
        (feed.status === 'ok' ? '' : ' · ' + feed.status + ': ' + feed.detail)
      panel.classList.toggle('stale', age > STALE_MS)
    }
  }
}

async function refresh() {
  // Sequentially rather than in parallel: §33.7's listeners handle one request per connection and
  // are single-threaded, so five concurrent fetches would queue anyway — and doing it in order
  // keeps the ages meaningful relative to each other.
  for (const feed of feeds) {
    await feed.poll()
    try {
      feed.render(feed)
    } catch (error) {
      // A panel that throws must not take the poll loop, and therefore the age labels, with it.
      const panel = $('panel-' + feed.name)
      if (panel) {
        const note = el('p', 'note', 'this panel failed to render: ' + error)
        panel.appendChild(note)
      }
    }
  }
  paintAges()
  $('poll-state').textContent =
    'Polling every ' + (POLL_MS / 1000) + 's · this page talks only to the robot that served it'
}

function applyToken() {
  writeToken($('token').value.trim())
  $('token').value = ''
  paintTokenState()
  refresh()
}

function paintTokenState() {
  $('token-state').textContent = readToken()
    ? 'A token is held for this tab. It is sent as a bearer credential and is not stored on disk.'
    : 'No token. Stop and status work; every other panel will report that it needs one.'
}

function start() {
  $('stop').addEventListener('click', stopAll)
  $('token-apply').addEventListener('click', applyToken)
  $('token-clear').addEventListener('click', () => {
    writeToken('')
    paintTokenState()
    refresh()
  })
  $('token').addEventListener('keydown', event => {
    if (event.key === 'Enter') applyToken()
  })

  paintTokenState()
  refresh()
  setInterval(refresh, POLL_MS)
  // Faster than the poll, so an age reads as counting up rather than jumping in two-second steps.
  setInterval(paintAges, 500)
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', start)
} else {
  start()
}
