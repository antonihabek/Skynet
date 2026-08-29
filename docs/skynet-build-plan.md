# Build Skynet — Physical AI Platform for BotMarket

You are the lead software architect and engineer for **Skynet**, a new major product within the existing **BotMarket** ecosystem.

Skynet is **not a standalone company or standalone website**. It is a product/module inside BotMarket, alongside:

- **BotMarket** — robot marketplace
- **BotNews** — robotics news
- **BotGuides** — robotics guides
- **BotPapers** — robotics research papers
- **Robot Database** — robot database
- **BotEdu** — robotics education
- **Skynet** — physical AI infrastructure

The existing website is:

**https://botmarket24.com/**

Skynet should feel like a natural part of the same ecosystem while having its own product identity and interface.

## 1. Product vision

Build **Skynet: the open platform for physical AI**.

The conceptual analogy is:

- GitHub → source code
- Hugging Face → AI models and datasets
- Docker Hub → deployable software containers
- Kaggle → datasets/experiments
- Skynet → **robot AI models, policies, datasets, skills, simulations, benchmarks, and deployment**

The ultimate goal is to become the infrastructure layer where developers can discover, upload, version, test, evaluate, train, and deploy AI software for physical robots.

Do NOT make this merely a directory of robots.

The core objects should be:

**Models → Datasets → Skills → Robots → Environments → Evaluations → Deployments**

The platform should eventually support humanoids, quadrupeds, robotic arms, drones, and other physical-AI systems, even though humanoids are the initial focus.

---

# 2. IMPORTANT: first task is architecture

Before writing substantial application code, independently determine the best technical architecture.

You are explicitly authorized to choose:

- frontend framework
- backend framework
- database
- object storage
- search engine
- authentication
- compute architecture
- job queues
- container infrastructure
- simulation infrastructure
- ML infrastructure
- API architecture
- deployment infrastructure
- observability stack
- CI/CD
- package structure

Do not blindly follow a predetermined stack.

Evaluate the requirements and select a stack that can realistically evolve from an MVP into a large-scale platform.

The existing BotMarket codebase and infrastructure should be inspected first so Skynet integrates cleanly rather than creating unnecessary parallel infrastructure.

Prefer reusing existing infrastructure where technically sensible.

---

# 3. First inspect the existing project

Before implementation:

1. Inspect the existing repository.
2. Understand its frontend architecture.
3. Understand its backend/API architecture.
4. Inspect the existing database schema.
5. Inspect authentication.
6. Inspect storage.
7. Inspect deployment configuration.
8. Inspect design system/components.
9. Inspect navigation.
10. Inspect existing user/account models.
11. Inspect existing SEO architecture.
12. Inspect existing robots/database entities.
13. Determine how Skynet should integrate with existing BotMarket data.

Do not duplicate functionality that already exists.

If Robot Database already contains a robot, Skynet should reference that robot rather than creating a second incompatible robot record.

---

# 4. Core information architecture

Create a Skynet section with navigation similar to:

```text
Skynet

Explore
├── Models
├── Datasets
├── Skills
├── Robots
├── Environments
├── Benchmarks
└── Leaderboards

Build
├── Upload Model
├── Upload Dataset
├── Create Skill
├── Create Environment
└── Create Benchmark

Run
├── Simulation
├── Evaluation
├── Training
└── Deployments

Community
├── Organizations
├── Discussions
├── Projects
└── Activity
```

The exact IA can be improved if you identify something better.

---

# 5. Models / Policies Hub

Build a model repository system inspired by Hugging Face.

Every model should have:

- unique namespace
- repository name
- description
- README
- owner
- organization
- version
- releases
- files
- model weights
- configuration
- inference code
- training code
- dependencies
- license
- tags
- supported robots
- supported environments
- supported tasks
- hardware requirements
- benchmarks
- performance
- downloads
- stars
- forks
- likes
- version history

Example:

```text
openrobot/
└── door-opening-v3
```

Model page:

```text
Door Opening V3

Task:
Door manipulation

Supported robots:
✓ Unitree G1
✓ Unitree H1
✓ GR-1

Supported simulation:
✓ Isaac Lab
✓ MuJoCo

Parameters:
340M

Success rate:
87.4%

License:
Apache 2.0
```

Implement repository versioning rather than treating uploaded files as static attachments.

---

# 6. Dataset Hub

Build a first-class dataset platform.

Datasets should support:

- robot trajectories
- demonstrations
- video
- RGB
- depth
- proprioception
- joint states
- actions
- force/torque
- rewards
- task labels
- success/failure
- metadata

Dataset repository structure should support large files and future streaming.

Example:

```text
openrobot/
└── humanoid-manipulation-1m
```

Include:

- dataset viewer
- metadata
- versions
- train/test splits
- filtering
- statistics
- schema
- licenses
- contributors
- download/streaming
- API access

Do not store huge datasets directly in the relational database.

Use appropriate object storage and metadata architecture.

---

# 7. Robot Registry

Create a standardized robot registry.

Robots should include:

- manufacturer
- model
- version
- robot type
- degrees of freedom
- sensors
- actuators
- compute hardware
- operating system
- middleware
- ROS/ROS 2 support
- SDK
- URDF/USD
- simulation assets
- supported models
- supported skills
- supported environments

Crucially, integrate this with the existing **Robot Database**.

Do not create unnecessary duplicate robot entities.

If a robot already exists in BotMarket's Robot Database, Skynet should reference it.

---

# 8. Standard Robot API / Adapter system

This is one of the most important technical components.

Create a standardized abstraction layer so policies can target multiple robots.

Conceptually:

```python
robot.move(...)
robot.walk(...)
robot.grasp(...)
robot.get_state(...)
robot.get_camera(...)
robot.execute(...)
```

Different physical robots should have adapters implementing the standardized interface.

Example:

```text
Skynet Policy
      ↓
Skynet Runtime
      ↓
Robot Adapter
      ↓
Unitree G1
```

or:

```text
Skynet Policy
      ↓
Skynet Runtime
      ↓
Robot Adapter
      ↓
Figure
```

Design this carefully so the abstraction does not prevent access to robot-specific capabilities.

---

# 9. Skills

Create **Skills** as a first-class object.

A Skill is more than a model.

Example:

```text
Open Door
Pick Up Cup
Pour Liquid
Fold Shirt
Walk Upstairs
Insert Plug
```

A skill should be able to reference:

```text
Skill
├── Model
├── Dataset
├── Robot adapters
├── Simulation environment
├── Evaluation suite
└── Documentation
```

A user should eventually be able to:

```text
Select Skill
→ Select Robot
→ Select Environment
→ Run
```

Skills should have versions and benchmarks.

---

# 10. Simulation

Create a simulation infrastructure abstraction supporting major robotics simulators.

Initial architecture should be compatible with:

- NVIDIA Isaac Sim / Isaac Lab
- MuJoCo
- Gazebo
- PyBullet
- Webots

Do not attempt to fully implement every simulator immediately.

Build a clean abstraction layer.

A simulation object should contain:

```text
Environment
Robot
Task
Policy
Configuration
Evaluation
```

---

# 11. Browser-based "Spaces"

Create the Skynet equivalent of Hugging Face Spaces.

Users should eventually be able to visit:

```text
/skynet/spaces/open-door
```

and interact with a robot simulation.

Example:

```text
Robot: Unitree G1
Policy: OpenDoor-v3
Environment: Kitchen-v2

Task:
[ Open the refrigerator ]

             [ RUN ]
```

The architecture must allow simulation jobs to execute asynchronously.

Do not attempt to run serious robotics simulation directly inside the web server.

Build the abstraction for remote workers/jobs.

---

# 12. Evaluation and benchmarks

Build a standardized evaluation framework.

A benchmark should contain:

- tasks
- environments
- supported robots
- evaluation metrics
- scoring methodology
- version
- reproducibility configuration

Example:

```text
Manipulation Benchmark v1

Pick object        94%
Place object       91%
Open drawer        82%
Pour liquid        73%
Plug insertion     68%
```

Create:

- benchmark pages
- leaderboard pages
- model rankings
- robot rankings
- task-specific rankings
- historical results

Results should be reproducible and tied to exact:

- model version
- dataset version
- environment version
- benchmark version
- hardware configuration

This is essential.

---

# 13. Compatibility Engine

Build a compatibility system connecting:

```text
Model
+
Robot
+
Environment
+
Hardware
```

The platform should be able to answer:

```text
Model X
Robot Y
GPU Z

✓ Model compatible
✓ Robot adapter available
✓ ROS 2 supported
✓ GPU sufficient
⚠ Requires adapter v2
✗ Depth camera configuration unsupported
```

Create compatibility metadata and a rules engine rather than hardcoding compatibility into frontend pages.

---

# 14. Training infrastructure

Design the architecture for cloud training.

Users should eventually be able to:

```text
Dataset:
Humanoid Manipulation 1M

Base model:
OpenVLA

Robot:
Unitree G1

Training:
4 × H100

→ Start Training
```

Support the architecture for:

- imitation learning
- reinforcement learning
- VLA fine-tuning
- LoRA
- distributed training
- checkpoints
- experiments
- hyperparameters
- logs
- metrics

Training should run through asynchronous jobs/workers.

The web application should orchestrate jobs, not execute GPU training inside the web request process.

---

# 15. Deployment

Create a deployment system for robot policies.

Conceptually:

```bash
skynet deploy openrobot/door-v3 --robot unitree-g1
```

The platform should eventually manage:

- model version
- dependencies
- runtime
- container
- robot adapter
- deployment configuration
- health
- logs
- metrics
- rollback
- version history

Design a standardized **Skynet Robot Runtime**.

Conceptually:

```text
AI Policy
    ↓
Skynet Runtime
    ↓
ROS 2 / Middleware
    ↓
Robot Adapter
    ↓
Robot Hardware
```

---

# 16. Real-world data collection

Design the architecture for a Skynet robot agent.

Conceptually:

```text
Robot
  ↓
Skynet Agent
  ↓
Cloud
  ↓
Dataset
```

It should eventually collect:

- video
- sensor data
- joint state
- actions
- interventions
- task outcome
- failures

Data should automatically become usable for dataset creation and model training.

---

# 17. Teleoperation

Design infrastructure for demonstration collection.

Potential input sources:

- VR
- motion capture
- controllers
- gamepads
- keyboard
- remote teleoperation
- vision-based interfaces

Each demonstration should be capable of becoming a trajectory dataset.

---

# 18. Developer platform

Build:

### CLI

```bash
skynet login
skynet clone
skynet push
skynet pull
skynet run
skynet evaluate
skynet deploy
```

### Python SDK

```python
from skynet import Robot, Policy, Dataset

robot = Robot("unitree/g1")
policy = Policy("openrobot/grasp-v4")

robot.run(policy)
```

### API

Provide a clean API for:

- repositories
- models
- datasets
- robots
- skills
- environments
- evaluations
- deployments
- jobs

Design the API for external developers from the beginning.

---

# 19. GitHub-like collaboration

Implement:

- organizations
- users
- repositories
- stars
- forks
- issues
- discussions
- comments
- releases
- versioning
- activity feeds
- followers
- permissions
- private repositories
- teams

Repositories should feel familiar to developers.

---

# 20. Marketplace

Design the architecture for eventually allowing users to monetize:

- models
- datasets
- skills
- environments
- training
- inference
- simulation
- enterprise deployments

Do not prioritize marketplace functionality over the core developer platform.

The ecosystem should first create useful open infrastructure and then introduce monetization.

---

# 21. Search

Search must work across the entire Skynet ecosystem.

Users should be able to search:

```text
"humanoid manipulation"
"Unitree G1 grasping"
"door opening"
"VLA"
"Isaac Lab"
"robot trajectory"
```

Search across:

- models
- datasets
- skills
- robots
- environments
- benchmarks
- users
- organizations

Design the search architecture so it can scale beyond basic SQL queries.

---

# 22. Social / discovery layer

Every object should support appropriate:

- stars
- likes
- forks
- downloads
- followers
- activity
- related objects

Create discovery pages:

```text
Trending Models
Popular Datasets
Trending Skills
Popular Robots
New Benchmarks
Recently Updated
```

---

# 23. Security

Treat robot deployment as significantly more sensitive than normal web software.

Build with:

- authentication
- authorization
- organization permissions
- private repositories
- signed artifacts
- model provenance
- version pinning
- sandboxed execution
- container isolation
- secrets management
- deployment permissions
- audit logs

Never allow arbitrary uploaded code to execute directly on production infrastructure.

---

# 24. Database design

Create a normalized but extensible schema for at least:

```text
users
organizations
repositories
repository_versions
artifacts
models
datasets
robots
robot_adapters
skills
environments
benchmarks
benchmark_runs
evaluations
deployments
jobs
comments
issues
stars
forks
downloads
licenses
compatibility_rules
```

Use relationships rather than duplicating data.

Design for large files separately from metadata.

---

# 25. UX / design

Skynet should look like a serious developer platform rather than a generic SaaS dashboard.

Take inspiration from:

- Hugging Face
- GitHub
- Docker Hub
- Kaggle
- modern developer tools

But create an original BotMarket/Skynet visual identity.

Prioritize:

- information density
- technical credibility
- excellent search
- repository pages
- clear versioning
- code/data previews
- benchmark visualization
- robot visualization
- clean developer workflows

Do not make the interface unnecessarily flashy.

---

# 26. BotMarket ecosystem integration

Skynet must be integrated into the existing BotMarket navigation.

The ecosystem should conceptually become:

```text
BotMarket
BotNews
BotGuides
BotPapers
Robot Database
BotEdu
Skynet
```

Cross-link the products intelligently.

For example:

```text
Robot Database
     ↓
Unitree G1
     ↓
"AI Models"
     ↓
Skynet
```

A Skynet robot page should link back to its Robot Database profile.

A BotPaper page should be able to reference:

- models
- datasets
- benchmarks
- robots

A BotGuide should be able to reference Skynet resources.

This creates a connected robotics knowledge + commerce + developer ecosystem.

---

# 27. SEO

Build Skynet with strong programmatic SEO architecture.

Indexable pages should eventually include:

```text
/skynet/models/...
/skynet/datasets/...
/skynet/skills/...
/skynet/robots/...
/skynet/environments/...
/skynet/benchmarks/...
```

Each page should have:

- unique metadata
- structured data where appropriate
- canonical URL
- Open Graph metadata
- semantic HTML
- crawlable content
- strong internal linking

Avoid generating thousands of low-value pages.

---

# 28. Architecture principles

Follow these principles throughout implementation:

### API-first

The web UI is one client of Skynet, not the entire platform.

### Version everything

Models, datasets, environments, skills, benchmarks, and deployments need explicit versions.

### Reproducibility

A benchmark result must be reproducible from exact artifact versions.

### Hardware abstraction

Never tightly couple the platform to one robot manufacturer.

### Async by default

Training, simulation, evaluation, data processing, and deployment operations should use jobs/workers.

### Object storage for large artifacts

Do not put model weights/videos/large datasets directly into PostgreSQL.

### Containerized workloads

Use containers for reproducibility and isolation.

### Extensible architecture

A third-party developer should eventually be able to add:

```text
new robot
new simulator
new model
new dataset format
new benchmark
new hardware
```

without modifying the entire platform.

---

# 29. MVP vs future architecture

You should distinguish between:

### Build now

- Skynet navigation
- Model repositories
- Dataset repositories
- Robot registry
- Skills
- Basic versioning
- Search
- Profiles
- Organizations
- Stars/forks
- Robot compatibility
- API
- CLI foundations
- Basic evaluation framework
- Simulation abstraction
- BotMarket integration

### Architect now, implement progressively

- GPU training
- large-scale simulation
- browser Spaces
- real robot deployment
- teleoperation
- fleet management
- real-world data collection
- marketplace
- inference infrastructure

Do not create fake implementations of advanced robotics infrastructure simply to claim that the feature exists.

Where a real simulator/GPU cluster/robot is unavailable, build the correct abstraction, job system, APIs, UI, and interfaces so the actual infrastructure can be plugged in later.

---

# 30. Quality requirements

The finished product must feel like a **real developer platform**, not a mockup.

Avoid:

- placeholder dashboards
- fake statistics presented as real
- fake benchmark results
- fake robot execution
- fake training results
- unnecessary dummy data
- hardcoded compatibility
- monolithic code
- tightly coupled components

Use realistic seeded/demo data only where necessary, and clearly separate demo data from real user-generated data.

Every major feature should have:

- loading states
- empty states
- error handling
- permissions
- validation
- responsive UI
- API support where appropriate

---

# 31. Execution process

Work autonomously.

### Phase 1 — Discovery

Inspect the existing BotMarket codebase and infrastructure.

### Phase 2 — Architecture

Produce a concise technical architecture document covering:

- stack
- database
- storage
- API
- authentication
- jobs
- containers
- simulation architecture
- deployment
- search
- frontend architecture
- integration with BotMarket

Then proceed with implementation.

### Phase 3 — Foundation

Implement:

- database schema
- authentication integration
- repositories
- artifact storage
- versioning
- API
- permissions
- core UI

### Phase 4 — Core product

Implement:

- Models
- Datasets
- Robots
- Skills
- Environments
- Benchmarks
- Compatibility
- Search

### Phase 5 — Developer experience

Implement:

- repository pages
- CLI
- Python SDK foundations
- API documentation
- organizations
- collaboration

### Phase 6 — Simulation/evaluation

Implement the simulation/job abstraction and at least one real simulator integration if the existing infrastructure permits it.

### Phase 7 — Advanced infrastructure

Prepare the architecture for:

- training
- deployment
- teleoperation
- robot data collection
- fleet management

### Phase 8 — Production evidence gate

Before expanding the public surface, prove the deployed Hub path end to end:

- migrations, RLS, private artifact storage, signed upload/download, scanning, and reconciliation
- public/private repository authorization for session, API-token, worker, and device credentials
- immutable draft → upload → scan → publish → authorized download → digest/signature verification
- lease expiry, retry/idempotency, audit logs, monitoring, backup/restore, and incident alerts

Do not call a repository, artifact, worker, or security feature operational only because its schema,
route, or UI exists. Require deploy-path integration evidence.

### Phase 9 — Repository and dataset completion

Complete the workflows expected from a serious repository hub while keeping immutable artifact
releases as the physical-AI source of truth:

- Git interoperability or an external-Git mirror for source code, with commit/branch/tag provenance
- protected release channels, review approvals, required checks, release policy, and source-to-release traceability
- file/manifest diffs and line-level source review where the artifact is text/code
- scalable dataset preview, sample/trajectory/video viewer, split/shard manifest, range/stream access,
  quality reports, consent/license lineage, retention, deletion, and reproducibility links
- CLI, SDK, and API contract tests covering repository, artifact, release, pull-request, evaluation,
  device, and deployment workflows

If native Git clone/push/pull is required, implement it in a dedicated Git service rather than a
Next.js route handler.

### Phase 10 — Real worker, simulation, and evaluation execution

Convert the durable job control plane into real, reproducible execution:

- signed reference worker images, ephemeral sandboxed workspaces, least-privilege leases, resource
  limits, cancellation, structured logs, artifact/result upload, and validated completion
- one real first simulator integration and deterministic replay fixtures
- simulator/runtime/hardware attestation, result validation, leaderboard eligibility, and quarantine
  for unverifiable runs
- worker health, capacity, queue depth, cost, and stale-lease monitoring

A non-seeded evaluation must run from immutable inputs on a registered worker and survive retry
without double-writing before it is represented as a real result.

### Phase 11 — Managed compute and interactive Spaces

Build on the verified worker path instead of creating separate execution systems:

- GPU training with reviewed recipes, quotas, hardware attestation, checkpoints, resume, metrics,
  cancellation, retention, and verifiable output-version publication
- version-pinned inference endpoints with supported-runtime declarations, authentication, metering,
  resource limits, audit boundaries, and teardown
- browser Spaces as isolated short-lived worker sessions with declared image/policy/environment
  versions, session authorization, WebSocket transport, resource caps, queue visibility, and no
  actuator credentials

Never execute untrusted uploaded code in the web tier.

### Phase 12 — Device agent, fleet, deployment, and teleoperation

Start in a simulator, then use a controlled physical test device:

- device enrollment, rotating credentials, hardware/image attestation, offline-safe local state,
  buffered telemetry, and explicit capability declarations
- desired-state reconciliation, signed package install, canary rollout, readiness/health acknowledgement,
  pause, rollback, drift detection, and auditable deployment events
- teleoperation and episode ingestion with time synchronization, calibration metadata, operator
  intervention identity, consent/retention, encrypted upload, integrity verification, and dataset finalization
- fleet inventory, rollout, health, logs, black-box artifacts, and incident workflows

The platform must never treat an optimistic API write as a successful robot deployment.

### Phase 13 — Skynet OS, RHAL, and hardware safety

Implement the runtime specified in `docs/skynet-os-architecture.md` as hardware-backed software,
not as Hub-side TypeScript alone:

- shared Rust contracts for embodiment, transport, package admission, and safety, compiled for Hub/WASM
  and device use with differential tests against the TypeScript reference
- allocation-free C/Rust Safety Kernel and an RHAL simulator driver, then one real driver under bench supervision
- `skynetd`, black-box recording, calibration expiry, signed A/B OTA, measured boot/secure-element identity,
  conformance suites, and an independent safety-MCU path for the Certified tier
- clear Guest/Host/Certified enforcement labels; Guest observation never becomes a safety claim

No Certified-tier or real-safety claim is allowed until hardware tests demonstrate independent stop,
watchdog/envelope protection, package provenance enforcement, rollback, and Hub/device plan parity.

### Phase 14 — Governance and commercial readiness

Only after the verified execution and fleet paths exist:

- workload accounting, tenant quotas, artifact/data retention, privacy/export/deletion, takedown,
  abuse reporting, incident response, and published service-level objectives
- marketplace entitlements for paid artifacts, compute, inference, and enterprise fleet access that
  cannot bypass repository authorization, provenance, worker isolation, device grants, or safety gates
- clear evidence levels on compatibility, benchmark, deployment, and conformance badges

---

# 32. Most important product principle

Do not think of Skynet as:

> "a website where people upload robot models."

Think of it as:

> **the software infrastructure and community layer connecting AI models to physical robots.**

The ultimate workflow should become:

```text
DISCOVER
   ↓
Model / Dataset / Skill
   ↓
SELECT ROBOT
   ↓
CHECK COMPATIBILITY
   ↓
SIMULATE
   ↓
EVALUATE
   ↓
TRAIN / FINE-TUNE
   ↓
DEPLOY
   ↓
COLLECT REAL-WORLD DATA
   ↓
IMPROVE MODEL
   ↓
PUBLISH NEW VERSION
```

Build the architecture around this lifecycle.

Your job is not merely to reproduce Hugging Face's UI.

Your job is to determine what **Hugging Face would look like if the artifact being distributed were a robot brain rather than an LLM**—and then build that product as **Skynet within the existing BotMarket ecosystem**.

Start by inspecting the existing codebase and producing the architecture decision. Then implement the platform end-to-end, prioritizing working core functionality over superficial breadth.
---

# 33. Progress log

> No log existed before 2026-08-28; this section was reconstructed by auditing the working tree
> against the plan above, and is maintained from here on. Architecture decisions live in
> `docs/skynet-architecture.md` — this section tracks *build state* only.

## Audit — 2026-08-28

Verified against the working tree, not from memory:

* `npx tsc --noEmit` — clean for all Skynet code (the only 2 errors are pre-existing
  Stripe API-version mismatches in `lib/stripe.ts` / `lib/tax/stripe-tax.ts`, unrelated).
* `npx vitest run lib/__tests__/skynet-*.test.ts` — **66/66 passing** across compatibility,
  reproducibility and semver suites.
* 649 `skynet_*` keys in `messages/en.json`.
* Migration `supabase/migrations/20260828120000_skynet_platform.sql` — 1078 lines, 35 tables.

### Done

| Plan § | Area | State |
|---|---|---|
| 2, 3, 31·P1–P2 | Discovery + architecture decision | `docs/skynet-architecture.md` (208 lines) |
| 24, 31·P3 | Database schema | 35 `skynet_*` tables + enums, RLS, triggers, FTS |
| 28 | Repository as unifying object | `lib/skynet/repos.ts`, `versions.ts`, `manifests.ts` |
| 5, 6 | Model + dataset hubs | repo kinds `model`/`dataset`, typed manifests, versions |
| 9 | Skills | repo kind `skill` with cross-object links |
| 10 | Simulation abstraction | `lib/skynet/simulation/registry.ts`, `simulators` table |
| 12 | Evaluations + benchmarks + leaderboards | `lib/skynet/evaluations.ts`, leaderboard API + UI |
| 7 | Robot registry bound to `botindex` | `lib/skynet/robots/registry.ts` — references, never forks |
| 8 | Standard Robot Interface + adapters | `lib/skynet/robots/interface.ts`, capability tokens |
| 13 | Compatibility engine | `compatibility/{engine,predicate,builtin-rules}.ts`, rules as data |
| 21 | Search | Postgres FTS behind a `SearchDriver`, `skynet_search_documents` |
| 23 | Auth / tokens / audit | `auth.ts`, `tokens.ts`, `permissions.ts`; 4 credential classes |
| 3·async, 14 | Job queue + workers | `jobs.ts`, `workers/in-process.ts`, `/v1/worker/*`, cron every 5 min |
| 28 | Object storage | content-addressed `ArtifactStore`, signed upload/download |
| 18 | REST API | 31 route files under `app/api/skynet/v1/**` |
| 18 | CLI | `cli/skynet/` — login, clone, push, pull, run, evaluate, deploy |
| 18 | Python SDK | `sdk/python/skynet/` — client + resources |
| 4, 25 | Skynet UI | 26 pages under `app/[locale]/skynet/`, own visual identity |
| 26 | BotMarket integration | Navbar pill + mobile links, Robot Database cross-link |
| 27 | SEO | `lib/skynet/seo.ts`, JSON-LD, canonicals, `sitemap-skynet.xml`, noindex rules |

### Gaps found (this session's work)

1. **Broken link** — `app/[locale]/skynet/robots/page.tsx:107` links to `/skynet/robots/new`,
   which has no page. It falls through to `robots/[slug]` and 404s. `POST /v1/robots` exists,
   so only the UI is missing.
2. **Discussions (§19)** — `skynet_discussions` + `skynet_discussion_comments` are in the schema
   but have no lib module, no API routes and no UI.
3. **Follows (§19, §22)** — `skynet_follows` is in the schema and nothing reads or writes it.
4. **Devices + episodes (§16, §17)** — tables exist and `resolveDevice()` is implemented in
   `auth.ts`, but no route calls it: the robot-agent ingestion path is unreachable.

## Session — 2026-08-28

Closed all four gaps above. Everything below was verified, not assumed.

### 1. Discussions and issues (§19)

One table pair, two labels: an "issue" on a robot-policy platform is usually "this fails on my
G1", so `kind` separates them and numbering, permissions and moderation stay in one place.

* `lib/skynet/discussions.ts` — list/get/create/comment/close, per-repo `#N` numbering. The
  allocation reads `max(number)+1` and lets the unique index on `(repo_id, number)` arbitrate,
  retrying on `23505` rather than serialising every insert.
* API: `GET|POST …/discussions`, `GET|PATCH …/discussions/{n}`, `POST …/discussions/{n}/comments`.
  Anyone who can read a repository may open a thread; closing and reopening need write access,
  or authorship of the thread.
* UI: a `discussions` tab on all five repo kinds via `RepositoryView`, server-rendered at
  `?tab=discussions&thread=N` so threads are crawlable and readable without JavaScript.
  `DiscussionComposer` is the only client part.
* The tab is deliberately **not** gated on a published version — a repo with nothing published
  is exactly when people ask questions.

### 2. Follows (§19, §22)

* `lib/skynet/follows.ts` + `GET|POST /namespaces/{handle}/follow`.
* `FollowButton` resolves its own state after mount. The profile page is cached for every viewer
  (`revalidate = 300`), so a server-rendered guess would show "Follow" to someone who already
  follows — a small lie the UI now cannot tell. Self-follow shows the count with no control.

### 3. Devices and episodes (§16, §17)

The real-world data collection path, previously schema-only with an unreachable `resolveDevice()`.

* `lib/skynet/devices.ts` — device registration and token minting, status transitions, episode
  ingestion, and `episodeStats()` counted from real rows (zeros and an empty state when nothing
  has been collected — no invented collection statistics).
* API: `GET|POST /devices`, `GET|PATCH|DELETE /devices/{id}`, `GET|POST /episodes`.
* `POST /episodes` is the **only** route authenticated by a device credential. Its namespace
  comes from the device record, never the request body, so a token recovered from a robot in the
  field cannot redirect uploads, read repositories or claim jobs. Revocation is one-way.
* UI: `/skynet/settings/devices`, alongside API tokens — token shown once, hash stored.
* Validation was extracted as a pure `validateEpisodeInput()`: an agent in the field is the
  least controlled writer on the platform, and a malformed row becomes a corrupt training set
  months later.

### 4. Robot registration page (the broken link)

* `/skynet/robots/new` now exists; `robots/page.tsx:107` no longer 404s through `[slug]`.
* The Robot Database lookup is the point of the form — linking an existing `botindex` record
  keeps one robot profile across BotMarket instead of two (§7, §26). It prefills from the match.
* Capabilities are picked from the known token set, because the compatibility engine matches
  those exact strings and free text would silently never match.
* Added to the `next.config.ts` noindex rules with the other authoring surfaces.

### Also

* CLI: `skynet discussions`, `skynet discuss`, `skynet episode` (+ usage and README).
* Python SDK: `discussions()` / `discussion()` / `discuss()` / `reply()` on every repository
  object, and a new `Agent` class for the device-token ingestion path.
* Docs page: new endpoints in the API table, a Robot agent section, a devices link.
* `docs/skynet-architecture.md` §6 updated with the new API surface.
* 124 new `en.json` keys + `i18n-catalog.mjs sync` across all 35 locales.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean except the 2 pre-existing Stripe API-version errors |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **84 passed** (66 before + 18 new) |
| `npx next build` | Succeeds; all 9 new routes registered |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 4529 source keys |
| CLI `--help` and `node --check` | OK |
| Pages rendered in browser | `/skynet/robots/new`, `/skynet/settings/devices`, `/skynet/docs` |
| API responses | 401/404 with correct translated envelopes; bogus `skd_` token rejected |

New tests are in `lib/__tests__/skynet-collaboration.test.ts`: discussion numbering, self-follow,
and the episode input rules (unknown source/modality rejected, future timestamps rejected, clock
skew tolerated, task trimmed and capped).

---

## ⛔ Blocker — the migration has never been applied

`supabase/migrations/20260828120000_skynet_platform.sql` has **not** been run against the
BotMarket database. Confirmed by loading `/en/skynet/` on the dev server:

```
Could not list repositories: Could not find the table 'public.skynet_repos' in the schema cache
```

Every DB-backed Skynet page and endpoint 404s or 409s until it is applied. Pages that do not read
at render time (`/skynet/robots/new`, `/skynet/settings/devices`, `/skynet/docs`) render correctly.

This was not applied in this session: it is a 1078-line migration against a production database,
and the Supabase MCP connection in this environment points at an unrelated project
(`OpenFlow` / `sqkebwrypllwjdkrioqd`), while the app targets `efyazkprfdpligccotwc`. **Needs the
owner to run it.** After it lands, `scripts/skynet-seed.mjs` seeds licenses, simulators,
compatibility rules and the robot registry.

### Next up

1. Apply the migration, then seed, then re-verify the Skynet pages end to end.
2. `node scripts/translate-with-openai.mjs` — the 124 new keys are currently English in all 35
   locales. Sync fills English rather than blanks, so the audit reads OK; the translate script
   picks them up as "same-as-English". Not run here because it spends OpenAI budget.
3. Still architected-but-unimplemented by design (§29), awaiting real infrastructure: GPU
   training execution, large-scale simulation, browser Spaces rendering, physical deployment
   rollout, teleoperation capture, fleet management.

---

## Session — 2026-08-28 (later): migration applied, UI hidden, advisors cleared

### Migration applied to production

The blocker above is resolved. `efyazkprfdpligccotwc` (BotMarket) was connected and the schema
applied in four verified chunks, then seeded:

| Check | Result |
|---|---|
| Tables | 35, RLS enabled on all 35 |
| Policies / functions / triggers / indexes | 35 / 11 / 11 / 96 |
| Reference data | 10 licenses, 5 simulators |
| Seed (`scripts/skynet-seed.mjs --apply`) | 18 compatibility rules, 4 robots + adapters |

The seed skipped `franka-research-3` and `universal-robots-ur5e` because no Robot Database entry
matches them — reported rather than invented. No models, datasets, skills or evaluation results
were seeded; those must come from real uploads and real runs.

### Skynet is now in SHADOW MODE

Gated on one flag, `SKYNET_PUBLIC` in `lib/skynet/constants.ts` (currently `false`):

* **Navbar** — desktop pill and both mobile links hidden.
* **Sitemap** — `sitemap-skynet.xml` dropped from `sitemap-index.xml`. The route still builds and
  serves so it can be checked before launch; nothing advertises it.
* **Robot Database cross-link** — `RobotDatabaseSkynetLink` returns null. That page *is* indexed,
  and a link from an indexed page is exactly how a crawler would find the hidden product.
* **Metadata** — `buildSkynetMetadata()` forces `noindex` on every Skynet page, so the page and
  the header agree instead of contradicting each other.
* **Headers** — blanket `noindex, nofollow` on `/:locale/skynet/:path*`.

Everything still works by direct URL: pages 200, API 200, CLI and SDK unaffected.

**To launch:** set `SKYNET_PUBLIC = true` *and* delete the blanket Skynet noindex block at the end
of the `headers()` array in `next.config.ts` (header rules are static config and cannot read the
flag).

#### Pre-existing bug found while doing this

**Every `X-Robots-Tag` rule in `next.config.ts` is dead.** Next.js applies matching header rules in
order and a later rule replaces an earlier one with the same key — and the site-wide `/(.*)` rule
sets `X-Robots-Tag: max-image-preview:large` *after* all of them. Verified: `/en/login/` and
`/en/dashboard/` serve no `noindex` today, contrary to SEO V3 §6.9 and the project's noindex rule.
The Skynet block was moved after the catch-all to work around it, and carries a comment saying so.
**The dashboard, settings, login, signup, cart and checkout rules still need the same fix** — left
alone because they are outside this task's scope. Verified after the move:

```
/en/skynet/            X-Robots-Tag: noindex, nofollow, max-image-preview:large
/en/humanoid-robots/   X-Robots-Tag: max-image-preview:large        (control, still indexable)
```

### Supabase advisors: all warnings cleared

Security went from ~25 lints to **1**, performance warnings from **3 to 0**.

| Lint | Level | Action |
|---|---|---|
| `anon`/`authenticated_security_definer_function_executable` × 6 fns | WARN | **Fixed** — `EXECUTE` revoked from `PUBLIC`/`anon`/`authenticated`, granted to `service_role`. `anon` could otherwise call `skynet_bump_repo_counter` to rewrite any repo's star/fork/download counts, and `skynet_lease_jobs` to lease jobs and read full payloads. Every caller uses the service role, so nothing broke. |
| same, for `skynet_is_member` / `skynet_can_read_repo` | WARN | **Fixed** — moved to a new `skynet_private` schema PostgREST does not expose, and all 17 policies repointed. Revoking was *not* an option: a policy expression runs with the caller's privileges, so revoking EXECUTE makes every anon read fail. Both facts were verified empirically against the database in rolled-back transactions before changing anything. |
| `function_search_path_mutable` × 3 | WARN | **Fixed** — `SET search_path = public` pinned. |
| `auth_rls_initplan` × 3 | WARN | **Fixed** — `auth.uid()` → `(SELECT auth.uid())` so it is an InitPlan evaluated once per query instead of once per row. |
| `rls_enabled_no_policy` × 6 | INFO | **Fixed** — explicit `RESTRICTIVE ... USING (false)` policies on the credential/artifact/audit tables. Behaviour is identical; the point is that deliberate deny no longer looks like a forgotten policy. |
| `auth_leaked_password_protection` | WARN | **Not fixable from here** — a project Auth setting, not schema. No auth-config tool exists in this MCP server. Enable it in Dashboard → Authentication → Policies. |

RLS was re-verified after every change with rolled-back probes: a public repo is visible to `anon`,
a private one is not, robots/licenses/simulators read fine, and the credential tables return zero.

Applied as migrations `skynet_platform_05..08`; repo files added so the source matches the database:
`20260828130115_skynet_private_rls_helpers.sql`, `20260828130130_skynet_explicit_deny_policies.sql`,
`20260828130245_skynet_rls_initplan.sql` (05 is appended to the base file).

#### Deliberately not changed (INFO only, 255 lints)

* `unused_index` × 190 (30 Skynet) — every index is unused on a schema with no traffic yet.
* `unindexed_foreign_keys` × 64 (60 Skynet) — adding 60 speculative indexes to empty tables would
  cost write throughput and immediately show up as 60 more unused indexes. Worth revisiting under
  real query patterns, not before.
* `auth_db_connections_absolute` — Auth server pinned to 10 connections; a project setting.

### Verification

`npm run typecheck` clean (bar the 2 pre-existing Stripe errors) · 84/84 tests · `next build`
succeeds (16,308 static pages) · APIs return live seeded data (4 robots, 10 licenses, 5 simulators,
FTS search ranking) · no server errors in the dev log.

---

## Session — 2026-08-28 (later still): Spaces/deployments/orgs logged, then Training built

### First: the previous session's work, which was never logged

Found in the working tree, unlogged, and verified before writing anything new — `npm run typecheck`
clean, 104/104 tests, all routes present:

* **Spaces (§11)** — `lib/skynet/spaces.ts`, `GET|POST /v1/spaces`, `GET /v1/spaces/{ns}/{slug}`,
  `/skynet/spaces`, `/skynet/spaces/new`, `/skynet/spaces/{ns}/{slug}`, `SpaceRunner`,
  `NewSpaceForm`. A Space pins policy + environment + robot + simulator to published versions, so
  a run from the web and a run from the CLI are the same run.
* **Deployments (§15)** — `lib/skynet/deployments.ts`, `GET|POST /v1/deployments`,
  `GET|PATCH /v1/deployments/{id}`, `/rollback`, `/checkin`, `/skynet/deployments[/{id}]`,
  `DeploymentsClient`, `DeploymentDetailClient`. Created `pending`; only a runtime check-in makes
  it `active`; a stopped deployment stays stopped.
* **Organisations (§19)** — `/skynet/orgs` index, `POST /v1/namespaces` (connect an existing
  BotMarket org to a handle), `GET|POST /v1/namespaces/{handle}/members`, `MembersPanel`,
  `ConnectOrgPanel`.
* **Discussions index** — `/skynet/discussions` across all repositories.
* 20 tests in `lib/__tests__/skynet-deployments.test.ts`.

### 1. Training, end to end (§14, §29 "Run → Training")

The largest remaining hole: `POST /v1/training` existed and worked, and nothing else did — no
list, no detail, no cancel, no UI, no CLI, no SDK, and the endpoint was one line in the docs
table. Following an expensive GPU run was impossible without querying Postgres by hand.

* **`lib/skynet/training.ts`** — the module the route should always have had. `TRAINING_METHODS`
  and `METHOD_REQUIREMENTS` (which inputs each method genuinely needs), `GPU_CATALOG`,
  `validateHardware`, `validateHyperparameters`, `validateRunName`, `listTrainingRuns`,
  `requireTrainingRun`, `cancelTrainingRun`, `completeTrainingRunFromWorker`, `failTrainingRun`,
  `effectiveTrainingStatus`, `awaitingWorker`, `splitMetrics`, `normalizeCheckpoints`.
* **`METHOD_REQUIREMENTS` replaced the ad-hoc conditionals in the route.** The old rule was "a
  dataset unless the method is RL" — which let `vla_finetune` and `lora` be queued with no base
  model to fine-tune. The table is now one source of truth, enforced by the API and passed down to
  the form, so the browser cannot disagree with the server about what a run needs.
* **`GPU_CATALOG` exists to make a check honest, not to decorate a dropdown.** Each entry carries
  real VRAM, which is fed to the compatibility engine as `hardware.vram_gb` before the run is
  queued — a base model declaring `min_vram_gb: 140` is refused on a 4090 by the same rules the
  rest of the platform uses. A GPU that is not in the catalog is still accepted (real clusters run
  things we have not heard of) and its VRAM is left null, so the memory rule does not fire on a
  guess rather than passing on one.
* **API** — `GET /v1/training` now lists across every namespace the caller belongs to (was: the
  personal namespace only, so an organisation's maintainers could not see its runs).
  `GET|PATCH /v1/training/{id}` are new: the run, its job, its worker logs, its live queue
  position; and cancellation, which is the **only** status a person may set. A non-member gets
  404, not 403 — what an organisation is training, on whose data, is not public.
* **Worker completion moved into the module.** The route was writing `skynet_training_runs`
  inline. It now calls `completeTrainingRunFromWorker`, which additionally verifies that a
  worker-supplied `output_version_id` belongs to the run's declared output repository. Holding a
  job lease is not authority to point a training run at an arbitrary version row.
* **UI** — `/skynet/training` (queue health from real worker registrations, a form, the run list)
  and `/skynet/training/{id}` (provenance, metrics, checkpoints, logs, cancel). Log lines are
  fetched incrementally by `seq`; polling stops the moment the run is terminal.
* **No invented progress anywhere.** `progress` is whatever the worker last heartbeated. With no
  GPU worker online the pages say the run is recorded and queued and nothing is executing it —
  which is the state observed against the production database while verifying: `getQueueHealth()`
  reports 0 active GPU workers, so every page rendered the "no workers connected" branch.
* **CLI** — `skynet train`, `skynet training [<id>]`, `skynet cancel <id>`, with usage and README.
* **Python SDK** — `train()`, `training_runs()`, and a `TrainingRun` class with `status`,
  `metrics`, `checkpoints`, `awaiting_worker`, `logs()`, `cancel()` and a `wait()` that raises
  `TimeoutError` rather than returning a half-finished run.
* Subnav entry, docs table (4 endpoints, was 1), a `train` block in the CLI example, and the
  overview page's lifecycle strip now points "train" at `/skynet/training` and "deploy" at
  `/skynet/deployments` instead of both landing on `/skynet/docs`.

### 2. Collected episodes surfaced on the dataset they belong to (§16)

`episodeStats()` had been written and was called by nothing, so `POST /episodes` wrote rows that
no page ever showed. Ingested data that cannot be seen is not "usable for dataset creation".

* A **Collection** tab on dataset repositories: episode count, outcomes, recorded time,
  interventions, sources (autonomous vs each teleoperation input), modality coverage, and the
  robots the data came from — every figure counted from real rows, zeros and an explicit empty
  state when nothing has been collected.
* The tab is **not** gated on a published version. Episodes arrive from robots long before a
  version is cut, and watching them arrive is the point.
* `episodeStats()` was rewritten as an exact `count` plus a bounded 5,000-row scan, with
  `summarizeEpisodes()` extracted as a pure function. The old version pulled every episode row for
  the dataset to add up four numbers. When the scan is capped the page says so instead of passing
  a partial scan off as the whole set.
* Aggregates only — no individual episodes on the public page. The counts describe the dataset;
  the task text and device metadata describe the operator, and members can read those through the
  authenticated `GET /episodes`.

### 3. Following now does something (§19, §22)

`listFollowedNamespaces()` existed, its comment said "follows drive the personalised activity
feed", and no feed consumed it: following someone incremented a counter and changed nothing.

* `GET /api/skynet/v1/activity` — public feed, `?namespace={handle}`, or `?following=1` for the
  caller's follows. Only `visibility = 'public'` rows are ever returned, so a personalised feed
  cannot become a side channel for private-repository activity.
* `/skynet/activity` now leads with a **From namespaces you follow** section above the global
  feed. It renders in the browser because it differs per viewer; the global feed below it stays
  server-rendered and crawlable.
* "You follow nobody" and "the people you follow have been quiet" are separate empty states —
  the fix for each is different, and one message would send half the readers the wrong way.

### Shadow mode: unchanged, and extended to the new surfaces

`SKYNET_PUBLIC` is still `false`. Navbar hidden, `sitemap-skynet.xml` still out of
`sitemap-index.xml`, Robot Database cross-link still null, `buildSkynetMetadata()` still forces
`noindex` on every Skynet page, and the blanket `/:locale/skynet/:path*` header block is
untouched. Added `/:locale/skynet/training/:path*` → `noindex, nofollow` next to the deployments
rule, so the per-surface rules stay correct for the day the blanket block is deleted.

**`robots.txt` was deliberately left alone.** A `Disallow: /skynet` would announce the product to
everyone who reads robots.txt — it is public, diffed and monitored — and worse, a disallowed URL
cannot be crawled, so the `noindex` header would never be read and a discovered link could still
be indexed URL-only. Nothing linking to it, plus a header that is actually fetched, is the
stronger shadow.

### Shadow-mode hole found while verifying (pre-existing, not fixed here)

**The entire i18n catalog ships inside every public page, so Skynet's product copy is in the
source of `/en/` right now.** Measured on the production build: 1,027 `skynet_*` occurrences in
the 400 KB homepage payload, including `skynet_home_meta_title` ("Skynet — Open Platform for
Physical AI Models") and every nav label.

Cause: `app/[locale]/layout.tsx` passes the whole merged catalog to `NextIntlClientProvider`, and
`i18n/request.ts` merges all 4,783 keys. This is site-wide and predates Skynet — it is also ~330 KB
of JSON on every page for every locale, so it costs more than secrecy.

It does **not** break the indexing side of shadow mode: no page or link to `/skynet/*` is created,
so Search Console still has nothing to crawl, and the `noindex, nofollow` header stands. What it
loses is discretion — a person reading source, or an LLM crawler ingesting the RSC payload, sees
the product exists.

Not fixed here, deliberately. The only routes to a fix are (a) scoping messages per route across
the whole site, or (b) making the root layout read the pathname from a header, which would make it
dynamic and forfeit static generation of all 16,308 pages. Both are site-wide changes well outside
"finish Skynet", and (b) is a straight regression. Worth doing as its own task — the payload win
alone justifies it — but it needs to be a decision, not a side effect.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean — the 2 Stripe API-version errors noted in earlier sessions are gone too |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **134 passed** (104 before + 24 training + 6 episode-summary) |
| `npx next build` | Succeeds; the new routes register |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 4,783 source keys |
| CLI | `--help` lists the Train block; `node --check` clean |
| Python SDK | `ast.parse` clean on the changed modules |

New tests: `lib/__tests__/skynet-training.test.ts` (method requirements, hardware bounds, catalog
VRAM beating a client-supplied figure, hyperparameter shape, and what the platform may claim about
a run's state before a worker has spoken) and a `summarizeEpisodes` block in
`skynet-collaboration.test.ts`.

### Audit on request — what the plan asks for that is not built

An earlier version of this log said §29's "Build now" list was complete. That is true at the level
of the list, and it skipped over enumerated sub-items inside §6, §22, §23 and §26. Audited
against the tree; this is the accurate account.

**Deferred by the plan itself (§29 second list, §30 "do not create fake implementations").** Each
has schema, job kind, API, permissions and UI, and reports that it is waiting on infrastructure:
GPU training execution, large-scale simulation, browser Spaces *rendering*, physical deployment
rollout, teleoperation capture, fleet management, inference infrastructure, marketplace (§20).

**Deferred by choice, with reasons, in earlier sessions:** Projects (§4 IA — folded into
organisations), Teams (§19 — needs a table; no migration can be applied from this environment).

**Enumerated in the plan, simply not built, and not previously flagged:**

| Plan | Item | State |
|---|---|---|
| §6 | **Dataset viewer** | Not built. `DatasetManifest.schema` carries a comment saying it exists "so a viewer can render the data without downloading it" — and nothing renders it. |
| §6 | **Schema table, train/test splits, per-field statistics** | Declared in `DatasetManifest` (`schema`, `splits`, `statistics`), validated on publish, returned by the API and the SDK. `ManifestPanel` renders none of the three. |
| §6 | **Contributors** | No concept of a dataset contributor anywhere. |
| §22 | **Popular Robots** | `listRobots()` supports `sort: name \| recent \| stars \| manufacturer`; `/skynet/robots` hardcodes `manufacturer` and exposes no control, so the other three are unreachable. |
| §22 | **Recently Updated** rail | Reachable as `?sort=recent` on each explore page; no named rail on the overview, which shows 3 of the 6 the plan lists. |
| §23 | **Signed artifacts** | Not built, and worth stating precisely: what exists is content-addressed SHA-256 digests (the bytes did not change) and signed *URLs* (access control). Neither is a publisher signature — nothing attests *who* produced an artifact. On a platform whose artifacts end up driving hardware, that is the gap that matters most of the three. |
| §23 | **Secrets management** | Nothing stores a credential a deployment or worker needs (robot API keys, private registry auth). Only platform-issued tokens exist. |
| §26 | **BotPaper → models/datasets/benchmarks/robots** | Not built. `app/[locale]/papers` and `lib/papers` contain no reference to Skynet. |
| §26 | **BotGuide → Skynet resources** | Not built. Same for `app/[locale]/botguides` and `lib/botguides`. |

The Robot Database ↔ Skynet bridge (§26's worked example) *is* built, and correctly renders
nothing while `SKYNET_PUBLIC` is false.

None of the above is blocked. The dataset viewer and the two cross-link surfaces are ordinary
work against data that already exists; artifact signing and secrets management are design
decisions that should be made deliberately rather than improvised.

### Next up

1. `node scripts/translate-with-openai.mjs` — 111 new English keys are sitting in all 35 catalogs
   as English. Not run here: it spends OpenAI budget.
2. Register a GPU worker against `POST /v1/worker/register` and run one real training job end to
   end. Every layer above it is now built and untested against a real worker.
3. Launch checklist when Skynet goes public: set `SKYNET_PUBLIC = true` **and** delete the blanket
   Skynet noindex block at the end of the `headers()` array in `next.config.ts`.

---

## Session — 2026-08-28 (fourth): the audited gaps, closed

Working the list from the audit above. Cross-stack items (BotPapers → Skynet, BotGuides →
Skynet) were explicitly deferred by the owner for now; everything else in that table is done, and
migrations are back on the table — one SQL file, applied by the owner.

### 1. Dataset viewer (§6)

`DatasetManifest` declared `schema`, `splits` and `statistics`, with a comment on `schema` saying
it existed "so a viewer can render the data without downloading it". Nothing rendered any of them.

* A **Data** tab on dataset repositories: field schema (name, dtype, shape, unit, description),
  train/test splits with their shares, per-field min/max/mean/std, and task labels.
* Everything comes from the version manifest, which is validated on publish and covered by the
  version digest — so it describes the files *as published*, not an inspection of their contents.
  The tab says as much rather than implying the platform opened the trajectories.
* A dataset declaring none of the four gets an empty state naming what a publisher would add,
  instead of four empty tables.

### 2. Contributors (§6)

* `listContributors()` derives them from `created_by` on **published** versions, so the list
  cannot name someone who never shipped. Drafts are excluded: an abandoned draft is not a
  contribution.
* Rendered on every repository kind, not just datasets — the question is the same everywhere.

### 3. Discovery (§22)

* **Popular robots** was unreachable: `listRobots()` supported `name | recent | stars |
  manufacturer` and the page hardcoded `manufacturer` with no control. The vocabulary is now
  exported as `ROBOT_SORTS` from the registry, so the page and the lib cannot drift apart, and
  all four are selectable.
* **Recently updated** rail added to the overview, across every repository kind — "what moved
  lately" is the one question that does not care whether the answer is a model or a benchmark.
  The robots rail is now titled *Popular robots* rather than *Robots*, because it is ordered by
  stars and was quietly claiming to be the whole registry.
* **Bug found and fixed while there:** every pagination link on `/skynet/robots` emitted a bare
  `?page=2`, silently dropping the reader's type, manufacturer and search — page 2 of "Unitree
  quadrupeds" was page 2 of everything. All links now carry the whole query forward.

### 4. Release signing (§23) — new migration

The gap I had ranked highest. `digest` proves the bytes did not change; it says nothing about who
produced them, and these artifacts end up driving hardware.

* `supabase/migrations/20260828190000_skynet_signing_and_secrets.sql` — `skynet_signing_keys`,
  `skynet_version_signatures`, `skynet_secrets`. Purely additive: no existing table, policy or
  RLS helper is modified, so applying it cannot change who can read anything today.
* `lib/skynet/signing.ts` — Ed25519 only, deliberately. One curve, no hash choice, no parameter
  negotiation: a signing format with options is a signing format with footguns.
* **The platform never holds a private key.** No endpoint accepts one; `skynet keygen` writes the
  private half locally at mode 0600 and only the public half is registered. The web form refuses
  a pasted private key in the browser, before it can reach a server log.
* **An unverified signature is never stored.** The server checks that the key belongs to the
  repository's namespace, is active, and that the signature verifies against the digest it
  holds — then inserts. A row in `skynet_version_signatures` is therefore always a signature that
  checked out, which is what lets the "Signed" badge mean something.
* Keys are **revoked, never deleted** (`ON DELETE RESTRICT`). A release signed by a since-retired
  key must keep saying so; that is a different fact from being unsigned.
* Public keys and signatures are readable wherever the repository is, so a third party can verify
  a release without trusting this platform's badge — which is the entire point.
* UI: a Provenance panel stating signed *or* unsigned either way (a badge shown only when a
  signature exists would let "no badge" read as "not checked yet"), and a Signature column across
  the version history from one batched query.
* CLI: `keygen`, `key add`, `key`, `sign`, `verify`. `sign` fetches the digest rather than
  recomputing it — signing a digest we derived ourselves would prove only that our own arithmetic
  is self-consistent.
* SDK: a `Signature` class that submits but never signs. A library that accepts private keys
  becomes the place they leak from.

### 5. Secrets (§23) — same migration

* AES-256-GCM under `SKYNET_SECRETS_KEY`, so a database dump alone discloses nothing.
* **Write-only from the outside.** No route returns a stored value to a person or a browser, with
  an admin token or without. Rotating means setting it again. That is the difference between a
  secret store and a password list.
* The only reader is `POST /v1/worker/jobs/{id}/secrets`, which needs **both** a worker credential
  and the live lease token, and takes the namespace from the **job row** rather than the request —
  a worker leasing a job in one namespace cannot ask for another's credentials. POST, not GET, so
  the lease token stays out of access logs.
* **No plaintext fallback.** With no key configured, storing fails loudly and the UI disables the
  form and says why. A platform that quietly degrades to plaintext when misconfigured is worse
  than one with no secret store, because it looks like it has one.
* Names are constrained to what a shell can export, so a typo becomes an error rather than a
  variable the worker silently never sees. The audit log records the name and never the value.
* `last_read_at` is surfaced: "when was this last handed to a worker" is the first thing anyone
  asks after a suspected leak, and it is a fact the platform actually knows.

### Deploying before the SQL is applied

Verified empirically against the production database rather than assumed: a query against a
table that does not exist yet returns `{ data: null, error: PGRST205 }` — it does not throw. Both
signature readers use `data ?? []`, so **repository pages render as "unsigned" until the migration
lands and start showing signatures afterwards.** Nothing breaks in between. The keys and secrets
pages are the only surfaces that need it applied before they do anything.

### Shadow mode

Unchanged. `SKYNET_PUBLIC` still `false`, blanket noindex untouched, nothing new links in from an
indexed page. The two new settings pages sit under `/skynet/settings/*`, already covered by the
per-surface `noindex, nofollow` rule.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **156 passed** (134 + 22 new) |
| `npx next build` | Succeeds; keys/secrets pages and both API routes register |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 4,894 source keys |
| CLI keygen → sign → platform verifier | Cross-checked: a signature the CLI produced verifies against `verifyDigestSignature`, and fails against a different digest |
| Live probes | Settings pages 200; `/v1/secrets` 401; worker secrets route 401 without a worker credential; robot sort and filter combinations 200 |

`lib/__tests__/skynet-signing.test.ts` uses **real** keys and real ciphertext — a signing test
that mocks its verifier proves nothing. It covers PEM/DER equivalence, rejection of non-Ed25519
keys, signatures over a different digest, signatures from a different key, a flipped bit, wrong
signature length, GCM tamper detection, decryption under a rotated key, and the refusal to seal
anything when no key is configured.

### Still not built

* **BotPapers / BotGuides → Skynet cross-links (§26)** — deferred by the owner for now.
* **Teams (§19)** — the one item I would not do blind. Teams are only worth having if they carry
  access grants, which means changing `skynet_private.can_read_repo` — the function every private
  repository read depends on. The earlier session verified RLS changes empirically against the
  live database in rolled-back transactions; there is no Supabase connection in this session to
  do that with, and editing that function unverified is how a private repository quietly becomes
  readable. It needs a session that can probe the database, not a bigger SQL file.
* **§29's architected-but-unimplemented list**, unchanged: GPU training execution, large-scale
  simulation, browser Spaces rendering, physical deployment rollout, teleoperation capture, fleet
  management, marketplace.

### Next up

1. Apply `20260828190000_skynet_signing_and_secrets.sql`, then set `SKYNET_SECRETS_KEY` to a
   32-byte base64 value (`openssl rand -base64 32`) wherever the app runs. Without it, signing
   works and secrets refuse to store.
2. `node scripts/translate-with-openai.mjs` — 193 new English keys across the last two sessions.
3. Register a GPU worker and run one real training job end to end.

---

## Session — 2026-08-28 (fifth): signing/secrets verified live, teams built

### The signing and secrets migration is applied and verified

`20260828190000_skynet_signing_and_secrets.sql` was applied by the owner. Verified against the
live database by **observation**, not by reading the policy text back:

| Invariant | Result |
|---|---|
| `skynet_signing_keys`, `skynet_version_signatures`, `skynet_secrets` exist | yes, RLS on |
| A secret row visible to the service role | yes |
| The same row visible to `anon` | **no** — the restrictive deny holds with real rows present |
| An **active** signing key readable by `anon` | yes — third parties can verify signatures |
| A **revoked** signing key readable by `anon` | no — retired-key history stays internal |
| Repositories, robots, licences still readable by `anon` | yes, unchanged |

That last column is the point: a policy that was *created* and a policy that *behaves* are
different claims, and the second one is the one that matters.

### `scripts/skynet-verify-rls.mjs`

The probe above is now a script rather than something rewritten each time. It creates
`zz-rls-probe` fixtures, checks each invariant with both the service-role and anon clients,
removes them in a `finally` (including on failure), and exits non-zero if anything is violated,
so it can gate a deploy.

It reports a missing table as `skip`, never as a pass. "Nothing came back" is the same answer for
*denied* and *no such table*, and treating those as equivalent is how a verification script comes
to certify a schema that was never applied.

Run it after any migration that adds a table, changes a policy, or edits a `skynet_private`
helper — those helpers gate every private-repository read on the platform.

### Teams (§19)

The item I declined to build blind last session. What made it possible was not a bigger SQL file
but the ability to *check* the result, which the script above now provides.

`supabase/migrations/20260828200000_skynet_teams.sql` — `skynet_teams`, `skynet_team_members`,
`skynet_repo_team_access`, plus one edited function.

**Shape of the feature.** A team is a group of namespace members granted a role on chosen
repositories — the axis the four namespace roles cannot express ("this group writes to these
repos and reads the rest").

**The two boundaries that keep it safe:**

1. **A team grants repository access only.** Never namespace administration, deployments or
   token issue. `owner` is not a grantable team role, so a team cannot become a route to taking
   over an organisation. Tested directly: no team role satisfies `namespace.admin` or
   `deployment.admin`.
2. **Effective access is the maximum of direct role and team grant, never the minimum.** A
   maintainer who is also in a reader team is still a maintainer. If this were an overwrite,
   adding someone to a team as a convenience would silently strip their ability to publish.

**The one risky edit, and how it is contained.** Teams are only worth having if they carry
access, so `skynet_private.can_read_repo` — the function every private-repository read depends
on — gains a branch. It is shaped to be as boring as possible: purely additive (`OR
has_team_access(...)`), so it can widen but never narrow; and it requires a row in
`skynet_repo_team_access` joined to `skynet_team_members` matching the caller, both created empty,
so **behaviour is identical until somebody deliberately grants a team access to a repository.**

**Also built:** `lib/skynet/teams.ts`; `GET|POST /v1/namespaces/{handle}/teams` and
`GET|PATCH|DELETE /v1/namespaces/{handle}/teams/{slug}` (one PATCH with an `action`, because
every variant needs the same owner check and splitting them invites a route that forgets one);
`TeamsPanel` on the organisation page, rendered client-side because the org page is cached for
every viewer and team structure is members-only. Team membership is restricted to people already
in the namespace — a team is a subdivision of an organisation, not a side door into one.

`teamRoleForRepo` deliberately uses two indexed queries rather than one embedded PostgREST join:
the two tables have no foreign key between them, so relating them means a nested embed whose
filter semantics are easy to get subtly wrong, and this sits on the permission path of every
repository read.

### ⚠ The teams migration has not been applied yet — RESOLVED, see the seventh session below

`20260828200000_skynet_teams.sql` is written and the code is complete, but the SQL has not run.
Until it does:

* The teams panel renders nothing (the API returns no rows) — the org page is otherwise unaffected.
* `teamRoleForRepo` returns null on a missing table, so repository access falls back to direct
  membership exactly as before. Verified in the previous session that a missing table returns
  `PGRST205` rather than throwing.
* **The `can_read_repo` change is therefore unverified.** After applying, run
  `node scripts/skynet-verify-rls.mjs` — it already contains the team checks and currently
  reports them as `skip`. It must print "All RLS invariants hold" before this is trusted.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **167 passed** (156 + 11 team-permission) |
| `npx next build` | Succeeds; both team routes register |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 4,922 source keys |
| `node scripts/skynet-verify-rls.mjs` | All invariants hold; team checks skipped pending migration |

### Still not built

* **BotPapers / BotGuides → Skynet cross-links (§26)** — deferred by the owner.
* **§29's architected-but-unimplemented list**, unchanged: GPU training execution, large-scale
  simulation, browser Spaces rendering, physical deployment rollout, teleoperation capture, fleet
  management, marketplace.
* **Projects (§4 IA)** — still folded into organisations. `skynet_repo_links` provides the wiring
  and a Space provides the runnable composition; a third container over the same rows would be
  navigation, not capability.

### Next up

1. Apply `20260828200000_skynet_teams.sql`, then run `node scripts/skynet-verify-rls.mjs` and
   confirm it prints "All RLS invariants hold" with no skips.
2. Set `SKYNET_SECRETS_KEY` (`openssl rand -base64 32`) wherever the app runs — signing works
   without it, secrets refuse to store.
3. `node scripts/translate-with-openai.mjs` — 221 new English keys across the last three sessions.
4. Register a GPU worker and run one real training job end to end.

---

## Session — 2026-08-28 (sixth): one membership roster

### The problem

Skynet had its own `skynet_members` table. BotMarket already has `org_members` — the roster the
marketplace bills seats against, the one the org settings page edits, the one people actually
maintain. Every organisation therefore had two membership lists that could disagree, and the one
that governed Skynet access was the copy nobody was looking at.

The concrete failure: **nothing propagated a removal.** No route under `app/api/organisations/`
referenced Skynet in any way. Taking somebody out of a BotMarket organisation left their
`skynet_members` row untouched, so they kept read and write access to that organisation's private
repositories — and to the robots those repositories deploy to — after they left.

A sync job would have been the smaller change and the worse one: it fails silently, and the
failure mode is somebody keeping access they should have lost. Deriving the role removes the
class of bug instead of monitoring for it.

### After

There is one roster, and it belongs to the marketplace:

* a **user** namespace has exactly one member — the profile in `skynet_namespaces.user_id`;
* an **org** namespace's members *are* `org_members` for the organisation behind it.

| BotMarket | Skynet | What it carries |
|---|---|---|
| `owner` | `owner` | namespace administration, API tokens, deployment rollback |
| `admin` | `maintainer` | publish and yank versions, deploy to hardware |
| anything else | `writer` | create repositories, push files, open draft versions |

`writer` rather than `reader` for a plain member is a judgement call, not a default: an
organisation member is somebody the owner invited and pays a seat for, so the useful baseline is
that they can do the work. It still stops short of publishing a release or deploying to a physical
robot, both of which need `maintainer` — which is what a team grant provides without making
somebody an organisation admin. One line in each of two places if you want it stricter.

An unrecognised role maps to `writer`, never to `owner`. `org_members.role` is unconstrained text,
and the only safe direction to fail is down.

### The two enforcement layers are held to each other

The mapping exists twice, necessarily: `skynet_private.member_role` is what RLS enforces for the
browser's anon client, `ORG_ROLE_TO_SKYNET` is what the service-role API routes enforce. If they
drift, the same person gets different answers depending on which door they came through — the kind
of permission bug that reads as "works in the app, not in the API" and takes a day to find.

`lib/__tests__/skynet-org-membership.test.ts` parses the migration's `CASE` branches and asserts
they equal the TypeScript map, including the fallback. Drift becomes a failing test rather than a
support ticket.

### A hazard the probe was extended to catch

`public.org_members` currently has a **recursive RLS policy** (42P17) — reading it as `anon`
errors, and so does reading `organisations`, whose policy references it. This is pre-existing and
unrelated to Skynet, but membership now depends on that table, so it matters: `member_role` is
`SECURITY DEFINER` and bypasses RLS, but "is supposed to bypass" is exactly the sort of assumption
this work is meant to stop making. If it did not, every read of an org namespace's repositories
would fail with a policy error instead of returning a permission answer.

A probe against a *user* namespace cannot catch that — `member_role` short-circuits before
touching `org_members` when `org_id` is null. So `scripts/skynet-verify-rls.mjs` now builds an
**org-backed** namespace, puts a public and a private repository in it, and asserts the anon read
returns rows rather than an error. Both new sections skip cleanly while the migration is
unapplied, as everything else in that script does.

Worth fixing on the marketplace side independently: while that policy recurses, no browser-side
query to `organisations` or `org_members` can succeed.

### Changed

* `supabase/migrations/20260828210000_skynet_org_membership.sql` — `skynet_private.member_role`,
  `is_member` rewritten to derive from it, `skynet_members` dropped. The drop is guarded: if the
  old table ever holds a row, the migration raises instead of discarding grants. It is empty today
  (no namespace has ever been created), which is why this is a clean unification and not a data
  migration.
* `lib/skynet/namespaces.ts` — `ORG_ROLE_TO_SKYNET`, `skynetRoleForOrgRole`, `memberRoleIn`;
  `getMemberRole`, `listMembers`, `listNamespacesForUser` and `namespaceStats` rewritten;
  `upsertMember`, `removeMember`, `countOwners` and `ASSIGNABLE_ROLES` deleted.
* `app/api/skynet/v1/namespaces/{handle}/members` — GET only. A second place to edit membership is
  what allowed the drift, so POST and DELETE are gone rather than restricted.
* `components/skynet/MembersPanel.tsx` — read-only, and now a **server** component. With nothing to
  manage it no longer needs the viewer's role, so the client bundle and its round trip both go.
* `lib/skynet/db.ts` — `skynet_members` out of the table whitelist, `org_members` into `skExternal`.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **175 passed** (167 + 8) |
| `npx next build` | Succeeds |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 4,924 source keys |
| `node scripts/skynet-verify-rls.mjs` | All invariants hold; both migrations correctly reported as skipped |
| Shadow mode | `SKYNET_PUBLIC` still false; `sitemap-skynet.xml` still absent from the sitemap index |

### ⚠ Two migrations are now pending, in order — RESOLVED, see the seventh session below

1. `20260828200000_skynet_teams.sql`
2. `20260828210000_skynet_org_membership.sql`

Then `node scripts/skynet-verify-rls.mjs` must print **"All RLS invariants hold" with no skips**.
Until it does, `can_read_repo` (teams) and `member_role` (this migration) are both unverified, and
between them they gate every private-repository read on the platform.

Behaviour before they are applied is unchanged and safe: `teamRoleForRepo` and the teams panel see
a missing table and fall back to direct membership; the roster code, however, is already live
against `org_members` — so applying **210000 is what makes the running app and the database agree**.

---

## Session — 2026-08-28 (seventh): the audit log becomes readable, plus discovery and the two cross-links

### First: both pending migrations are applied, and verified

`20260828200000_skynet_teams.sql` and `20260828210000_skynet_org_membership.sql` had been applied
by the owner between sessions. Confirmed against the live database rather than assumed:
`skynet_teams` / `skynet_team_members` / `skynet_repo_team_access` exist, `skynet_members` is
gone, and `skynet_private` holds `can_read_repo`, `has_team_access`, `is_member`, `member_role`.

`node scripts/skynet-verify-rls.mjs` now prints **"All RLS invariants hold" with no skips**. The
`can_read_repo` team branch and the derived `member_role` are therefore no longer unverified —
which was the outstanding condition on every private-repository read on the platform.

---

### The gap that mattered: the audit log was write-only

`recordAudit` has been called since the platform's first migration — deployments, rollbacks,
token issue and revocation, signing keys, team membership, secrets. Nothing had ever read it.
`listAudit()` existed with no caller, no API route, no UI. §23 asks for audit logs, and an
organisation owner could not answer "who deployed this policy to that robot, and when".

That was structural, not an oversight. A row recorded `target_type` + `target_id` and nothing
else, so scoping the log to one organisation meant resolving eleven target types back to a
namespace through eleven joins — in the request path, on a security surface. That is how an
audit reader ends up showing one organisation another's rows.

**`supabase/migrations/20260828220000_skynet_audit_namespace.sql`** — `namespace_id` on
`skynet_audit_log`, plus an index on `(namespace_id, created_at DESC)`. Applied and verified in
this session. The table was empty (0 rows), so no back-fill was needed or attempted. RLS is
untouched: the restrictive deny-all stands, and the reader runs service-role behind an owner
check, exactly like secrets and tokens.

The column is **nullable on purpose**. A personal API token belongs to a user and works across
every namespace they are in; a compute worker serves the whole queue. Filing either under one
organisation would misstate its reach, and storing a wrong answer is worse than storing none.

**`AUDIT_ACTIONS` — a typed union rather than free-form strings.** Making `action` typed found
five actions that were being written and that I had not listed: `team.grant`, `team.revoke`,
`worker.register`, `device.register`, and the three `device.{status}` variants. Those had been
invisible to any filter that would have been built from a hand-written list. The compiler found
them in one pass; a reviewer would probably not have.

**Built on top of it:**

* `listNamespaceAudit()` — allow-list intersection on `action` (an unrecognised filter returns
  nothing, never everything), timestamp paging, and `ip_hash` deliberately not selected.
* `GET /api/skynet/v1/namespaces/{handle}/audit` — **owner only, not members**. Reading a record
  of what the people in an organisation did is supervisory, not collaborative; same bar as the
  namespace's secrets. Non-owners get 404, matching every other route here — a 403 on an
  unauthorised read confirms the resource exists.
* `AuditPanel` on the organisation page, client-rendered for the same reason `TeamsPanel` is:
  that page is cached for every viewer and this is owner-only content.
* `skynet audit <ns>` and `skynet teams <ns>` in the CLI; `audit()` and `teams()` in the Python
  SDK. Both read-only. Granting a team access to a repository changes what a group can push to
  hardware, and that belongs where the operator can see the team before they change it.

**Paging is by timestamp, not offset.** The log only grows at the head, so an offset silently
repeats rows as new entries arrive — during an incident, the difference between a clean walk
backwards and a duplicated one.

### The check that catches the failure mode

`recordAudit` swallows its own errors by design: an audit write must never roll back the work it
describes. The cost is that a missing column, a renamed table or a broken policy drops every
entry silently and looks exactly like a quiet week.

Two things now catch that:

* `lib/__tests__/skynet-audit.test.ts` parses every `recordAudit({…})` in the tree (balancing
  braces, so a nested `metadata: {…}` does not truncate the parse) and fails if a call whose
  action is namespace-scoped omits `namespaceId`. Verified by deliberately removing one — the
  test named the file. It also asserts every namespace action has a translated label, and that
  no secret audit row carries a `value`.
* `scripts/skynet-verify-rls.mjs` gained an **audit log** section: it writes a probe row with a
  namespace, checks the namespace reader finds it, and checks `anon` still cannot — with a real
  row present, not against an empty table.

---

### Discovery and history (§22, §12)

**Related repositories.** `listRelatedRepos` / `rankRelated`, rendered on the overview tab of
every repository page. Cross-kind on purpose: the useful neighbours of a grasping policy are the
datasets it trains on and the benchmark that scores it, not other grasping policies.

The ranking started as a weighted sum (tasks ×3, tags ×1) and a test immediately falsified it —
three coincidental tags tied one real task match and then won on stars. It is now lexicographic:
shared tasks first, shared tags only as a tie-break. With *any* weight there is some number of
tags that outranks a task, and tags are free text while a task is a claim the manifest validates.

Each result shows what it shares. A recommendation that cannot say why is a guess.

Public-only, even for a member who could see more: this is a rail on a cached page, and
"related" is not a strong enough reason to put a private repository into a shared cache.

**New Benchmarks** rail on the overview — the last of §22's six named discovery pages. Sorted by
*newest*, not by stars: a benchmark earns stars by being submitted to, which takes months, so
ranking by popularity would show the same three indefinitely and bury the ones needing
submissions.

**Record history** on benchmark pages — `getLeaderboardHistory` / `recordProgression`: every run
that beat the standing best, with the improvement over the previous record.

Scoped to a benchmark **version**, not the repository. A v2 that adds a task or reweights the
score produces numbers that are not comparable with v1's, and a single line across versions would
draw a trend out of a change of ruler.

Ordered by completion, not submission: a run queued first can finish second, and ordering by
submission shows a record being "beaten" by a number that already existed. A private subject is
excluded rather than anonymised — an unnamed top row still discloses that a private repository
exists and how well it scores.

Shown only when a record has actually been beaten. A "history" containing one row is the
leaderboard's first row, restated.

---

### The two cross-links (§26), previously deferred

`matchByKeywords()` plus `SkynetResourcesLink`, rendered on BotPaper and BotGuide pages.

**Exact matching, not relevance search.** A paper on manipulation matches dozens of models on a
ranked query, and a cross-product panel that surfaces approximately-related things is one readers
learn to skip within a week. Only a tag the publisher chose or a slug the keyword spells out
counts. Keywords are normalised the way repository tags are on write, or "Isaac Lab" would never
match the `isaac-lab` tag it refers to. No match → the section does not render.

---

### Shadow mode: unchanged, and it covers the new surfaces

`SKYNET_PUBLIC` is still `false`. Verified live against the running app, not by reading the flag:

| Check | Result |
|---|---|
| `GET /en/skynet/` response header | `X-Robots-Tag: noindex, nofollow, max-image-preview:large` |
| `/en/skynet/` document `<meta name="robots">` | `noindex, follow` |
| `sitemap-index.xml` | zero occurrences of `skynet` |
| A published BotPaper page | renders no Skynet section |
| A published BotGuide page | renders no Skynet section |

The two new cross-links are the ones that needed checking, because BotPapers and BotGuides *are*
indexed and a link from an indexed page is exactly how a crawler would find the hidden product.
Both return null on `!SKYNET_PUBLIC` before doing any work.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **194 passed** (175 + 19) |
| `npx next build` | Succeeds; `/api/skynet/v1/namespaces/[handle]/audit` registers; 16,308 pages generated |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 4,980 source keys |
| `node scripts/skynet-verify-rls.mjs` | **All invariants hold, no skips** — teams and org-membership now verified, audit section added |
| `GET /api/skynet/v1/namespaces/{h}/audit` unauthenticated | 401 `unauthorized` |
| `npx eslint` | Cannot run in this repo — `typescript-eslint` does not support TS 7.0 (pre-existing) |

---

### Not created — the accurate list, after this session

**Deferred by the plan itself** (§29's second list; §30 forbids fake implementations). Each has
schema, job kind, API, permissions and UI, and reports that it is waiting on infrastructure:

| Item | What exists | What is missing |
|---|---|---|
| GPU training execution | run records, job queue, worker lease/secrets protocol, UI, CLI | a registered GPU worker; no run has ever executed |
| Large-scale simulation | simulator registry, job kinds, `sim` queue | sim workers |
| Browser Spaces **rendering** | Spaces are creatable, pinned and runnable as jobs | a streaming renderer; the browser shows job state, not a robot |
| Physical deployment rollout | deployments, pinned digests, check-in, rollback, audit | a real robot running the runtime agent |
| Teleoperation capture | device tokens, episode API, `teleop_*` sources | a VR/mocap/gamepad capture client |
| Fleet management | `target: 'fleet'` on a deployment | fleet grouping, staged rollout, health aggregation |
| Inference infrastructure | — | hosted inference endpoints |
| Marketplace (§20) | — | monetisation; the plan explicitly says not to prioritise it |

**Deferred by decision, with reasons:**

* **Projects (§4 IA)** — still folded into organisations. `skynet_repo_links` provides the
  wiring and a Space provides the runnable composition; a third container over the same rows
  would be navigation, not capability.

**Thin rather than absent:**

* **Search** is Postgres FTS. The `SearchDriver` seam exists so OpenSearch or Typesense is a new
  driver rather than a rewrite, but no external engine is wired.
* **Dataset streaming (§6)** yields per-file signed URLs; there is no server-side range or
  partial-record streaming, and the SDK does not decode RLDS/LeRobot/HDF5.
* **Sandboxed execution / container isolation (§23)** is assumed by the worker protocol. The
  platform never executes uploaded code itself — which satisfies §23's actual requirement — but
  it does not run the containers either.

### Next up

1. **Set `SKYNET_SECRETS_KEY`** (`openssl rand -base64 32`) wherever the app runs. It is not in
   `.env.local` or `.env.production`. Signing works without it; secrets refuse to store.
2. `node scripts/translate-with-openai.mjs` — 55 new English keys this session, sitting in all 35
   catalogs as English.
3. Register a GPU worker against `POST /v1/worker/register` and run one real training job end to
   end. Every layer above it is built and still untested against a real worker.
4. Launch checklist: set `SKYNET_PUBLIC = true` **and** delete the Skynet noindex block at the end
   of the `headers()` array in `next.config.ts` — header rules are static config and cannot read
   the flag.

---

## Session — 2026-08-28 (eighth): measured as a hub — scanning, envelopes, gating, one-line load, pull requests

An audit of the platform *as a hub* rather than as robotics infrastructure produced a different
gap list from the earlier ones, and an order to close it in:

> artifact scanning → one-line load → pull requests → gated repos + safety envelopes.
> The first is the one that hurts if skipped; the second is what makes people come back.

The previous session took the first, fourth and fifth of those and was cut off before logging any
of it. This entry covers both halves: what that session built, and the two items it never reached.

---

### First: the previous session's work, which was never logged

**1. Artifact scanning (`lib/skynet/scanning.ts`, 429 lines).** The dominant serialisation format
for weights is Python's `pickle`, and unpickling *executes code by construction* — `GLOBAL` names
a callable, `REDUCE` calls it. `torch.load` on an untrusted `.pt` is remote code execution, and
here the machine doing the loading is bolted to a humanoid. `artifact_scan` had been a declared
job kind with no scanner behind it since the first migration.

It is a **format identifier**, not a malware scanner, and the verdicts say so: `safe`,
`executes_on_load`, `unknown` — with `unknown` explicitly not a synonym for safe. Nothing tries to
decide whether a pickle is *malicious*; that cannot be answered from bytes, and the useful answer
— "reading this runs code" — can be. Only the first 64 KB is read, because every format below is
identified by a header.

Wired into upload completion and into the deployment gate (`hardwareSafetyGate`), which blocks on
`weights` and `code` roles only: a documentation image with an unrecognised format is not a reason
to refuse a deployment, and blocking on it would train operators to route around the check.

The test caught a real detection gap while it was being written: `'|O'`, NumPy's actual object
descriptor, was not matched, so an object-dtype `.npy` — which NumPy stores as a pickle and
unpickles on load — was being cleared as safe.

**2. Safety envelopes (`20260828230000`).** One nullable `limits` column on `skynet_robots`, plus
compatibility rules that compare what a policy declares it commands against what the machine is
rated to survive. The compatibility engine could already answer "will this run"; it could not
answer "should this be allowed to run on hardware". A policy trained in simulation against 200 Nm
joints and deployed to an arm rated for 80 Nm passes every capability, action-space and adapter
check, and then breaks a joint on the first command.

Nullable rather than defaulted to `'{}'`, and that distinction is the whole design: `NULL` means
nobody has assessed this robot and the rules **skip**; `'{}'` means somebody looked and declared
nothing. Collapsing the two would render an unassessed robot as "checked, fine". Nothing is
seeded — inventing plausible torque figures would produce a safety check that lies, which is the
worst outcome available here.

While seeding the new rules, a real defect surfaced in `loadRules`: it returned the database rows
whenever there were any and used the built-ins only as a cold-start fallback, so every built-in
rule shipped *after* an environment was first seeded would be dead code. A new safety check that
silently never runs is worse than no check, because the report still says "compatible". The union
is now keyed on the row *existing* rather than on it being enabled — row present and disabled
still means off, so operators keep the ability to switch a rule off.

**3. Version diffs (`lib/skynet/diff.ts`).** Versions are snapshots, not commits, which is right
for artifacts and cost the one question a hub is asked constantly: *what actually changed in
1.3.0?* Recoverable without git, because both halves are already recorded — files are
content-addressed, so a changed file is a changed SHA-256, and the manifest is a validated
document comparable key by key. Renames are detected by content, which content addressing makes
exact rather than heuristic. It reports that a file's content changed, never how: for a weights
file, "the bytes are different" is the entire useful answer.

**4. Download graph.** `skynet_downloads` had been recording every event with a timestamp since
the first migration while the UI showed only the scalar count. The time series was one query away.

**5. Gated repositories (`20260828240000`).** The third state between public and private:
publicly listed, described and indexable, downloadable only after accepting the publisher's terms
or being individually approved. It matters more here than on a model hub — the artifact behind the
gate drives a two-metre humanoid, and "which robot are you putting this on, and did you read the
safety notes" is a reasonable condition. The alternative publishers had was making the repository
private, which removes it from discovery entirely: the work becomes invisible in order to be
controlled.

Enforced on the artifact download route — the single chokepoint every fetch already passes for
access control and counting — and deliberately **not** in RLS, because a gated repository *is*
public and its page, file list and digests must keep rendering. `terms_snapshot` stores the exact
text accepted rather than a pointer to the current terms: a publisher editing their terms later
must not retroactively change what somebody agreed to.

**What that session left unfinished, found and fixed here:** all 35 of its user-facing strings —
the whole access-gate panel, the gate badge, the gate-mode labels and seven API error messages —
were referenced by key and never added to `messages/en.json`. Every one of them was rendering as a
raw `skynet_gate_reason_placeholder` in all 35 locales. They are in now.

---

### 6. One-line load (§6, and the item the audit called the moat)

> *"HF's actual moat is `from_pretrained()`. Until `Policy(...).load()` returns something LeRobot
> or Isaac Lab can step, every user writes their own loader and the manifest is documentation
> rather than an interface."*

`download()` gives you files. `Policy("ns/name").load(robot="unitree-g1")` gives you an object a
control loop can call:

```python
policy = Policy("openrobot/door-opening").load(robot="unitree-g1")
action = policy.act(observation)
```

**`lib/skynet/loading.ts` — the load plan.** Computed server-side, exposed at
`GET /repos/{ns}/{name}/versions/{v}/load?robot=`, because three of its inputs are things a client
cannot work out for itself:

* **the loader**, derived from the format the *scanner* identified in the bytes rather than from
  the file extension. This is where phase 1 pays for itself: `.pt` is written both by `torch.save`
  (a pickle) and by `torch.jit.save` (a TorchScript archive), the extension cannot separate them,
  and the header can. A version shipping both safetensors and a pickle resolves to safetensors,
  which is the entire reason publishers are steered towards it.
* **the trust cost**, so a caller is told which files execute code on load *before* fetching them.
* **the safety envelope** of the named robot — phase 2 again — so the runtime doing the stepping
  holds the ceilings locally. A clamp that needs a network round trip to a hub is a clamp that
  fails exactly when the link does.

Three decisions worth stating:

* **An ambiguous entrypoint is reported, never guessed.** Two weight files and no declaration
  yields no entrypoint and an `entrypoint_ambiguous` warning. A runtime silently choosing between
  `policy_ema.safetensors` and `policy_final.safetensors` is a reproducibility hole that *works*,
  which is the worst kind.
* **An unrecognised file yields no loader at all**, not `custom`. Answering `custom` would tell a
  caller "the publisher chose this", which is exactly the false claim to avoid.
* **The plan reports; it does not block.** Anyone who can download the files can load them however
  they like, and pretending otherwise would be theatre. The two places that can genuinely refuse
  do so from the same facts: the SDK, and the deployment gate.

**The manifest becomes an interface.** `manifest.runtime` gained `loader` and `config` alongside
the existing `entrypoint`/`framework`/`container_image`, and it is now *validated* — an unknown
loader name or a path like `/abs/model.pt` is rejected by ordinary manifest validation, so the
API, the CLI and the web form all refuse it identically. The half that needs the file list runs at
**publish** time: a card naming `policy.pt` in a version containing only `model.safetensors` is a
load instruction that fails on every machine that follows it, and publish is the last moment it
can be caught cheaply. Not at draft time — a manifest written before the upload finishes is a
normal intermediate state, not an error.

**`sdk/python/skynet/loading.py` — the client half.** Still standard-library-only at import time;
every backend is imported lazily inside the function that needs it, so a robot's onboard image
does not install a dependency tree to pull a policy.

* **Verified cache.** `~/.skynet/cache/<ns>/<name>/<version>-<digest12>/`, keyed by version *and*
  digest. A file that does not hash to the digest the platform published is deleted rather than
  kept for inspection — a corrupt checkpoint sitting in a cache directory is one the next run
  picks up. `Client.download_artifact` now verifies on every path, computing the hash as the bytes
  are written; it previously verified nothing at all.
* **Refusals the caller has to answer for.** Code-bearing weights need `trust_remote_code=True`;
  a policy declaring commands above the robot's rating needs `allow_over_envelope=True`. Clamping
  the second silently would hide the fact that the policy was written for a different machine.
* **Backends:** lerobot (`PreTrainedPolicy.from_pretrained`), TorchScript, ONNX, pickle (only
  behind the flag), safetensors. safetensors returns its tensors on `.weights` and raises from
  `act()` with the reason — tensors carry no architecture, and inventing one to make `act` appear
  to work would produce a policy that runs and means nothing. No backend at all still returns a
  usable object: the files are verified and local, which is most of the value, and `act()` is
  where the gap is explained, at the moment somebody needs it.
* **The clamp is the part no model hub has an analogue for.** `LoadedPolicy.act()` bounds every
  action to the robot's envelope before it is returned, symmetrically (80 Nm rated means
  [−80, 80], because a one-sided clamp passes every reverse command through untouched), and only
  against the ceiling that governs that action space — a position command is not a rate, and
  clamping one against a velocity limit would be arithmetic dressed as a safety check.
  `runner.report()` ends an episode with what was enforced, the peak reached and how many commands
  were clamped.
* **Datasets are read, not just streamed.** `Dataset.read()` decodes Parquet (and LeRobot, which
  is Parquet underneath) and HDF5 through pyarrow/h5py, batch by batch because trajectory shards
  are routinely larger than the memory of the machine reading them. Formats needing a heavyweight
  dependency raise **by name** rather than pretending the dataset is unreadable. `stream()` stays
  as the raw primitive.

**On the page.** The repository overview leads with the generated snippet, the loader, whether it
was declared or inferred, the entrypoint, and what a fetch costs. It replaces the previous "Use
this" panel, which was a template describing an API that did not exist — worse than nothing for
anyone who pasted it. `skynet load <ref> [--robot R]` prints the same contract and **exits 2**
when the plan carries something an operator must answer for, so it works as a CI gate; an inferred
loader or a missing control rate prints and does not fail, so the gate does not become noise.

---

### 7. Pull requests (§19, and the largest structural gap)

`forked_from_id` has existed since the first migration and nothing has ever merged back. Forks,
discussions and no contribution path make a publishing site, not a hub: an improvement to
somebody's grasping policy lives in the improver's namespace forever, and the fork graph records a
split that never healed.

**A pull request is a discussion with a proposal attached.** `skynet_discussion_kind` gains
`pull_request`; `skynet_pull_requests` holds only what a proposal has that a conversation does not
— where the work is, what it targets, what became of it. That is not a shortcut: it inherits
per-repository numbering, comments, state, notification and moderation unchanged and already
tested, and it is the only shape in which issue #7 and pull request #7 cannot both exist.

**Merging is publishing.** There is no three-way merge to do — a version is a complete, digest-
pinned snapshot — so merging publishes a **new version of the target** whose files and manifest
are the proposal's. Content addressing makes it free: not one byte moves, because both
repositories reference the same artifacts. It runs through the ordinary `publishVersion` path, so
digest computation, the new runtime-contract check, benchmark task validation and the activity
record all apply. A contribution path that bypassed those would be the cheapest way to get an
unchecked artifact into a repository people deploy from. Accordingly it requires `repo.publish` —
the same capability a direct release needs — and is written to the audit log as
`pull_request.merge`.

**The number it lands as is decided, not assumed.** `mergeVersionFor` takes the proposal's own
number when the target has never seen it and it sorts above the head — a fork that bumped to 1.3.0
should land as 1.3.0, because that is the number its README and any paper citing it already use.
Otherwise it takes the target's next free minor. The case this exists for is ordinary rather than
exotic: two forks that both published 1.3.0 both proposing upstream, where reusing the number
would overwrite a published version that cannot be edited afterwards. The comparison is semver,
not string — `'1.10.0' < '1.9.0'` lexicographically, and a merge landing below the head would be
invisible as the latest version.

**Staleness instead of conflicts.** `base_version_id` records the target's head when the proposal
was opened; the diff is measured from there and never re-based against a moving head, because
that would silently change what a reviewer is looking at between two page loads. A target that has
since moved is *reported* as stale and never automatically refused — a proposal against an older
head is frequently still exactly right.

**Five refusals at open time**, each for a specific way the alternative goes wrong: a repository
proposing to itself; a kind mismatch (a dataset merged into a model repository produces a version
whose manifest cannot validate against its own schema); a draft (its files can still change after
a reviewer approves them, which is the one property a proposal must not have); a source less
visible than its target (maintainers asked to merge work they are not allowed to look at); and
bytes the target already has. RLS mirrors the fourth from the other direction — a proposal is
readable only to someone who can read **both** repositories.

Declared robot support travels with a merged version. Dropping it would silently downgrade a
policy from "verified on this arm" to "nobody has said" — a claim being lost, not a field being
skipped.

**Provenance.** `skynet_repo_versions.merged_from_version_id` records where a merged version came
from, and the release notes say so in prose. A published version that appeared out of another
namespace with no trail is precisely the hole this platform exists to close.

**Surfaces.** A `Pull requests` tab on every repository kind — a dataset with a thousand extra
episodes and a benchmark with a corrected task are contributions in the same sense a checkpoint
is. On a fork the tab points *outwards*: the fork notice now names and links its parent, and the
tab carries the propose form plus what this fork has already sent upstream. Merge asks for
confirmation, which nothing else in this UI does, because it is the one control that publishes
into somebody else's repository irreversibly. Whether the viewer may merge is asked of the server
on the thread view rather than passed down — the repository page is public and cached, so it is
rendered without a viewer; showing the button to everyone would offer an action most readers
cannot take, and hiding it from everyone would hide it from the maintainer it exists for.
`skynet pulls | propose | merge | decline`, and `Policy.propose()/merge()/decline()` in the SDK,
because the people most likely to be sending work back are already scripting `push` and `publish`.

---

### One root-cause fix found on the way

`forkRepo` inserted `skynet_version_files` rows directly and never bumped `ref_count`. Merging
needed the same operation, and duplicating it would have duplicated the bug. Both now call
`copyVersionFiles(from, to)`, which inserts the rows *and* bumps the counts. Nothing reclaims
unreferenced blobs today, which is exactly why this is worth fixing now: the day something does,
an undercounted reference deletes weights that three repositories are using.

---

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **295 passed** (16 files; +26 loading, +19 pull requests) |
| `npx next build` | Succeeds; the `load`, `pulls`, `pulls/{n}` and `pulls/{n}/merge` routes register |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK |
| `node scripts/skynet-verify-rls.mjs` | All invariants hold; the new pull-request section skips itself until its migration is applied |
| Live schema probe (service role, read-only) | `20260828230000` and `20260828240000` **are applied**; `20260828250000` is not |
| `npx eslint` | Still cannot run in this repo — `typescript-eslint` does not support TS 7.0 (pre-existing) |

The scanner's own tests are the ones worth keeping an eye on: they are the piece that must not be
wrong, and they have already caught one false `safe` verdict during development.

---

### ⚠ One migration is pending

`supabase/migrations/20260828250000_skynet_pull_requests.sql` has **not** been applied — this
session had no Supabase MCP and no database credential that can run DDL. Until it is:

* the Pull requests tab renders as empty rather than erroring (every read returns `[]` on a
  missing table, which is the correct degradation);
* opening a proposal fails with a PostgREST message;
* `scripts/skynet-verify-rls.mjs` prints `pull requests: migration not applied yet, skipped`.

Applying it also adds `merged_from_version_id` to `skynet_repo_versions`. Note that
`ALTER TYPE … ADD VALUE` for the new discussion kind is in the same file and is only *declared*
there — no row uses the new value in that transaction, so it is safe on PG 12+.

---

### Still not built — the accurate list after this session

Unchanged from the seventh session's table (GPU workers, sim workers, Spaces rendering, physical
deployment, teleop capture, fleet, inference, marketplace), plus the hub-audit items this session
did not reach:

| Item | Why not, and what it needs |
|---|---|
| Webhooks | Nothing exists. A lab's CI cannot react to a published version. Small and self-contained; a good next one. |
| Storage quotas | Nothing bounds what a namespace uploads. Needs a policy decision before code. |
| Collections | Curated cross-repo sets — the Projects gap under another name, still deliberately deferred. |
| Sim-vs-real result separation | The leaderboard distinguishes verified from self-reported, not simulated from embodied. These are different claims and should not share a column. |
| Incident reports | Deployments check in with a status; nothing records "this policy failed on hardware this way" back onto the repository page. |
| Per-unit calibration | Two G1s are not identical, and nothing captures the calibration a policy depends on. |

**And the one that is not a feature.** The live probe run this session confirms it directly:
**0 repositories.** The hub is code-complete and empty. Nothing above is blocking it.

### Next up

1. Apply `20260828250000_skynet_pull_requests.sql`, then re-run `node scripts/skynet-verify-rls.mjs`
   — the new section will stop skipping and check that a proposal from a private source stays
   invisible to anon.
2. `node scripts/translate-with-openai.mjs` — 95 new English keys this session (89 UI/API plus 6
   endpoint descriptions), sitting in all 35 catalogs as English. 26 of those were the previous
   session's gating strings, which had never been added at all.
3. Set `SKYNET_SECRETS_KEY` (`openssl rand -base64 32`) wherever the app runs — still outstanding
   from the fourth session.
4. Seed real repositories. Every surface built in the last four sessions renders an empty state.
5. Launch checklist unchanged: `SKYNET_PUBLIC = true` **and** delete the Skynet noindex block at
   the end of the `headers()` array in `next.config.ts`.
