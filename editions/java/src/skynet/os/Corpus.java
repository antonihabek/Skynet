// Reading and running the conformance corpus.
//
// Split from the contracts because two programs read the corpus and only one of them computes anything:
// `Roundtrip` re-serialises every `expected` string before a single contract exists, and `Main` runs the
// contracts against it. One reader means the two cannot disagree about which files the corpus has.
//
// # The four buckets, and why none of them is useful alone
//
// §31.4: an edition that has not passed the corpus is not an edition. A runner that printed only failures
// would report an edition with one handler as flawless, so a report here says how many cases *passed*, how
// many *failed*, and how many are *not implemented* — split into the corpora with no handler at all and the
// individual files a handler has not been written for.
//
// The floor those three share is zero, which is the one regression this design cannot see: delete a handler
// and its cases move from passed into not-implemented, nothing goes red, and the runner exits 0.
// `--min-passed` is the ratchet that closes it.

package skynet.os;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;

public final class Corpus {
    private Corpus() {}

    /**
     * One vector: an input, the bytes the reference produced from it, and the note that says what it is for.
     *
     * <p>{@code expected} is a string and not a tree, because the requirement is byte-identical output and the
     * bytes are what the file holds.
     */
    public record Case(String id, String note, Value input, String expected) {}

    /** One corpus file, and the {@code corpus/name} key a dispatch is registered under. */
    public record File(Path path, String corpus, String name, List<Case> cases) {
        public String key() {
            return corpus + "/" + name;
        }
    }

    /**
     * A contract, as the runner sees it.
     *
     * <p>Returns the value the contract produced; the runner encodes it and compares bytes. A thrown exception
     * is a case this edition could not answer <i>at all</i> — not one it answered wrongly. The two are counted
     * separately and neither may hide in the other.
     */
    public interface Handler {
        Value apply(Value input);
    }

    public record Failure(String file, String id, String note, String expected, String actual, String error) {}

    public record Report(
            int passed,
            int failed,
            int unanswered,
            List<String> complete,
            List<String> partial,
            List<String> missingHandlers,
            List<String> declinedFiles,
            List<Failure> failures) {
        public int total() {
            return passed + failed + unanswered;
        }
    }

    /**
     * The corpus directory: an explicit path, else {@code $SKYNET_CORPUS}, else the nearest {@code corpus/} at
     * or above the working directory.
     *
     * <p>Null when there is none, which the caller reports rather than defaulting to something plausible — a
     * runner that measured an empty corpus would exit 0 having proved nothing.
     */
    public static Path root(String explicit) {
        if (explicit != null && !explicit.isEmpty()) {
            Path given = Path.of(explicit);
            return Files.isDirectory(given) ? given : null;
        }
        String fromEnvironment = System.getenv("SKYNET_CORPUS");
        if (fromEnvironment != null && !fromEnvironment.isEmpty()) {
            Path given = Path.of(fromEnvironment);
            return Files.isDirectory(given) ? given : null;
        }
        // Walk up looking for a `corpus/README.md`, not merely a directory called `corpus`: a build tree with
        // an empty `corpus/` in it would otherwise be found first and measured as zero cases.
        Path here = Path.of("").toAbsolutePath();
        for (int levels = 0; levels < 64 && here != null; levels++) {
            Path candidate = here.resolve("corpus");
            if (Files.isRegularFile(candidate.resolve("README.md"))) return candidate;
            here = here.getParent();
        }
        return null;
    }

    /** Every JSON file one directory below {@code root}, sorted, so a run is reproducible. */
    public static List<File> load(Path corpusRoot) {
        List<File> files = new ArrayList<>();
        try (Stream<Path> corpora = Files.list(corpusRoot)) {
            List<Path> directories = corpora.filter(Files::isDirectory)
                    .filter(path -> !path.getFileName().toString().startsWith("."))
                    .sorted(Comparator.comparing(path -> path.getFileName().toString()))
                    .toList();
            for (Path directory : directories) {
                try (Stream<Path> held = Files.list(directory)) {
                    List<Path> names = held.filter(Files::isRegularFile)
                            .filter(path -> path.getFileName().toString().endsWith(".json"))
                            .sorted(Comparator.comparing(path -> path.getFileName().toString()))
                            .toList();
                    for (Path path : names) {
                        files.add(read(directory.getFileName().toString(), path));
                    }
                }
            }
        } catch (IOException error) {
            throw new UncheckedIOException(error);
        }
        return files;
    }

    private static File read(String corpusName, Path path) throws IOException {
        String text = Files.readString(path, StandardCharsets.UTF_8);
        Value document = Parse.parse(text);
        Value cases = Value.get(document, "cases");
        if (cases == null || cases.kind() != Value.Kind.ARRAY) {
            throw new IllegalStateException(path + " has no `cases` array");
        }
        List<Case> held = new ArrayList<>();
        for (int at = 0; at < Value.length(cases); at++) {
            Value vector = Value.at(cases, at);
            Value expected = Value.get(vector, "expected");
            if (expected == null || expected.kind() != Value.Kind.STRING) {
                throw new IllegalStateException(path + " has a case with no `expected` string."
                        + " corpus/README.md's first property is that the bytes are what the file holds.");
            }
            held.add(new Case(
                    Value.asString(Value.get(vector, "id"), ""),
                    Value.asString(Value.get(vector, "note"), ""),
                    Value.get(vector, "input"),
                    Value.asString(expected, "")));
        }
        String name = path.getFileName().toString();
        return new File(path, corpusName, name.substring(0, name.length() - ".json".length()), held);
    }

    private static final class Tally {
        int files;
        int handled;
        boolean failed;
    }

    /** Measure {@code dispatch} against {@code files}. */
    public static Report run(List<File> files, Map<String, Handler> dispatch) {
        int passed = 0;
        int failed = 0;
        int unanswered = 0;
        List<Failure> failures = new ArrayList<>();
        List<String> declined = new ArrayList<>();
        Map<String, Tally> tallies = new LinkedHashMap<>();

        for (File file : files) {
            Tally tally = tallies.computeIfAbsent(file.corpus(), key -> new Tally());
            tally.files += 1;

            Handler handler = dispatch.get(file.key());
            if (handler == null) {
                unanswered += file.cases().size();
                declined.add(file.key());
                continue;
            }
            tally.handled += 1;

            for (Case vector : file.cases()) {
                String encoded;
                try {
                    // A handler that throws is caught and recorded as a failure of its case rather than taking
                    // the runner down. This is not leniency about exceptions — every one of them is a bug in
                    // this edition and is reported as a red case — it is finding 164: a runner that crashes
                    // describing a failure hides the failure.
                    encoded = Canonical.encode(handler.apply(vector.input()));
                } catch (RuntimeException error) {
                    failed += 1;
                    failures.add(new Failure(file.key(), vector.id(), vector.note(), null, null,
                            error.getClass().getSimpleName() + ": " + error.getMessage()));
                    tally.failed = true;
                    continue;
                }
                if (!encoded.equals(vector.expected())) {
                    failed += 1;
                    failures.add(new Failure(file.key(), vector.id(), vector.note(), vector.expected(),
                            encoded, null));
                    tally.failed = true;
                    continue;
                }
                passed += 1;
            }
        }

        List<String> complete = new ArrayList<>();
        List<String> partial = new ArrayList<>();
        List<String> missing = new ArrayList<>();
        for (Map.Entry<String, Tally> entry : tallies.entrySet()) {
            Tally tally = entry.getValue();
            if (tally.handled == 0) missing.add(entry.getKey());
            else if (tally.handled < tally.files || tally.failed) partial.add(entry.getKey());
            else complete.add(entry.getKey());
        }

        // A corpus with no handler at all is named once in `missingHandlers`; `declinedFiles` is for the files
        // of corpora that are partly answered, which is the bucket a reader needs in order to see that a
        // corpus is not finished.
        List<String> kept = new ArrayList<>();
        for (String key : declined) {
            String corpusName = key.substring(0, key.indexOf('/'));
            if (!missing.contains(corpusName)) kept.add(key);
        }

        complete.sort(null);
        partial.sort(null);
        missing.sort(null);
        kept.sort(null);
        return new Report(passed, failed, unanswered, complete, partial, missing, kept, failures);
    }

    private static String window(String text, int at) {
        return text.substring(Math.max(0, at - 24), Math.min(text.length(), at + 24));
    }

    /**
     * {@code file :: case}, the note, and where the two documents part company — a diff of two four-kilobyte
     * documents is not readable without that last line.
     */
    public static String describe(Failure failure) {
        StringBuilder out = new StringBuilder();
        out.append(failure.file()).append(" :: ").append(failure.id()).append("\n  ").append(failure.note());
        if (failure.error() != null) {
            return out.append("\n  error: ").append(failure.error()).toString();
        }
        String expected = failure.expected();
        String actual = failure.actual();
        out.append("\n  expected ").append(expected).append("\n  actual   ").append(actual).append("\n  ");
        int limit = Math.min(expected.length(), actual.length());
        for (int at = 0; at < limit; at++) {
            if (expected.charAt(at) != actual.charAt(at)) {
                return out.append("first difference at byte ").append(at).append(": expected \"")
                        .append(window(expected, at)).append("\", got \"").append(window(actual, at))
                        .append('"').toString();
            }
        }
        if (expected.length() == actual.length()) return out.append("identical").toString();
        return out.append("identical for ").append(limit).append(" bytes, then one is longer").toString();
    }
}
