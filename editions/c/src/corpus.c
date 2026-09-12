#include "skynet_os/corpus.h"

#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "skynet_os/parse.h"
#include "skynet_os/write.h"

/* A growable vector of pointers, which four of the report's buckets and the file list all are. */
typedef struct {
    sk_arena *arena;
    const void **items;
    size_t count;
    size_t capacity;
} vec;

static void vec_init(vec *v, sk_arena *arena)
{
    v->arena = arena;
    v->items = NULL;
    v->count = 0;
    v->capacity = 0;
}

static void vec_push(vec *v, const void *item)
{
    if (v->count == v->capacity) {
        size_t capacity = v->capacity == 0 ? 16 : v->capacity * 2;
        const void **items = sk_arena_alloc_array(v->arena, capacity, sizeof *items);
        if (v->count != 0) {
            memcpy(items, v->items, v->count * sizeof *items);
        }
        v->items = items;
        v->capacity = capacity;
    }
    v->items[v->count] = item;
    v->count += 1;
}

static int compare_strings(const void *left, const void *right)
{
    return strcmp(*(const char *const *)left, *(const char *const *)right);
}

static const char *join(sk_arena *arena, const char *left, const char *right)
{
    size_t left_len = strlen(left);
    size_t right_len = strlen(right);
    bool needs_separator = left_len != 0 && left[left_len - 1] != '/';
    size_t len = left_len + (needs_separator ? 1 : 0) + right_len;
    char *out = sk_arena_alloc(arena, len + 1);
    memcpy(out, left, left_len);
    size_t at = left_len;
    if (needs_separator) {
        out[at] = '/';
        at += 1;
    }
    memcpy(out + at, right, right_len);
    out[len] = '\0';
    return out;
}

static bool is_directory(const char *path)
{
    struct stat info;
    return stat(path, &info) == 0 && S_ISDIR(info.st_mode);
}

static bool is_file(const char *path)
{
    struct stat info;
    return stat(path, &info) == 0 && S_ISREG(info.st_mode);
}

const char *sk_corpus_root(sk_arena *arena, const char *explicit_root)
{
    if (explicit_root != NULL && explicit_root[0] != '\0') {
        return is_directory(explicit_root) ? explicit_root : NULL;
    }
    const char *from_environment = getenv("SKYNET_CORPUS");
    if (from_environment != NULL && from_environment[0] != '\0') {
        return is_directory(from_environment) ? from_environment : NULL;
    }

    /*
     * Walk up from the working directory looking for a `corpus/README.md`, not merely a directory
     * called `corpus`. The README is what makes the answer unambiguous: a build tree with an empty
     * `corpus/` in it would otherwise be found first and measured as zero cases.
     */
    char here[4096];
    if (getcwd(here, sizeof here) == NULL) {
        return NULL;
    }
    for (int levels = 0; levels < 64; levels += 1) {
        const char *candidate = join(arena, here, "corpus");
        if (is_file(join(arena, candidate, "README.md"))) {
            return candidate;
        }
        char *slash = strrchr(here, '/');
        if (slash == NULL || slash == here) {
            return NULL;
        }
        *slash = '\0';
    }
    return NULL;
}

static const char *read_file(sk_arena *arena, const char *path, size_t *len, const char **error)
{
    FILE *handle = fopen(path, "rb");
    if (handle == NULL) {
        *error = "cannot be opened";
        return NULL;
    }
    sk_buf out;
    sk_buf_init(&out, arena);
    char chunk[65536];
    size_t read = 0;
    while ((read = fread(chunk, 1, sizeof chunk, handle)) != 0) {
        sk_buf_append(&out, chunk, read);
    }
    bool failed = ferror(handle) != 0;
    (void)fclose(handle);
    if (failed) {
        *error = "could not be read to the end";
        return NULL;
    }
    *len = out.len;
    return out.bytes == NULL ? "" : out.bytes;
}

/* The stem of `scaling.json`, or NULL for a name that is not a `.json` file. */
static const char *stem(sk_arena *arena, const char *name)
{
    size_t len = strlen(name);
    if (len <= 5 || strcmp(name + len - 5, ".json") != 0) {
        return NULL;
    }
    return sk_arena_copy(arena, name, len - 5);
}

static const char **entries(sk_arena *arena, const char *path, size_t *count)
{
    DIR *directory = opendir(path);
    if (directory == NULL) {
        *count = 0;
        return NULL;
    }
    vec found;
    vec_init(&found, arena);
    struct dirent *entry = NULL;
    while ((entry = readdir(directory)) != NULL) {
        if (entry->d_name[0] == '.') {
            continue;
        }
        vec_push(&found, sk_arena_copy(arena, entry->d_name, strlen(entry->d_name)));
    }
    (void)closedir(directory);
    if (found.count > 1) {
        qsort(found.items, found.count, sizeof *found.items, compare_strings);
    }
    *count = found.count;
    return (const char **)found.items;
}

sk_file *sk_corpus_load(sk_arena *arena, const char *root, size_t *count, const char **error)
{
    size_t corpus_count = 0;
    const char **corpora = entries(arena, root, &corpus_count);
    if (corpora == NULL) {
        *error = "the corpus directory cannot be listed";
        return NULL;
    }

    vec files;
    vec_init(&files, arena);
    for (size_t at = 0; at < corpus_count; at += 1) {
        const char *corpus_path = join(arena, root, corpora[at]);
        if (!is_directory(corpus_path)) {
            continue;
        }
        size_t name_count = 0;
        const char **names = entries(arena, corpus_path, &name_count);
        for (size_t which = 0; which < name_count; which += 1) {
            const char *name = stem(arena, names[which]);
            if (name == NULL) {
                continue;
            }
            const char *path = join(arena, corpus_path, names[which]);
            size_t len = 0;
            const char *bytes = read_file(arena, path, &len, error);
            if (bytes == NULL) {
                return NULL;
            }
            sk_parse_error parse_error;
            const sk_value *document = sk_parse(arena, bytes, len, &parse_error);
            if (document == NULL) {
                char message[512];
                (void)snprintf(message, sizeof message, "%s is not JSON: %s", path,
                               sk_parse_error_text(arena, &parse_error));
                *error = sk_arena_copy(arena, message, strlen(message));
                return NULL;
            }

            const sk_value *cases = sk_get(document, "cases");
            if (sk_kind_of(cases) != SK_ARRAY) {
                char message[512];
                (void)snprintf(message, sizeof message, "%s has no `cases` array", path);
                *error = sk_arena_copy(arena, message, strlen(message));
                return NULL;
            }

            sk_file *file = sk_arena_alloc(arena, sizeof *file);
            file->path = path;
            file->corpus = corpora[at];
            file->name = name;
            file->key = join(arena, corpora[at], name);
            file->count = sk_len(cases);
            file->cases = sk_arena_alloc_array(arena, file->count + 1, sizeof *file->cases);
            for (size_t index = 0; index < file->count; index += 1) {
                const sk_value *vector = sk_at(cases, index);
                sk_case *held = &file->cases[index];
                held->id = sk_as_string(sk_get(vector, "id"), NULL, "");
                held->note = sk_as_string(sk_get(vector, "note"), NULL, "");
                held->input = sk_get(vector, "input");
                held->expected = sk_as_string(sk_get(vector, "expected"), &held->expected_len, "");
                if (sk_kind_of(sk_get(vector, "expected")) != SK_STRING) {
                    char message[512];
                    (void)snprintf(message, sizeof message,
                                   "%s case %s has no `expected` string. corpus/README.md's first "
                                   "property is that the bytes are what the file holds.",
                                   path, held->id);
                    *error = sk_arena_copy(arena, message, strlen(message));
                    return NULL;
                }
            }
            vec_push(&files, file);
        }
    }

    *count = files.count;
    /* The vector holds `sk_file *`; the caller wants an array of `sk_file`. Copied rather than
     * handed a pointer array, so a caller indexes cases the way the loop above wrote them. */
    sk_file *out = sk_arena_alloc_array(arena, files.count + 1, sizeof *out);
    for (size_t at = 0; at < files.count; at += 1) {
        out[at] = *(const sk_file *)files.items[at];
    }
    return out;
}

static sk_handler lookup(const sk_dispatch_entry *dispatch, size_t count, const char *key)
{
    for (size_t at = 0; at < count; at += 1) {
        if (strcmp(dispatch[at].key, key) == 0) {
            return dispatch[at].handler;
        }
    }
    return NULL;
}

/* Names, deduplicated, in the order they are first seen; sorted by the caller. */
typedef struct {
    const char *name;
    size_t files;
    size_t handled;
    bool failed;
} corpus_tally;

static corpus_tally *tally_for(corpus_tally *tallies, size_t *count, const char *name)
{
    for (size_t at = 0; at < *count; at += 1) {
        if (strcmp(tallies[at].name, name) == 0) {
            return &tallies[at];
        }
    }
    tallies[*count].name = name;
    tallies[*count].files = 0;
    tallies[*count].handled = 0;
    tallies[*count].failed = false;
    *count += 1;
    return &tallies[*count - 1];
}

sk_report sk_corpus_run(sk_arena *arena, const sk_file *files, size_t file_count,
                        const sk_dispatch_entry *dispatch, size_t dispatch_count)
{
    sk_report report;
    memset(&report, 0, sizeof report);

    vec failures;
    vec_init(&failures, arena);
    vec declined;
    vec_init(&declined, arena);
    corpus_tally *tallies = sk_arena_alloc_array(arena, file_count + 1, sizeof *tallies);
    size_t tally_count = 0;

    for (size_t at = 0; at < file_count; at += 1) {
        const sk_file *file = &files[at];
        corpus_tally *tally = tally_for(tallies, &tally_count, file->corpus);
        tally->files += 1;

        sk_handler handler = lookup(dispatch, dispatch_count, file->key);
        if (handler == NULL) {
            report.unanswered += (int)file->count;
            vec_push(&declined, file->key);
            continue;
        }
        tally->handled += 1;

        for (size_t index = 0; index < file->count; index += 1) {
            const sk_case *vector = &file->cases[index];
            /*
             * One arena per case, freed after it. This is what makes "no contract contains a free"
             * affordable: the whole region a case allocated goes at once, so a run of 1,199 cases
             * holds one case's working set rather than all of them.
             */
            sk_arena *scratch = sk_arena_new();
            const char *error = NULL;
            const sk_value *actual = handler(scratch, vector->input, &error);
            const char *encoded = NULL;
            size_t encoded_len = 0;
            if (actual != NULL) {
                encoded = sk_encode(scratch, actual, &encoded_len, &error);
            }

            if (actual == NULL || encoded == NULL) {
                report.failed += 1;
                sk_failure *failure = sk_arena_alloc(arena, sizeof *failure);
                failure->file = file->key;
                failure->id = vector->id;
                failure->note = vector->note;
                failure->error = error == NULL ? "the handler declined the case and said no why" : error;
                failure->expected = NULL;
                failure->actual = NULL;
                vec_push(&failures, failure);
                tally->failed = true;
                sk_arena_free(scratch);
                continue;
            }

            if (encoded_len != vector->expected_len ||
                memcmp(encoded, vector->expected, encoded_len) != 0) {
                report.failed += 1;
                sk_failure *failure = sk_arena_alloc(arena, sizeof *failure);
                failure->file = file->key;
                failure->id = vector->id;
                failure->note = vector->note;
                failure->expected = vector->expected;
                /* Copied into the run's arena before the case's is freed. */
                failure->actual = sk_arena_copy(arena, encoded, encoded_len);
                failure->error = NULL;
                vec_push(&failures, failure);
                tally->failed = true;
                sk_arena_free(scratch);
                continue;
            }

            report.passed += 1;
            sk_arena_free(scratch);
        }
    }

    vec complete;
    vec_init(&complete, arena);
    vec partial;
    vec_init(&partial, arena);
    vec missing;
    vec_init(&missing, arena);
    for (size_t at = 0; at < tally_count; at += 1) {
        const corpus_tally *tally = &tallies[at];
        if (tally->handled == 0) {
            vec_push(&missing, tally->name);
        } else if (tally->handled < tally->files || tally->failed) {
            vec_push(&partial, tally->name);
        } else {
            vec_push(&complete, tally->name);
        }
    }

    /* A corpus with no handler at all is named once in `missing_handlers`; `declined_files` is for
     * the files of corpora that are partly answered, which is the bucket a reader needs in order to
     * see that `transport` is not finished. */
    vec kept;
    vec_init(&kept, arena);
    for (size_t at = 0; at < declined.count; at += 1) {
        const char *key = declined.items[at];
        const char *slash = strchr(key, '/');
        size_t corpus_len = slash == NULL ? strlen(key) : (size_t)(slash - key);
        bool whole_corpus_missing = false;
        for (size_t which = 0; which < missing.count; which += 1) {
            const char *name = missing.items[which];
            if (strlen(name) == corpus_len && memcmp(name, key, corpus_len) == 0) {
                whole_corpus_missing = true;
                break;
            }
        }
        if (!whole_corpus_missing) {
            vec_push(&kept, key);
        }
    }

    if (complete.count > 1) {
        qsort(complete.items, complete.count, sizeof *complete.items, compare_strings);
    }
    if (partial.count > 1) {
        qsort(partial.items, partial.count, sizeof *partial.items, compare_strings);
    }
    if (missing.count > 1) {
        qsort(missing.items, missing.count, sizeof *missing.items, compare_strings);
    }
    if (kept.count > 1) {
        qsort(kept.items, kept.count, sizeof *kept.items, compare_strings);
    }

    report.complete = (const char **)complete.items;
    report.complete_count = complete.count;
    report.partial = (const char **)partial.items;
    report.partial_count = partial.count;
    report.missing_handlers = (const char **)missing.items;
    report.missing_count = missing.count;
    report.declined_files = (const char **)kept.items;
    report.declined_count = kept.count;

    report.failures = sk_arena_alloc_array(arena, failures.count + 1, sizeof *report.failures);
    for (size_t at = 0; at < failures.count; at += 1) {
        report.failures[at] = *(const sk_failure *)failures.items[at];
    }
    report.failure_count = failures.count;
    return report;
}

int sk_report_total(const sk_report *report)
{
    return report->passed + report->failed + report->unanswered;
}

static const char *window(sk_arena *arena, const char *text, size_t at)
{
    size_t len = strlen(text);
    size_t start = at > 24 ? at - 24 : 0;
    size_t end = at + 24 < len ? at + 24 : len;
    return sk_arena_copy(arena, text + start, end - start);
}

static const char *first_difference(sk_arena *arena, const char *expected, const char *actual)
{
    size_t expected_len = strlen(expected);
    size_t actual_len = strlen(actual);
    size_t limit = expected_len < actual_len ? expected_len : actual_len;
    char rendered[512];
    for (size_t at = 0; at < limit; at += 1) {
        if (expected[at] != actual[at]) {
            (void)snprintf(rendered, sizeof rendered,
                           "first difference at byte %zu: expected \"%s\", got \"%s\"", at,
                           window(arena, expected, at), window(arena, actual, at));
            return sk_arena_copy(arena, rendered, strlen(rendered));
        }
    }
    if (expected_len == actual_len) {
        return "identical";
    }
    (void)snprintf(rendered, sizeof rendered, "identical for %zu bytes, then one is longer", limit);
    return sk_arena_copy(arena, rendered, strlen(rendered));
}

const char *sk_failure_describe(sk_arena *arena, const sk_failure *failure)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    sk_buf_append_str(&out, failure->file);
    sk_buf_append_str(&out, " :: ");
    sk_buf_append_str(&out, failure->id);
    sk_buf_append_str(&out, "\n  ");
    sk_buf_append_str(&out, failure->note);
    if (failure->error != NULL) {
        sk_buf_append_str(&out, "\n  error: ");
        sk_buf_append_str(&out, failure->error);
        return out.bytes;
    }
    sk_buf_append_str(&out, "\n  expected ");
    sk_buf_append_str(&out, failure->expected);
    sk_buf_append_str(&out, "\n  actual   ");
    sk_buf_append_str(&out, failure->actual);
    sk_buf_append_str(&out, "\n  ");
    sk_buf_append_str(&out, first_difference(arena, failure->expected, failure->actual));
    return out.bytes;
}
