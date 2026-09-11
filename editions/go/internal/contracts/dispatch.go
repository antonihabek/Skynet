// The contracts, and the table that routes a corpus file to one.
//
// Every function under this package is written from `reference/typescript/*` and from nothing else
// — never from `corpus/`, which would agree by construction, and never from another edition, which
// would prove only that one hand did both. What holds them honest is the corpus, in the one
// direction that means something: this edition reproducing what the oracle produced, byte for byte.
//
// One file here per module there, named the same, so a reader holding the reference open can find
// the Go beside it. The dispatch is the only file with no counterpart: the reference has no such
// table, because in TypeScript the corpus generator calls the functions directly.
package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/corpus"
)

// Dispatch is every corpus file this edition answers, keyed by `corpus/file`.
//
// A file absent from this table is counted as *not implemented* rather than as passing, and the
// runner prints the two separately — §31.4's "an edition that has not passed the corpus is not an
// edition" is not satisfied by a table with fewer entries in it.
func Dispatch() corpus.Dispatch {
	return corpus.Dispatch{
		"digest/version":         versionDigest,
		"roles/vocabulary":       rolesVocabulary,
		"semver/compare":         semverCompare,
		"semver/satisfies":       semverSatisfies,
		"vocabulary/closed-sets": closedSets,
	}
}
