//! `safety.kernel.parity`, generalised.
//!
//! The SCTS test that holds an on-robot kernel to the reference becomes, across editions, "this
//! edition reproduces the corpus". This is that test for the Rust edition, and it is the one that
//! decides whether this codebase may call itself an edition at all.

#[test]
fn this_edition_reproduces_the_corpus() {
    let root = skynet_corpus::default_root();
    assert!(
        root.is_dir(),
        "no corpus at {}. Run `node tools/corpus-gen/generate.ts`.",
        root.display()
    );

    let report = skynet_corpus::run(&root);

    assert!(report.files > 0, "the corpus is empty");
    assert!(report.passed > 0, "no case ran");

    if !report.unsupported.is_empty() {
        panic!(
            "{} case(s) this edition does not implement:\n  {}",
            report.unsupported.len(),
            report.unsupported.join("\n  ")
        );
    }

    if !report.failures.is_empty() {
        let mut message = format!(
            "{} of {} cases diverged from the corpus.\n\n\
             A divergence is either a specification ambiguity or an implementation defect. It \n\
             blocks this edition and no other.\n\n",
            report.failures.len(),
            report.passed + report.failures.len()
        );
        for failure in report.failures.iter().take(10) {
            message.push_str(&failure.to_string());
            message.push('\n');
        }
        if report.failures.len() > 10 {
            message.push_str(&format!("… and {} more\n", report.failures.len() - 10));
        }
        panic!("{message}");
    }

    println!(
        "corpus: {} cases across {} files, all reproduced byte for byte",
        report.passed, report.files
    );
}
