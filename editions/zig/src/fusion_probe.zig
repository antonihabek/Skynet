//! A multiply-add the compiler is *asked* to fuse.
//!
//! `tools/fusion-scan.sh` scans every source file's assembly for a fused multiply-add and fails on one.
//! A scan that finds nothing and a scan that cannot see anything print the same line (finding 171), so
//! this file asks for contraction explicitly and the scan fails if *this* listing has none — which is
//! what makes an empty result over the edition's own sources evidence rather than silence.
//!
//! Never imported by anything. It exists to be compiled.

/// Zig's default float mode is `.strict`, which forbids contraction. This is the one place in the
/// edition that asks for `.optimized`, which permits it.
pub export fn skynet_fusion_probe(x: f64, y: f64, z: f64) f64 {
    @setFloatMode(.optimized);
    return x * y + z;
}
