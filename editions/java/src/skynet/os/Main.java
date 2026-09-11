// Run the conformance corpus against the Java edition.
//
//   java -cp build skynet.os.Main [--corpus-root corpus] [--verbose] [--json] [--min-passed N]
//
// Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary always says
// how many cases are not implemented — split into the corpora with no handler at all and the individual files
// a handler has not been written for. Those facts together are the honest sentence about this edition, and
// none of them is useful without the others.
//
// `--min-passed` is the ratchet, and it exists because everything above is a **floor of zero**. An edition
// whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move from *passed* into
// *not implemented*, which is the bucket that reads as honest progress. That is the one regression this
// runner's own design cannot see, so the number a build passes here is the number this edition has reached.

package skynet.os;

import java.nio.file.Path;
import java.util.List;
import skynet.os.contracts.Dispatch;

public final class Main {
    private Main() {}

    private static void printList(String heading, List<String> values) {
        if (values.isEmpty()) return;
        System.err.println(heading + " " + String.join(", ", values));
    }

    private static Value strings(List<String> values) {
        Value out = Value.array();
        for (String held : values) out.push(Value.of(held));
        return out;
    }

    public static void main(String[] argv) {
        String rootArgument = null;
        boolean verbose = false;
        boolean asJson = false;
        long minPassed = -1;

        for (int at = 0; at < argv.length; at++) {
            switch (argv[at]) {
                case "--corpus-root" -> rootArgument = argv[++at];
                case "--verbose" -> verbose = true;
                case "--json" -> asJson = true;
                case "--min-passed" -> minPassed = Long.parseLong(argv[++at]);
                default -> {
                    System.err.println("usage: skynet.os.Main [--corpus-root DIR] [--verbose] [--json]"
                            + " [--min-passed N]");
                    System.exit(2);
                }
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

        Corpus.Report report = Corpus.run(files, Dispatch.entries());

        if (asJson) {
            // Deliberately the same fields the human report prints, and no derived ones: a summary that
            // computed a percentage here and a page that computed it differently would be two answers to one
            // question.
            Value out = Value.object();
            out.set("passed", Value.of((double) report.passed()));
            out.set("failed", Value.of((double) report.failed()));
            out.set("unimplemented", Value.of((double) report.unanswered()));
            out.set("total", Value.of((double) report.total()));
            out.set("complete", strings(report.complete()));
            out.set("partial", strings(report.partial()));
            out.set("missing_handlers", strings(report.missingHandlers()));
            out.set("declined_files", strings(report.declinedFiles()));
            // stdout, because this is the output a tool parses; everything else here goes to stderr.
            System.out.println(Canonical.encode(out));
            System.exit(report.failed() == 0 ? 0 : 1);
            return;
        }

        System.err.println("corpus     " + corpusRoot);
        System.err.printf("result     %d passed, %d failed, %d not implemented, of %d%n",
                report.passed(), report.failed(), report.unanswered(), report.total());
        printList("complete  ", report.complete());
        printList("no handler", report.missingHandlers());
        printList("declined  ", report.declinedFiles());
        // Named separately from `complete` on purpose: these are corpora this edition answers part of, and a
        // reader who saw only the passing files would take them for finished ones.
        printList("partial   ", report.partial());

        if (!report.failures().isEmpty()) {
            System.err.println();
            int shown = verbose ? report.failures().size() : Math.min(report.failures().size(), 10);
            for (Corpus.Failure failure : report.failures().subList(0, shown)) {
                System.err.println(Corpus.describe(failure) + "\n");
            }
            if (shown < report.failures().size()) {
                System.err.println("... and " + (report.failures().size() - shown) + " more (pass --verbose)");
            }
        }

        if (minPassed >= 0 && report.passed() < minPassed) {
            System.err.printf("%nregression: %d passed, and this edition had reached %d. Cases that stop being"
                    + " answered are counted as *not implemented* rather than as failures, so nothing above"
                    + " this line went red — which is why the floor is checked separately from the failures.%n",
                    report.passed(), minPassed);
            System.exit(1);
            return;
        }
        System.exit(report.failed() == 0 ? 0 : 1);
    }
}
