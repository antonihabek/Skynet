//! The Zig edition (§31.3, §42 level L2 — Contracts).
//!
//!   zig build                 # build the three programs
//!   zig build corpus          # run the conformance corpus at its ratchet
//!   zig build test            # the canonical round trip and the self-check
//!   zig build fusion          # the fused multiply-add scan
//!   zig build check           # everything, in the order a CI step wants it
//!
//! `-Doptimize=ReleaseSafe` is the default rather than `Debug`, and that is a decision rather than a
//! habit: this edition's whole job is to produce bytes, and the bounds and overflow checks
//! ReleaseSafe keeps are the ones that turn a wrong index into a stop instead of a wrong document.
//! `ReleaseFast` would remove them, and there is nothing here whose speed is worth that.

const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.option(
        std.builtin.OptimizeMode,
        "optimize",
        "Optimisation mode; ReleaseSafe by default, for the reason in this file's header",
    ) orelse .ReleaseSafe;

    // Where the corpus is. Overridable so a vendor can point the runner at a pinned copy.
    const corpus_root = b.option([]const u8, "corpus", "The corpus directory, relative to editions/zig") orelse "../../corpus";

    // The ratchet. This is the number of cases this edition has reached; see `--min-passed` in
    // `src/main.zig` for why a floor is checked separately from the failures. Lowering it is a decision
    // somebody makes in a diff.
    const min_passed = b.option([]const u8, "min-passed", "The corpus ratchet") orelse "339";

    const programs = .{
        .{ "skynet-corpus-zig", "src/main.zig" },
        .{ "skynet-roundtrip-zig", "src/roundtrip.zig" },
        .{ "skynet-selfcheck-zig", "src/selfcheck.zig" },
    };

    var built: [programs.len]*std.Build.Step.Compile = undefined;
    inline for (programs, 0..) |program, index| {
        const exe = b.addExecutable(.{
            .name = program[0],
            .root_module = b.createModule(.{
                .root_source_file = b.path(program[1]),
                .target = target,
                .optimize = optimize,
            }),
        });
        b.installArtifact(exe);
        built[index] = exe;
    }

    // **Every run step's working directory is this build root, set rather than inherited.** A `Run` step
    // with no `setCwd` inherits the cwd of the `zig build` process, so `corpus_root`'s default resolved
    // against whatever directory the caller happened to be in: `zig build check` worked from
    // `editions/zig` and failed from the repository root, which is where the CI step invokes it with
    // `--build-file`. The default is documented as *the repository's `corpus/`*, so it has to mean that
    // from anywhere. A caller passing `-Dcorpus=` a relative path is naming it relative to this
    // directory too, which is the same rule as the default and is why the option's help says so.
    const corpus_run = b.addRunArtifact(built[0]);
    corpus_run.setCwd(b.path("."));
    corpus_run.addArgs(&.{ "--corpus-root", corpus_root, "--min-passed", min_passed });
    const corpus_step = b.step("corpus", "Run the conformance corpus at its ratchet");
    corpus_step.dependOn(&corpus_run.step);

    const roundtrip_run = b.addRunArtifact(built[1]);
    roundtrip_run.setCwd(b.path("."));
    roundtrip_run.addArgs(&.{ "--corpus-root", corpus_root });
    const selfcheck_run = b.addRunArtifact(built[2]);
    selfcheck_run.setCwd(b.path("."));
    selfcheck_run.addArgs(&.{ "--corpus-root", corpus_root });
    const test_step = b.step("test", "The canonical round trip and the self-check");
    test_step.dependOn(&roundtrip_run.step);
    test_step.dependOn(&selfcheck_run.step);

    // ── The fused multiply-add scan ───────────────────────────────────────────
    //
    // Zig's default float mode is `.strict`, which forbids contraction, so unlike the C and C++ editions
    // there is no flag to set — and that is exactly why the scan is here rather than omitted. A default
    // is a thing that can change, and `@setFloatMode(.optimized)` is one line in one file away. §31.2
    // pins the kernel's sine, cosine and square root as a sequence of correctly rounded operations so
    // every edition computes the same bits; a fused multiply-add is a different sequence.
    //
    // The scan compiles every source file to assembly for the architectures that fuse and fails on any
    // fused instruction. `src/fusion_probe.zig` sets `.optimized` on purpose and must fuse, so a host
    // where the scan cannot see an FMA fails rather than passing by not looking (finding 171).
    const fusion_step = b.step("fusion", "Refuse a fused multiply-add in any translation unit");
    const fusion_run = b.addSystemCommand(&.{ "sh", "tools/fusion-scan.sh" });
    // The scan shells out because what it inspects is the *compiler's own assembly*, for architectures
    // this host is not. Expressing that as build-system steps would mean one `addObject` per source per
    // target and a Zig program to grep the listings; a twelve-line script that calls `zig build-obj
    // -femit-asm` is the same gate and a reader can run it by hand.
    fusion_run.setCwd(b.path("."));
    fusion_step.dependOn(&fusion_run.step);

    // ── Formatting ────────────────────────────────────────────────────────────
    //
    // `zig fmt` is not advisory here; a formatting diff fails the build, for the reason the Go edition
    // makes `gofmt` one. The two generated files are excluded because `zig fmt` aligns an array literal
    // into columns whose width depends on the widest element in each run, which no generator can predict
    // without reimplementing the formatter — `tools/unicode-case.ts --check` is their gate instead.
    const fmt_step = b.step("fmt", "Refuse a formatting diff");
    const fmt_check = b.addFmt(.{
        .paths = &.{ "src", "build.zig" },
        .exclude_paths = &.{ "src/unicode_case.zig", "src/case_vectors.zig" },
        .check = true,
    });
    fmt_step.dependOn(&fmt_check.step);

    const check_step = b.step("check", "Everything, in the order a CI step wants it");
    check_step.dependOn(fmt_step);
    check_step.dependOn(test_step);
    check_step.dependOn(fusion_step);
    check_step.dependOn(corpus_step);

    b.getInstallStep().dependOn(&built[0].step);
}
