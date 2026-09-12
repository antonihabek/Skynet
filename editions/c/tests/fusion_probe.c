/*
 * A multiply-add the compiler is *asked* to fuse.
 *
 * The `fusion` target in the Makefile scans every translation unit's assembly for a fused
 * multiply-add and fails on one. A scan that finds nothing and a scan that cannot see anything print
 * the same line (finding 171), so this file is compiled the same way but without `FP_FLAGS`, and the
 * step fails if *this* listing has no fused instruction — which is what makes an empty result over
 * the edition's own sources evidence rather than silence.
 *
 * Never linked into anything. It exists to be compiled.
 */

double skynet_fusion_probe(double x, double y, double z);

double skynet_fusion_probe(double x, double y, double z)
{
    return x * y + z;
}
