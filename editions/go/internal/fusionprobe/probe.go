// Package fusionprobe is the positive control for cmd/selfcheck's fusion scan, and nothing else.
//
// The scan reads the compiler's assembly for the six architectures on which Go fuses a multiply
// into an add, and refuses any fused instruction in this edition. A scan that finds nothing has
// either found nothing or failed to look — a renamed mnemonic, an output format that changed, a
// build that silently produced no assembly — and the two are indistinguishable from its result
// alone (A-18). So this package fuses on purpose, and the scan requires it to: a target on which
// the probe does not fuse is a target the scan cannot see, and it fails rather than passing.
//
// Nothing imports it. It is compiled only because `go build ./internal/...` compiles everything.
package fusionprobe

// MulAdd is the expression the Go specification permits an implementation to fuse: "some
// architectures provide a fused multiply and add (FMA) instruction that computes x*y + z without
// rounding the intermediate result x*y". Every fusing target does, with this exact spelling.
//
// Written without the float64 conversion every contract in this edition uses, which is the point.
func MulAdd(x, y, z float64) float64 { return x*y + z }
