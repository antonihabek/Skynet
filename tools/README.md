# Tools

Scripts that generate and check things. Most of them run on every change in CI; the rules they enforce
are listed in [CONTRIBUTING.md](../CONTRIBUTING.md), with the commands to run before you open a pull
request.

| Tool | What it does |
|---|---|
| `check-*.mjs` | The checks: documents agree with the code, every stage is true, every link resolves, mirrored files match, and so on. Each says what it refuses at the top of the file. |
| `robot-support.mjs` | Writes the robot list in the front page and in `docs/robot-support.md` from `drivers/models.json` and `drivers/robots.json`; `--check` fails if either is stale |
| `scale-report.mjs`, `edition-report.mjs` | The size and status figures the documents quote |
| `run-ci-job.mjs` | Runs one of CI's jobs on your own machine |
| `corpus-gen/` | Generates the shared test cases from the TypeScript reference code |
| `seed-defects/` | The seeded-defect campaigns: put a known defect into a driver and see whether its tests notice |
| `faultproxy/` | A relay that delays, drops and freezes a driver's link, to test how the driver copes |
| `fault-vocab/` | Holds a driver to the faults its maker defines |
| `flake-hunt.sh` | Runs a command many times, quiet and loaded, to find tests that fail now and then |

Run any `.mjs` tool with `node tools/<name>.mjs`; most take `--help` or say what they need when they
fail.
