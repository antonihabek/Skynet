# The Java edition (§31.3, §42 level L2 — Contracts).
#
#   make            # compile
#   make corpus     # run the conformance corpus at its ratchet
#   make test       # the canonical round trip and the self-check
#   make check      # everything, in the order a CI step wants it
#   make clean
#
# Warnings are errors. `-Xlint:all -Werror` is the gate rather than a preference, for the reason §6.1 of the
# coding standard ranks the compiler second only to making a violation unrepresentable — and it has already
# earned it here: it refused a `serialVersionUID` nobody would have added, on an exception class whose absence
# would have been a warning nobody read.
#
# No Maven, no Gradle, no `pom.xml`. §31.4 requires byte-identical output, so the number conversion and the
# duplicate-key rule have to be ours rather than whatever a dependency does this year — which is the same
# reason the Rust workspace has no dependencies. A build that needs a package index is also a build that
# cannot run on the machine an edition is being reviewed on.

JAVAC ?= javac
JAVA ?= java
JAVAFLAGS ?= -Xlint:all -Werror

# **UTF-8 on every stream, explicitly.** JDK 18 made `file.encoding` UTF-8 by default and left the console
# streams on the platform's native encoding, so `System.out` on a machine whose locale is not UTF-8 writes
# mangled bytes — and this edition prints its report to stdout for `tools/edition-report.mjs` to parse. A
# figure the Hub publishes must not depend on the locale of the machine that measured it.
ENCODING := -Dstdout.encoding=UTF-8 -Dstderr.encoding=UTF-8 -Dfile.encoding=UTF-8

BUILD := build
SOURCES := $(shell find src -name '*.java')

# Where the corpus is. Overridable so a vendor can point the runner at a pinned copy.
CORPUS ?= ../../corpus

# The ratchet. This is the number of cases this edition has reached; see `--min-passed` in `Main.java` for why
# a floor is checked separately from the failures. Lowering it is a decision somebody makes in a diff.
MIN_PASSED ?= 339

.PHONY: all corpus test check clean

all: $(BUILD)/.stamp

# One compile of the whole tree, because javac resolves the package graph itself and a per-file rule would
# either miss a dependency or rebuild everything anyway. The stamp is what `make` tracks.
$(BUILD)/.stamp: $(SOURCES)
	@mkdir -p $(BUILD)
	$(JAVAC) $(JAVAFLAGS) -d $(BUILD) $(SOURCES)
	@touch $@

corpus: all
	$(JAVA) $(ENCODING) -cp $(BUILD) skynet.os.Main --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

test: all
	$(JAVA) $(ENCODING) -cp $(BUILD) skynet.os.Roundtrip --corpus-root $(CORPUS)
	$(JAVA) $(ENCODING) -cp $(BUILD) skynet.os.SelfCheck --corpus-root $(CORPUS)

# No fused multiply-add scan, and that is a property of the platform rather than an omission. The JVM
# specification defines `dmul` and `dadd` as separate IEEE-754 binary64 operations with no licence to contract
# them, and `strictfp` became the only behaviour in Java 17 — so there is no flag to set and no default to
# distrust, which is not true of the C, C++ or Zig editions. What a JIT is permitted to do is fuse nothing here;
# §31.2's bits are the specification's, not the compiler's.
check: test corpus

clean:
	rm -rf $(BUILD)
