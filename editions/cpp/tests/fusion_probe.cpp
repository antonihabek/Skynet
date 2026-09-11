// The positive control for the Makefile's `fusion` step, and nothing else.
//
// That step compiles every translation unit with a fused multiply-add available and refuses any
// fused instruction in the listing. A scan that finds nothing has either found nothing or failed to
// look — a mnemonic its pattern does not know, a target flag this compiler ignored, a listing that
// came out empty — and the two are indistinguishable from its result alone. So this file is compiled
// the same way *without* `-ffp-contract=off`, and the step requires it to fuse: a host on which it
// does not is a host where the scan cannot see what it is looking for, and the step fails rather than
// passing.
//
// Not linked into anything.

double fusion_probe(double x, double y, double z);

double fusion_probe(double x, double y, double z) { return x * y + z; }
