# The Go edition.
#
#   make -C editions/go check     everything CI runs, all of it blocking
#   make -C editions/go corpus    just the corpus, at the ratchet
#
# `check` is six things and every one of them fails the build:
#
#   | fmt          | gofmt is not advisory here; a formatting diff is a build failure       |
#   | vet          | Go's own correctness checks, which are warnings nowhere else            |
#   | test         | the 219 case vectors and the tables' order                             |
#   | roundtrip    | the canonical writer against every `expected` string in the corpus     |
#   | selfcheck    | the source scan, the runner's accounting, rule 3 at its edges, and the |
#   |              | compiler's assembly on the six architectures that fuse a multiply-add  |
#   | corpus       | every case, at MIN_PASSED                                             |
#
# The fusion scan cross-compiles the edition for arm64, amd64 (v3), riscv64, ppc64le, s390x and
# loong64. On a cold build cache that compiles the standard library for each first, a minute or two;
# after that it replays from the cache in seconds.
#
# MIN_PASSED is the ratchet (§6.2), and it exists because the corpus runner's floor is **zero**: an
# edition whose handler was deleted answers nothing, fails nothing, and exits 0, because the cases
# move from *passed* into *not implemented*. That is the one regression the runner's own design
# cannot see, so the number is written down here and lowering it is a decision somebody makes in a
# diff rather than a number that quietly slipped.
#
# Raise it whenever the edition reaches higher. Never lower it to make a build pass.

GO ?= go
MIN_PASSED ?= 1510

# The corpus lives above this directory. Passed explicitly so the runner does not have to guess from
# a working directory that `make -C` has already changed.
CORPUS ?= ../../corpus

.PHONY: check fmt vet test roundtrip selfcheck corpus tables clean

check: fmt vet test roundtrip selfcheck corpus

# gofmt ships beside the compiler, so it is invoked directly. `go run cmd/gofmt` also works and
# builds the formatter from the standard library's source first, which takes minutes — it was in
# this recipe for one session and presented as a hung build.
fmt:
	@echo "== gofmt"
	@test -z "$$(gofmt -l .)" || { \
		echo "these files are not gofmt-clean:"; gofmt -l .; exit 1; }

vet:
	@echo "== go vet"
	$(GO) vet ./...

test:
	@echo "== go test"
	$(GO) test ./...

roundtrip:
	@echo "== canonical writer against the corpus"
	$(GO) run ./cmd/roundtrip --corpus-root $(CORPUS)

selfcheck:
	@echo "== self-check"
	$(GO) run ./cmd/selfcheck

corpus:
	@echo "== corpus (ratchet: $(MIN_PASSED))"
	$(GO) run ./cmd/corpus --corpus-root $(CORPUS) --min-passed $(MIN_PASSED)

# The case tables are generated from the engine the reference runs on, by a generator shared with
# the C++ edition. Here for discoverability; CI runs it under a pinned Node, because Node's Unicode
# version decides what it produces.
tables:
	node ../../tools/unicode-case.ts

clean:
	$(GO) clean -cache -testcache
