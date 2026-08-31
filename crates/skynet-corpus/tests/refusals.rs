//! §64, checked mechanically.
//!
//! > Every refusal in this system names the fix, and this is already how the implemented contracts
//! > behave rather than an aspiration.
//! >
//! > The rule behind them: **a refusal that does not tell you what to change is a bug report the
//! > user has to write themselves.**
//!
//! §64 is the one section of Part VIII that claims something about code that already exists, which
//! makes it the one that can decay without anybody noticing: a message shortened to fit a console,
//! a `detail` left empty on the third arm of a new match, and the sentence above quietly stops
//! being true. It has no test because it is not a feature. This is that test.
//!
//! # What it reads
//!
//! Every refusal **the corpus contains** — which is every refusal any edition is required to
//! produce, since §31.4 makes the corpus the normative artifact. A refusal shape nobody has written
//! a case for is outside this check, and that is the same gap `vocabulary/closed-sets.json` exists
//! to narrow rather than a weakness peculiar to this file.
//!
//! # The rules, and why each is the bar it is
//!
//! Each rule below is a shape from §64's own table. The recurring one is that a message must be a
//! **sentence** — non-empty, and containing a space — because the failure this guards against is
//! not an empty string, which somebody would notice. It is `message: "refused"`: a field that is
//! populated, passes every "is it set" check, and tells the operator exactly what the code beside
//! it already told them.

use std::fs;

use skynet_json::Json;

/// One refusal that names nothing.
struct Violation {
    rule: &'static str,
    case: String,
    value: String,
}

/// Non-empty, and a sentence rather than a token.
///
/// A refusal that restates its own code is the failure mode this looks for. `"refused"` is set,
/// non-empty, and useless; `"The policy's action space cannot be transported onto this body."` is
/// the bar.
fn sentence(value: Option<&Json>) -> bool {
    match value.and_then(Json::as_str) {
        Some(text) => !text.trim().is_empty() && text.trim().contains(' '),
        None => false,
    }
}

fn non_empty_list(value: Option<&Json>) -> bool {
    value
        .and_then(Json::as_arr)
        .is_some_and(|items| !items.is_empty())
}

fn has(node: &Json, key: &str) -> bool {
    node.get(key).is_some()
}

fn check(node: &Json, case: &str, out: &mut Vec<Violation>) {
    let mut fail = |rule: &'static str| {
        out.push(Violation {
            rule,
            case: case.to_string(),
            value: node.to_canonical().unwrap_or_default(),
        });
    };

    // A package finding (§8.2), and everything else that carries a code and a message.
    if has(node, "code") && has(node, "message") {
        if !sentence(node.get("message")) {
            fail("a finding's message is not a sentence");
        }
        if node.get("message").and_then(Json::as_str) == node.get("code").and_then(Json::as_str) {
            fail("a finding's message only restates its code");
        }
    }

    // A frame fault (§44.3) or a cell fault (§55.2): a code, what it is about, and a sentence.
    if has(node, "code") && has(node, "detail") && !sentence(node.get("detail")) {
        fail("a fault's detail is not a sentence");
    }

    // §54.2: a precondition that cannot be evaluated refuses, naming why.
    if node.get("outcome").and_then(Json::as_str) == Some("unevaluable")
        && node.get("reason").and_then(Json::as_str).is_none()
    {
        fail("an unevaluable outcome names no reason");
    }

    // A transport binding (§6.2). Every one carries a reason, including the ones that succeeded —
    // "why is this channel scaled to 0.44" is the same question as "why was it refused".
    if has(node, "method") && has(node, "source_index") && !sentence(node.get("reason")) {
        fail("a transport binding names no reason");
    }

    // A conformance result (§15): the test id, the measured value, and the threshold.
    if has(node, "check")
        && has(node, "required_for")
        && node.get("ok").and_then(Json::as_bool) == Some(false)
        && !sentence(node.get("detail"))
    {
        fail("a failed conformance check names no detail");
    }

    // §7.2: a guard that is unavailable names the exact missing input.
    if has(node, "guard") && has(node, "missing") && !non_empty_list(node.get("missing")) {
        fail("an unavailable guard names no missing input");
    }

    // §45.5: an expired parameter set names itself, its state and the guards it feeds.
    if has(node, "guard") && has(node, "set") && has(node, "state") && !sentence(node.get("detail"))
    {
        fail("a calibration verdict names no detail");
    }

    // §47.2: a held observation stream names which stream and why.
    if has(node, "stream") && has(node, "reason") && !sentence(node.get("detail")) {
        fail("an observation hold names no detail");
    }

    // §15: a refused award names what is missing rather than only that it was refused.
    if has(node, "awarded")
        && node
            .get("awarded")
            .is_some_and(|value| *value == Json::Null)
        && !(non_empty_list(node.get("reasons"))
            || non_empty_list(node.get("missing"))
            || non_empty_list(node.get("failures")))
    {
        fail("a refused certification names nothing");
    }

    // The `{ "fault": … }` wrapper both frame validation and the cell descriptor use.
    if let Some(fault) = node.get("fault") {
        if *fault != Json::Null && !sentence(fault.get("detail")) {
            fail("a fault names no detail");
        }
    }
}

fn walk(node: &Json, case: &str, out: &mut Vec<Violation>) {
    match node {
        Json::Obj(map) => {
            check(node, case, out);
            for value in map.values() {
                walk(value, case, out);
            }
        }
        Json::Arr(items) => {
            for value in items {
                walk(value, case, out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_refusal_names_the_fix() {
    let root = skynet_corpus::default_root();
    assert!(root.is_dir(), "no corpus at {}", root.display());

    let mut violations = Vec::new();
    let mut cases = 0;

    for path in skynet_corpus::corpus_files(&root) {
        let label = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(&path).expect("a corpus file is readable");
        let document = Json::parse(&text).expect("a corpus file parses");
        for case in document.arr_or_empty("cases") {
            let id = case.get("id").and_then(Json::as_str).unwrap_or("<unnamed>");
            let Some(expected) = case.get("expected").and_then(Json::as_str) else {
                continue;
            };
            // Not every corpus holds a JSON document — the black box freezes bytes by digest — and
            // one that does not is not a refusal that names nothing.
            let Ok(value) = Json::parse(expected) else {
                continue;
            };
            cases += 1;
            walk(&value, &format!("{label} :: {id}"), &mut violations);
        }
    }

    assert!(cases > 0, "no case produced a document to check");

    if !violations.is_empty() {
        let mut message = format!(
            "{} refusal(s) in {cases} cases name nothing an operator could act on.\n\n\
             §64: a refusal that does not tell you what to change is a bug report the user has to \n\
             write themselves.\n\n",
            violations.len()
        );
        for violation in violations.iter().take(10) {
            message.push_str(&format!(
                "  {}\n    {}\n    {}\n",
                violation.rule, violation.case, violation.value
            ));
        }
        if violations.len() > 10 {
            message.push_str(&format!("  … and {} more\n", violations.len() - 10));
        }
        panic!("{message}");
    }

    println!("§64: {cases} cases checked, every refusal names the fix");
}
