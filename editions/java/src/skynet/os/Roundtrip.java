// The canonical writer, proven before anything is built on it.
//
//   java -cp build skynet.os.Roundtrip [--corpus-root corpus]
//
// Every `expected` string in the corpus is parsed and re-serialised, and the bytes must be identical. That is
// a stronger exam than it looks, and it is why this program runs before the corpus runner:
//
// * **Rule 3 over every number the reference ever wrote.** `Double.toString` is not rule 3 in four separate
//   ways, so this edition runs the search by hand — and this step is what says the hand-written one is right.
// * **Rule 2 over keys nobody chose.** The corpus holds documents whose keys collide on their first bytes and
//   differ in length, which is the comparison a shared-prefix compare gets wrong; and keys are compared as
//   UTF-8 bytes, which is not `String.compareTo`.
// * **Rule 4 over strings the generator escaped.** The corpus is written by `JSON.stringify`, which escapes
//   non-ASCII; this edition emits it as itself. A file that round-trips has had both halves agree.
//
// A round trip that fails is a defect in this edition and never in the corpus, because the corpus is the
// oracle's output and canonical by construction.

package skynet.os;

import java.nio.file.Path;
import java.util.List;

public final class Roundtrip {
    private Roundtrip() {}

    public static void main(String[] argv) {
        String rootArgument = null;
        for (int at = 0; at < argv.length; at++) {
            if ("--corpus-root".equals(argv[at]) && at + 1 < argv.length) {
                rootArgument = argv[++at];
            } else {
                System.err.println("usage: skynet.os.Roundtrip [--corpus-root DIR]");
                System.exit(2);
            }
        }

        Path corpusRoot = Corpus.root(rootArgument);
        if (corpusRoot == null) {
            System.err.println("no corpus found: pass --corpus-root, or set SKYNET_CORPUS");
            System.exit(2);
            return;
        }
        List<Corpus.File> files = Corpus.load(corpusRoot);
        if (files.isEmpty()) {
            System.err.println("no corpus files under " + corpusRoot);
            System.exit(2);
            return;
        }

        int checked = 0;
        int failed = 0;
        for (Corpus.File file : files) {
            for (Corpus.Case vector : file.cases()) {
                String encoded;
                try {
                    encoded = Canonical.encode(Parse.parse(vector.expected()));
                } catch (RuntimeException error) {
                    failed += 1;
                    System.err.printf("%s :: %s%n  the expectation could not be re-serialised: %s%n",
                            file.key(), vector.id(), error.getMessage());
                    continue;
                }
                if (!encoded.equals(vector.expected())) {
                    failed += 1;
                    System.err.println(Corpus.describe(new Corpus.Failure(file.key(), vector.id(),
                            "a canonical document did not survive a parse and a write",
                            vector.expected(), encoded, null)) + "\n");
                    continue;
                }
                checked += 1;
            }
        }

        System.err.printf("round trip  %d of %d expectations re-serialised identically%n",
                checked, checked + failed);
        System.exit(failed == 0 ? 0 : 1);
    }
}
