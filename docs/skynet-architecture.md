# Skynet — Technical Architecture

> Skynet is the physical-AI product inside the BotMarket ecosystem (BotMarket · BotNews ·
> BotGuides · BotPapers · Robot Database · BotEdu · **Skynet**). It is the layer that connects
> AI policies to physical robots: models, datasets, skills, robots, environments, benchmarks,
> evaluations and deployments — versioned, reproducible and API-first.

---

## 0. Decision summary

| Layer | Decision | Why |
|---|---|---|
| Frontend | **Reuse** Next.js 16 App Router / React 19 / TS 7, under `app/[locale]/skynet/` | Shares navbar, locale routing, SEO helpers, ISR, deploy. A second frontend would fork auth, i18n and SEO for no gain. |
| Backend | **Reuse** Next.js route handlers, new versioned surface `app/api/skynet/v1/**` | The web UI becomes *one client* of a public REST API that also serves the CLI and Python SDK. |
| Database | **Reuse** Supabase Postgres, `public` schema, `skynet_*` table prefix | Shares `auth.users`, `profiles`, `organisations`, `botindex`. PostgREST only exposes `public`; a separate schema would need config changes and break the existing admin client. |
| Object storage | **Supabase Storage**, private bucket `skynet-artifacts`, content-addressed | Already provisioned and in the CSP. Access goes through an `ArtifactStore` driver so S3/R2/GCS can be swapped in without touching callers. |
| Large artifacts | Never in Postgres. `sha256` content addressing + direct-to-storage signed upload/download URLs | Weights and trajectory shards never pass through the web process; identical blobs dedupe across repos (git-LFS-like). |
| Search | Postgres FTS (`tsvector` + GIN) over a denormalised `skynet_search_documents` table kept in sync by triggers, behind one `SearchDriver` interface | Real relevance ranking today, zero new infrastructure; the driver boundary is the migration path to OpenSearch/Typesense. |
| Auth | **Reuse** Supabase Auth. Session (cookie/bearer) for the UI, hashed **API tokens** for CLI/SDK, hashed **worker tokens** for compute, hashed **device tokens** for robot agents | One identity, four credential classes with different blast radius. |
| Authorization | Namespace role derived from BotMarket `org_members`, evaluated in `lib/skynet/permissions.ts`; RLS on every table for the anon client | App-layer checks for service-role paths, RLS as defence in depth. One roster, so leaving an organisation removes Skynet access. |
| Jobs | Durable Postgres queue `skynet_jobs` with lease/heartbeat/retry semantics | No new broker. Correct semantics (at-least-once + idempotency keys) matter more than throughput at this stage. |
| Compute | **External workers** claim jobs over `/api/skynet/v1/worker/*`. Vercel crons only reap leases and run lightweight in-process job kinds | GPU training and Isaac Sim cannot and must not run in a web request. |
| Containers | Worker contract is a container image + entrypoint declared per job kind | Reproducibility and isolation; uploaded code is never executed on platform infrastructure. |
| Simulation | Capability-registry abstraction (Isaac Lab, MuJoCo, Gazebo, PyBullet, Webots) producing a normalised `SimulationSpec` → job payload | Honest: the platform plans and queues real runs; execution lands when a worker registers. Nothing is faked. |
| Compatibility | Declarative rule rows (`skynet_compatibility_rules`) evaluated by a pure predicate engine | Third parties add robots/hardware without shipping frontend code. |
| Deployment | Vercel (unchanged) for web/API; workers deploy anywhere that can reach the worker API | Keeps a single production deploy. |
| Observability | Structured job logs (`skynet_job_logs`), audit log (`skynet_audit_log`), activity feed, existing Vercel/Mixpanel analytics | Robot deployment needs an auditable trail. |
| i18n / SEO | **Reuse** next-intl (35 locales) and `lib/seo-locales.ts`, `safeJsonLd()` | Non-negotiable project rules apply to Skynet like everything else. |

### Rejected alternatives

* **Separate service (FastAPI/Django + own Postgres).** Would duplicate identity, orgs, i18n,
  SEO and deployment, and would force a second robot entity next to `botindex`. The web tier
  here is orchestration and metadata; the heavy work is already off-box in workers, which is
  exactly where a different language belongs.
* **Dedicated search cluster on day one.** Postgres FTS handles this corpus size with real
  ranking. The `SearchDriver` seam makes the swap a configuration change, not a rewrite.
* **S3 + presigned PUT on day one.** Same shape as Supabase signed uploads but adds a second
  vendor, a second CSP entry and a second credential. The driver interface keeps it a one-file
  change later.
* **Separate `skynet` Postgres schema.** Cleaner namespacing, but PostgREST exposes only the
  schemas listed in project config, and every existing helper is bound to `public`. Prefixed
  tables give the same isolation with none of the breakage.

---

## 1. Core object model

```
Namespace ─┬─ Repository ── RepositoryVersion ─┬─ VersionFile ── Artifact (sha256 blob)
           │   kind:                           ├─ RepoLink   (uses_model / trained_on / …)
           │   model | dataset | skill         └─ RepoRobot  (declared/verified support)
           │   environment | benchmark
           │
           ├─ Robot ── RobotAdapter         (Robot references botindex.slug — never duplicated)
           ├─ Evaluation ── EvaluationTaskResult
           ├─ TrainingRun
           ├─ Deployment ── DeploymentEvent
           └─ Device ── Episode
```

**Repository is the single unifying object.** A model, a dataset, a skill, an environment and a
benchmark are all repositories with a different `kind` and a different typed `manifest` on each
version. That gives every object type versioning, stars, forks, discussions, permissions,
search and API surface for free — and means a new object kind is a manifest schema, not a new
subsystem.

**Robots are a registry, not a repository**, because they describe hardware that exists
independently of any upload, and because they must reference the existing Robot Database
(`botindex.slug`) rather than fork it.

### Versioning

Semver per repository (`major`, `minor`, `patch`, `prerelease` stored as columns for correct
ordering). A version moves `draft → published` and may be `yanked`. Published versions are
immutable: files are content-addressed and the version carries a `digest` computed over the
manifest plus the sorted `(path, sha256)` list. That digest is the reproducibility key quoted
by every evaluation.

---

## 2. Storage architecture

```
client ──1. POST /artifacts/upload-init {sha256,size}
       │      ↳ blob already known?  → { deduplicated: true }   (no bytes transferred)
       │      ↳ otherwise            → { uploadUrl, token }     (signed, expires)
       ├──2. PUT bytes directly to Supabase Storage
       └──3. POST /artifacts/complete → server verifies object, records artifact + version file
```

* Path: `blobs/<sha[0:2]>/<sha[2:4]>/<sha>` — content addressed, immutable, dedup by construction.
* The bucket is **private**. Downloads are issued as short-lived signed URLs by
  `/artifacts/{id}/download`, which is also where access control and download counting happen.
* `ArtifactStore` (`lib/skynet/storage/store.ts`) is the only module that knows about Supabase
  Storage. Swapping to S3 means one new driver.

---

## 3. Job & compute architecture

```
API (enqueue) → skynet_jobs(queued) → worker POST /worker/lease → leased (lease_expires_at)
                                     ↳ heartbeat → running
                                     ↳ complete  → succeeded (+ result, artifacts, logs)
                                     ↳ fail      → failed | requeued (attempts < max_attempts)
cron /api/cron/skynet/jobs → reaps expired leases, retries, expires stale uploads
```

* At-least-once with `idempotency_key` uniqueness so retries cannot double-write.
* Queues: `cpu`, `gpu`, `sim`. Workers advertise queues and capabilities at registration.
* Job kinds: `evaluation`, `simulation`, `training`, `deployment`, `dataset_index`,
  `search_reindex`, `artifact_scan`.
* Domain side-effects of a finished job are applied by the worker-completion route, never by the
  worker's own request body: an evaluation's task results and a training run's metrics,
  checkpoints and output version are written server-side after the lease is verified. A training
  run's `output_version_id` is additionally checked to belong to the run's declared output
  repository, so holding a lease does not let a worker point a run at an arbitrary version.
* Only pure-metadata kinds (`dataset_index`, `search_reindex`) may run in-process on the cron;
  everything else requires a registered worker. **When no worker is connected the UI says so** —
  jobs sit in `queued` and nothing is fabricated.

---

## 4. Robot abstraction (Standard Robot Interface)

`lib/skynet/robots/interface.ts` defines a capability-typed interface — `move`, `walk`, `grasp`,
`get_state`, `get_camera`, `execute` — where every method maps to a **capability token**
(`locomotion.walk`, `manipulation.grasp`, `sensing.camera.rgb`, …). An adapter declares which
tokens it implements plus an `interface_version` range.

```
Policy → Skynet Runtime → Robot Adapter → (ROS 2 | gRPC | vendor SDK | simulator) → Robot
```

The abstraction does not collapse to a lowest common denominator: adapters also declare
`extensions` (vendor-specific capability tokens and raw passthrough), and a policy may require
them. The compatibility engine then reports precisely which robot/adapter pair satisfies a
policy's requirement set.

---

## 5. Compatibility engine

A **pure** evaluator over a context assembled from model manifest + robot + adapter +
environment + hardware:

```
predicate := {op: and|or|not|eq|neq|in|gte|lte|includes|includes_all|exists|semver_satisfies,
              path, value}
rule      := {scope, severity: error|warning|info, when?, require, message, remediation}
```

Rules live in `skynet_compatibility_rules`, seeded from
`lib/skynet/compatibility/builtin-rules.ts`, so adding hardware or a new middleware constraint
is a data change. The engine returns per-rule findings; no page hardcodes compatibility.

---

## 6. API surface (`/api/skynet/v1`)

```
GET    /search                          cross-object search
GET    /repos                           list/filter (kind, namespace, tag, sort)
POST   /repos                           create
GET    /repos/{ns}/{name}               repo + latest version
PATCH  /repos/{ns}/{name}               update metadata / readme / visibility
GET    /repos/{ns}/{name}/versions      list versions
POST   /repos/{ns}/{name}/versions      create draft version
POST   /repos/{ns}/{name}/versions/{v}/publish
GET    /repos/{ns}/{name}/versions/{v}/files
POST   /repos/{ns}/{name}/star | /fork
GET    /repos/{ns}/{name}/compatibility?robot=&adapter=&hardware=
GET    /repos/{ns}/{name}/compare?from=&to=   what changed between two versions
GET    /repos/{ns}/{name}/versions/{v}/load?robot=   the load contract for this version
GET|POST|PATCH /repos/{ns}/{name}/access      gated-repository terms, requests and decisions
GET    /repos/{ns}/{name}/pulls           proposals into this repository (status)
POST   /repos/{ns}/{name}/pulls           propose a version from a fork
GET    /repos/{ns}/{name}/pulls/{n}       proposal + comments + the diff a merge would apply
PATCH  /repos/{ns}/{name}/pulls/{n}       close | reopen
POST   /repos/{ns}/{name}/pulls/{n}/merge publish it as a version of this repository
GET    /repos/{ns}/{name}/discussions     list threads (kind, state)
POST   /repos/{ns}/{name}/discussions     open a discussion or issue
GET    /repos/{ns}/{name}/discussions/{n} thread + comments
PATCH  /repos/{ns}/{name}/discussions/{n} open | close
POST   /repos/{ns}/{name}/discussions/{n}/comments
POST   /artifacts/upload-init | /artifacts/complete | GET /artifacts/{id}/download
GET    /robots  POST /robots  GET /robots/{slug}  GET /robots/{slug}/adapters
GET    /benchmarks/{ns}/{name}/leaderboard
POST   /evaluations  GET /evaluations/{id}
POST   /simulations
GET    /training                        runs across every namespace you belong to
POST   /training                        queue a run; pins base/dataset/environment digests
GET    /training/{id}                   run + job + worker logs + live queue position
PATCH  /training/{id}                   cancel (the only status a person may set)
POST   /deployments
GET    /jobs  GET /jobs/{id}  GET /jobs/{id}/logs
POST   /worker/register | /worker/lease | /worker/{job}/heartbeat | /complete | /fail | /logs
GET    /devices  POST /devices  GET|PATCH|DELETE /devices/{id}
GET    /episodes                        namespace-scoped, member only
POST   /episodes                        ingest — device token only
GET    /me  GET /namespaces/{handle}  POST /tokens
GET|POST /namespaces/{handle}/follow    follow state / toggle
GET    /activity                        public feed; ?following=1 for your follows
GET|POST /namespaces/{handle}/teams      teams; members only, owner to create
GET|PATCH|DELETE /namespaces/{handle}/teams/{slug}   membership and repository grants
GET|POST|DELETE /namespaces/{handle}/keys        signing keys (public keys only)
GET|POST /repos/{ns}/{name}/versions/{v}/signature
GET|POST|DELETE /secrets                namespace secrets; values are never returned
POST   /worker/jobs/{id}/secrets        the sole route that returns a decrypted secret
```

Auth precedence: `Authorization: Bearer skn_…` (API token) → Supabase bearer → cookie session.
Every write is rate-limited and audit-logged.

`POST /episodes` is the sole route authenticated by a **device** credential. Its namespace is
taken from the device record rather than the request body, so an agent recovered from a robot in
the field cannot redirect uploads, read repositories or claim jobs.

---

## 6a. The load contract

`GET /repos/{ns}/{name}/versions/{v}/load` returns everything a runtime needs to turn a published
version into a running policy, and it is what `Policy(...).load()` and `skynet load` are built on.

The plan is computed server-side rather than by each client, because three of its inputs are not
available to a client:

* **the loader** is derived from the format the artifact scanner identified in the bytes, not from
  the file extension. `.pt` is written both by `torch.save` (a pickle, which executes code when
  read) and by `torch.jit.save` (a TorchScript archive); the extension cannot separate them and
  the header can;
* **the trust cost** travels with the plan, so a caller learns which files execute code *before*
  fetching them. The SDK refuses those by default and makes the caller pass `trust_remote_code`;
* **the safety envelope** of a named robot rides along, so the runtime doing the stepping holds
  the torque, velocity and watchdog ceilings locally. A clamp that needs a network round trip is
  a clamp that fails exactly when the link does.

A version may declare its contract in `manifest.runtime`: `loader`, `entrypoint`, `config`,
`framework`, `container_image`. `entrypoint` and `config` are validated **at publish time**
against the files actually uploaded — a card naming a file the version does not contain is a load
instruction that fails on every machine that follows it, and this is the last cheap moment to
catch it. A version that declares nothing is still loadable; the plan then says the loader was
inferred rather than promised.

The plan reports; it does not block. A caller who can download the files can load them however
they like. The two places that can genuinely refuse do so from the same facts: the SDK, and the
deployment gate.

## 6b. Pull requests

`forked_from_id` existed from the first migration and nothing merged back, which made forks a
one-way split. A pull request is a **discussion of kind `pull_request`** with a proposal row
attached, so it inherits per-repository numbering, comments, state and moderation, and issue #7
cannot collide with pull request #7.

Merging publishes a new version of the **target** whose files and manifest are the proposal's.
Content addressing makes that free — no bytes move, both repositories reference the same
artifacts. It runs through the ordinary `publishVersion` path, so the digest computation, the
runtime-contract check and benchmark task validation all apply: a proposal cannot be used to
route around a gate a direct push would hit. Accordingly it requires `repo.publish`, the same
capability a direct release needs, and is written to the audit log.

There is no three-way merge, because a version is a snapshot rather than a commit. What matters
instead is **staleness**: `base_version_id` records the target's head when the proposal was
opened, the diff is measured from there, and a target that has since moved is reported rather
than silently re-diffed. It is never an automatic refusal — a proposal against an older head is
frequently still exactly right.

`skynet_repo_versions.merged_from_version_id` records where a merged version came from. A
published version that appeared out of another namespace with no trail is precisely the
provenance hole this platform exists to close.

Read access is two-sided: a proposal is visible only to someone who can read both repositories,
and one whose source is less visible than its target is refused at open time — a reviewer who
cannot see what they are being asked to merge cannot review it.

---

## 7. Security posture

Robot deployment is treated as higher-risk than ordinary web software:

* Uploaded code is **never executed** on platform infrastructure — only inside worker containers
  the operator controls, with the job payload pinned to immutable artifact digests.
* **Release signatures.** A digest proves the bytes did not change; a signature proves who
  produced them. Publishers register Ed25519 public keys and sign the version digest locally;
  the platform verifies before storing and never records an unverified signature, so "signed" on
  a version page cannot be a lie. Private keys never reach the platform — no endpoint accepts
  one. Keys are revoked, never deleted, so earlier signatures stay attributable.
* **Secrets.** Credentials a job needs and the platform must not own are stored per namespace
  under AES-256-GCM with a key held in the environment, so a database dump alone discloses
  nothing. Values are write-only from the outside: no route returns one to a person or a
  browser. The only reader is a worker presenting both a worker credential and the live lease
  token for a job, and the namespace is taken from the job row rather than the request — a
  worker leasing a job in one namespace cannot ask for another's credentials. With no key
  configured, storing fails loudly rather than silently falling back to plaintext.
* Tokens are stored as SHA-256 hashes with a visible prefix only; scopes are enforced per route.
* Private repositories are enforced in the permission layer *and* in RLS.
* **Membership is not Skynet's to own.** A user namespace has exactly one member — the profile in
  `skynet_namespaces.user_id`. An org namespace's members *are* `org_members` for the organisation
  behind it, mapped `owner → owner`, `admin → maintainer`, anything else → `writer`. Skynet had
  its own `skynet_members` table until 20260828210000 dropped it: two rosters could disagree, and
  the one that governed access was the copy nobody maintained, so removing somebody from a
  BotMarket organisation left their access to its private repositories — and to the robots those
  repositories deploy to — intact. The mapping lives in `skynet_private.member_role` (what RLS
  enforces) and `ORG_ROLE_TO_SKYNET` in `lib/skynet/namespaces.ts` (what the service-role routes
  enforce); a unit test reads the migration and holds the two to each other. There is deliberately
  no way to edit membership from Skynet — that happens in BotMarket organisation settings, where
  seats are billed.
* **Teams** add a second access axis: a group of namespace members granted a role on chosen
  repositories. Effective repository access is the **maximum** of a person's direct namespace
  role and any team grant, so joining a team can never remove access. A team grant confers
  repository capabilities only — never namespace administration, deployments or token issue —
  and `owner` is not a grantable team role, so a team cannot be a route to taking over an
  organisation. RLS follows through `skynet_private.has_team_access`, a purely additive branch
  in `can_read_repo`.
* `scripts/skynet-verify-rls.mjs` probes these invariants against a live database by observation
  rather than by reading policy text — public repo visible to anon, private not, credential and
  team tables invisible, reference data still readable, and a team grant never widening anything
  for an anonymous caller. Run it after any migration that touches a policy or a
  `skynet_private` helper.
* Signed, expiring download URLs; no public bucket listing.
* `skynet_audit_log` records every deployment transition, token issue/revoke and permission change.
* Version pinning plus digests give provenance: a deployment records exactly which digest it runs.

---

## 8. Build order

* **Now:** schema, permissions, repositories + versions + artifacts, robot registry bound to
  `botindex`, skills, environments, benchmarks, evaluations (submission + results), compatibility
  engine, search, orgs/namespaces, stars/forks, discussions, activity + personalised feed,
  training orchestration, dataset schema/splits/statistics views, collected-episode summaries,
  release signing, namespace secrets, teams, REST API, CLI, Python SDK, Skynet UI, BotMarket nav +
  Robot Database cross-links, SEO, sitemap.
* **Architected, awaiting infrastructure:** GPU training *execution* (definition, queueing,
  provenance, cancellation and reporting are built), large-scale simulation,
  browser Spaces rendering, physical deployment rollout, teleoperation capture, fleet management.
  Each has its schema, API, job kind, permission model and UI — and reports honestly that it is
  waiting on a registered worker rather than displaying invented results.

## 9. Completion roadmap and evidence gates

The repository/control plane above is not enough to call Skynet a complete GitHub/Hugging
Face-style physical-AI platform. The following phases close the remaining gaps in dependency
order. A feature is **not** described as operational, publicly launched, or safety-enforcing until
its phase's exit gate has evidence from the deployed path.

### Phase A — prove the existing Hub in production

Before adding more surface area, verify the deployed foundation:

* the Skynet migrations, private `skynet-artifacts` bucket, Storage policies, CSP, signed upload
  URLs, signed download URLs, scanner, and artifact deletion/reconciliation path;
* RLS and service-role authorization for public/private repositories, API tokens, worker tokens,
  device tokens, teams, secrets, and gated repository access;
* a real end-to-end release: authenticated draft → direct upload → scan → publish → authorized
  download → signed digest/signature verification;
* rate limits, audit records, worker-lease expiry, retry/idempotency, backup/restore, and alerting.

**Exit gate:** automated integration tests against the deployed project pass these flows, and an
operator can produce their evidence without seeding fake models, benchmark results, or workers.

### Phase B — complete repository and data-product workflows

Keep immutable artifact releases as Skynet's source of truth, but add the developer workflows a
repository hub still lacks:

* Git interoperability for source repositories: clone/push/pull transport or a proven external-Git
  mirror, commit/branch/tag provenance, protected release channels, review approvals, required
  status checks, and source-to-release traceability. Do not implement a Git smart-HTTP server in
  a Next.js route; isolate it as a dedicated service if native transport is required.
* repository review UX: file and manifest diffs, line-level review for text/source artifacts,
  review assignments, checks, release approval records, and policy-controlled merge/publish.
* dataset product workflows: schema previews, sample/trajectory/video viewing, split and shard
  manifests, resumable/ranged download or streaming, quality reports, licensing/consent lineage,
  deletion/takedown, and dataset-to-training/evaluation provenance.
* real SDK and CLI contract tests for login, upload, download, release, pull-request, evaluation,
  device, and deployment paths; generated OpenAPI/API compatibility checks for public clients.

**Exit gate:** a developer can reproduce a published release from its source revision and artifact
digest, review it through a protected release workflow, and consume a dataset without moving the
entire dataset through the web application.

### Phase C — make workers, simulation, and evaluation real

The job protocol becomes a product only when an external worker can execute safely and report a
reproducible result:

* ship a reference worker image with pinned runtime versions, signed image digest, least-privilege
  worker credentials, sandboxed ephemeral workspaces, resource limits, cancellation, structured
  logs, result/artifact upload, and secret access only for its verified live lease;
* register a first real simulator adapter — start with MuJoCo or another CI-feasible simulator —
  and run one published policy/environment/robot evaluation end to end;
* add deterministic replay fixtures, simulator-version and hardware/driver attestation, result
  validation, leaderboard eligibility rules, and a quarantine path for failed or unverifiable runs;
* add worker health, queue depth, capacity, cost, and stale-lease monitoring before admitting
  unbounded user workloads.

**Exit gate:** a non-seeded evaluation starts from immutable versions, runs on a registered worker,
produces signed provenance and artifacts, survives a worker crash/retry without double-writing, and
appears on a leaderboard only after result validation.

### Phase D — managed training, inference, and Spaces

After one verified worker path exists, expand it rather than creating parallel execution systems:

* GPU training workers for declared, reviewed recipes with quota, hardware attestation,
  checkpoint/resume, metric streaming, experiment comparison, cancellation, retention, and
  output-version publication;
* inference workers behind version-pinned endpoints with explicit supported runtimes, hardware
  limits, authentication, metering, rate limits, request/response audit boundaries, and teardown;
* browser Spaces as isolated, short-lived worker sessions with a declared image, policy/environment
  versions, WebSocket/session authorization, resource caps, queue position, user-visible failure
  state, and no direct actuator credential;
* tenant quotas and budgets before public submission, so simulations, training, and inference cannot
  consume unbounded GPU capacity.

**Exit gate:** an approved workload can train or run from pinned inputs, resume after an interruption,
publish a verifiable output version, and expose a bounded interactive demo without executing
untrusted uploaded code in the web tier.

### Phase E — robot agent, fleet control plane, and data collection

Build the edge path in a simulator first, then on a controlled physical test robot:

* a minimal Skynet agent with device enrollment, hardware/image attestation, rotating credentials,
  offline-safe local state, telemetry buffering, and explicit capabilities;
* desired-state reconciliation for signed package install, capability grant, health/readiness,
  canary waves, pause, rollback, and audit trails; deployment status must be derived from device
  acknowledgement, never from an optimistic API write;
* episode/teleoperation ingestion with clock synchronization, calibrated sensor metadata, operator
  identity/intervention records, consent/retention controls, encrypted upload, and dataset
  finalization only after integrity checks;
* fleet dashboards for device inventory, version drift, rollout progress, health, logs, black-box
  artifacts, and incident handling.

**Exit gate:** a test device can enroll, pull a signed desired state after reconnecting, reject an
unauthorized or incompatible package, complete a canary deployment, roll back from a failed health
check, and upload an integrity-verified episode without a network dependency in its control loop.

### Phase F — Skynet OS and the safety/runtime boundary

The TypeScript Hub-side reference modules are not a robot OS. Implement and verify the runtime in
layers described by `docs/skynet-os-architecture.md`:

* move shared embodiment, transport, package, and safety contracts into a Rust crate with Hub/WASM
  and device consumers plus differential tests against the TypeScript reference;
* build the allocation-free C or Rust Safety Kernel and an RHAL simulator driver first; prove that
  the kernel is the only actuator writer, guards stale/over-limit commands, and fails safely;
* add one real vendor or ROS 2/RHAL driver under bench supervision, then a hardware-backed
  conformance suite for timing, command latency, stop path, embodiment signatures, and transport;
* add `skynetd`, signed A/B OTA, measured boot/secure-element device identity, black-box logging,
  calibration expiry, and a safety-MCU path before any certified-tier claim;
* keep Guest, Host, and Certified enforcement tiers visibly distinct. Guest monitoring is not
  safety enforcement, and no Hub UI may imply otherwise.

**Exit gate:** a hardware-backed test demonstrates independent safe stop, watchdog/envelope
protection, package provenance enforcement, deterministic rollback, and parity between the Hub's
reported plan and the device's enforced plan. Certification claims require independent assessment;
Skynet only supplies evidence.

### Phase G — operations, governance, and commercial readiness

Only after the verified execution and fleet paths exist:

* define workload cost accounting, tenant quotas, retention, legal/privacy controls for robot data,
  abuse reporting, artifact takedown, export/deletion, and incident response;
* add marketplace entitlements for paid artifacts, compute, inference, and enterprise fleet access
  without weakening immutable provenance, authorization, or the safety boundary;
* set service-level objectives for Hub APIs, workers, artifact availability, fleet reconciliation,
  and incident recovery; publish the evidence level of every verification badge.

**Exit gate:** paid or enterprise capabilities cannot bypass repository permissions, signed release
provenance, worker isolation, device capability grants, or rollout safety gates.
