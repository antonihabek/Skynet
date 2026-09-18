# The C# / .NET edition (§31.3, §42 level L2 — Contracts).
#
#   make            # build
#   make corpus     # run the conformance corpus at its ratchet
#   make test       # the canonical round trip and the self-check
#   make check      # everything, in the order a CI step wants it
#   make clean
#
# Warnings are errors, set in `SkynetOs.csproj` rather than here so that an editor and a CI step see the same
# gate: a rule that only the Makefile enforces is a rule that passes in an IDE and fails in CI, which teaches a
# contributor to distrust the gate rather than the code.
#
# No `dotnet restore` from a package index. §31.4 requires byte-identical output, so the number conversion and
# the duplicate-key rule have to be ours rather than whatever a dependency does this year — which is the same
# reason the Rust workspace has none. It also means every target here works with no network.

DOTNET ?= dotnet
CONFIGURATION ?= Release

# `-warnaserror` is already in the project file; passing `--nologo` keeps a failure's first line the failure.
BUILDFLAGS ?= --nologo -c $(CONFIGURATION)

# The built assembly, run directly rather than through `dotnet run`. `dotnet run` forwards any option it does
# not recognise to the program — so `--nologo` arrived as this edition's first argument and the runner answered
# with its usage line, which is exactly right of the runner and a trap in the Makefile. Running the dll also
# skips a project evaluation per target.
ASSEMBLY := bin/$(CONFIGURATION)/net8.0/skynet-corpus-csharp.dll
RUN := $(DOTNET) $(ASSEMBLY)

# Where the corpus is. Overridable so a vendor can point the runner at a pinned copy.
CORPUS ?= ../../corpus

# The ratchet. This is the number of cases this edition has reached; see `--min-passed` in `src/Program.cs` for
# why a floor is checked separately from the failures. Lowering it is a decision somebody makes in a diff.
MIN_PASSED ?= 332

# `DOTNET_CLI_TELEMETRY_OPTOUT` and `DOTNET_NOLOGO` are set for every target, not as a preference but because
# the first run of the CLI otherwise prints a welcome banner to stdout — and this edition prints its report to
# stdout for `tools/edition-report.mjs` to parse.
export DOTNET_CLI_TELEMETRY_OPTOUT = 1
export DOTNET_NOLOGO = 1
# Culture is not allowed to decide any of this either; `InvariantGlobalization` in the project file removes ICU,
# and this says so to a runtime that was built without it.
export DOTNET_SYSTEM_GLOBALIZATION_INVARIANT = 1

.PHONY: all corpus test check clean

all:
	$(DOTNET) build $(BUILDFLAGS)

corpus: all
	$(RUN) corpus --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

test: all
	$(RUN) roundtrip --corpus-root $(CORPUS)
	$(RUN) selfcheck --corpus-root $(CORPUS)

# No fused multiply-add *flag*, and that is a property of the platform rather than an omission: the CLR
# specification defines `mul` and `add` on float64 as separate IEEE-754 operations with no licence to contract
# them, so there is nothing to switch off the way there is in C, C++ and Zig. What there is on this platform is
# `System.Math.FusedMultiplyAdd`, which fuses when a caller asks — so the rule is that nothing asks, and
# `selfcheck` scans the sources and proves the ban is not vacuous by fusing one triple on purpose. §31.2's bits
# are the specification's, not the compiler's.
check: test corpus

clean:
	rm -rf bin obj
