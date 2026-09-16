// Package fusionprobe is the positive control for cmd/selfcheck's fusion scan, and nothing else.
//
// The scan reads the compiler's assembly for every architecture on which a fused multiply-add can
// reach a binary, and refuses any fused instruction in this edition. A scan that finds nothing has
// either found nothing or failed to look — a renamed mnemonic, an output format that changed, a
// build that silently produced no assembly — and the two are indistinguishable from its result
// alone (A-18). So this package fuses on purpose, and the scan requires it to: a target on which
// the probe does not fuse is a target the scan cannot see, and it fails rather than passing.
//
// It takes **two** functions to cover the list, because the two ways the instruction arrives are
// not one mechanism and only one of them is a contraction:
//
//   - arm64, riscv64, ppc64le, s390x and loong64 contract `x*y + z` on their own, and [MulAdd] is
//     the control there.
//   - amd64 never does — measured at GOAMD64 v1, v2, v3 and v4 under go1.24, every one of which
//     emits MULSD then ADDSD — so [MulAdd] cannot be its control, and for as long as it was the
//     only probe the scan correctly refused amd64 as a target it could not see into. What does
//     reach an amd64 listing is an explicit math.FMA, and [Intrinsic] is that control.
//
// Nothing imports them. They are compiled only because `go build ./internal/...` compiles
// everything.
package fusionprobe

import "math"

// MulAdd is the expression the Go specification permits an implementation to fuse: "some
// architectures provide a fused multiply and add (FMA) instruction that computes x*y + z without
// rounding the intermediate result x*y". Every contracting target does, with this exact spelling.
//
// Written without the float64 conversion every contract in this edition uses, which is the point.
func MulAdd(x, y, z float64) float64 { return x*y + z }

// Intrinsic is the same instruction asked for by name rather than formed by the optimiser.
// math.FMA computes x*y + z with one rounding by definition, so it is exactly what §31.2's
// argument forbids this edition to contain — the same sequence of correctly rounded operations has
// to produce the same bits everywhere, and a fused operation is a different sequence.
//
// On amd64 it is more than a control. Since the compiler contracts nothing there, a call to
// math.FMA is the *only* way a fused instruction appears under internal/, which makes the scan on
// that target a check for exactly this spelling rather than redundant cover for the other five.
func Intrinsic(x, y, z float64) float64 { return math.FMA(x, y, z) }
