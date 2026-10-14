# Reference

What the rest of the repository is checked against.

| Folder | What is inside |
|---|---|
| [`typescript/`](typescript/README.md) | The reference code for the core rules. The shared test cases in [`corpus/`](../corpus/) are generated from it, and every edition (Rust, Python, C++, Go, C, Zig, Java, C#) must reproduce them byte for byte. |
| [`wire/`](wire/README.md) | The wire formats that other software depends on, frozen, with what is deliberately not frozen. |
| `reports/` | Generated measurements that CI compares against: the editions' status, the fleet sweep, the code-size report and the seeded-defect campaigns. Do not edit by hand; the tools in [`tools/`](../tools/) write them. |
