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
 *      shape. The guards panel is the whole of that case: §32.2 asks for live guard state, this
 *      daemon has none, and eight green ticks would be a lie told convincingly.
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
    this.at = 0
    this.status = 'pending'
    this.detail = ''
  }

  async poll() {
    try {
      const result = await call('GET', this.path)
      if (result.ok) {
        this.value = result.body
        this.at = Date.now()
        this.status = 'ok'
        this.detail = ''
      } else if (result.status === 401 || result.status === 403) {
        this.status = 'denied'
        this.detail = (result.body && result.body.error) || 'this needs a token'
      } else {
        this.status = 'error'
        this.detail = (result.body && result.body.error) || ('HTTP ' + result.status)
      }
    } catch (error) {
      this.status = 'unreachable'
      this.detail = String(error && error.message ? error.message : error)
    }
  }

  ageMs() {
    return this.at ? Date.now() - this.at : null
  }
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

  // §27's state machine. Nothing implements it, so the console names the state it cannot report
  // rather than picking the most likely one.
  add('State machine', absent(
    'unreported — §27’s HOLD / RUNNING / DEGRADED / STOPPING / SAFE needs the kernel and the ' +
    'policy runtime, and neither is written'))

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
 * The guards panel, which has nothing to show and says so precisely.
 *
 * §32.2 asks for live guard state: which are active, which are unavailable with the exact input
 * missing, trip counts this session. That needs the Safety Kernel evaluating cycles on this robot,
 * and §33.1 keeps `skynetd` away from the body entirely — it supervises processes, it does not
 * command joints. So there is no guard state here to report, and there will not be until the
 * policy runtime and the kernel channel exist.
 *
 * Written out rather than left as an empty panel because an empty panel reads as "nothing wrong".
 *
 * The two pose guards are the reason this is redrawn when the body arrives. With no descriptor,
 * what they are missing is a frame graph; with one, the tree is here and what is missing is a
 * control loop to walk it in cycle. Both are "unavailable" and they are different jobs, and a
 * panel that named the wrong one would send somebody to write a generator that already ran.
 */
function renderGuards() {
  const body = clear($('guards-body'))
  const known = bodyFeed.value && bodyFeed.value.state === 'present' ? bodyFeed.value : null
  const frames = known && known.frames && known.frames.present ? known.frames : null

  const list = el('table')
  const head = el('tr')
  head.appendChild(el('th', null, 'Guard'))
  head.appendChild(el('th', null, 'State'))
  head.appendChild(el('th', null, 'Missing input'))
  list.appendChild(head)

  // The eight of §7.1, in evaluation order, each with the input this robot does not have. Naming
  // them individually is §7.2's rule applied to the console: a guard that cannot run reports what
  // it wanted, not just that it is off.
  const guards = [
    ['mode_gate', 'the capability token on a command cycle'],
    ['envelope', 'a resolved envelope and a command to clamp'],
    ['rate_limit', 'two successive commands to difference'],
    ['workspace', frames
      ? frames.dynamic + ' of ' + frames.nodes + ' frames move with a joint — what is missing is a ' +
        'control loop to walk them in cycle'
      : 'a frame graph on this robot, and forward kinematics in cycle'],
    ['self_collision', frames
      ? (frames.collision_bodies >= 2
        ? frames.collision_bodies + ' frames carry collision geometry — what is missing is a ' +
          'control loop to place them'
        : frames.collision_bodies + ' frame(s) carry collision geometry, and a body with fewer ' +
          'than two has nothing to collide with')
      : 'a frame graph with collision geometry, and forward kinematics in cycle'],
    ['proximity', 'a separation distance from a sensor'],
    ['balance', 'a centre of pressure — implemented by no edition'],
    ['watchdog', 'a command deadline from a running control loop'],
  ]
  for (const [name, wants] of guards) {
    const row = el('tr')
    row.appendChild(el('td', null, name))
    const state = el('td')
    state.appendChild(pill('unavailable', 'off'))
    row.appendChild(state)
    row.appendChild(el('td', 'dim', wants))
    list.appendChild(row)
  }
  body.appendChild(list)

  body.appendChild(el('p', 'note',
    'No guard state is reported because none exists on this robot. §33.1 keeps skynetd away from ' +
    'the body — it supervises processes and never commands a joint — so the Safety Kernel is not ' +
    'evaluating cycles here, and trip counts have nothing to count. This panel will carry live ' +
    'state when the policy runtime and the kernel channel are written; until then eight green ' +
    'ticks would be the one thing worse than an empty panel.'))
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
    // Redraw the guards, because what they are missing depends on this.
    renderGuards()
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

  renderGuards()
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

  // The other plan §32.2 asks for, and the reason it is not above.
  const transport = clear($('transport-body'))
  transport.appendChild(el('h2', null, 'Transport plan'))
  transport.appendChild(absent('unavailable on this robot'))
  const hasBody = bodyFeed.value && bodyFeed.value.state === 'present'
  transport.appendChild(el('p', 'note',
    '§32.2 asks for the resolved transport plan and the clamps in force, with provenance for ' +
    'every number. Resolving one needs a body and a policy trained on some other body. ' +
    (hasBody
      ? 'This robot now has the first. What is missing is a policy declaring the embodiment it ' +
        'was trained on, which arrives with the package that carries it.'
      : 'This robot has neither.') +
    ' `skynet transport` computes exactly this plan on a workstation, and the numbers it produces ' +
    'are the numbers this panel will show.'))
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

  const sealed = b.sealed || []
  if (sealed.length === 0) {
    body.appendChild(el('p', 'note', 'No sealed recording on this robot yet.'))
  } else {
    const table = el('table')
    const head = el('tr')
    for (const label of ['Sealed file', 'Size', '']) head.appendChild(el('th', null, label))
    table.appendChild(head)
    for (const file of sealed) {
      const row = el('tr')
      row.appendChild(el('td', null, file.name))
      row.appendChild(el('td', 'num', bytes(file.bytes)))
      const cell = el('td')
      const download = el('button', 'act', 'download')
      download.addEventListener('click', () => downloadSealed(file.name))
      cell.appendChild(download)
      row.appendChild(cell)
      table.appendChild(row)
    }
    body.appendChild(table)
  }
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

// ── The poll loop ────────────────────────────────────────────────────────────

const bodyFeed = new Feed('embodiment', '/embodiment', renderEmbodiment)

const feeds = [
  new Feed('status', '/status', renderStatus),
  bodyFeed,
  new Feed('plan', '/plan', renderPlan),
  new Feed('packages', '/packages', renderPackages),
  new Feed('health', '/health', renderHealth),
  new Feed('blackbox', '/blackbox', renderBlackbox),
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

  renderGuards()
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
