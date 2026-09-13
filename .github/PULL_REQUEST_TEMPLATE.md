## What this changes

<!-- One concern per pull request. What it does, and the section of the specification behind it (§). -->

## How it was verified

<!-- What you ran. For a bug fix: the test that fails without this change and passes with it. -->

## Checklist

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets` and `cargo test --workspace` pass
- [ ] A bug fix includes the test that would have caught it
- [ ] New errors say what happened and what to do next
- [ ] No third-party Rust dependency, and no new `unsafe`
- [ ] If behaviour changed: the corpus is regenerated and its diff is part of this pull request
- [ ] The specification is not edited here (open an issue instead — it is shared with the Hub)
