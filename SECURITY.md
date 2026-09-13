# Security and safety

Skynet OS commands physical machines. A vulnerability here can be a safety problem, not only a data
problem, so please report privately first.

## Reporting

**Use GitHub's private vulnerability reporting:** the **Security** tab of this repository →
**Report a vulnerability**. It opens an advisory only the maintainers can see, and the fix, the
credit and the disclosure are coordinated there.

Please do **not** open a public issue, pull request or discussion for anything below until an
advisory has been published.

## What counts

Anything that breaks one of the guarantees the specification makes, in particular:

* a command that reaches an actuator **without passing the Safety Kernel** at Host or Certified
  tier, or a guard that can be disabled, skipped or starved without being reported (§33, §40);
* a stop that can be delayed or suppressed (§39.1 makes stop unauthenticated on purpose — anything
  that prevents it is in scope);
* authentication or authorisation bypass on the local API or the LAN console (§39);
* a package, OS image or descriptor accepted **without its signature being verified**, or with a
  forged one (§19, §37);
* escape from a package's sandbox, cgroup limits or granted capabilities;
* a black box recording that can be altered, truncated or silenced without detection (§13, §26);
* anything that makes a surface claim a higher tier, or more safety, than the evidence supports
  (§15, §35).

A **Guest-tier** installation is advisory by design: it computes verdicts and enforces nothing,
and every surface says so. "Guest tier does not stop a bad command" is the documented behaviour,
not a vulnerability — but a Guest installation that *claims* to enforce is.

## What to include

What you did, what happened, and what should have happened. For anything involving a running
robot, `skynet doctor --out bundle.json` collects its state into one file — check it before sharing,
since it describes the machine it came from.

## No safety certification

Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity
assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified body
against a specific robot in a specific application. The software is provided under the Apache-2.0
licence, without warranty of any kind.
