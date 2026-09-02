# Build Skynet — Physical AI Platform for RobotOverflow

You are the lead software architect and engineer for **Skynet**, a new major product within the existing **RobotOverflow** ecosystem.

Skynet is **not a standalone company or standalone website**. It is a product/module inside RobotOverflow, alongside:

- **RobotOverflow** — robot marketplace
- **BotNews** — robotics news
- **BotGuides** — robotics guides
- **BotPapers** — robotics research papers
- **Robot Database** — robot database
- **BotEdu** — robotics education
- **Skynet** — physical AI infrastructure

The existing website is:

**https://robotoverflow.com/**

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

The existing RobotOverflow codebase and infrastructure should be inspected first so Skynet integrates cleanly rather than creating unnecessary parallel infrastructure.

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
13. Determine how Skynet should integrate with existing RobotOverflow data.

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

If a robot already exists in RobotOverflow's Robot Database, Skynet should reference it.

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

But create an original RobotOverflow/Skynet visual identity.

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

# 26. RobotOverflow ecosystem integration

Skynet must be integrated into the existing RobotOverflow navigation.

The ecosystem should conceptually become:

```text
RobotOverflow
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
- RobotOverflow integration

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

Inspect the existing RobotOverflow codebase and infrastructure.

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
- integration with RobotOverflow

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

Your job is to determine what **Hugging Face would look like if the artifact being distributed were a robot brain rather than an LLM**—and then build that product as **Skynet within the existing RobotOverflow ecosystem**.

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
| 26 | RobotOverflow integration | Navbar pill + mobile links, Robot Database cross-link |
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
  keeps one robot profile across RobotOverflow instead of two (§7, §26). It prefills from the match.
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
RobotOverflow database. Confirmed by loading `/en/skynet/` on the dev server:

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

The blocker above is resolved. `efyazkprfdpligccotwc` (RobotOverflow) was connected and the schema
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
  RobotOverflow org to a handle), `GET|POST /v1/namespaces/{handle}/members`, `MembersPanel`,
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

Skynet had its own `skynet_members` table. RobotOverflow already has `org_members` — the roster the
marketplace bills seats against, the one the org settings page edits, the one people actually
maintain. Every organisation therefore had two membership lists that could disagree, and the one
that governed Skynet access was the copy nobody was looking at.

The concrete failure: **nothing propagated a removal.** No route under `app/api/organisations/`
referenced Skynet in any way. Taking somebody out of a RobotOverflow organisation left their
`skynet_members` row untouched, so they kept read and write access to that organisation's private
repositories — and to the robots those repositories deploy to — after they left.

A sync job would have been the smaller change and the worse one: it fails silently, and the
failure mode is somebody keeping access they should have lost. Deriving the role removes the
class of bug instead of monitoring for it.

### After

There is one roster, and it belongs to the marketplace:

* a **user** namespace has exactly one member — the profile in `skynet_namespaces.user_id`;
* an **org** namespace's members *are* `org_members` for the organisation behind it.

| RobotOverflow | Skynet | What it carries |
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

---

## Session — 2026-09-02 (ninth): the Hub answers a robot

Eight sessions built a hub for artifacts, versions, signatures, evaluations and deployments. Since
then a separate repository has built twenty-four sessions of **Skynet OS** — a robot that pairs,
pulls its desired state, reconciles against it and reports what it applied — and each of the last
four of those sessions ended with the same line: *there is no Hub on the other side.*

Four routes existed on paper (§17, §57) and nowhere else:

```
POST /devices/pair             a code becomes a credential          §10
GET  /devices/{id}/desired     the Hub states intent                §11
POST /devices/{id}/checkin     the robot says what it applied       §17
POST /devices/{id}/incidents   the black box sends what it sealed   §13
```

All four exist now, over three new tables, and nothing about the wire format was invented here.

### The contracts are imported, not restated

`lib/skynet/os/` already held ten byte-identical copies of the OS reference modules, hashed against
a manifest by `skynet-os-mirrors.test.ts`. Three joined it — `identity`, `checkin` and `intent` —
and each earns its place from a different direction, which is worth writing down because *"the Hub
touches this now"* is not the test:

* **`identity`**, because both sides must agree what a pairing code *is*. The robot folds `I`, `L`
  and `O` into `1`, `1` and `0` before sending, since §18's alphabet excludes them; a Hub that
  minted from a different alphabet, or compared without folding, would refuse codes an operator
  typed exactly as printed. `DEVICE_SCOPES` is the same property from the other end — the robot
  refuses a credential carrying a scope outside its allow-list, so a Hub minting one outside it
  produces a robot that pairs and then reports itself broken.
* **`checkin`**, because this Hub is the only consumer of `claimFor`'s four states, and §17's rule
  is enforced by whichever side decides what `applied: false` means.
* **`intent`**, because `assessOffer`'s own header asks for it: *"the same function runs on the
  robot and on a Hub checking what it just served."*

### §17's rule, enforced by the party it is about

> **The platform never claims a robot is running something it has not heard from.**

`assessClaim` runs the reference's own `claimFor` over the check-in and compares the answer with
what the document claimed. A check-in saying `applied: true` beside a pending list that is not empty
is **recorded as sent and not believed** — stored whole, because it is evidence, and shown as not
converged, with the disagreement written to the audit log where the next check-in cannot overwrite
it.

The interesting part is what it deliberately does *not* check. The obvious version runs `claimFor`
against **this Hub's** generation, and it is wrong: a robot that has not yet pulled the newest
intent is *behind*, not inconsistent, and `claimFor` would label it `inconsistent` — a word whose
detail text blames the robot's supervisor. Every robot in a fleet is in that state for the seconds
after an operator saves an edit, and a console that cried supervisor-bug at all of them is a console
people stop reading. How far behind a robot is is a separate and honest number: `generation_lag`.

### The three tables

**`skynet_pairing_codes`.** The code is never stored — only an HMAC of it under
`SKYNET_SECRETS_KEY`, and deliberately **no prefix**, which is the one place this schema breaks its
own convention. Every other credential here has a `token_prefix` and it discloses nothing, because
those are 32 random bytes. A pairing code is *eight characters of forty bits*: a four-character
prefix leaves twenty, which a laptop enumerates. So a code is identified to an operator by the name
they gave it and by when they made it, and by nothing derived from the secret. Keyed rather than
plain sha256 for the same arithmetic — forty bits of unkeyed digest is a rainbow table — and with no
key configured, minting **fails loudly**, exactly as the secret store does and for its reason.

**`skynet_device_state`.** One row per device, updated in place: the Hub's intent on one side and
the robot's last check-in on the other, and nothing that merges the two into a third thing. Not a
history table — §34.4 collapses check-ins on the robot because only the newest matters, so storing
every one here would be keeping a series the robot has already decided is not one.

**`skynet_incidents`.** Append-only, because an incident is an event rather than a state. `kind` has
no CHECK constraint on purpose: a robot on an older edition may report something this Hub has never
heard of, and turning that into a 422 would discard the one queue class §34.4 promises never to
drop.

### Four decisions worth recording

**An unconfigured device gets a document, not a 404.** Generation 0, no packages — which decodes on
the robot and says "run nothing", already §33.8's safe default. A 404 would be indistinguishable,
from the robot's side, from a Hub it could not reach, and §34.1 makes an unreachable Hub mean *carry
on with the last good intent*. Those are opposite behaviours, and the difference is a robot that
keeps running software an operator has deleted.

**Every pairing refusal is the same refusal.** Expired, already redeemed, guessed at ten times and
never existed are four rows and one sentence. `POST /devices/pair` is necessarily unauthenticated —
obtaining the credential is what it is for — and a refusal that distinguished those four would let a
caller sort a stream of guesses into "wrong" and "right but late".

**`{id}` not matching the credential is a 404, not a 403.** §19 assumes a device credential is lost.
The holder of a stolen one learns nothing from a refusal that separates *not yours* from *does not
exist*, and a fleet's device ids are exactly what they would enumerate.

**The generation is the server's.** `PUT /devices/{id}/desired` does not accept one. §11 rests
idempotence on the counter and the robot enforces it by refusing a *different* document under a
generation it has already applied; a counter a client could choose is one two operators choose
simultaneously, and the result is a fleet that stops taking updates until somebody notices.

### Verification

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **507 passed** (24 files; +19 fleet, +3 mirrors) |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK |
| `node tools/check-contract-mirrors.mjs` (OS repo) | 13 contracts match |
| `node --check scripts/skynet-verify-rls.mjs` | Parses; the three new tables are in the anon-sees-nothing sweep |

One pre-existing failure was fixed on the way in: `skynet-org-membership.test.ts` reads
`supabase/migrations/20260828210000_skynet_org_membership.sql`, and every migration file in the
repository was deleted in the rename commit. The suite had been red since. The one file that test
depends on is restored from history; the rest stay deleted, and the schema of record is
`db/01.09.2026.sql`.

Four i18n keys that were missing before this session are also filled in — the `transport` route's
refusals had never been added to any catalogue, so those four errors rendered as their own key names
in all 35 locales.


### Then the pages the tables were the blocker for

§32.4 orders the OS interface and puts *"devices, certification, and the on-robot console"* last,
with one reason: *"these follow their tables and their first real device, and until then they would
be showing invented state."* `skynet_device_state` and `skynet_incidents` exist as of an hour ago,
so `/skynet/os/devices` and `/skynet/os/devices/[id]` are built — and the second half of that
sentence turned out to be the interesting half.

**There is no first real device.** Nothing has completed a round trip against this Hub. So the
question the console faced three sessions ago is the one these pages faced: does the discipline hold
when there is nothing to show?

#### The fleet table, and the column that must never be a zero

A robot that has never checked in has `null` in every reported column, and the tempting reading of
`applied: null` is *not applied*. That renders a machine nobody has heard from beside one that is
genuinely behind, in the same row shape, with the same tone. Worse, `generation - null` is 41 in
JavaScript and reads on a page as *"41 generations behind"* — a specific, actionable, entirely
invented number about a physical machine.

So `silent` is its own tone, it sorts **first**, and `lagOf` returns null rather than a number for a
robot that has said nothing. Both have tests, because both failures are one careless `?? 0` away and
neither would look wrong.

The same rule kills the summary tile everybody builds. *"38 of 40 converged"* counts robots that
have never spoken as not-converged — arithmetic that is true and a sentence that misleads. The
silent ones are counted on their own line.

Rows sort by how much attention they want — silent, disputed, degraded, behind — rather than by
insertion order or name. A fleet page is read top-down by somebody with limited time.

#### The device page is mostly a list of what this Hub has not been told

§32.1 asks `/skynet/os/devices/[id]` for *"active and unavailable guards with the missing input
named, resolved limits and their provenance, current transport plan, black box index"*. This Hub can
supply **one** of those four, and only because the incident route landed this morning.

The other three all exist on the robot. It computes guard availability every control cycle, resolves
the limits from the profile, and holds the transport plan it is running under; §32.2 puts all three
on its own LAN console. Nothing carries any of them to a Hub. Every one could be approximated here —
guards from the health rollup, limits from the profile digest, the plan from the policy version and
the descriptor — and every approximation would be a confident statement about a machine that nobody
measured.

So `GET /devices/{id}/state` returns a `gaps` array naming all three with the reason each is missing,
and the page renders it as a panel. That is the honest version of the page rather than a reduced one,
and it is the same discipline that made the on-robot console worth building empty: what it reports is
exactly the shape of what is not connected yet.

#### Three states, kept as three

The attestation panel renders `known`, `nothing` and `unmeasured` differently, because §10 keeps
them apart and collapsing them is the specific mistake it warns about: *"there is no policy running"*
and *"a policy is running and its digest could not be read"* send different people to different
places. Health does the same with §33.6's ages — every reading is rendered with its age beside it,
because a number without one reads as current, and the four-hour-old battery is the one somebody
dispatches an engineer on.

#### Also

* `next.config.ts` gets `/:locale/skynet/os/devices/:path*` under `noindex, nofollow`, alongside
  deployments and training, for a stronger version of the same reason: a fleet page names a physical
  machine and the digest of what it is running, which is what an incident investigation wants and
  also exactly what somebody choosing a target would.
* The pairing form lives on the fleet page rather than in settings, because pairing is the action a
  fleet's empty state asks for.
* `.skynet-h3` joins the stylesheet — the device page is the first with more than one thing to say
  about one subject.
* 74 new English keys, in all 35 catalogues as English.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **517 passed** (24 files; +29 fleet in total) |
| `npx next build` | Succeeds; `/skynet/os/devices` and `/skynet/os/devices/[id]` register |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 5364 keys |

Both pages render their empty state today and will keep rendering it until the migration is applied
and a robot pairs. That is the correct output and not a placeholder.


### And the release channel, which was the last thing between §12 and a fleet that can update

`GET /os/releases?channel=&tier=&arch=` is another of §57's nine, and it is the one with a
consumer waiting: the robot's `admit_bundle` has been able to answer §12's third rule — *"an update
that changes the Safety Kernel or the envelope is refused without a fresh bench pass"* — since its
twenty-first session, and has had nothing to answer it about. `assess` reads a release manifest off
the robot's disk and nothing had ever put one there, so the only verdict a fleet could produce was
*"nothing on this robot describes release 1.4.0"*.

#### The Hub does not decide whether a robot may update, and the schema says so

§11's invariant is that **no control decision may require a network round trip**, and applying an OS
image is the most consequential control decision in the system. So `skynet_os_releases` has no
`admitted` column and the route does no per-device filtering. Either would be this Hub answering a
question it is not the authority on, and the failure it invites is exactly the one §12 spends its
length on: a fleet that applied an image because a server said it could.

What the Hub *does* compute is the same verdict for a **person**, at publish time, through the same
function — `ota` is now a mirrored contract, taking the set to fourteen. `POST /os/releases` answers
with `assessment`, so a publisher is told *"this bundle changes the safety kernel and every robot
will refuse it without a bench pass for these exact bytes"* before it is published rather than after
forty robots have refused it.

#### The vocabulary is the reference's, and it fails closed

§12's anti-gaming rule is the sharpest thing in that contract: `OS_COMPONENTS` is an allow-list, and
an unrecognised component is treated **as though it were the safety kernel** — because a publisher
wanting to slip a kernel change past the gate would write `core` or `rt`, not `safety_kernel`.

A Hub with its own retyped copy of that list would be the one place the strictness could quietly be
lost, so `changes` has no CHECK constraint and is validated in the application against the mirrored
list. There is one asymmetry, deliberately: the **Hub refuses** an unrecognised component and the
**robot accepts and gates** it. Both are right. On a robot, refusing to parse a bundle is a robot
that cannot update; on a Hub, at publish time, with the publisher present, a typo is a thing to fix
now rather than a fleet-wide refusal to debug next week.

#### Two smaller decisions

**Identity is (channel, version, arch), not the digest.** Promoting a beta build to stable is the
same bytes under a second channel, and it must not require rebuilding an image to change one string.

**The read is public and unauthenticated.** A published OS release is a public statement, and the
party most likely to fetch one holds nothing but a device credential — which `requireActor`
deliberately refuses, because a device token is not a user. Requiring authentication would mean
either widening that credential or inventing a second one for the least sensitive read on the
platform. Publishing is gated on an explicit allow-list in the environment
(`SKYNET_OS_PUBLISHERS`), which is empty by default and therefore nobody: the platform has no notion
of a staff role, and inventing half of one behind the endpoint that ships operating systems is not
the place to start.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **529 passed** (25 files; +11 releases) |
| `npx next build` | `/api/skynet/v1/os/releases` registers |
| `node tools/check-contract-mirrors.mjs` (OS repo) | 14 contracts match |

`supabase/migrations/20260902140000_skynet_os_releases.sql` is the second migration this session
that has **not** been applied.


### Conformance and certification, where a certificate stops being a document

Two more of §57's nine — `POST /devices/{id}/conformance` and `GET /certifications/{slug}` — over
two tables whose separation is the whole design:

```
skynet_conformance_results   what somebody measured        — evidence, append-only
skynet_certifications        what somebody awarded from it — a decision, at a moment
```

§15 ends on the sentence this arrangement exists to make true: field conformance *"is where
certification stops being a document and becomes a live property."*

#### The award is not a column on skynet_robots, because it decays

A stored tier is right on the day it is written and quietly wrong afterwards. §15 lapses bench
results after a year, carries an award in between on field conformance, and **suspends rather than
downgrades** below 0.999 — *"a robot whose commands have been leaving its envelope is not a
lower-tier robot, it is one whose certificate no longer describes it."*

So the row records the award as issued — claimed tier, awarded tier, reasons, the evidence it rested
on, when it lapses — and **every read recomputes** the verdict from `awardCertification`, the same
mirrored function the robot's edition runs. `GET /certifications/{slug}` returns both, and a
`divergence` sentence when they disagree. There are three ways to disagree and they mean three
different things to do next: an award that has lapsed, an award the evidence no longer supports, and
evidence that would support an award nobody has made.

#### Absence is not a pass, and the schema had to make that possible

§15's first award rule decides how results are *stored* as much as how they are read. A required
test with no result is `missing`, never a silent success — so the writer must never be able to turn a
re-run into an overwrite of a failure. The unique key is `(robot_id, suite, test_id, ran_at)`: two
runs of one test are two measurements, both on the record, and the award reads the newest per test.
An upsert on `(robot_id, suite, test_id)` would have been the obvious schema and would have let a
second run silently delete the first one's failure.

#### The robot comes from the credential, never the request

A clean bench run filed against somebody else's robot is the most valuable forgery on this platform,
because §12's certification gate reads it to decide whether an OS update that changes the safety
kernel may be applied. §19 assumes a device credential is lost; the one thing a recovered one must
not be able to do is make a statement about a machine it is not attached to. A device with no robot
is refused rather than filed under a placeholder.

#### Two things named rather than papered over

**Nothing is signed.** §57 calls these *"signed SCTS results"* and no runner on any robot produces
one, so `verified` is `false` on every row this route writes — set by the server or not at all. The
gate's evidence is currently *asserted* rather than attested, and that sentence is in the route's
header rather than only here.

**Field conformance has never been measured, and is not assumed.** §34.4 gives telemetry a priority,
a drop policy and a counter; neither §17's eight routes nor §57's nine gives it one, so nothing
carries the cycle counts it is computed from. `certificationFor` therefore passes `undefined` rather
than `1`, and the answer carries a `field_note` saying why. Passing 1 would award on evidence nobody
collected, which is the one mistake §15's whole third suite exists to prevent.

#### An unrecognised test id is stored and named, not refused

A robot on a newer edition may measure a test this Hub's contract does not have. Refusing the
submission would discard thirty-five recognised results over one name. `awardCertification` ignores
what it does not know, so the row is inert until this Hub catches up — and the reporter is told which
ids landed in that state, so the drift is visible from both ends rather than from neither.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **543 passed** (26 files; +14 certification) |
| `npx next build` | `/api/skynet/v1/certifications/[slug]` and `.../conformance` register |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK |
| `node --check scripts/skynet-verify-rls.mjs` | Parses; the three publicly-readable OS tables joined the readable sweep |


### And the tenth endpoint, which the specification did not have

`POST /devices/{id}/telemetry` is new to §57 as well as to this Hub. §34.4 gave the class a
priority, a drop policy, a counter and a paragraph of its own; neither §17's eight endpoints nor
§57's nine gave it anywhere to go, and for three sessions the robot's route table returned nothing
for it on the grounds that guessing would be inventing a Hub surface.

Building this side turned that from an observation into a decision, because **two things already
built here were waiting on the same absence**: the field-conformance figure the section above had to
pass as `undefined`, and §32.1's guard-trip-rate column — which a check-in structurally *cannot*
carry, since a rate is a count over a window and §34.4 collapses check-ins precisely because every
field in one is a statement about now.

So the OS architecture document gained a Part X and a §74, and `skynet_telemetry_windows` is the
table under it.

#### A row is a window, and that buys the honesty for free

Each row covers a half-open interval and carries counts over it — never a running total, which
absorbs a drop and leaves a tidy series, and never a sample, which produces ten thousand rows after
a week offline. What falls out is §47.4's rule *without an annotation*: a dropped window is visible
because the windows stop tiling, and a Hub holding `[10:00, 10:05)` and `[10:15, 10:20)` knows
exactly what it does not have.

#### The denominator is what was observed

The single reason this Hub mirrors `coverage` rather than writing its own. §15 puts a floor of 0.999
on field conformance and **suspends** an award below it, so dividing by elapsed time rather than
observed time is the difference between suspending a robot and not — and a robot that sent windows
for one hour out of a day has demonstrated nothing about the other twenty-three.

`certificationFor` now passes a real figure where it passed `undefined` an hour ago, and still passes
`undefined` when nothing counted cycles. That distinction is load-bearing: `awardCertification`
reads `undefined` as *no fleet data yet* and `1` as *a perfect measurement*, and substituting the
second for the first would award a certified tier on the strength of a robot that has never reported.
The answer carries a note saying which of the two absences it is, because *"no window has ever
arrived"* and *"windows arrived and none counted a cycle"* send different people to different places.

#### Every counter is nullable, and six are null on every robot

§74.3 is §33.6's rule one field over: zero and unmeasured are opposite facts, and *"no measurement of
guard trips"* rendered as `0` is the number an operator relaxes on. Four of the eight need a kernel,
one needs teleoperation, one needs the supervisor to count restarts. A column default of 0 would have
filled a fleet page with reassuring numbers nobody measured.

#### A refused window is named back to the robot

`assessWindow` is the same function the robot ran before queueing, so a refusal here should never
fire — which is why the answer carries them rather than swallowing them. A robot whose clock has
moved under it is producing windows nothing will ever count, and its own queue accounting would show
them as delivered.

#### And it lands on the device page

`GET /devices/{id}/state` carries a `telemetry` block and the page renders it — windows held,
**observed against spanned**, the intervals with no evidence, and the field-conformance figure or the
sentence saying which of the two absences it is.

Observed against spanned is the pair worth putting on a screen. They are the same number until §34.4
drops something, and §34.4 drops this class *first*; the moment they diverge, the difference is
exactly what nobody has evidence for. §15 divides by the first and an operator reading the second
would think they had a day's data.

§32.1 asked this page for four things and got three named as gaps. Guard trip rate is the fourth and
it is no longer a gap — not because a way to derive it turned up, but because a rate is a count over
a window and telemetry now has a route to arrive on.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **557 passed** (27 files; +13 telemetry) |
| `npx next build` | `/api/skynet/v1/devices/[id]/telemetry` registers |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK |
| `node tools/check-contract-mirrors.mjs` (OS repo) | **15** contracts match |
| `node tools/check-mirrors.mjs` (OS repo) | The amended architecture document is synced to both trees |


### Calibration, and the first gap on the device page to narrow

`GET|POST /devices/{id}/calibration` — the eighth of §57's ten, and the one §32.1's device page has
been naming as a gap since it was built this morning.

§45.2's table is the deliverable of the whole section, and `guardsUnavailable` is how it is read: it
converts *"we should calibrate the cameras sometime"* into *"an expired extrinsic disables the
proximity guard"*, which is the sentence §32.1 asks for by name. `calibration` is now a mirrored
contract, taking the set to sixteen.

#### Absent is not valid

§45's own warning, and the reading the page had to get right: *"treating never calibrated as no
evidence of a problem is what puts a proximity guard on a robot whose camera transform is whatever
the CAD model said."* So a parameter set that applies to the body and has no record is `absent`, its
guards are down, and the page lists it — rather than an empty table reading as a clean bill.

The panel gives **one row per (guard, set) pair** rather than per guard. `workspace` is fed by three
sets, and two of them expired is two measurements somebody has to take; one line would hide the
second until the first was done.

#### This Hub can judge half of §45.4 and says which half

`evaluate(record, now)` takes the conditions a record is judged against. Age and invalidating events
are in the record. **Joint travel and temperature are not** — they are properties of the robot at the
moment of asking, and only the robot has them.

So the reporter sends the conditions it observed when it filed, they are stored beside the record,
and every verdict on the page carries **the age of the reading it was judged against**. The clock is
aged forward — a record judged against a two-hour-old `at_s` would report valid for two hours after
it expired — and the temperature is not, because a temperature from an hour ago is not a temperature
now.

The answer also carries `unjudged`: the rules this Hub could not check at all, because the policy
names a condition the reporter did not send. A `valid` from a party that could not check half a
policy is exactly the green tick §32.2 is written against.

#### The one route that takes either credential

Every other device route takes a device token and only a device token. This one takes an operator's
as well, because §45.3 records *who* took a calibration and the honest answer is that both parties
do: a robot re-runs joint zeros after homing, and a technician with a fixture files camera
extrinsics from a bench. A route that accepted only the first would push the second into a
spreadsheet, which is the state §45.3 exists to end.

The device path is checked first and explicitly, so a device token can never fall through to the
operator path.

#### The gap narrowed rather than closing

§32.1 asks for *"active and unavailable guards with the missing input named"*. Calibration answers
the unavailable-for-want-of-a-calibration half. A guard can also be unavailable for want of a
sensor, a descriptor, or a driver that has not come up — none of which crosses — so the `guards`
entry in the gaps panel was rewritten rather than removed. Two of §32.1's four asks are now answered,
one is half answered, and one is still named.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **570 passed** (28 files; +12 calibration) |
| `npx next build` | `/api/skynet/v1/devices/[id]/calibration` registers |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, 5441 keys |
| `node tools/check-contract-mirrors.mjs` (OS repo) | **16** contracts match |

Nothing on a robot reports a calibration yet — `skynetd` holds records and serves them on its own
console, and no queue class carries them. The bench path works today; the robot path is the next
thing on that side.

### ⚠ The migration is not applied

Three of them, in order:

* `20260902120000_skynet_os_fleet.sql` — the three fleet tables, plus five columns on
  `skynet_devices`;
* `20260902140000_skynet_os_releases.sql` — the release channel;
* `20260902160000_skynet_certification.sql` — conformance evidence and the award;
* `20260902180000_skynet_telemetry.sql` — §74's windows;
* `20260902200000_skynet_calibration.sql` — §45's records.

None has been run: applying DDL to the live project is the owner's call rather than a session's.
Until they are:

* `POST /devices/pairing-codes` and `POST /devices/pair` fail with a PostgREST "no such table";
* `GET /devices/{id}/desired` and `POST .../checkin` do the same;
* every test above still passes, because they exercise the decision layer and not the storage.

Then: `node scripts/skynet-verify-migrations.mjs` to confirm, and
`node scripts/skynet-verify-rls.mjs` to watch the three tables go from `skip (not created yet)` to
`invisible to anon`.

### Still not built

Unchanged from the eighth session, plus the OS-specific gaps this session names rather than closes:

| Item | Why not |
|---|---|
| The sealed black box itself | §17 calls the incident route an upload. It accepts a document. §13's seal produces hundreds of megabytes and needs the artifact path plus a size budget nobody has set. `skynet_incidents.artifact_id` exists and is always NULL so the gap is visible in the schema. |
| `POST /robots/{slug}/attestations` | One of §57's ten. Untouched — §50's birth certificates, and it wants the `birth` contract mirrored. |
| A robot that reports its own calibration | `skynetd` holds records and serves them on its own console; no queue class carries them. The bench path works today. |
| A signed conformance result | §57 calls them signed and nothing on a robot produces a signature, so §12's gate reads evidence that is asserted rather than attested. |
| A field-conformance figure that is not null | §74's route exists and the windows arrive; `cycles` is one of six counters nothing on the current OS edition measures, so the figure is honestly absent rather than absent for want of plumbing. |
| Episodes | `Class::Episode` is now the only queue class with a route and no producer. |
| `/skynet/os/editions` | §32.4 item 2, and its own reason for waiting: it is *"needed the moment there is more than one"*, and there is one. A page listing the other eight rows of §31.3's matrix would be listing editions that do not exist. |

---

## Session — 2026-09-02 (tenth): the vendor gets a say

The ninth session built nine of §57's ten and left one line in *Still not built*:

> `POST /robots/{slug}/attestations` — one of §57's ten. Untouched — §50's birth certificates, and
> it wants the `birth` contract mirrored.

It exists now. So does the contract, in a language it did not previously have.

### §50.3 is the sentence the whole thing exists to make false

> Without it, *"this is a Unitree G1 with these joint ratings"* is a claim the robot makes about
> itself, and the entire safety envelope rests on believing it.

Every other surface on this platform is downstream of that sentence. A transport plan is computed
against a descriptor; a conformance result is a measurement of a body; a calibration record says a
camera on *that* machine was measured; §12's gate reads a certification about a robot. All four ask
"which machine is this" and, until this session, took the machine's own word for it.

A birth certificate is the first document on the platform signed by a party who actually knows —
the vendor, at manufacture, over a key generated inside a secure element that the private half
never left. §50.2 keeps two facts apart and both halves matter:

```
the birth certificate proves   what it is     — the vendor, at manufacture
pairing (§10) proves           whose it is    — the operator, in a warehouse
```

So the table holds no ownership. `namespace_id` on `skynet_attestations` records **whose key was
checked**, not whose robot it is, and that difference is written into the migration header rather
than left to be inferred from a column name.

### The contract had one edition and needed two

`crates/skynet-contracts/src/birth.rs` had existed since Part V. There was no TypeScript oracle,
which meant there was no corpus, which meant the Rust edition was the only edition and nothing held
it to anything. The mirror tool's own comment said as much: *"`birth` waits on §57's `POST
/robots/{slug}/attestations`, which is unwritten."*

Writing the endpoint required the contract, so `reference/typescript/birth.ts` was written first —
and the ordering is the interesting part, because a hand-written second edition of a finished
module is exactly where a paraphrase creeps in.

**The Rust edition reproduced the corpus on the first run.** Eleven cases and three vocabularies,
generated from the TypeScript, byte-identical on the other side without a single adjustment. That
is worth recording precisely because it is the boring outcome: it means the messages, the finding
order, the subject ordering and the empty-string handling all matched, and none of those was
checked by anything until the corpus existed.

#### The findings are frozen with their sentences, not their codes

§50 gives its own reason and the corpus takes it literally:

> The remedies for *"the serial does not match"*, *"the descriptor digest does not match"* and
> *"the vendor's key is not one this robot trusts"* are three different phone calls.

An edition that agreed about every code while paraphrasing the messages would send people to the
wrong one, and would pass a corpus that only froze codes.

#### `subjects` moved onto the finding, in both editions

Rust carries a `Finding` enum with associated data; TypeScript carries a code and a list. The
projection between them is `Finding::subjects()`, added to `birth.rs` rather than to the encoder,
and its ordering is load-bearing: **what the certificate says, then what the machine reports**,
never the reverse. A reader who has to work out which of two hex strings came from the document is
a reader who will get it wrong once.

| Check | Result |
|---|---|
| `node tools/corpus-gen/generate.ts` | 66 files, **954 cases** (+14) |
| `cargo test -p skynet-corpus` | all 954 reproduced byte for byte |
| `cargo test -p skynet-contracts birth` | 21 passed (+3) |
| `npx tsc -p .` (OS repo) | Clean |
| `node tools/check-contract-mirrors.mjs` | **17** contracts match |

### The asymmetry that makes mirroring `birth` correct

Worth stating, because "the Hub touches this now" has never been the test for adding a name to the
mirror list, and this one splits cleanly down the middle:

```
decide the signature   →  only the Hub can. It holds the vendor's registered key.
check what it proves   →  the same question the robot answers at boot. Shared.
```

`decideSignature` is the Hub's, and it produces one of §37.1's five states — `absent`,
`unverifiable`, `failed`, `untrusted`, `verified`. Then `check` from `lib/skynet/os/birth` runs,
byte-identical to the robot's copy. Deciding and checking are two jobs and only the second is
shared.

That is also why `skynet_attestations` stores `signature_state` rather than the `verified boolean`
its two sibling tables carry. The four states that are not `verified` send four different people to
four different places, and a boolean would throw the reason away at the point of storage — after
which the badge would be recomputed from a column that no longer knew.

### A second cross-edition vector, and the reason it is not optional

A birth certificate **binds an embodiment descriptor digest**. So a vendor signing a certificate
signs a document containing a hex string that is also a descriptor digest — and without separate
domain tags, a signature over one could be presented as a signature over the other. The whole of
§50.3 would then rest on a substitution nobody would notice.

`crates/skynetd-io/src/birth_vector.rs` and `lib/__tests__/skynet-attestations.test.ts` freeze the
same four numbers, the way the embodiment pair has since §5. The two vectors deliberately use **the
same seed and the same key**, so the assertion that they produce different digests and different
signatures is the assertion that both tags actually reach the hash and the message.

```
skynet.embodiment.v1  digest 29fe4b8d…  signature cde12b51…
skynet.birth.v1       digest fded47c4…  signature e0fac9e0…
```

The sharper form of the test is the third one: signing the *embodiment* vector's digest under the
*birth* domain must not reproduce the embodiment vector's signature. It does not.

### What is observed never comes from the submission

The decision that shapes the whole route. A vendor files the document; everything it is compared
against comes from the machine's own reports. One party filling in both sides of a comparison
produces a certificate that confirms itself, which is the self-attestation §50.3 describes with
extra steps.

| Field | Source | How it got there |
|---|---|---|
| `device_key` | `skynet_devices.device_key` | The robot presented it at pairing (§10) |
| `serial` | `skynet_devices.serial` | The same |
| `descriptor_digest` | `skynet_device_state.embodiment_digest` | §10's check-in attestation |
| `hardware_revision` | **nothing** | Nothing on either side of this platform reports one |

The fourth row is left absent rather than filled from the certificate, so `check` reports it as
`unobservable` — *"the binding is neither confirmed nor contradicted"* — on every robot. The GET
answer names it once at the top level as `unobservable_here` rather than leaving a reader to notice
that every verdict carries the same finding, because it is a property of the platform and not of
any one certificate.

**Not paired is the ordinary case, not an error.** §50.2 ships a robot with a certificate and no
namespace — *"it belongs to nobody yet"* — so a certificate filed before anybody pairs the machine
has every binding unchecked, and that is the correct reading.

### Two partial unique indexes, each a physical fact

```sql
UNIQUE (robot_id, serial)  WHERE superseded_at IS NULL   -- one current certificate per machine
UNIQUE (device_key)        WHERE superseded_at IS NULL   -- one element attests one machine
```

A serial identifies a machine; a device key identifies the secure element currently in it. §50.4
makes a mainboard swap produce a robot whose certificate no longer matches its hardware — same
serial, new element — and the remedy is a vendor-signed re-issue, which supersedes rather than
overwrites. An investigation into what a robot was in March needs the document that was in force in
March, and a board swap is exactly the event that makes the two differ.

The second index is the sharper one: **two current certificates over one device key is a cloned
identity**, and the database refuses it rather than a code path noticing. The route checks it too,
so the refusal is a sentence rather than a constraint name.

### The reading is the robot's reading, not a tidied version of it

The bug this session came closest to shipping. `parseCertificate` originally trimmed whitespace,
lower-cased the device key and truncated over-long fields — all of which look like hygiene and none
of which the robot does.

A vendor writing `"device_key": "79B5…"` would then have produced a Hub row holding the lower-case
form and a robot holding the upper-case one, and the two would have compared *different strings
against the same secure element* while both reported a verdict. Neither could see the other making
the comparison.

So the reading is now `skynetd_io::birth::from_json`'s, function for function: the value when it is
a string, the empty string otherwise, nothing normalised. A badly written document is **refused
with a sentence** instead — including an over-long serial, because a truncated serial is a
certificate about a machine whose number is not the one the vendor wrote, and it would sit in the
table looking perfectly well formed.

`claims_ownership` moved the same way, to `get_present`'s rule: `"namespace": ""` is still a vendor
who put the field in the document. An emptiness test would have reported an assertion the robot
reports and this Hub does not.

### Ed25519 verification stopped being duplicated on the way past

`rawKeyFromDer` and the raw-hex verifier lived in `lib/skynet/robots/embodiment.ts` and were about
to acquire a second copy. They are now in `lib/skynet/signing.ts` — the module whose whole subject
is *"the platform never holds a private key; it stores public ones and verifies with them"* — and
`verifyEmbodimentSignature` is a four-line wrapper. Three copies of Ed25519 verification in one
codebase is three chances to get the `null` algorithm argument wrong in a way that returns `true`.

`embodiment.ts` re-exports `rawKeyFromDer`, so every existing import still resolves; its seventeen
tests pass unchanged.

### On the device page

§32.1's page gains a **Birth certificate** panel, above calibration, because identity precedes
measurement. Three badges rather than one:

* **does this document attest anything** — false when the signature cannot be believed or the key
  is another machine's;
* **was every binding it makes actually checked** — the question a single green tick answers with
  the first, which is precisely how an unchecked comparison comes to read as a match;
* the signature state, spelled out.

The findings render as full sentences rather than codes, for §50's own reason. A device with no
certificate gets the sentence rather than an empty panel: *"what it is remains a claim it makes
about itself."*

The existing panel named "attestation" on that page is §10's **image** attestation — what software
the robot is running. The two are now adjacent and are different facts attested by different
parties, so both the component and the view type say which is which.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **608 passed** (29 files; +37 attestations, +1 audit vocabulary) |
| `npx next build` | `/api/skynet/v1/robots/[slug]/attestations` registers |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, **5468** keys |
| `cargo test` (OS repo) | 1139 passed |
| `node tools/check-contract-mirrors.mjs` | 17 contracts match |
| `node tools/check-mirrors.mjs` | The amended architecture document is synced to both trees |

### §57 is now answered in full, and §57.1 records what that is worth

All eleven endpoints exist — ten from §57's own count plus §74's telemetry route. The architecture
document gained a §57.1 saying so, and the useful half of it is not the list: it is the column
beside the list saying **which carry evidence and which carry assertions**, which is invisible from
a set of routes that all return 200.

Two rows are the ones to read together. Conformance is *asserted* — §57 calls those results signed
and no runner on any robot signs one — and certification is computed from conformance, so §12's
gate, the one deciding whether an OS update may change the safety kernel, reads evidence nobody
signed. A birth certificate closes the smaller half of that: it establishes **what the machine is**,
which is the subject every other measurement is about. It does not establish that the measurements
were taken.

### ⚠ The migration is not applied

Six now, in order:

* `20260902120000_skynet_os_fleet.sql` — the three fleet tables, plus five columns on
  `skynet_devices`;
* `20260902140000_skynet_os_releases.sql` — the release channel;
* `20260902160000_skynet_certification.sql` — conformance evidence and the award;
* `20260902180000_skynet_telemetry.sql` — §74's windows;
* `20260902200000_skynet_calibration.sql` — §45's records;
* `20260902220000_skynet_attestations.sql` — §50's certificates.

None has been run: applying DDL to the live project is the owner's call rather than a session's.
Until they are, the routes above fail with a PostgREST "no such table" and every test still passes,
because they exercise the decision layer and not the storage.

Then: `node scripts/skynet-verify-migrations.mjs` to confirm — it parses the migration files, so
`skynet_attestations` is already in its sweep — and `node scripts/skynet-verify-rls.mjs`, where the
new table joins the **readable** list rather than the invisible one. That placement is a decision
and not an oversight: §50.3's argument is that a certificate nobody can inspect is a document
rather than an attestation, the state most worth making visible is an *unsigned* one, and the table
can be public at all only because §50.2 kept ownership out of it.

### Still not built

| Item | Why not |
|---|---|
| The sealed black box itself | §17 calls the incident route an upload. It accepts a document. §13's seal produces hundreds of megabytes and needs the artifact path plus a size budget nobody has set. `skynet_incidents.artifact_id` exists and is always NULL so the gap is visible in the schema. |
| A robot that files its own certificate | The bench and vendor paths work; nothing on a robot posts to this route. `skynetd` reads its certificate at boot and serves the verdict on its own console (§32.2), which is the same function over the same document — so the two agree today and neither tells the other. |
| A hardware revision anybody can observe | §50.2 binds it and nothing on either side reports one, so the comparison is `unobservable` on every machine. Named in the GET answer rather than left to be inferred. |
| A robot that reports its own calibration | Unchanged. `skynetd` holds records and serves them on its own console; no queue class carries them. |
| A signed conformance result | Unchanged, and now the sharpest gap on the platform: it is the one input to §12's gate that is asserted rather than attested, and §50 has just demonstrated what the attested version looks like. |
| A field-conformance figure that is not null | §74's route exists and the windows arrive; `cycles` is one of six counters nothing on the current OS edition measures. |
| Episodes | `Class::Episode` is still the only queue class with a route and no producer. |
| `/skynet/os/editions` | §32.4 item 2. Still one edition, so a page listing the matrix would be listing editions that do not exist. |

---

## Session — 2026-09-02 (tenth, continued): what a number is evidence of

Building §50 made a second thing buildable, and the connection is the whole reason this half
follows the first rather than waiting for its own session.

§56.2 defines three provenance classes for on-robot evaluation results:

| Class | Means | Leaderboard |
|---|---|---|
| `field` | A real robot, **valid birth certificate**, attested image, non-simulator driver | Eligible |
| `simulated` | A simulator RHAL driver, which must declare itself | Eligible, separately ranked |
| `unattested` | No device signature, or a device the Hub does not know | Accepted and never ranked |

The second condition on the first row is a birth certificate. Until an hour ago this Hub had no way
to evaluate it, and a `field` class would have been **three conditions wearing the name of four** —
which is worse than not having the class at all, because it would have looked like evidence.

### §56.1 is what the whole thing is downstream of

> A leaderboard fed by self-reported on-robot numbers is trivially gameable — a publisher evaluating
> their own policy on their own robot, choosing their own seeds, and reporting their own score is
> not producing evidence. **So an on-robot evaluation result is only worth as much as its
> attestation.**

### The one mirrored contract where the Hub is the consumer

Every other module under `lib/skynet/os/` is a *second opinion*: the robot decides and the Hub
computes the same verdict so a person can read it. §56.2 inverts that — the **Hub** assigns the
class — and the robot's edition exists so a robot can predict its own offline.

That inversion argues for sharing the code more strongly rather than less. Two implementations of
an anti-gaming rule that disagree is an anti-gaming rule with a hole, and a hole in this one always
points the same way: towards admitting something.

### Writing the second edition found a latent divergence

`provenance.rs` had existed since Part VI and had no TypeScript oracle, so it had no corpus and
nothing held it to anything. Writing `reference/typescript/provenance.ts` turned up a real bug
before a single test ran:

```rust
format!("The robot rolled this run up to {on_robot} and the Hub to {on_hub}. …")
```

Rust's `Display` for `f64`, where every other sentence in that crate uses `crate::json::number_text`
— ECMAScript's `ToString(Number)`, which is what the reference produces with `${value}`. The two
agree for every score anybody will ever roll up and disagree above 1e21 and below 1e-6.

It is a small window and it is in **§56.5's disagreement sentence** — the one module whose entire
subject is two implementations disagreeing about a number. It is now `number_text` on both sides,
and the corpus would have caught it the moment a case reached the window.

The second edition reproduced everything else byte for byte on the first run.

| Check | Result |
|---|---|
| `node tools/corpus-gen/generate.ts` | 68 files, **973 cases** (+19) |
| `cargo test -p skynet-corpus` | all 973 reproduced byte for byte |
| `npx tsc -p .` (OS repo) | Clean |
| `node tools/check-contract-mirrors.mjs` | **18** contracts match |

`subjects()` moved onto `Reason` the way it moved onto `birth::Finding`, with the same ordering rule
— **what was expected, then what was supplied** — and one deliberate difference: `SeedsNotSupplied`
projects to the two *counts* rather than the two lists. That is what its sentence reads, it is what
a flat array of strings can hold without a separator a seed could collide with, and nothing is lost
because a consumer wanting the seeds already holds both the bundle and the manifest.

### It looked like a reconciliation and it was not

`tools/check-contract-mirrors.mjs` had recorded, for several sessions, that `provenance` was waiting
on *"a schema decision rather than an endpoint"* — `skynet_evaluations` already carrying `verified`
and `source`, which look like a provenance class and are not.

They are not two spellings of one axis. They are two axes:

```
verified   boolean                                — a registered platform worker measured it
source     platform | self_reported | imported    — who submitted it
provenance field | simulated | unattested         — what it is evidence of          (new)
```

The first two are about the **submission**; §56.2 is about the **run**. A platform worker can
measure a simulated run, and a robot can produce an embodied one nobody verified. Collapsing them
would have destroyed exactly the distinction §56 exists to draw — so `provenance` is a third column
beside them, and the "reconciliation" was a decision not to reconcile.

#### NULL is not `unattested`, and nothing was backfilled

The sharpest line in the migration. `unattested` is a **verdict**: §56.2 reached it by looking at a
run bundle and finding nothing to attribute. NULL is the absence of a verdict — no bundle was ever
submitted, so §56 has not classified the run and cannot.

Every row on the platform is in the second state. Backfilling them to `unattested` would have been
the platform asserting a §56 verdict about runs it holds no bundle for — §15's *"absence is not a
pass"* pointed the other way, since absence is not a failure either — and it would have emptied the
existing leaderboard overnight.

So a row with no class renders exactly as it did yesterday, and the tests assert that.

### An eighth domain tag, and the broadest collision surface yet

A run bundle binds a **policy digest, an embodiment descriptor digest and an OS image digest**.
Three hex strings, each already signed under one of the seven existing tags — which makes a bundle
the one document on this platform that contains other documents' subjects wholesale.

Without its own tag, a device signature over a descriptor could be presented as a device signature
over the run that names it, and §56.2 would class as `field` a result nobody attested. `BUNDLE_DOMAIN
= "skynet.bundle.v1\n"` is the eighth, in `crates/skynetd-io/src/trust.rs` and in
`lib/skynet/provenance.ts`.

The tags also gained the check their comments have been describing for eight sessions:
`every_domain_tag_is_distinct_and_newline_terminated`, plus the sharper form — one digest under the
bundle domain and the embodiment domain must be two different messages. Eight schemes over one
shape, and the tag is the only thing making a signature say what it is a signature *of*.

### Every fact the class turns on comes from the Hub

The rule the whole route is built around, and the one assertion in the test file that matters most:
**`readBundle` produces a bundle with all four attestation facts false**, whatever the submitted
body said. A bundle that could set its own `birth_attested` would be a publisher grading their own
paper.

| `RunBundle` field | Where this Hub gets it |
|---|---|
| `device_signature` | Decided here, and only against **this device's own key** |
| `device_known` | Whether the second credential resolved to a device row |
| `birth_attested` | §50's certificate, recomputed against what the Hub observes of the machine |
| `image_attested` | §10's check-in attestation (`attestation_complete`) |
| `simulator` | The bundle's, and §35.3 puts that obligation on the driver |

`decideBundleSignature` is deliberately **narrower** than the birth certificate's equivalent. A
certificate may be signed by any key registered to the vendor's namespace; a run bundle must be
signed by the machine that produced it, because §56.2 asks whether the numbers are attributable to a
machine — and a bundle signed by an operator's laptop key is attributable to a person who says a
machine produced them.

### Two credentials on one request

§57 lists an attested evaluation result as an *"existing shape"* — `POST /evaluations` with a device
credential and a signature field. Both credentials are read and neither substitutes for the other:

```
Authorization: Bearer sk_…      who may submit against these repositories
X-Skynet-Device: skd_…          which machine produced the numbers
```

A robot has no permission to publish against somebody's repository and a person is not a machine, so
a single header could only ever carry one of the two. `optionalDevice` returns `null` for every
failure — absent, malformed, unknown, inactive — rather than throwing, because §56.2 accepts an
unattributable result and never ranks it, and a refusal would turn *"this is not evidence"* into
*"you may not record this"*.

### On the board, the class is part of the key rather than a badge

The eighth session's gap list had an item reading: *"Sim-vs-real result separation — the leaderboard
distinguishes verified from self-reported, not simulated from embodied. These are different claims
and should not share a column."*

They no longer do.

* **`unattested` reaches no board at all** — not even its own. §56.2: *"accepted and never ranked."*
  The exclusion is in `getLeaderboard` and `recordProgression` rather than in the rendering, so no
  display decision can accidentally admit one, and the count of held-back rows is carried rather
  than dropped silently.
* **The class is part of the leaderboard key.** One policy on one robot can hold a field record and
  a simulated record at once; they are two claims. Collapsing them would let a simulated number
  displace an embodied one, which is the single outcome §56 exists to prevent.
* **Two columns on the table**, because there are two questions. *Evidence* (§56.2's class) sits
  beside *Provenance* (`verified` / self-reported / imported) rather than inside it.

A `simulated` badge is neutral rather than a warning: the driver declaring itself is §35.3's
obligation being met, not a fault.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **639 passed** (30 files; +30 provenance) |
| `npx next build` | Clean |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, **5473** keys |
| `cargo test` (OS repo) | **1141 passed** (+4 domain tags, +the corpus) |
| `node tools/check-contract-mirrors.mjs` | 18 contracts match |
| `node tools/check-mirrors.mjs` | §56.6 and §57's amended row synced to both trees |

### ⚠ The migration is not applied

Seven now, in order — the sixth from the first half of this session, and:

* `20260902240000_skynet_provenance.sql` — §56.2's class on `skynet_evaluations`, plus the run
  bundle, the reasons and the device that signed it.

Additive only: four nullable columns and one partial index. Nothing is backfilled and no existing
column changes, so applying it changes no page until a run bundle is submitted.

### Still not built

Unchanged from the first half, minus the sim-vs-real item, plus what §56 names rather than closes:

| Item | Why not |
|---|---|
| A robot that produces a run bundle | The Hub classifies one and no `skynetd` edition emits one, so every row's class is NULL today. The receiving side is complete and the anti-gaming rule is enforced; nothing has yet tested it against a real robot. |
| §56.5's roll-up comparison, wired | `compareScores` exists and nothing calls it, because nothing submits an on-robot score to disagree with. It is one line at the point a bundle carries a score. |
| A signed conformance result | Unchanged, and the gap is now sharper by contrast: §56 shows what an attested result looks like, and §12's gate still reads conformance evidence that is asserted. |
| The sealed black box itself | Unchanged. §56.4's replayability rests on a black box segment that this Hub records the hash of and cannot fetch. |
| Episodes | `Class::Episode` is still the only queue class with a route and no producer. |
| `/skynet/os/editions` | Still one edition. |

---

## Session — 2026-09-02 (tenth, third): an incident that knows when it happened

Found while reading the incident path for something else, and it is a defect rather than a gap —
the code was doing something it had a comment explaining, and the comment was right about the
wrong moment.

`skynet_incidents.generation` and `.state` carried this:

> The generation and state the robot was in. Copied at receipt rather than joined at read time,
> because `skynet_device_state` moves and an incident is a statement about a moment.

The reasoning is correct and the moment is wrong. Copying at receipt beats joining at read time,
and it is still not the moment the incident is about. §13 seals a recording **when something goes
wrong**; §34.4 delivers it **whenever the link comes back** — those are two moments, and for the one
class §34.4 promises never to drop they can be hours apart, across a reboot, two generations on.

So a robot that faulted on generation 40 and reconnected on 42 had its incident filed under 42, and
nothing on the row said so. An investigation reading it would have been reading a statement about
the reconnection attached to an event in the past.

### Fixed at the root, which is the robot

`skynetd` now stamps every incident with the generation and §27 state **at the fault**, through a
`Daemon::incident_context()` that every future incident site picks up for free rather than
remembering to add.

Two fields, and the choice of which two is the interesting part. They are the ones a supervisor can
state **exactly and cheaply without an observation pass**. The three digests are deliberately not
among them: §10's attestation needs a trust store, a release manifest and a descriptor read, and an
incident path that did all three would be doing that work on the one queue class §34.4 says is never
dropped.

So an incident says which generation and does not say which policy — and the Hub reports that as a
gap rather than substituting whatever the robot happens to be running now, which is the same
substitution this whole change exists to end.

`state` earns its place separately: §27's `SAFE` is sticky, so it is frequently the most useful
field on the row. It says whether the robot had already stopped when the incident was raised or was
still moving.

### And a column that says which of the two happened

The fallback stays, because a fleet contains robots on older editions for as long as the fleet
exists and their incidents will go on arriving with no context at all. What changes is that the two
are no longer indistinguishable:

```
fault    the robot said. The generation is a fact.
receipt  the incident carried nothing. The generation is an upper bound.
```

`incidentContext` is **all or nothing** on purpose. An incident naming a generation and not a state
would produce a row half about the fault and half about the receipt, which `context_at` could not
describe — and that is worse than no context at all, because it would look precise.

`occurred_at` is the one part that is optional rather than all-or-nothing: a generation and a §27
state are what an investigation reads, and a missing timestamp does not demote them. What it must
never do is default to `received_at`, which would make a delivery delay invisible.

The device page renders the generation with an *(at receipt)* marker and its explanation, rather
than rendering a fact and a guess identically.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **644 passed** (30 files; +5 incident context) |
| `npx next build` | Clean |
| `node scripts/i18n-catalog.mjs sync` | 2 new keys across 35 catalogs |
| `cargo test` (OS repo) | 1141 passed; the profile-incident test now asserts the context is on the wire |

### ⚠ The migration is not applied

Eight now. The new one is `20260902260000_skynet_incident_context.sql` — two columns, additive, no
backfill. Existing rows default to `context_at = 'receipt'`, which is exactly what they are.

---

## Session — 2026-09-02 (tenth, fourth): webhooks

The eighth session's audit nominated this one by name:

> **Webhooks** — Nothing exists. A lab's CI cannot react to a published version. Small and
> self-contained; a good next one.

Half right. It is self-contained and it is not small, because almost all of it is one problem.

### A webhook is an SSRF primitive with a feature attached

The whole difficulty is in one sentence: **a webhook is a user-supplied URL that the server
fetches.** This platform runs on a host that can reach a cloud metadata endpoint, an internal
Supabase address and a Redis instance, and the difference between a webhook feature and an
authenticated proxy into all three is entirely in what the URL is allowed to be.

Four defences, and each covers a hole the others do not:

| | Defence | The hole it covers |
|---|---|---|
| 1 | `https://` only, no `user:pass@`, bounded length — at registration | Plaintext puts the payload *and its signature* on the wire; a credential in a URL ends up in this table, a log line and a screenshot |
| 2 | A literal private, loopback or link-local address — at registration | The obvious one, and the only one a CHECK constraint could nearly express |
| 3 | **The resolved address, at delivery** | The one a registration check always misses: an attacker controls the DNS record, and a hostname that resolved publicly on Tuesday resolves to `169.254.169.254` on Wednesday |
| 4 | `redirect: 'manual'` | A 302 to a private address walks past all three above, because they ran on the URL the operator registered |

Defence 3 is the one that matters and it is the one a lot of implementations skip, because at
registration time everything looks fine. It runs immediately before **every** request, on every
attempt, and it checks **every** address the hostname answers with — not the first, because a
hostname resolving to one public address and one private one is a hostname whose resolution order
would decide whether this is an attack.

**What that leaves open is written down rather than papered over.** Between the resolution and the
connection there is a window — the classic DNS-rebinding race. Closing it needs a socket-level hook
this runtime does not offer. What remains is a race an attacker has to win repeatedly against a
check on every attempt, rather than a check they beat once at registration; that is a real
reduction and it is not a closure, and the module header says so.

Each refusal names the range it hit, because *"private address"* and *"that is the cloud metadata
endpoint"* send two very different people to two very different places — and only one of them is
the person this list exists to stop.

### The signature, and why there is a verifier in the repository

```
X-Skynet-Signature: t=<unix-seconds>,v1=<hex>
                    HMAC-SHA256 over "<t>.<body>"
```

The timestamp is **inside the signed material**, not merely beside it. A receiver checks the age
and then the MAC; an attacker who captured a delivery cannot present it later with a fresh `t`,
because the MAC covers it. A scheme where the timestamp were an unsigned header would let whoever
captured the request rewrite it.

`verifySignature` is exported, and that is deliberate: a signing scheme with no verifier in the
repository is a scheme whose first independent implementation is a guess. It is also what the tests
use, so the documentation and the check are the same function.

The secret is sealed with `skynet_secrets`' AES-256-GCM machinery and returned exactly once. A
signing secret a support tool can print is a signing secret an attacker can read, and the receiver's
entire guarantee is that a valid signature means this platform sent it. The audit row records the
URL and never the secret — an audit log holding it would be the second place it is readable, and
there is meant to be no first.

### Three decisions in the schema

**Empty means none, never all.** A webhook that subscribed to everything because a field was omitted
would send a namespace's private activity to whoever holds the endpoint.

**Suspended, not deleted, after 20 consecutive failures** — with the reason on the row. An operator
whose CI went quiet has to be able to find out why *here*; a row that vanished would be
indistinguishable from one nobody created.

**A retry is a row, not a counter.** *"It failed twice and then worked"* and *"it worked"* are
different histories, and the first is the one a receiver is debugging. Every attempt is kept,
because the single most common question about a webhook is *"did you send it"* — and that is
unanswerable without the table.

`refused` is a status of its own, apart from `failed`: the URL did not pass the address checks, or
the endpoint is suspended, or the secret could not be decrypted. Nothing was sent, it is not the
receiver's problem, and retrying will not fix it.

### Delivery is a cron, and the bound is the point

A publish that awaited a receiver would be a publish whose latency is somebody else's server: a lab
with a down endpoint would slow every release in its namespace, and one with a *hanging* endpoint
would hold a request open until the platform's own timeout. That is the mechanism by which an
optional feature takes down a required one.

So `emit` writes rows and returns — it never throws, for the same reason — and
`/api/cron/skynet/webhooks` drains fifty per run. Bounded rather than draining the table, because a
run that could take an unbounded amount of time is a run that gets killed halfway with no record of
where it was.

### Two events wired, and one of them for a reason worth stating

`version.published` is the one the audit asked for. It fires for **private** repositories too: a
webhook is registered by the namespace that owns the repository, so this is a namespace being told
about its own work, and a private repository is exactly where an internal pipeline lives.
Visibility governs who may *read* a repository, not whether its owner may be told it changed.

`incident.received` is the one somebody will page on, and its payload deliberately **does not carry
the incident body**. That is §13's evidence, it can be a quarter of a megabyte, and a webhook
payload is the wrong place for a document somebody will want to read whole. It carries what an
alert needs — including `context_at`, so an alert never treats an upper-bound generation as the one
the fault happened on.

### And then the rest of them, because a vocabulary of one emitter is not a feature

Five more, each one `emit` call at a site that already existed:

| Event | Where, and the decision behind it |
|---|---|
| `version.signed` | `recordVersionSignature`. A stored signature is always a *valid* one — rule 2 of that module's header — so the event means exactly what a subscriber reads it as. |
| `evaluation.completed` | **Only** the worker path. A self-reported submission finishes the moment it is filed, and firing there would tell a subscriber a benchmark had been run when somebody typed a number in. Carries §56.2's class, so a subscriber ranking results need not fetch the row to learn whether it may be ranked. |
| `deployment.rollback` | The event most worth waking somebody for: the platform recording that a build somebody put on hardware was taken back off it. |
| `attestation.filed` | Carries the signature **state** rather than a boolean, because four of the five mean the certificate attests nothing. |
| `pull_request.merged` | A second event beside the `version.published` the merge already emits, and not a duplicate: one says a version exists, the other says whose work it was and which proposal carried it. |

**`certification.awarded` was removed from the vocabulary.** It was declared in the first pass and
then found to be unemittable: nothing in this Hub writes `skynet_certifications` — §15's award is
recomputed on every read of `GET /certifications/{slug}` and never *issued*. A vocabulary is what a
client builds a form from, and a form offering a subscription that structurally never delivers is a
form that lies about what the platform does. It goes back in when something awards, and a test now
pins the list so it cannot drift back in by accident.

### A check that was failing on correct code

`skynet-audit.test.ts` scans every `recordAudit` call site for a namespace, and it matched
`namespaceId:` — so a call using the shorthand `namespaceId,` was reported as an offender. It fired
on the first correct call site that used shorthand, which was this session's.

Fixed as `/\bnamespaceId\s*[:,]/` rather than by contorting the call site. A check that fails on
correct code is a check somebody weakens, and the weakened version is the one that stops catching
the thing it was written for.

| Check | Result |
|---|---|
| `npm run typecheck` | Clean |
| `npx vitest run lib/__tests__/skynet-*.test.ts` | **676 passed** (31 files; +32 webhooks) |
| `npx next build` | Three routes register: the two API routes and the cron |
| `node scripts/i18n-catalog.mjs audit` | All 35 catalogs OK, **5514** keys, and it now checks value **types** |
| `node --check scripts/skynet-verify-rls.mjs` | Parses; both webhook tables joined the **invisible** sweep |

The RLS placement is the opposite of §50's, and the reason is worth the line: a birth certificate is
public because §50.3's argument is that a certificate nobody can inspect is a document. A webhook
list is invisible to anon *and* authenticated alike, because it is a map of where a namespace's
automation lives, and the sealed signing secret is on the same row.

### And §50 reached the page a buyer reads

The robot registry page gained a **vendor attestation** panel, which is the surface §50.3's argument
was actually about:

> Without it, *"this is a Unitree G1 with these joint ratings"* is a claim the robot makes about
> itself, and the entire safety envelope rests on believing it.

Until this, a birth certificate was visible on a device page — which is behind `namespace.write`,
and therefore visible only to the people who already knew. A buyer comparing two robots could not
see that one vendor attests its machines and the other does not.

Two numbers rather than one: **how many certificates are on file** and **how many actually attest
the machine they name**. Those are different facts, and a panel showing only the first would be
counting documents. The signature state is spelled out rather than reduced to a tick, because four
of the five mean the certificate attests nothing and they are four different things to do about it.

The panel is worth reading when it is empty, which is why it renders a sentence rather than
disappearing.

#### And an N+1 that would have been invisible until it was the page

`attestationsFor` looked up the paired machine one certificate at a time — two round trips each,
so twenty on a model with ten machines on file, on a **public, server-rendered** page anybody can
load. It was fine as an API handler and became a problem the moment it rendered.

`observedForMany` is the same answer in two queries regardless of how many certificates there are.
The two share the shaping rather than each having their own, so they cannot drift into two readings
of one comparison.

### ⚠ The migration is not applied

Nine now. The new one is `20260902280000_skynet_webhooks.sql` — two tables, both denied to anon and
authenticated. Until it is applied, `emit` catches and returns 0, which is exactly the behaviour it
has for a namespace with no webhooks: nothing else on the platform notices.

### Still not built

| Item | Why not |
|---|---|
| Anything that awards a certification | Which is why `certification.awarded` is not an event. §15's verdict is recomputed on every read and no path issues an award, so `skynet_certifications` is written by nothing. |
| An org-namespace switcher on the page | `/skynet/settings/webhooks` manages the signed-in user's own namespace, the way the keys and secrets pages do. The API is namespace-scoped and works for an org today; the page has no control to point at one. |
| Everything from the previous three entries | Unchanged. |

### The page

`/skynet/settings/webhooks`, built the way `SecretsClient` and `SigningKeysClient` are, with two
decisions carried over and one new one.

**The secret is a modal state of the component, not a row in the list.** A value that can never be
fetched again must not be rendered somewhere a reader assumes they can come back to. There is no
reveal control and there is no endpoint behind one — the same posture the secrets page takes, for
the same reason.

**There is no "subscribe to all" control**, and the help text says why rather than leaving it as an
omission somebody files a request about. §19's rule in the interface: a subscription made by
default sends a namespace's private activity to whoever holds the endpoint.

**The delivery list is the feature.** Every attempt, retries included, because a receiver that
failed twice and then succeeded is debugging all three. `refused` renders in a different tone from
`failed`: one is ours and one is theirs, and collapsing them would send somebody to debug a server
that was never contacted.

The event checkboxes are built from the vocabulary the **server** returns rather than from a list in
the component, so the form cannot offer an event the platform does not have — which is the mistake
`certification.awarded` would have been if it had shipped.

### Replay, and the way back from suspension

Two actions on `POST .../webhooks/{id}`, and the first has a decision in it worth writing down.

A **replay queues a new delivery** rather than re-sending the recorded request, and the reason is
the signature scheme rather than tidiness: the MAC covers a timestamp, and a receiver refuses
anything stale. That is the property making a captured delivery unreplayable by an attacker — so a
legitimate replay is necessarily a new signature at a new moment, and re-sending the recorded
headers would produce a request the receiver is right to reject.

The payload is the original's, byte for byte, which is what makes it a replay rather than a new
event: the `sent_at` inside still says when the thing actually happened. The attempt counter goes
back to 1, because a manual replay is not a continuation of the platform's own retry chain and gets
its own five attempts.

**`enable`** takes an endpoint out of suspension and resets the failure count with it. Leaving the
count at the limit would suspend it again on the very next failure, which is a re-enable that does
not re-enable. It exists because the alternative was an operator deleting the endpoint to get a
working one — and losing the delivery history they were about to read.

A pending delivery has no replay button: it is already on its way, and replaying it would queue a
second copy.

### A check that did not exist, found by needing it

Thirty-five catalogs briefly held a **number** where a string belongs. A generator bug put `17` into
`en.json` under a title key, and `i18n-catalog.mjs sync` copies the English value verbatim for a
missing key — so one wrong value became thirty-five in a single command.

Every existing check passed the whole time. The keys were present, non-empty and known, which is all
`audit` looked at. `next-intl` renders a number as itself, so the visible result would have been a
page showing `17` where a heading goes, in every locale. The only thing that caught it was `tsc`,
and only because one test file happens to import the catalog as `Record<string, string>`.

So `audit` now checks value types, and checks **en.json first and loudly** — because a bad value
there is a bad value everywhere after the next sync. Confirmed by injecting one and watching it
fail, then removing it.

Fixed for real rather than by hand-editing the one file: all thirty-four locale catalogs were
repaired from `en.json` in place, preserving their key order, so the diff is only the broken values.

---

## Session — 2026-09-02 (eleventh): the migrations are applied, and the verifier that said they were

Two things happened in this order and the order matters: the nine pending migrations were applied,
and on the way to applying them the script that reports whether a migration is applied was found to
have been answering `applied` for tables that have never existed.

### The verifier was reporting a clean bill of health for an empty database

`scripts/skynet-verify-migrations.mjs` opens with the sentence it exists for:

> A migration file existing in the repository and a migration having been run are different facts,
> and only the second one decides whether the app works.

Its first run this session said six applied, one partial, two not applied. The live database had
**none** of the nine. `skynet_os_releases`, `skynet_certifications`, `skynet_conformance_results`,
`skynet_telemetry_windows`, `skynet_calibration_records`, `skynet_attestations`, `skynet_webhooks`
and `skynet_webhook_deliveries` were all absent and all reported present.

The probe was one line:

```js
await admin.from(table).select('*', { head: true, count: 'exact' }).limit(1)
```

**A HEAD response has no body**, so there is no PostgREST error document for supabase-js to parse,
and `error` comes back null for a 404 exactly as it does for a 200. Confirmed by asking it about
`skynet_totally_made_up_table`, which it reported as present. The column probe was a GET and worked
correctly the whole time, which is why the fleet migration showed as `PARTIAL` — its five columns on
`skynet_devices` were genuinely detected as missing while its three tables were not.

The fix is `select('*').limit(0)`: PostgREST resolves the table before it applies the range, so an
absent one still answers PGRST205 and a present one answers `[]`. No rows are read either way, which
is what the HEAD was for.

### The check that would have caught it, which is now the first thing the script does

A verifier whose probe stops discriminating reports success for everything, and that is the one
output nobody re-reads. So `selfTest()` runs before any migration is examined: it asks both probes
about `skynet_verifier_sentinel_does_not_exist` and exits 2 if either says present.

```
probe self-test FAILED: absent table reported as {"ok":true}
The table probe is not discriminating. Every "applied" below would be a guess.
```

Verified by putting the HEAD probe back and watching it fire, then reverting.

### Then the nine, in order

Applied through the Supabase MCP `apply_migration`, oldest first, each as its own migration:

| Migration | What it created |
|---|---|
| `20260902120000_skynet_os_fleet` | `skynet_pairing_codes`, `skynet_device_state`, `skynet_incidents`, + 5 columns on `skynet_devices` |
| `20260902140000_skynet_os_releases` | `skynet_os_releases` |
| `20260902160000_skynet_certification` | `skynet_conformance_results`, `skynet_certifications` |
| `20260902180000_skynet_telemetry` | `skynet_telemetry_windows` |
| `20260902200000_skynet_calibration` | `skynet_calibration_records` |
| `20260902220000_skynet_attestations` | `skynet_attestations` |
| `20260902240000_skynet_provenance` | 4 columns on `skynet_evaluations` |
| `20260902260000_skynet_incident_context` | 2 columns on `skynet_incidents` |
| `20260902280000_skynet_webhooks` | `skynet_webhooks`, `skynet_webhook_deliveries` |

`20260828210000_skynet_org_membership` was already applied — both `skynet_private` functions exist
and the second roster is gone. It reports `NOT CHECKABLE` because it declares no table or column,
which is the honest answer rather than a guess.

### Verification

* `node scripts/skynet-verify-migrations.mjs` — **9 applied, 0 partial, 0 not applied**, exit 0.
* `node scripts/skynet-verify-rls.mjs` — **all RLS invariants hold**, including the seven new
  deny-everything tables in the invisible-to-anon list and the four §15/§50 tables in the readable
  one. The placement is the decision the previous session argued for, now enforced against the real
  policies rather than against the intent of the SQL.
* Supabase security advisors: one WARN, `auth_leaked_password_protection`, which is a dashboard
  setting rather than anything this DDL touched. No missing-RLS finding, which is what nine new
  tables would have produced if any policy statement had silently failed.
* Performance advisors are INFO-only and the new tables' share of them is `unused_index` on tables
  with zero rows.

### And then the thing the tables were the blocker for: something awards a certification

Top of the previous session's *Still not built*:

> **Anything that awards a certification.** §15's verdict is recomputed on every read and no path
> issues an award, so `skynet_certifications` is written by nothing.

`POST /api/skynet/v1/certifications/{slug}` issues one and `DELETE` withdraws it.

**The body carries a claimed tier and nothing else.** There is no field for the awarded tier, the
failures, the expiry or the reasons, and that is the design rather than an omission: every one of
them comes out of `awardCertification` over the stored evidence. §15 caps an award by its evidence,
and the cheapest way to enforce a cap is to make the over-claim unrepresentable — this route cannot
express an award the evidence does not support, so nothing has to check that it did not.

**Reading and issuing go through the same function.** `certificationFor` and `issueCertification`
both call a new `assess()`, so a certificate's `awarded` and the `live` verdict every reader
recomputes are the same computation over the same rows. A certificate that disagreed with the live
verdict on the day it was issued would have made §15's central comparison meaningless, and the way
to not have that bug is to not have two code paths.

**An award of `none` is refused rather than recorded.** The column accepts it because it mirrors
`CertificationVerdict['awarded']`, not because issuing one means anything: a row awarding nothing
would occupy the one current-award slot per robot and silence `divergence`, so the platform would
read as *assessed and concluded* when what it has is evidence supporting no tier. That state is
already public in detail on the GET.

**Re-issuing the same decision writes nothing.** Compared on claim, tier, failures, missing and
reasons — deliberately not on `bench_age_days` or `expires_at`, which move continuously as the
evidence ages. Including them would make every re-issue a new row and turn the supersede chain into
a clock.

**Withdrawal exists because supersede without it is a one-way door.** An award issued in error, a
model withdrawn from sale, a vendor leaving the programme: without a withdrawal the only way back
would be issuing a lower tier, which says something different and false. The row is superseded, not
deleted — §15's award is a decision at a moment and who decided what is the reason the table exists.

### A new capability rather than reusing one

`certification.issue`, minimum role **maintainer** — the same rank as `deployment.write`, whose
comment says *"deploying a policy to hardware is deliberately stricter than pushing code."* Issuing
an award is a claim §12's readiness gate reads before a policy may reach hardware, so it is that
weight. It stops short of `owner`: the award is capped by evidence and recomputed on every read, so
the capability governs **who may make the claim**, never what the claim is allowed to say.

Filing a birth certificate stays at `namespace.write`. §50 records what a vendor signed; §15 records
a measurement somebody is standing behind.

### The two certification events, and why one of them is not an award of `none`

`WEBHOOK_EVENTS` carried a comment for several sessions explaining that `certification.awarded` was
held out because nothing could fire it, ending *"it goes in when something awards."* Both go in now:

* `certification.awarded` — issued, with the verdict's own reasons in the payload so a subscriber
  can render §15's tooltip without fetching the certificate back;
* `certification.withdrawn` — because `issueCertification` refuses to record `none`, a subscriber
  watching only the first would see an award go up and never see it come down. That is the one
  transition an operator most needs to hear about.

### The test that was a snapshot and is now a check

`offers only events something actually emits` asserted a hardcoded copy of the list. That is not the
same check: it re-stated the answer instead of asking the question, so it could only ever fail when
somebody edited the vocabulary — never when somebody deleted the last emitter of an event that
stayed in it. It had nothing to do with `certification.awarded` being held out, which was noticed by
a person reading the code.

It now scans `lib/skynet/*.ts` for the emitter of every event in the vocabulary, and a second test
runs the check the other way: an `emit` naming an event no endpoint can subscribe to is a queued
delivery nobody receives, which costs a row per event forever and reports success.

### One role ranking, because a browser needed it too

`ROLE_RANK` moved from `permissions.ts` to `types.ts`, with a `roleAtLeast` beside it.
`permissions.ts` reaches the database and cannot be imported into a browser bundle, and the control
that issues an award must not be drawn for somebody the API will refuse — a control rendered for
someone who cannot use it reads as a permission they have. Two copies of the ordering is the same
hazard the single membership roster removed, pointed at the interface: the layer deciding what to
*show* and the layer deciding what to *allow* disagreeing about who is a maintainer.

### §15 reached the page a buyer reads

`certificationFor` had existed since §57 and **nothing rendered it**, so §15's argument for its whole
programme — *a certificate nobody can inspect is a document rather than a measurement* — was true of
an API response and of nothing a person could see.

`CertificationPanel` sits immediately above the §50 attestation panel on `/skynet/robots/{slug}`,
because the two answer different questions about one machine: what it is, and what it has been
measured to do. A page carrying only the second would let a vendor's signature stand in for a bench
result.

**Both verdicts are shown even when they agree.** The recomputation is the feature; a badge that
appeared only on a disagreement would make agreement look like the absence of one.

**The divergence is chosen by kind, not printed from the server.** `certificationFor` writes the
sentence in English for API clients, and now returns `divergence_kind` — `agreed`, `unissued`,
`lapsed`, `downgraded` — produced by the same decision that writes the prose. The page picks a
translated sentence from the kind, so it cannot render a different disagreement from the one the API
reports, and it renders it in the reader's language.

**Never-measured and measured-as-perfect stay apart** in the panel, because `awardCertification`
reads the first as "no fleet data exists yet" and the second as a perfect score, and a panel showing
`—` for both would be the one substitution §15 cannot survive.

### Verification

* `npm run typecheck` — clean.
* `npm test` — **1436 passed**, 53 files. Four new refusal tests on `issueCertification` and the two
  rewritten webhook vocabulary checks.
* `npm run build` — succeeds, so the server/client boundary on the new panel holds.
* A read-only probe of `certificationFor` against the live database for three registry robots
  returns `divergence_kind: agreed`, `live.awarded: none`, and the field note explaining that no
  telemetry window has ever arrived. Before this session's migrations the same call raised PGRST205.
* `node scripts/i18n-catalog.mjs audit` — OK for all 34 locales after `sync`.

### ⚠ Two things left standing, deliberately

**The 25 new locale values are English.** `sync` fills a new key with the English source and
`translate-with-openai.mjs` replaces it. A dry run for one locale projects **2,013 keys** needing
translation, which is a pre-existing backlog rather than anything this session added — running it
across 34 locales is a long, paid, 68,000-key job with a diff touching almost nothing to do with
Skynet. It is the owner's call, not a session's, and the keys fall back to English until then.

**`eslint` does not run in this repository.** `typescript-eslint` refuses TS 7.0 and the command
fails before reading a single file. Pre-existing and unrelated to this session; recorded because
"lint passed" is not something that can be said here today.

### Still not built

| Item | Why not |
|---|---|
| An org-namespace switcher on the webhooks page | Unchanged. `/skynet/settings/webhooks` manages the signed-in user's own namespace. |
| A robot that produces a run bundle | Unchanged. The Hub classifies one and no `skynetd` edition emits one. |
| A signed conformance result | Unchanged, and now the sharpest gap by some distance: an award can be *issued* from evidence that is still asserted rather than attested, and the issue path makes that asymmetry load-bearing. |
| The sealed black box itself | Unchanged. |
| A field-conformance figure that is not null | Unchanged. `cycles` is one of six counters nothing on the current OS edition measures. |
| Episodes | Unchanged. |
| `/skynet/os/editions` | Unchanged. Still one edition. |

---

## Session — 2026-09-02 (eleventh, continued): the sharpest gap, from the Hub's side

The entry above ended by calling this the sharpest gap on the platform, and it is worth quoting the
architecture's own audit rather than paraphrasing it. §57's eleven endpoints are listed there with a
column marking each as evidence or assertion, and one row reads:

| `POST /devices/{id}/conformance` | **Assertion.** §57 calls these signed and no runner on any robot signs one. |

with two sentences under the table:

> Conformance is asserted and certification is computed from it, which means §12's gate — the one
> deciding whether an OS update may change the safety kernel — currently reads evidence nobody
> signed.

The gap has two halves and only one of them is on this side. **The Hub half was that nothing could
have checked a signature even if a robot had sent one**: the route did not read the fields,
`submitConformance` accepted `signature` and `signingKey` from a caller that never passed them, and
`verified` was the literal `false` on every row. That half is closed.

### The key that may sign a run is the machine's, and only the machine's

`decideSctsSignature` is shaped like `decideBundleSignature` in `provenance.ts` rather than like
`decideSignature` in `attestations.ts`, and the difference is the whole decision:

* a **birth certificate** may be signed by any key registered to the vendor's namespace, because it
  is the vendor's statement about a machine they made;
* a **conformance result** is a statement about *a run*, and the only party who can attest that a
  run happened is the machine it happened on.

So the one accepted key is `skynet_devices.device_key` — the key §50's certificate binds to this
robot. A bench result signed by an operator's laptop reaches `untrusted`, not `verified`, and the
sentence the route returns for it says why in the terms a vendor will otherwise misread: the
attestation route's `untrusted` means *register the key*, and doing that here would still not make
it evidence that a run happened.

### Its own domain, because a signature that verifies twice is transferable

`skynet.scts.v1`, beside `skynet.birth.v1` and `skynet.bundle.v1`. A shared domain would mean a
robot signing a §56 run bundle is simultaneously signing a §15 conformance result — which hands back
exactly the self-production §56 exists to prevent. Pinned in a test rather than left as a constant
somebody could tidy.

The digest is over the submission **as sent**, before the trimming, truncation and defaulting that
builds the rows. A digest taken afterwards would be over this Hub's reading of the run, and a robot
that signed the run would fail against it. `canonical` is what makes two submissions differing only
in key order the same submission, and there is a test for that too.

### A state, not a boolean — the fifth time this argument has been made and the reason it holds

`20260902300000_skynet_conformance_signature.sql` adds `signature_state` and `digest`.
`skynet_attestations.signature_state` already carries the same five states with the reason on the
column: *a publisher who did not sign, a key nobody registered, bytes that are not the ones signed,
and a platform that could not check are four different remedies.* Collapsed into `verified = false`
they are one shrug, and the pair that matters most here is `failed` against `absent` — a robot that
never signed is a fleet on an older edition, and a run whose signature does not verify over its own
numbers is somebody editing results.

`verified` stays and stays a boolean, because that is what other code reads. It is now derived from
the state rather than pinned to false: the same fact, reduced to the one bit.

The column default is `absent`, and every existing row keeps it. `digest` is **not** backfilled: a
digest computed here would be over a reconstruction rather than over the bytes a robot sent, and a
signature checked against it would be checking the wrong document.

### What was deliberately not changed

`awardCertification` still counts an unsigned result exactly as it counts a signed one. Twice
deliberate. It is a **mirrored contract** — `lib/skynet/os/mirror-digests.json` pins its sha256 and
the robot runs the same function — so this side does not get to change the rule on its own. And §56
already settled what to do instead: record the class, publish it beside the number, let the reader
see what the award rests on.

So `GET /certifications/{slug}` now reports `attested`, `considered` and the five-state breakdown
over **the results the verdict was actually computed from** — the newest per test, not everything
ever filed. A robot with one signed result and four hundred unsigned re-runs of it should not read
as 0.2% attested.

On the page: *"None of the 36 results behind this verdict were signed, so the award rests on
asserted evidence."* A `failed` or `untrusted` count is listed separately and never folded into that
line, because a failing signature is not a quieter version of an unsigned one.

### `device_key` moved onto the credential

`resolveDevice` selected six columns and not that one, so every route that wanted it would have had
to fetch it again. It is on the credential now — no path can forget it and silently fall back to
accepting any signer, which is the failure this whole change exists to make impossible.

### Verification

* `node scripts/skynet-verify-migrations.mjs` — **10 applied**, exit 0.
* `node scripts/skynet-verify-rls.mjs` — all invariants hold.
* `npm run typecheck` — clean. `npm test` — **1445 passed** (nine new signature tests, including a
  real Ed25519 key pair reaching `verified`, a second key reaching `untrusted`, and a signature made
  over a *failing* run presented against the passing one reaching `failed`).
* `npm run build` — compiled, 16,167 static pages generated.

### Still not built

| Item | Why not |
|---|---|
| **A robot that signs a conformance result** | The receiving half is complete and no `skynetd` edition produces a signature, so every submission that arrives today reaches `absent`. This is now the same shape as §56's run bundle: the Hub is ready and the producer is not. |
| An org-namespace switcher on the webhooks page | Unchanged. |
| A robot that produces a run bundle | Unchanged. |
| The sealed black box itself | Unchanged. |
| A field-conformance figure that is not null | Unchanged. |
| Episodes | Unchanged. |
| `/skynet/os/editions` | Unchanged. |

---

## Session — 2026-09-02 (eleventh, third): the switcher, and §32.4 item 5

Two items, and the second one had been waiting on a table for as long as the table did not exist.

### The webhooks page can point at an organisation

The gap as the previous entry stated it: *"the API is namespace-scoped and works for an org today;
the page has no control to point at one."* An organisation's endpoints were reachable by `curl` and
by nothing else.

**Only namespaces the caller owns are offered.** The webhook routes require `namespace.admin` and
`owner` is the role that satisfies it, so listing anything else would offer a choice that 403s —
the same rule the certification control follows, and it uses the same `roleAtLeast` from `types.ts`
for it. One ordering, two layers, no browser opinion about who is a maintainer.

**With one option there is no control**, because a switcher that cannot switch is furniture. Most
accounts see the page exactly as it was.

**The default is the user's own namespace, not the first alphabetically.** Somebody arriving from
the settings nav is nearly always looking at their own, and a page that opened on an organisation's
endpoint list because of a sort order is a surprise on a screen whose entire content is a map of
where a namespace's automation lives.

Switching clears the open delivery list, the create-result notice and the endpoint list before
loading the new one. Leaving a delivery list on screen while the endpoints under it change would
attribute one namespace's attempts to another, which is the one thing a delivery log must never do.

### `/skynet/os/certification/[slug]` — §32.4 item 5

§32.1 has specified this page since the architecture was written:

| `/skynet/os/certification/[slug]` | Award, the evidence behind it, expiry, live field conformance | `skynet_certifications` |

and §32.4 put it in item 5, *"these follow their tables and their first real device, and until then
they would be showing invented state."* The table exists now, something writes it, and the evidence
rows have a route that fills them — so the page shows the platform's real state, which for every
robot in the registry today is *nobody has measured this machine*. That is the state §15 says is
worth publishing.

**Every test is a row, including the thirty-two nobody ran.** `certificationDetailFor` builds the
list from `SCTS_TESTS` and joins results onto it, never the other way round. A page rendering the
rows that exist would show a robot with four passes as four green lines — and §15's first rule is
that the unmeasured ones are the finding. Absence has to be a row for absence to be visible; a blank
cell reads like a pass, so an unmeasured required test renders as a warned badge saying so.

**Tests the claimed tier does not require are still shown**, marked. A vendor who ran a
certified-tier test while claiming `host` produced a real measurement, and hiding evidence because
of a claim is the wrong direction for a page whose purpose is inspection.

**The signature state is a column.** The work in the previous entry is what makes it worth having:
a reader can see, per test, whether the number was signed by the machine that produced it.

The page renders the same `CertificationPanel` the robot page does, above the table — one component,
so the summary and the detail cannot disagree — with a `linkToEvidence` flag that is on for the
robot page and off here, where it would link to itself.

### One thing about the noindex rule, so it is not mistaken for working

`next.config.ts` gained `noindex, follow` for `/:locale/skynet/os/certification/:path*` — `follow`
rather than the fleet pages' `nofollow`, because this page is deliberately public and its links go
to the robot it is about; it is kept out of the index only so it does not compete with
`/skynet/robots/{slug}` for the machine's name.

**It has no effect today**, and the config says why in a comment written several sessions ago: a
catch-all `/:locale/skynet/:path*` block noindexes the entire Skynet surface until launch, and
*"the per-surface Skynet rules above still need the same ordering fix to have any effect."* Verified
by fetching the page and reading the header back: `noindex, nofollow, max-image-preview:large`, the
catch-all's value. The rule is in place for when that block is deleted, and it is recorded here as
dormant rather than as done.

### Verification

* `npm run typecheck` — clean. `npm test` — **1447 passed**, 53 files.
* `npm run build` — compiled; the route registers as `ƒ /[locale]/skynet/os/certification/[slug]`.
* **Rendered against the live database**, not only built: `next start` on a spare port, then
  `GET /en/skynet/os/certification/unitree-g1/` → 200, and the HTML contains
  *"Conformance suite — 0 of 36 required tests measured, 36 in the suite"*, nine area sections, and
  45 table rows (36 tests plus nine headers). `GET /en/skynet/robots/unitree-g1/` → 200 with the
  *"See every test behind this award"* link present.
* `node scripts/i18n-catalog.mjs audit` — all 34 locale catalogs hold 5,566 populated source keys.

### Still not built

Every remaining item is on the robot, or waits on something that is:

| Item | Why not |
|---|---|
| A robot that signs a conformance result | The Hub half is complete; no `skynetd` edition signs one. |
| A robot that produces a run bundle | Same shape, for §56. |
| The sealed black box itself | Needs the artifact path and a size budget nobody has set. |
| A field-conformance figure that is not null | `cycles` is one of six counters nothing on the current OS edition measures. |
| Episodes | `Class::Episode` is still the only queue class with a route and no producer. |
| `/skynet/os/editions` | Still one edition. A page listing the matrix would list editions that do not exist. |

---

## Session — 2026-09-02 (eleventh, fourth): the award reaches the CLI, and one blocker worth naming loudly

### `skynet certification`

§18 makes the CLI an ordinary consumer of the same public REST surface, and the award had no
command. It has one now, with the three actions the API has.

**Reading needs no credential**, and it is in the CLI's public list beside `search` and `verify`
rather than behind the token check. Same argument as the page: §15's evidence is public because a
certificate nobody can inspect is a document. Issuing and withdrawing are not public, and the list
does not have to know which flags were passed — the API refuses them itself, and the CLI prints the
server's sentence.

**`--issue` takes only a claimed tier.** No flag for the awarded tier, the expiry or the reasons, so
the command cannot express an award the evidence does not support. `unchanged` is printed rather
than `issued` when nothing was written, because a provisioning line running this after every bench
should be able to tell the two apart.

**`unmeasured` prints the test names, not a count.** §15's first rule is that a required test with
no result is never a pass, and the names are the work between a robot and the tier it is claiming.

Verified against a running server rather than only typechecked:

```
$ skynet certification unitree-g1
Unitree G1 (unitree-g1)
award            (none issued)
evidence supports none

results          0 (0 from the bench)
bench age        (no bench result)
field conformance (never measured)
attested         0 of 0 results signed by the machine that ran them

unmeasured: rhal.abi.version, rhal.state.completeness, rhal.command.rejection,
            embodiment.descriptor.valid, safety.kernel.parity, safety.policy.isolation,
            package.signature.required, package.grants.gate, offline.control.independent,
            security.credential.scope
```

Ten names rather than thirty-six, because the award walk stops at the first tier it cannot satisfy
and those ten are the guest tier's. That is the useful list: nothing above guest is worth measuring
until they pass.

`--issue` without a token prints `Error (401 unauthorized)` and an unknown slug prints
`Error (404 not_found)`, both from the server.

### ⚠ `SKYNET_SECRETS_KEY` is unset, and it now blocks three features that have their tables

This has been in *Next up* since the fifth session and it has become more expensive than it was.
The nine migrations applied at the start of this session created the storage for three features
that **still cannot run**, all for the same missing environment variable:

| Feature | Where it fails | What the code does |
|---|---|---|
| Device pairing (§10) | `lib/skynet/fleet.ts:94` | *"Pairing is not configured on this deployment: SKYNET_SECRETS_KEY is unset"* |
| Namespace secrets (§23) | `lib/skynet/secrets.ts:88` | Refuses to store |
| Webhook registration (§19) | `lib/skynet/webhooks.ts:373` | Refuses to create — the signing secret is encrypted under it |

All three fail **loudly**, which is the designed behaviour: a platform that quietly degrades when
misconfigured is worse than one with no feature at all, because it looks like it has one. So this is
not a bug to fix in code. It is one variable, in two places.

**It was deliberately not generated by this session**, and the reason is specific rather than
cautious. `.env.local` in this repository points at the **production** Supabase project. A key
generated here would encrypt rows in the production database that the production deployment — which
has no key — could not decrypt. The right move is one key, set in both the local file and the
hosting environment at the same time, by whoever owns both:

```
openssl rand -base64 32
```

Nothing is encrypted under it today, so there is no migration and no rotation problem — only the
narrow window in which setting it in one place and not the other would create rows the other cannot
read.

### Verification

* `npm run typecheck` — clean. `npm test` — **1447 passed**.
* `npx vitest run lib/__tests__/skynet-os-mirrors.test.ts` — **19 passed**. Worth stating
  explicitly: this session changed nothing under `lib/skynet/os/`, and `git diff` on
  `os/conformance.ts` is empty. Every §15 rule still comes from the contract the robot runs.
* The CLI exercised against a live server on a spare port, output above.

---

## Session — 2026-09-02 (eleventh, fifth): the SDK, and a package that would have published blank

§18 lists three developer surfaces — CLI, Python SDK, API — and the award had reached two of them.

### `Robot.certification()`

```python
from skynet import Robot

cert = Robot("unitree-g1").certification()
cert.raise_for_tier("host")     # raises Uncertified if the evidence does not reach it
```

**`raise_for_tier` checks `supports`, not `awarded`**, and that is the decision in the whole class.
An award whose bench evidence lapsed last month is still on the certificate, so a boot script gating
on the issued tier would be reading a document rather than a measurement — which is the exact
failure §15 designed the live recomputation to prevent. The SDK exposes both fields and gates on the
one that is a measurement.

**`Uncertified` is its own exception**, beside `Incompatible` rather than folded into it. A policy
that will not run on a robot and a robot nobody has measured are different refusals with different
remedies, and a caller catching one should not silently catch the other.

**The certification is not cached on the instance**, unlike `name`, `dof` and the rest. Those
describe hardware and do not change while a script runs; this is recomputed by the platform on every
read, and a cached copy inside a long-running process would be the stored-tier design again, one
layer out.

`issue_certification(claimed)` sends only the claim, for the reason the route and the CLI both have:
the award cannot be expressed above the evidence. `withdraw_certification()` supersedes.

Exercised against a running server rather than only imported:

```
unitree-g1: awarded (none issued), evidence supports none
unmeasured: rhal.abi.version, rhal.state.completeness, … security.credential.scope

guest refused -> uncertified | [422 uncertified] guest: 0 failing, 10 unmeasured.
issue without token -> unauthorized
```

### The SDK declared a README that did not exist

`pyproject.toml` has carried `readme = "README.md"` since it was written and `sdk/python/README.md`
was never created. `python -m build` says so and continues:

```
SetuptoolsWarning: File '…/sdk/python/README.md' cannot be found
warning: sdist: standard file not found: should have one of README, README.rst, README.txt, README.md
```

A warning rather than an error, which is why it survived — the package builds, and publishes with an
**empty long description**, so the PyPI project page for the SDK would have been blank. For a
platform whose argument is that developers should be able to reach it in one line, the page a
developer lands on being empty is not a small defect.

Written, and checked line by line against the source rather than from memory — which caught two
descriptions that were wrong before anybody read them. `UnsafeArtifact` is not "an artifact the scan
refuses"; it is weights that execute code on load with no opt-in. `EnvelopeError` is not "the
envelope could not be resolved"; it is a policy declaring commands the robot is not rated to
execute. Both now say what the docstrings say.

`python -m build --sdist` then succeeds with no warnings, and the build artifacts
(`skynet_sdk.egg-info/`, the `.pyc` files) were removed rather than left in the tree.

### One thing put back

Getting a clean import meant deleting `sdk/python/skynet/__pycache__/`, which turns out to be
**tracked in git** — four `.cpython-314.pyc` files. Restored with `git checkout`, because removing
them is a decision about the repository rather than part of this work. It is worth someone deciding:
committed bytecode goes stale silently, and these were compiled from the pre-session sources.

### Verification

* `npm run typecheck` — clean. `npm test` — 1447 passed. (No TypeScript changed here; run because
  the SDK and the API share a contract and a mismatch would show as neither.)
* `python -c "import skynet"` — imports, `Certification` and `Uncertified` both exported.
* `Certification.raise_for_tier` unit-checked against a synthetic payload, then against the live API.
* `python -m build --sdist --no-isolation` — succeeds, no warnings.

---

## Session — 2026-09-02 (eleventh, sixth): the docs table said the platform had an endpoint it removed

`/skynet/docs` opens with a claim about itself:

> Kept in the product rather than in a separate docs site so the endpoint list, the capability
> vocabulary and the simulator registry are rendered from the same constants the platform runs on
> and cannot drift out of date.

Two of the three are true. The simulator registry is read from the database and the capability
vocabulary is imported. **`API_ENDPOINTS` is a literal array**, and it had drifted in both possible
directions.

### It was missing the entire §57 surface

Twenty-one endpoints — pairing codes, pair, fleet, desired state (GET and PUT), check-in, state,
incidents, telemetry, calibration, conformance, certification, attestations, OS releases, webhooks —
built over the last several sessions, shipped, and appearing in the platform's own API table
nowhere. A developer reading the documentation would have concluded Skynet had no robot-facing API.

Added, with the methods **read off the route files** rather than remembered: `desired` is GET and
PUT, `incidents` and `calibration` are POST and GET, `certifications` is GET, POST and DELETE.

### And it advertised one that was deliberately removed

The new test found this on its first run, which is the whole reason to write it:

```
FAIL  documents no endpoint the platform does not serve
+ [ "POST /api/skynet/v1/namespaces/{handle}/members" ]
```

The route exports `GET` only, and the file says why:

> There is deliberately no POST or DELETE here. A second place to edit membership is what let the
> two lists disagree, and the copy that governed Skynet access was the one nobody maintained.

The docs row said *"Add a member to a namespace, or change the role they hold."* So the table was
advertising the endpoint the single-roster migration removed **for a security reason**, and telling
developers to call it. Six sessions of that. Fixed to `GET`, with a description that says the roster
is read-only and where membership is actually edited.

### The check, and its deliberate limit

`lib/__tests__/skynet-api-docs.test.ts` walks `app/api/skynet/v1` and holds every documented row to
a route file exporting that method. Placeholder spelling is normalised — the docs write `{ns}` and
`{v}` where the folders are `[handle]` and `[version]`, which is a readability choice rather than
drift — so the comparison is over the shape of the path.

**It does not require the reverse**, and that is a decision rather than an oversight. The table is a
selected surface, not a reference: `POST /repos/{ns}/{name}/star` does not need a row, and a test
demanding exhaustiveness would be satisfied by padding the table rather than by documenting
anything. What it catches is the failure that survives longest — a row advertising something the
platform does not serve, which keeps rendering perfectly.

Two smaller assertions carry their own weight. One fails if either regex matches nothing, because a
comparison between two empty sets passes every other check in the file. One names the twelve §57
paths explicitly, because their absence was invisible to a general rule that nothing was violating.

And the page's own header comment now says which of its three lists is generated and which is not,
rather than claiming all three are.

### Verification

* `npm run typecheck` — clean.
* `npm test` — **1452 passed**, 54 files (the new one is five tests).
* `npm run build` — compiled, 16,167 pages.
* `node scripts/i18n-catalog.mjs audit` — OK; 5,587 populated source keys across 34 locales. The
  members description changing invalidated its 34 translations, which `sync` refilled from English.

---

## Session — 2026-09-02 (eleventh, seventh): the write path, run for real

Everything above was verified in the two ways that do not touch Postgres — the typechecker and 1,452
unit tests — plus a read against the live database. **Nothing had ever executed
`issueCertification`.** The columns were confirmed to exist and the code was confirmed to compile,
and those are not the same claim as the code being able to write to them.

The failures that live in exactly that gap: a CHECK constraint the library violates, a column name
differing by an underscore, a partial unique index the supersede order collides with. None is
visible to a typechecker; none is visible to a test that stops at the module boundary.

### `scripts/skynet-verify-certification.mjs`

Built on `skynet-verify-rls.mjs`'s discipline — `zz-cert-probe` fixtures, a `finally` that removes
them including on failure — and on its principle: it calls the **real library functions**, never
inserts the rows it then asserts. A probe that writes its own rows tests the probe.

Thirty-nine checks, in the order the feature is actually used:

| Stage | What it pins |
|---|---|
| An unmeasured robot | no certificate, no tier, `agreed` rather than a divergence, and issuing is **refused with the missing tests named** |
| A passing guest run | every result accepted, `signature_state: absent`, a digest computed |
| The same run again | nothing accepted, all reported as duplicates, the same digest — re-sending is not re-running |
| Issuing | awards `guest`, records the claim, and the read agrees with the write |
| Issuing again | writes nothing and returns the standing award |
| Claiming `certified` | **awards `guest` anyway**, records the claim, supersedes, says why — and exactly one award still stands |
| Filing a `fail` for a required test | live verdict drops to `none`, the issued award is untouched, `divergence_kind` becomes `downgraded`, the failing test is named |
| Withdrawing | no award stands, **both records are kept**, and a second withdrawal is refused rather than silent |

All thirty-nine pass. `The award holds end to end.`

The most valuable rows are the ones about a claim above the evidence and the one about a later
failure. Together they are §15's whole argument executing against real storage: the claim was
`certified`, the row says `guest`, and when a required test later failed the certificate stayed
exactly as issued while the live verdict moved underneath it. That is what "a certificate is not a
column" means when it is a row in Postgres rather than a sentence in a plan.

### One line in it worth the comment it has

The probe reuses the profile's own namespace when it has one, because `skynet_namespaces` has an
owner-exclusive constraint and a user namespace is unique per profile. That means the namespace is
usually **a real person's**, so the cleanup deletes audit rows by `target_id` — the probe robot —
and never by `namespace_id`. Deleting somebody's audit history to tidy up after a probe would be the
worst bug in the file, and it is one character away.

### Verification of the verification

After the run, asked directly rather than trusted:

```sql
select 'robots', count(*) from skynet_robots where slug like 'zz-cert-probe%'
  union all select 'devices',        count(*) …
  union all select 'namespaces',     count(*) …
  union all select 'certifications', count(*) from skynet_certifications
  union all select 'conformance',    count(*) from skynet_conformance_results;
```

All zero — both tables back to empty, no fixture left behind. And no `certification.award`,
`certification.withdraw` or `conformance.report` row survives in the audit log.

### And the pages, rendered against a robot that has evidence

Everything rendered so far had been the empty state, which is the state every robot in the registry
is actually in — and it is not the state the table was written for. So the same fixture was built
again, a `512.5 Hz` bench result filed alongside the passing guest run, an award issued, and both
pages fetched from a running server before the rows were removed.

```
certification page
  ok    renders
  ok    names the award                                    (Awarded guest)
  ok    shows the live verdict beside it                   (Evidence supports guest)
  ok    counts the measured required tests                 (10 of 10)
  ok    renders the quantitative value with its unit       (512.5 Hz)
  ok    says the results were not signed
  ok    marks unmeasured tests
  ok    shows the absent signature state

robot page panel
  ok    renders
  ok    carries the award
  ok    links to the evidence
```

*10 of 10*, not *10 of 36*: the claim is `guest`, so ten tests are what the verdict turns on, and
the other twenty-six are listed as **not required at the claimed tier** rather than counted as
missing. That distinction only appears when a certificate exists to carry a claim, which is why it
had not been seen until now.

One incidental finding from building the fixture: **there are no namespaces in the production
database at all**. `select kind, count(*) from skynet_namespaces group by kind` returns nothing.
Skynet has never been used by anybody — which is consistent with shadow mode, and worth writing down
because it means every "no rows" result in this session is the platform's real state rather than a
query that missed.

---

## Session — 2026-09-02 (eleventh, eighth): a signature that arrives after the run

Found by reading this session's own code back rather than by anything failing, and it is the kind of
defect that only exists once both halves are built.

### The response said `verified` and the table said `absent`

`submitConformance` upserts with `ignoreDuplicates` on `(robot_id, suite, test_id, ran_at)`, which
is right for a **measurement**: re-sending a run is idempotent, re-*running* a test is not, and
nothing may quietly replace a failure.

It is wrong for an **attestation**. The realistic path to signed conformance is a fleet that starts
signing after it has already reported — the OS edition ships, the robots re-send what they measured,
now with a signature. Every row would be dropped as a duplicate, nothing would be stored as
verified, and `submitConformance` would return `signature_state: 'verified'` anyway. The response and
the table would disagree, and **the table is what §12's gate reads**.

### The digest is what makes the fix sound rather than convenient

The digest is over `{suite, results}` and the signature is *not inside that object*. So an identical
re-send produces the identical digest — the signature is over exactly the bytes already on file.
Attaching it to those rows is recording a fact about them, not replacing them. The probe pins this
directly: `the digest is unchanged by signing`.

So when nothing was accepted and the signature verified, the rows already on file are updated with
the signature, the key, the digest and `verified: true`. `outcome`, `value` and `ran_at` are never
touched.

**Upgrade only, and only to `verified`.** Three refusals in one `.neq('signature_state', 'verified')`
and one `signatureState === 'verified'` guard:

* a later **unsigned** re-send must not strip an attestation;
* a **failed** or **untrusted** re-send must not overwrite a good one;
* neither must be able to launder a signed result back into an unsigned one, which is what somebody
  holding a lost device credential would try.

`upgraded` is on the response and in the audit metadata, because "nothing was accepted" and "nothing
was accepted and forty results are now attested" are different events.

### One smaller thing, fixed while there

`withdrawCertification` called `new Date()` twice — once for the row and once for the value it
returned — so a caller got a timestamp a few milliseconds off the one stored. Small, and in a table
whose entire purpose is recording when a decision was taken.

### The probe grew the case, and the case passes

`scripts/skynet-verify-certification.mjs` now generates a real Ed25519 pair, writes the public half
to the probe device's `device_key`, signs the digest of the run already on file, and re-submits:

```
re-sending it signed, from the device that ran it
  ok    the digest is unchanged by signing
  ok    the signature verifies
  ok    nothing new was accepted
  ok    the rows on file were upgraded
  ok    and every one of them now reads verified

re-sending it unsigned again
  ok    upgrades nothing
  ok    and the attestation survives
```

And the award section changed with it. `all of it attested, now that the run is signed` replaces
`none of it attested` — which means **the §15 chain has now run end to end with attested evidence**,
against the real schema, rather than only with the `absent` state every submission reaches today.

47 checks, all passing. Afterwards `skynet_robots`, `skynet_namespaces`, `skynet_devices`,
`skynet_certifications` and `skynet_conformance_results` are all back to zero rows.

### Verification

* `npm run typecheck` — clean. `npm test` — 1452 passed.
* `node scripts/skynet-verify-migrations.mjs` — 10 applied, 0 partial, 0 not applied.
* `npx tsx scripts/skynet-verify-certification.mjs` — 47 checks, `The award holds end to end.`

---

## Session — 2026-09-02 (eleventh, ninth): the badge, and the one way to draw it that is never wrong

§15's argument is about a buyer comparing two robots, and until now the comparison was three clicks
deep: the grid at `/skynet/robots` said nothing about certification at all.

### The obvious implementation is the one §15 exists to forbid

A card showing `skynet_certifications.awarded` is a **stored tier**, and the module header has said
what that is since it was written: right on the day it is written and quietly wrong afterwards. Bench
evidence lapses after a year; field conformance below the floor suspends an award. A grid is
precisely where that goes wrong thirty-six times at once.

A card showing the *live* verdict instead is wrong in the other direction: it would put a tier on a
robot nobody has awarded anything, which is the platform making the award.

**So the badge is the lower of the two**, which is §15's own rule — *an award is capped by its
evidence, so the lower of the two is what describes the machine* — and it is the only reading that
cannot overstate. When they disagree the tone and the tooltip say so rather than downgrading
silently, because "this robot was certified and no longer is" is the more useful of the two facts.

And when the cap lands on `none` there is no badge at all. A suspended award has no tier left to
show, and `none certified` would be a badge that reads like one.

### Three queries for the whole grid

`certificationBadges(robotIds)` fetches the standing certificates, the conformance results and the
telemetry windows for every robot on the page in one query each, then runs the mirrored
`awardCertification` in memory per robot. `certificationFor` would have been four round trips per
card; thirty-six cards would have been a hundred and forty-four.

The newest-result-per-test rule and the oldest-bench rule are the same ones `assess` uses, applied
over the batched rows.

### Verified against the case it exists for

A scratch probe built two awarded robots — one whose evidence still supports the award, one whose
evidence moved underneath it — plus a third with passing evidence and no award:

```
  ok    a badge only where a tier survives          (one, not two)
  ok    the holding robot shows its tier
  ok    and is not diverged
  ok    the moved robot has no badge left
  ok    an unawarded robot gets no badge
  ok    the grid renders
  ok    shows the badge
  ok    once, for the one robot that holds it
```

The first assertion was written expecting **two** and the code returned one. The code was right: two
certificates existed and only one still had a tier the evidence supported, which is the whole
behaviour. The expectation was corrected rather than the code.

Both cases are now permanent, folded into `skynet-verify-certification.mjs` at the two points where
the fixture is already in the right state — `the listing badge shows the tier` while the award holds,
and `the listing badge is gone rather than stale` after a required test fails. 51 checks, all
passing, and the production tables are back to four seeded robots and nothing else.

### Verification

* `npm run typecheck` — clean. `npm test` — 1452 passed.
* `npm run build` — compiled, 16,167 pages.
* `npx tsx scripts/skynet-verify-certification.mjs` — 51 checks, `The award holds end to end.`
* `select count(*) from skynet_namespaces` → 0, `skynet_certifications` → 0,
  `skynet_conformance_results` → 0, `skynet_devices` → 0, and `skynet_robots` → the four seeded
  registry entries from 2026-08-28. No probe row survives.

---

## Session — 2026-09-02 (eleventh, tenth): two holes in this session's own code

Found by reading back what was written here rather than by anything failing. Both are the kind that
a passing test suite is silent about, because the tests exercise the path that works.

### An `if` around an authorization check

Both new handlers were written like this:

```ts
if (robot.namespace_id) {
  await requireNamespaceCapability(robot.namespace_id, actor.userId, 'certification.issue')
}
```

The reasoning was that `requireNamespaceCapability` needs a namespace and the library refuses an
unclaimed robot anyway. `issueCertification` does refuse one. **`withdrawCertification` does not** —
it looks up the standing award and supersedes it, and it never asks who owns the robot.

So a robot whose registry entry was unclaimed *after* an award was issued could have that award
withdrawn by **any authenticated caller**. Not reachable today, because unclaiming is not a path
anybody has built — which is exactly the property that would have kept it hidden until somebody
built one.

The shape is the defect, not the reachability. An `if` around an authorization check holds only
while every path behind it refuses on its own, and that is a property of code somebody else will
write later. `POST /robots/{slug}/attestations` already had the right shape and it was there to
copy: refuse the unclaimed robot **first**, then check the capability unconditionally. Both handlers
now do that.

### A second device could attest a run it did not perform

The signature-upgrade path added an entry earlier matched rows on `robot_id`, `suite` and
`test_id` — not on the device. A robot can carry more than one device row, and `signature_state:
verified` is read everywhere as *signed by the machine that ran them*. So a second credential on the
same robot, holding a different key, could sign the digest of a run the first device performed and
have that signature attached to the first device's rows.

The window is narrow — both credentials must be valid and on the same robot — and the claim it
produces is still false. `.eq('device_id', params.device.id)` closes it.

Pinned in the probe with a real second device and a real second key:

```
a second device on the same robot, signing the same run
  ok    the signature itself verifies
  ok    but upgrades no row filed by another device
  ok    the other device's results stay unattested
```

The first line is the point: the cryptography is fine and the *authority* is not, and those are
different checks. A probe that only asserted "the signature failed" would have passed against a
version that rejected the signature for the wrong reason.

### The probe stage that broke the two stages after it

Adding that check filed a second, later, unsigned run — which correctly became the newest result per
test, so the award below it was computed from unsigned rows and two assertions failed:

```
FAIL  and the attestation survives                (expected true, got false)
FAIL  all of it attested, now that the run is signed  (expected 10, got 0)
```

Both were the **fixture**, not the code: newest-per-test is the rule the verdict is supposed to
follow, and it followed it. The stage now deletes the second device's rows when it is done, and the
two assertions about the first device's attestation are scoped to that device rather than to the
robot. Worth recording because the first instinct on seeing those two lines is to go and look at
`assess`, and `assess` was right.

### Verification

* `npm run typecheck` — clean. `npm test` — 1452 passed. `npm run build` — 16,167 pages.
* `npx tsx scripts/skynet-verify-certification.mjs` — 54 checks, `The award holds end to end.`
* Production afterwards: `skynet_namespaces` 0, `skynet_devices` 0, `skynet_certifications` 0,
  `skynet_conformance_results` 0, `skynet_audit_log` 0, `skynet_robots` 4 — the seeded registry.

---

## Session — 2026-09-02 (eleventh, eleventh): the sweep after the bug, and what it did not find

Having found one conditional authorization check, the question is whether it is a pattern. It is
not — and that is worth writing down, because the next session should not have to re-derive it.

### The sweep

Every route file under `app/api/skynet/v1` was split at each exported handler — so a `requireActor`
in `GET` is never credited to the `POST` beside it — and each mutating handler checked for any of
`requireActor`, `resolveActor`, `resolveDevice`, `optionalDevice`, `resolveWorker`.

**67 mutating handlers. Three resolved nothing, and all three are correct:**

* `POST /devices/pair` — *"the only unauthenticated write on the Skynet API, and it has to be:
  obtaining the credential cannot require the credential."* The pairing code is the authentication.
* `POST` and `DELETE /namespaces/{handle}/webhooks/{id}` — a false positive. Both call a shared
  `resolve()` at the top of the file that does `requireActor('admin')` **and**
  `requireNamespaceCapability('namespace.admin')` together, which is the better pattern rather than
  a gap.

Twenty-seven resolve a credential and take no explicit capability check, and that is also right:
a device credential scopes itself to its own robot, `/tokens` is personal, the worker routes carry
worker auth, and `star` and `follow` are meant to be available to anyone signed in.

So the hole fixed in the previous entry was the only one.

### The sweep is now a test, because a negative result expires

`lib/__tests__/skynet-route-auth.test.ts`. Four assertions:

1. **Every mutating handler resolves a caller**, or appears on an `UNAUTHENTICATED` list that
   carries the reason it cannot. One entry today. Widening it takes a deliberate edit to a file
   whose whole subject is why that is a decision.
2. **The list holds no route that no longer exists** — an exemption for a deleted path is an
   exemption waiting to be inherited by a new one at the same path.
3. **No capability check is wrapped in a conditional.** The defect itself, as a rule.
4. A guard that the walk found handlers at all, so a broken traversal cannot make the rest pass
   vacuously.

It checks **authentication and not authorization**, deliberately. Whether a caller may do the thing
depends on what the thing is, and encoding that here would be a second copy of `permissions.ts` that
drifts from the first. What it catches is nobody having asked who was calling.

### Confirmed against the bug it was written for

The `if (robot.namespace_id) { … }` form was put back into the certifications route and the test
failed on exactly it:

```
× never wraps a capability check in a conditional
  expected [ 'POST /certifications/{slug}' ] to deeply equal []
```

Then reverted, and it passes. Three checks written this session now work this way — the migration
verifier's `selfTest`, the webhook vocabulary scan, and this — because a check nobody has watched
fail is a check nobody knows the polarity of.

### Verification

* `npm run typecheck` — clean. `npm test` — **1456 passed**, 55 files.

---

## Session — 2026-09-02 (eleventh, twelfth): Skynet had no e2e coverage at all

`grep -rn skynet e2e/` returned nothing. Thirty-six spec files, a registry, an OS section, a
certification programme and a documented API — and not one line of browser coverage. Everything had
been verified by typecheck, unit tests and hand-fetching a page, which between them never open the
built application.

`e2e/skynet.public.spec.ts`, nineteen tests, all passing against `next start`.

### Deliberately shallow, and the reason is a real limit

A browser test cannot build a fixture and remove it. So these check that a page renders, that an
empty state is the *right* empty state, and that the API answers the shape it documents — and
anything needing evidence on file stays in `skynet-verify-certification.mjs`, which can create a
robot, file conformance results, issue an award and delete all of it.

Splitting it that way keeps both honest. The e2e suite never invents data; the probe never asserts
about pixels.

### The four that are worth more than a smoke test

**`36` rows in the evidence table.** Not "some rows" — the suite has thirty-six tests and the page is
built from the suite rather than from the rows on file, so a robot nobody has measured still gets
thirty-six lines. If that assertion ever reads `0`, the page has quietly become a list of results,
which is §15's first rule inverted.

**`not measured` is visible.** A blank cell reads like a pass.

**An unknown robot is a 404**, not an empty certificate. A certification page that renders for any
slug is a page that can be linked from anywhere as evidence about a machine that does not exist.

**Every public page still carries `noindex`.** Skynet is in shadow mode because of a catch-all block
at the end of `headers()` in `next.config.ts`, and launch means deleting that block deliberately.
This test is what notices if it goes by accident.

### One assertion was wrong and the code was right

```
✗ the OS release list answers, empty or not
  expect(Array.isArray((await response.json()).data)).toBe(true)
```

The route answers `{ releases: [...] }`, not `{ data: [...] }` — the paginated envelope is used
elsewhere on the platform and this route does not use it. Written from the habit rather than from
the route. Corrected to `releases`; nothing in the application changed.

That is the third time this session a first-run failure was the expectation rather than the code —
the badge count, the probe fixture, and now this — which is roughly what should happen when tests
are written against a system rather than alongside it.

### Verification

* `npx playwright test e2e/skynet.public.spec.ts` — **19 passed**.
* `npm test` — 1456 passed. `npm run typecheck` — clean.
* `test-results/` and `playwright-report/` are gitignored; the run left nothing in the tree.

---

## Session — 2026-09-02 (eleventh, thirteenth): the two new events, confirmed to queue

`certification.awarded` and `certification.withdrawn` went into `WEBHOOK_EVENTS` and
`issueCertification` calls `emit`. Nothing had confirmed a row actually lands — and `emit` is
written to **never throw and never return an error**, because a publish that failed on an optional
feature would be a platform that stops working when the optional feature does. So a broken emit
looks exactly like a namespace with no webhooks: zero, silently.

That is the right behaviour and the reason it needed checking by hand.

### The blocker, and the way round it that leaves nothing behind

`createWebhook` refuses without `SKYNET_SECRETS_KEY`, which is unset on this deployment on purpose —
and the reason the previous entries gave for not setting one still holds: `.env.local` points at
production, and a key set only here would encrypt rows the deployed app could not read.

So the probe generates one **in process** with `randomBytes(32)`, never writes it anywhere, and
deletes the webhook row it encrypted before exiting. Nothing survives that a real key would later
fail to decrypt, and the production environment is untouched.

### What it found: nothing wrong, which is the answer wanted

```
the vocabulary
  ok    offers certification.awarded
  ok    offers certification.withdrawn

subscribing
  ok    the endpoint was registered
  ok    subscribed to exactly what was asked

issuing an award
  ok    one delivery is queued
  ok    for the award event
  ok    pending, not sent
  ok    on its first attempt
  ok    carrying the awarded tier
  ok    and the robot it is about
  ok    with a sent_at inside the body

withdrawing it
  ok    a second delivery is queued
  ok    for the withdrawal
```

`one delivery is queued`, not "at least one": a namespace subscribed to both events must get exactly
one row per event, and a fan-out bug would show here as two.

`carrying the awarded tier` and `and the robot it is about` are the payload assertions worth having.
A receiver builds its logic on those fields, and an event that fires with an empty `data` is worse
than one that does not fire — it looks like the platform is reporting nothing happened.

`with a sent_at inside the body` pins a decision from the webhook session: the timestamp is inside
the **signed** body rather than only in a header, so a receiver deduplicating retries has an
identifier a replay cannot change.

### The delivery itself is still unexercised, and honestly so

The rows stay `pending` and are deleted. Actually sending them would mean pointing the platform at a
receiver, and the SSRF rules correctly refuse a private address — so the only way to watch a real
delivery is to send a namespace's private activity to a third-party endpoint, which is not something
to do from a verification script.

The cron that drains them was exercised separately and answers cleanly against the live database:

```
GET /api/cron/skynet/webhooks  →  {"ok":true,"attempted":0,"delivered":0,"failed":0,"refused":0}
GET /api/cron/skynet/jobs      →  {"ok":true,"leases_reaped":0,"uploads_expired":0,…}
```

Worth noting what that means about the state before this session: `drainDeliveries` ignores the
error from its `select`, so with `skynet_webhook_deliveries` absent it returned zeros and the cron
reported `ok: true` every five minutes. Not a 500, not an alert — the same shape of silence as the
migration verifier. The table exists now, so the silence is over, but it is the second instance this
session of a check that could not fail.

### Afterwards

Every Skynet table is back to zero rows except `skynet_robots`, which holds the four seeded registry
entries. `skynet_webhooks` and `skynet_webhook_deliveries` included.

### And the emit check is now permanent

The one-off above proved the events queue; it would not have noticed the next time somebody breaks
them. Folded into `scripts/skynet-verify-certification.mjs` as a stage that subscribes **before**
the award is issued — because the question is whether issuing emits, not whether a delivery row can
be inserted.

The throwaway key is generated **only when the deployment has none**:

```js
const ephemeralSecretsKey = !process.env.SKYNET_SECRETS_KEY
if (ephemeralSecretsKey) process.env.SKYNET_SECRETS_KEY = randomBytes(32).toString('base64')
```

When a real key is configured, that one is used and nothing is generated — a probe that overrode a
real key would write rows the application could not read, which is the same hazard that argues
against setting one locally in the first place. The cleanup line says which happened:

```
  probe rows removed (webhook secret was encrypted under a key that existed only in this run)
```

The probe is now **60 checks** and still ends `The award holds end to end.` Afterwards every Skynet
table is empty except the four seeded registry robots — `skynet_webhooks` and
`skynet_webhook_deliveries` included.

---

## Session — 2026-09-02 (eleventh, fourteenth): §26, where the award reaches a buyer

`RobotDatabaseSkynetLink` is the marketplace → Skynet bridge: a robot's commercial profile links to
its software view, and only when there is one worth linking to. Its rule was

```ts
// Nothing published for this robot yet — a link to an empty page is worse than no link.
if (models.length === 0 && skills.length === 0 && adapters.length === 0) return null
```

which counted models, skills and adapters — everything the platform could publish about a robot when
the component was written. It could not count an award, because nothing could issue one.

**A robot with an award and none of those is not an empty page.** §15 makes the award and the
evidence behind it the most substantial thing this platform can say about a machine, and it is the
thing a person reading a marketplace listing is most likely to want — more than a count of adapters.
So a certification now counts as something to link to, and the tier renders on the bridge itself.

### It uses the capped badge, not the certificate

`certificationBadges`, the same function the registry grid uses, so the tier is the lower of the
award and what the evidence supports right now. **A stale tier on a commercial page is worse than a
stale tier on a technical one**: the marketplace is where somebody decides to spend money on a
machine, and "certified" there is a claim in a different register.

If the evidence stops supporting the award the badge disappears rather than downgrading, and the
bridge falls back to the models-and-skills rule it always had.

### Still invisible today, and correctly

The component's first line is `if (!SKYNET_PUBLIC) return null` — the Robot Database is indexed, and
a link from an indexed page is how a crawler finds a hidden product. So this renders on launch, not
before, which is the same state the `noindex` rule added earlier for the certification page is in.
Both are placed for the moment that block is deleted rather than left to be remembered then.

### Verification

* `npm run typecheck` — clean. `npm test` — 1456 passed. `npm run build` — 16,167 pages.
* `node scripts/i18n-catalog.mjs audit` — 5,594 populated source keys across 34 locales.

---

## Session — 2026-09-02 (eleventh, fifteenth): the bundle has a route, and the location that did not need signing

Written from the OS side. `Skynet/PROGRESS.md` has the robot half; this is what changed here and
why, and the specification gained **§75** for the pair of them.

### The gap was a route that nobody had noticed was missing

`GET /os/releases` has served bundle manifests since the ninth session: version, channel, digest,
compatible, changes — the five fields `admitBundle` takes, deliberately narrow so that no robot
edition starts reading the Hub's extras. `skynet_os_releases` has carried `artifact_id` and
`size_bytes` the whole time.

**Nothing had ever asked for the bytes.** A robot could pull a manifest, verify the publisher's
signature, match the compatibility list, pass §12's bench-pass gate, admit the release — and then
report that the image had to be put on its disk by hand, because there was no endpoint to fetch one
from. §17 lists eight endpoints and §57 lists eleven; not one of them serves an image.

That is the same shape as §74's missing telemetry route and it survived for the same reason: from
the robot's side it looks like a subsystem nobody has written yet, and from the Hub's side it looks
like nothing at all.

```
GET /api/skynet/v1/os/releases/{version}/bundle?arch=
```

### A redirect, not a stream

An OS image is hundreds of megabytes. Pulling that through a serverless function puts a Node process
in the path of every byte, and this platform settled the argument when it built artifact downloads:
the bucket is private, so the API mints a short-lived signed URL and redirects.

The reason it matters more here is `Range`. The robot fetches an image in **bounded slices** — its
reconcile loop is one thread that also serves a local API and pings a watchdog, so a pass that
blocked for a twenty-minute transfer would be a supervisor that had stopped supervising — and it
resumes each slice at a byte offset. Object storage serves ranged requests against these signed URLs
and has since the format scanner was written: `readHead` in `lib/skynet/storage/store.ts` asks for
`bytes=0-N` and accepts a `206`. The confidence for the whole design comes from a function written
for something else entirely.

### Public, like the catalogue, and the argument is stronger here

The party most likely to fetch an image holds a **device credential**, which `requireActor`
deliberately refuses because a device token is not a user. Requiring authentication would mean
either widening that credential or inventing a second one for the least sensitive read on the
platform — and it would protect nothing, because the bytes are checked on the robot against a digest
inside a document a publisher signed.

The robot presents its credential anyway, as it does on every outbound request, and `curl` drops it
at the cross-host redirect to storage. The robot side does not pass `--location-trusted`, which is
the one flag that would hand a device token to a CDN.

### A location is not evidence

The obvious design is a signed manifest carrying a download URL. It reads as the more secure option
— the URL is inside a signature — and it is the weaker one.

The digest is already signed, so a hostile mirror serving the wrong image produces a mismatch and a
deleted file: the location **cannot lie about the content**. What signing it buys is a manifest that
goes stale the day a hostname changes, and a signature over a fact that did not need one. So the
manifest stays at five fields, the robot derives the URL from the endpoint it was paired to, and
`manifestOf` still refuses to hand over the artifact id.

### `bundleSource`, because there are two ways to have no bytes

A decision function rather than four lines in the route, and it exists to keep two answers apart:

| Answer | What it means |
|---|---|
| `no_release` | Nobody published that version for that architecture. |
| `no_bundle` | The version exists, is published, and has **no artifact attached**. |

The second is a state nothing else on this Hub can name. It validates, it lists, `assessRelease`
admits it, and every robot on the channel will hold the manifest, run §12's gate, admit it, and then
wait for an image nobody uploaded — for ever, quietly, with no error anywhere. Collapsing it into a
generic 404 would hide the one failure this route can actually diagnose.

`releaseByVersion` is published-only, so a withdrawn release stops serving bytes: a withdrawal that
left a remembered URL working would not be a withdrawal.

### The bundle filename

`skynet-os-{version}-{arch}.raucb`, passed to the signed URL as its download name, because this name
lands on a robot's disk and in an operator's downloads folder rather than in a storage path.

### Verification

* `npx tsc --noEmit` — clean. `npx vitest run` — **1457 passed** (three new, for `bundleSource`).
* `npx next build` — 16,167 pages, every route registered.
* Two new i18n keys (`skynet_api_error_release_not_found`, `skynet_api_error_release_no_bundle`) plus
  the docs-table key, `node scripts/i18n-catalog.mjs sync` run across all 35 catalogues.
* The endpoint added to §18's documented surface, which `skynet-api-docs.test.ts` holds to the routes
  that exist.
* `node tools/check-mirrors.mjs` and `node tools/check-contract-mirrors.mjs` — matching; the
  specification's §75 is in both repositories.

### What is still one field short

The manifest declares **no size**, and the robot's fetch contract has a branch for one — *a partial
cannot be larger than what it is part of*. This Hub holds `size_bytes` on the row and does not put it
in the manifest, so that branch is unexercised and a robot reports progress as a count of bytes
rather than a fraction unless the server declares a total.

It was left open rather than closed with the server's own number, which would put an untrusted value
into a decision that deletes a file. Closing it is a mirrored contract change — `BundleManifest` in
the Rust edition, in `reference/typescript`, and in `lib/skynet/os/ota.ts` — plus regenerated corpus
vectors, and it changes the canonical bytes of every manifest, which a robot holding the old one
will correctly refuse as *"the Hub is serving a different manifest for release X"*. Cheap today, when
no robot holds one; not cheap after a fleet exists.

---

## Session — 2026-09-02 (eleventh, sixteenth): the episode both halves had built and never introduced

Written from the OS side, like the one above. `Skynet/PROGRESS.md` has the robot half; the
specification gained **§76** for the rule.

### `POST /episodes` and the robot did not agree about a single field name

Skynet OS emits one episode per skill invocation (§54.4), queues it (§34.4), and sends it to this
route. All of that has worked for five sessions. Nobody had ever compared the document it sends
against the document this route reads.

| The robot sent | This route reads |
|---|---|
| `skill` | `task` |
| `duration_ms` | `duration_seconds` |
| `intervention_count` | `interventions` |
| `invocation`, `digest`, `arguments`, `reason`, `detail`, `root` | — |

`readJson` ignores what it does not know and the route answers `201`, which is the right behaviour
for an ingest endpoint taking documents from agents in the field — and it means every episode would
have been stored with a null task, a null duration, and nothing identifying the skill. Both suites
passed the whole time. One asserted what the robot sends and the other asserted what this Hub
accepts, and neither is a test of the boundary.

The robot now writes this route's field names, and the facts §54.4 asks for that the table has no
column for go into `metadata` — the invocation id, the digest of the bytes that ran, the arguments,
the lifecycle reason. An incident investigation reading these rows needs the digest.

### The default that destroyed an absence

`validateEpisodeInput` did `optionalNumber(params.interventions) ?? 0`, and the column is
`integer default 0 not null`.

Skynet OS **deliberately omits** the intervention count, with a comment in the producer saying why:
this edition has no kernel channel, nothing counts interventions, and *a zero would be consumed by a
marketplace as evidence of a clean run*. Two defaults on this side turned that careful absence into
exactly that number.

§74.3 already states the rule and this Hub was already built to it — for telemetry.
`skynet_telemetry_windows.interventions` is nullable with a `CHECK`, written three sessions ago;
`skynet_episodes.interventions` predates the rule and was written when the only producers were
teleoperation clients that always know the number.

* **`supabase/migrations/20260902320000_skynet_episode_interventions.sql`** drops the default and the
  `not null`, and adds the telemetry table's `CHECK`. Existing rows keep their values: a `0` written
  by a teleop client was a measurement and stays one.
* **`validateEpisodeInput`** preserves the absence. An explicit `0` is still stored as `0`.
* **`summarizeEpisodes`** reports `interventions_measured` beside the total, because a total over
  four hundred episodes of which twelve counted is a smaller number presented as a better one.
* **The dataset collection panel** says *"Not measured"* rather than `0`, and *"3 across 12 of 400
  episodes that counted"* when only some did.

### `recorded_at` — a week of episodes filed at one minute

`normalizeRecordedAt` stamps `new Date()` when the field is absent, and the robot was not sending
one. §34.4's queue exists so that a robot offline for a week keeps its episodes; every one of them
would have arrived stamped with the minute the link came back, on the column whose only job is to
say when something happened.

The robot now stamps it from its own wall clock at the terminal transition. This side needed no
change — the tolerance for a minute of skew and the refusal of a future timestamp were already
right — which is worth recording: the defect was not in the validation, it was that nobody had ever
sent the field.

### The mechanism this Hub gained from it

A finding that stays a sentence gets forgotten, so the session built the thing that enforces it.

`lib/skynet/os/wire/` now holds three documents mirrored from the Skynet OS repository — the
episode, the telemetry window and the incident — and each one is **the exact canonical body a robot
sends**, produced by the real producer over there and frozen by its suite. Nothing in them was typed
by hand.

`lib/__tests__/skynet-wire.test.ts` pushes each through the receiving route's own decision layer:
`validateEpisodeInput`, `assessWindow`, `incidentContext`. Not through a parser written for the
test — through the functions the routes actually call, which is the only version of this test that
proves anything.

`skynet-os-mirrors.test.ts` covers them too. The pin manifest gained `wire/…` entries beside the
contract digests, so a fixture edited here fails in this repository's suite, and a fixture that moved
in the OS repository without a resync fails in that one. `.gitattributes` pins the directory to LF
for the reason the contracts directory already carries: a line ending presenting as a divergence is
how a check gets deleted.

What this buys is the property neither side had: a producer that changes fails over there with the
command that re-freezes it, a route that stops reading a field fails here, and the two land
together. The specification records it as **§31.6**, beside the conformance corpus — the same
mechanism pointed at a boundary the corpus structurally cannot see.

### Verification

* `npx tsc --noEmit` — clean. `npx vitest run` — **1475 passed**: two for the absence — one that
  it survives validation, one that the aggregate carries its denominator — and ten in the new
  `skynet-wire.test.ts`.
* `npx next build` — 16,167 pages.
* Two new i18n keys, synced across all 35 catalogues.
* `node tools/check-mirrors.mjs` and `node tools/check-contract-mirrors.mjs` — matching; §76 and
  §31.6 are in both repositories, and so are the three wire fixtures.

### Not applied

The migration. It is the fourth written and unapplied, and the only one of the four that alters an
existing column — until it runs, an episode that carries no intervention count is refused by the
`not null` constraint rather than stored as the absence it is.

---

## Session — 2026-09-02 (eleventh, seventeenth): the size the manifest was not carrying

One field, and it closes the gap the bundle route was published with.

`skynet_os_releases.size_bytes` has existed since OS releases were built and stopped at the row.
`manifestOf` produced five fields, so a robot fetching an image had no signed total — which cost two
things on the far side:

* **§12's partial check could not run.** A robot refuses to resume from a partial *larger than the
  whole bundle*, which is how a leftover from another image is caught before a transfer rather than
  after one. With no declared size there was nothing to compare against.
* **Progress could not be a fraction.** The only total available was whatever the server serving the
  bytes claimed, and that is not a number a publisher's signature covers.

`manifestOf` now carries `size` when the row has one, and `BundleManifest` gained `size?: number` in
`lib/skynet/os/ota.ts` — synced from the reference, like every other line in that directory.

### Omitted, never zero

`size` is absent from the manifest when `size_bytes` is null, rather than `null` or `0`. A `0` would
be this Hub declaring an **empty image**, and a robot reading it would discard its partial as
larger-than-the-whole on the first pass, for ever. The guard is `Number.isSafeInteger` plus a
non-negative check, so a row with a corrupt value produces no claim rather than a wrong one.

### One consequence, and it is cheap exactly now

The manifest's canonical bytes changed, so a robot already holding the five-field version of a
release refuses the six-field one: *"the Hub is serving a different manifest for release X."* That
is §12's immutability rule working as designed — a release that changes under its own version is
the failure it exists to catch. It costs nothing today because no robot holds a manifest from this
Hub, and after a fleet exists it costs a version number.

### Verification

* `npx tsc --noEmit` — clean. `npx vitest run` — **1476 passed**: the manifest-shape test now
  expects six fields, and a new one asserts the omission.
* `node tools/check-contract-mirrors.mjs` — `ota` resynced and matching in both repositories.
* On the OS side the corpus regenerates byte for byte: `admit_bundle` does not read the size, so an
  optional field changes no expected output.

---

## Session — 2026-09-02 (eleventh, eighteenth): the incident that now names its recording

`skynet_incidents.artifact_id` has been NULL on every row this platform can hold, and until today
that was the whole of what could be said: §13 seals a black box window on a fault, §17 calls the
incident route an upload, and nothing on either side connected a fault to a recording.

The robot half is in `Skynet/PROGRESS.md`. What changed here:

### `incidentRecording`, and the third state

An incident body now carries a `recording` block, and this Hub reads three states apart:

| State | What it means |
|---|---|
| `held` | There is a window: name, size, digest, records, and **nobody has collected it**. |
| `unsealed` | The robot had no ring, or its ring would not open. A fact about the machine. |
| `unsaid` | An older agent, or another producer. **Not** evidence of either of the above. |

The third is the one that would be misread. A page that rendered a missing field as *"no
recording"* would tell an investigator a robot has no black box on the strength of a field an old
agent never sent — which is §32.2's rule (*never show a value it did not receive*) in the shape it
actually bites.

### The device page says what is held, and that it is not here

The incidents table gained a column: the size, with the digest on hover, and *"No recording sealed"*
with the reason when there is none. `artifact_id` is still NULL and the row now explains why rather
than leaving a blank cell that reads as *nothing was recorded*.

### It lives in `fleet-types.ts`, not in `fleet.ts`

`DeviceStateClient` is a `use client` component and `lib/skynet/fleet.ts` reaches for `node:crypto`
and the service-role database client. So the reader went to the client-safe module beside the other
view helpers, and `formatBytes` is spelled a second time there rather than imported from
`lib/skynet/artifacts.ts` — whose two existing callers are both **server** components. Four lines
against pulling a database client into a browser bundle to format a number.

### The wire fixture earned its keep on the first change after it existed

`reference/wire/incident.json` is the frozen incident document, added yesterday. The moment the
robot started attaching a recording, the OS suite failed with the document that had changed and the
command to resync — and the resync put the new shape in front of this repository's tests before
anything shipped. That is exactly the loop §31.6 describes, and it ran on its first opportunity.

### Verification

* `npx tsc --noEmit` — clean. `npx vitest run` — **1478 passed**, two new for the recording.
* Three new i18n keys, synced across all 35 catalogues.
* `node tools/check-contract-mirrors.mjs` — the incident fixture resynced and matching.

### Still not here

The bytes. The honest blocker is on this side rather than the robot's: object storage takes a
single-shot signed PUT, and a two-hundred-megabyte PUT over a warehouse link is the thing §75.2
spends its length on not doing. Resumable upload (TUS), chunk-and-assemble through a worker job, or
a signed session are three designs with materially different costs and infrastructure implications
— an owner's decision rather than a session's.

---

## Session — 2026-09-02 (eleventh, nineteenth): the manifest sweep, and the column every episode was getting wrong

A sweep, prompted by the previous session's finding: **a schema field whose only consumer is a
sentence in a specification is a field to go and check.** Every field in `lib/skynet/manifests.ts`,
read against what the robot actually consumes — by `grep`, not by what either document claims.

### One defect, and it was in this repository's favourite column

`SkillManifest.task` — *"pick the cup off the table"* — has been validated and stored on every skill
card since skills existed, and **no robot had ever read one**. §54.4's episode carried the package
id in `task` instead.

So every episode a fleet produced would have arrived with `acme/pick` in the one column a dataset is
filtered by. Not a null, not an error, and nothing on either side would have flagged it: a wrong
string in a text column looks exactly like a right one. `episodeStats`, the collection panel and any
future training pipeline would all have been reading an identifier where they expected a
description.

The robot now sends the declared task, with the package id as the fallback and the id kept in
`metadata.skill` for grouping. The wire fixture is re-frozen and **this repository's own assertion
had to change** — `expect(stored.task).toBe('acme/pick')` was correct when it was written and wrong
the moment the robot started reading the card. That is the loop §31.6 exists for, and it has now run
in both directions in three sessions.

### One thing that is not a gap and is worth watching

`required_extensions` is consumed here by the **compatibility engine**; the robot consumes the
*package manifest's* `embodiment.extensions`. Two documents from the same publisher, two consumers,
and nothing compares them — so a publisher who fills in one and leaves the other empty gets a pass
from whichever side reads the empty one.

The check belongs here, at publish time, where both documents are in hand: the card is a column and
the package manifest is a file in the version. Left open as a piece of work rather than a line, and
recorded so it is not rediscovered.

### Everything else came back clean

`source_embodiment`, `required_capabilities`, `runtime`, `preconditions`, `success_criteria`,
`expected_duration_seconds` and `composition` are all read on the robot. The model card's prose —
`intended_use`, `limitations`, `architecture`, `parameters` — and every dataset, environment and
benchmark field are correctly Hub-only: none of them is installed on a robot.

### Verification

* `npx vitest run` — **1478 passed**. `npx tsc --noEmit` clean.
* `node tools/check-contract-mirrors.mjs` — the episode fixture resynced and matching.
