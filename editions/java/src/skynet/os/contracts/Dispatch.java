// The table that routes a corpus file to a contract.
//
// Every contract in this package is written from the modules under `reference/typescript` and from nothing
// else — never from `corpus/`, which would agree by construction, and never from the Python, C++, Go, C or
// Zig editions, which would prove only that one hand did both. What holds them honest is the corpus, in the
// one direction that means something: this edition reproducing what the oracle produced, byte for byte.
//
// One class here per module there, named for it, so a reader holding the reference open can find the Java
// beside it. This file is the only one with no counterpart: the reference has no such table, because in
// TypeScript the corpus generator calls the functions directly.

package skynet.os.contracts;

import java.util.LinkedHashMap;
import java.util.Map;
import skynet.os.Corpus;

public final class Dispatch {
    private Dispatch() {}

    /**
     * Every corpus file this edition answers, keyed by {@code corpus/file}.
     *
     * <p>A file absent from this table is counted as <i>not implemented</i> rather than as passing, and the
     * runner prints the two separately — §31.4's "an edition that has not passed the corpus is not an edition"
     * is not satisfied by a table with fewer entries in it.
     */
    public static Map<String, Corpus.Handler> entries() {
        Map<String, Corpus.Handler> out = new LinkedHashMap<>();
        out.put("predicate/evaluate", Predicate::evaluateCase);
        out.put("roles/vocabulary", Embodiment::rolesVocabularyCase);
        out.put("semver/compare", Semver::compareCase);
        out.put("semver/satisfies", Semver::satisfiesCase);
        out.put("vocabulary/closed-sets", Vocabulary::closedSetsCase);
        return out;
    }
}
