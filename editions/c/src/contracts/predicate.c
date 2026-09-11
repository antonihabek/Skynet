#include "skynet_os/predicate.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

#include "skynet_os/js.h"
#include "skynet_os/semver.h"
#include "skynet_os/write.h"

const char *sk_outcome_name(sk_outcome_kind kind)
{
    switch (kind) {
    case SK_SATISFIED: return "satisfied";
    case SK_UNSATISFIED: return "unsatisfied";
    case SK_UNEVALUABLE: return "unevaluable";
    }
    return "unevaluable";
}

const char *sk_reason_name(sk_unevaluable_reason reason)
{
    switch (reason) {
    case SK_REASON_NONE: return NULL;
    case SK_MALFORMED: return "malformed";
    case SK_UNKNOWN_OP: return "unknown_op";
    case SK_MISSING_PATH: return "missing_path";
    case SK_MISSING_OPERAND: return "missing_operand";
    case SK_UNRESOLVED_PATH: return "unresolved_path";
    case SK_NOT_A_NUMBER: return "not_a_number";
    case SK_NOT_A_VERSION: return "not_a_version";
    }
    return NULL;
}

/* ── Paths ─────────────────────────────────────────────────────────────────── */

/* `^(0|[1-9]\d*)$` — a canonical decimal index, which is what a JSON array member is named. `01` and
 * `1.0` and `-1` are not members of anything. */
static bool canonical_index(const char *segment, size_t len, size_t *out)
{
    if (len == 0) {
        return false;
    }
    if (segment[0] == '0') {
        if (len != 1) {
            return false;
        }
        *out = 0;
        return true;
    }
    if (segment[0] < '1' || segment[0] > '9') {
        return false;
    }
    size_t value = 0;
    for (size_t at = 0; at < len; at += 1) {
        if (segment[at] < '0' || segment[at] > '9') {
            return false;
        }
        /* A path naming an index beyond any array this document could hold resolves to nothing, which
         * is the same answer as an index past the end. Bounded so the multiplication cannot wrap. */
        if (value > (SIZE_MAX - 9) / 10) {
            return false;
        }
        value = value * 10 + (size_t)(segment[at] - '0');
    }
    *out = value;
    return true;
}

const sk_value *sk_read_path(const sk_value *context, const char *path, size_t len)
{
    if (path == NULL || len == 0) {
        return NULL;
    }
    const sk_value *current = context;
    size_t at = 0;
    while (true) {
        size_t start = at;
        while (at < len && path[at] != '.') {
            at += 1;
        }
        size_t segment_len = at - start;

        /* `current === null || current === undefined` and `typeof current !== 'object'` both answer
         * undefined: a path cannot descend into a string, a number or a boolean. */
        if (current == NULL) {
            return NULL;
        }
        sk_kind kind = sk_kind_of(current);
        if (kind != SK_ARRAY && kind != SK_OBJECT) {
            return NULL;
        }

        if (kind == SK_ARRAY) {
            size_t index = 0;
            if (!canonical_index(path + start, segment_len, &index)) {
                return NULL;
            }
            /* Past the end is `undefined`, which is NULL here — not a null member. */
            current = sk_at(current, index);
        } else {
            /* `hasOwnProperty`, so a member that is present and null resolves to null and a member
             * that is absent resolves to undefined. The two are different facts throughout. */
            const sk_member *found = NULL;
            for (const sk_member *member = sk_members(current); member != NULL;
                 member = member->next) {
                if (member->key_len == segment_len &&
                    memcmp(member->key, path + start, segment_len) == 0) {
                    found = member;
                    break;
                }
            }
            if (found == NULL) {
                return NULL;
            }
            current = found->value;
        }

        if (at >= len) {
            return current;
        }
        at += 1;
    }
}

/* ── Operands ──────────────────────────────────────────────────────────────── */

/*
 * One side of a comparison.
 *
 * `path` is where the value came from, or NULL for a literal, and it is load-bearing twice: it is what
 * a refusal names, and it is how `identical` decides object equality without a reference to compare.
 */
typedef struct {
    const sk_value *value;
    const char *path;
    /* False when the path named nothing. A literal is always resolved, even a literal null. */
    bool resolved;
} operand;

/* The path a rule value references, or NULL when it is a literal. */
static const char *referenced_path(sk_arena *arena, const sk_value *value, size_t *out_len)
{
    if (sk_kind_of(value) != SK_OBJECT) {
        return NULL;
    }
    const sk_value *ref = sk_get(value, "__ref");
    if (sk_kind_of(ref) == SK_STRING) {
        return sk_as_string(ref, out_len, "");
    }
    /* `{ path: ... }` is accepted as an alias so hand-written rules read naturally — and only when it
     * is the *whole* object, so a predicate carrying both a path and an op is not one. */
    const sk_value *alias = sk_get(value, "path");
    if (sk_kind_of(alias) == SK_STRING && sk_len(value) == 1) {
        return sk_as_string(alias, out_len, "");
    }
    (void)arena;
    return NULL;
}

static operand from_path(const sk_value *context, const char *path, size_t len)
{
    operand out;
    out.value = sk_read_path(context, path, len);
    out.path = path;
    /* `value !== undefined` — a path that resolved to an explicit null *is* resolved. */
    out.resolved = out.value != NULL;
    return out;
}

static operand from_value(sk_arena *arena, const sk_value *value, const sk_value *context)
{
    size_t ref_len = 0;
    const char *ref = referenced_path(arena, value, &ref_len);
    if (ref == NULL) {
        operand out = {value, NULL, true};
        return out;
    }
    return from_path(context, ref, ref_len);
}

/* An element of a collection operand, which is a member of the context in its own right — and so has
 * a path of its own, which is what makes two elements of one array distinguishable to `identical`. */
static operand element(sk_arena *arena, const operand *collection, const sk_value *item, size_t index)
{
    operand out;
    out.value = item;
    out.resolved = true;
    if (collection->path == NULL) {
        out.path = NULL;
    } else {
        char rendered[512];
        (void)snprintf(rendered, sizeof rendered, "%s.%zu", collection->path, index);
        out.path = sk_arena_copy(arena, rendered, strlen(rendered));
    }
    return out;
}

static sk_predicate_outcome decided(bool value)
{
    sk_predicate_outcome out = {value ? SK_SATISFIED : SK_UNSATISFIED, SK_REASON_NONE, NULL, NULL};
    return out;
}

static sk_predicate_outcome unevaluable(sk_unevaluable_reason reason, const char *path,
                                        const char *op)
{
    sk_predicate_outcome out = {SK_UNEVALUABLE, reason, path, op};
    return out;
}

/* The first operand that named nothing, as a refusal. `found` false when they all resolved. */
static sk_predicate_outcome first_unresolved(const char *op, const operand *operands, size_t count,
                                             bool *found)
{
    for (size_t at = 0; at < count; at += 1) {
        if (!operands[at].resolved) {
            *found = true;
            return unevaluable(SK_UNRESOLVED_PATH, operands[at].path, op);
        }
    }
    *found = false;
    return decided(false);
}

/*
 * JavaScript's `===` on two objects is reference equality, and the reference is the specification, so
 * the rule other editions must reproduce is written here rather than left to the host language.
 *
 * Two operands are identical when both are primitives and equal, or when both were read from the
 * **same path of the same context**, which is the only way two non-primitive operands can be one
 * object. A literal object or array in a rule is therefore never equal to anything — which is exactly
 * what `===` does and is worth knowing before writing `{"op":"eq","value":["a"]}` and wondering why it
 * never fires. `includes` is the operator for that.
 */
static bool primitive(const sk_value *value)
{
    sk_kind kind = sk_kind_of(value);
    return kind != SK_ARRAY && kind != SK_OBJECT;
}

/* `===` over two primitives, including the two absences: undefined equals undefined, null equals null,
 * and the two are not each other. NaN equals nothing, itself included. */
static bool strict_equal(const sk_value *left, const sk_value *right)
{
    bool left_absent = left == NULL;
    bool right_absent = right == NULL;
    if (left_absent || right_absent) {
        return left_absent && right_absent;
    }
    if (sk_kind_of(left) != sk_kind_of(right)) {
        return false;
    }
    switch (sk_kind_of(left)) {
    case SK_NULL:
        return true;
    case SK_BOOL:
        return sk_as_bool(left, false) == sk_as_bool(right, true);
    case SK_NUMBER: {
        double a = sk_as_number(left, 0);
        double b = sk_as_number(right, 0);
        /* `NaN === NaN` is false, which `==` on doubles already gives; written as a comparison rather
         * than a bit compare for exactly that reason, and because `0 === -0` is true. */
        return a == b;
    }
    case SK_STRING: {
        size_t left_len = 0;
        size_t right_len = 0;
        const char *left_text = sk_as_string(left, &left_len, "");
        const char *right_text = sk_as_string(right, &right_len, "");
        return left_len == right_len && memcmp(left_text, right_text, left_len) == 0;
    }
    case SK_ARRAY:
    case SK_OBJECT:
        return false;
    }
    return false;
}

static bool identical(const operand *left, const operand *right)
{
    if (primitive(left->value) && primitive(right->value)) {
        return strict_equal(left->value, right->value);
    }
    if (left->path == NULL || right->path == NULL) {
        return false;
    }
    return strcmp(left->path, right->path) == 0;
}

/* `toArray`: an array is itself, an absence is empty, and anything else is a list of one. */
static size_t operand_length(const sk_value *value)
{
    if (sk_kind_of(value) == SK_ARRAY) {
        return sk_len(value);
    }
    if (sk_nullish(value)) {
        return 0;
    }
    return 1;
}

static const sk_value *operand_at(const sk_value *value, size_t index)
{
    if (sk_kind_of(value) == SK_ARRAY) {
        return sk_at(value, index);
    }
    return value;
}

/* Membership, with `identical`'s rule applied element by element. */
static bool contains(sk_arena *arena, const operand *collection, const operand *needle)
{
    size_t count = operand_length(collection->value);
    for (size_t at = 0; at < count; at += 1) {
        operand item = element(arena, collection, operand_at(collection->value, at), at);
        if (identical(&item, needle)) {
            return true;
        }
    }
    return false;
}

/*
 * `toNumber`: a finite number is itself, a non-blank string goes through `Number(...)`, and everything
 * else — a boolean, an array, an absence, a blank string — is not a number.
 *
 * `has` false is the reference's `null` return, which the numeric operators name as `not_a_number`.
 */
static bool to_number(sk_arena *arena, const sk_value *value, double *out)
{
    if (sk_kind_of(value) == SK_NUMBER) {
        double number = sk_as_number(value, 0);
        if (!isfinite(number)) {
            return false;
        }
        *out = number;
        return true;
    }
    if (sk_kind_of(value) == SK_STRING) {
        size_t len = 0;
        const char *text = sk_as_string(value, &len, "");
        size_t trimmed_len = 0;
        (void)sk_js_trim(arena, text, len, &trimmed_len);
        /* A blank string converts to zero, and the reference refuses it before converting: a robot
         * that reported `"   "` for its battery has not reported a number. */
        if (trimmed_len == 0) {
            return false;
        }
        double parsed = sk_js_string_to_number(arena, text, len);
        if (!isfinite(parsed)) {
            return false;
        }
        *out = parsed;
        return true;
    }
    return false;
}

/* ── The evaluator ─────────────────────────────────────────────────────────── */

/* The predicate's own path, or NULL when it has none this edition can read. An empty path is none. */
static const char *path_of(const sk_value *predicate, size_t *out_len)
{
    const sk_value *path = sk_get(predicate, "path");
    if (sk_kind_of(path) != SK_STRING) {
        return NULL;
    }
    const char *text = sk_as_string(path, out_len, "");
    return *out_len == 0 ? NULL : text;
}

/*
 * The children of a composite, or "broken" when `predicates` is present and is not a list.
 *
 * Absent and non-array are deliberately different answers. Absent is *no children*, and a conjunction
 * over no children is satisfied — a skill declaring no preconditions does not fail them. A
 * `predicates` that is a string is a **broken rule**, and answering "no children" there would turn it
 * into a pass, which is the failure this module exists to stop.
 */
static bool children_of(const sk_value *predicate, const sk_value **children)
{
    const sk_value *held = sk_get(predicate, "predicates");
    if (sk_nullish(held)) {
        *children = NULL;
        return true;
    }
    if (sk_kind_of(held) != SK_ARRAY) {
        return false;
    }
    *children = held;
    return true;
}

static bool op_is(const sk_value *op, const char *name)
{
    return sk_string_is(op, name);
}

sk_predicate_outcome sk_predicate_evaluate_outcome(sk_arena *arena, const sk_value *predicate,
                                                   const sk_value *context)
{
    /* `!predicate || typeof predicate !== 'object' || typeof predicate.op !== 'string'` — there is no
     * predicate here, and this is the line that makes the module total. */
    if (sk_kind_of(predicate) != SK_OBJECT) {
        return unevaluable(SK_MALFORMED, NULL, NULL);
    }
    const sk_value *op_value = sk_get(predicate, "op");
    if (sk_kind_of(op_value) != SK_STRING) {
        return unevaluable(SK_MALFORMED, NULL, NULL);
    }
    size_t op_len = 0;
    const char *op = sk_as_string(op_value, &op_len, "");
    op = sk_arena_copy(arena, op, op_len);

    size_t path_len = 0;
    const char *path = path_of(predicate, &path_len);

    if (op_is(op_value, "and") || op_is(op_value, "or")) {
        const sk_value *children = NULL;
        if (!children_of(predicate, &children)) {
            return unevaluable(SK_MALFORMED, NULL, op);
        }
        bool conjunction = op_is(op_value, "and");
        /* Order is part of the answer: a refusal names *a* predicate, and which one it names must not
         * depend on how an edition iterates. */
        bool have_unevaluable = false;
        sk_predicate_outcome first_unevaluable = decided(false);
        for (size_t at = 0; at < sk_len(children); at += 1) {
            sk_predicate_outcome outcome =
                sk_predicate_evaluate_outcome(arena, sk_at(children, at), context);
            if (conjunction) {
                if (outcome.outcome != SK_SATISFIED) {
                    return outcome;
                }
                continue;
            }
            if (outcome.outcome == SK_SATISFIED) {
                return outcome;
            }
            if (outcome.outcome == SK_UNEVALUABLE && !have_unevaluable) {
                have_unevaluable = true;
                first_unevaluable = outcome;
            }
        }
        if (conjunction) {
            /* Nothing was asked for. A skill declaring no preconditions is not one that fails them. */
            return decided(true);
        }
        /* An alternative that held would have returned already; an alternative nobody could read means
         * this disjunction is not known to be false, only unread. */
        return have_unevaluable ? first_unevaluable : decided(false);
    }

    if (op_is(op_value, "not")) {
        const sk_value *children = NULL;
        if (!children_of(predicate, &children)) {
            return unevaluable(SK_MALFORMED, NULL, op);
        }
        const sk_value *child = sk_get(predicate, "predicate");
        if (child == NULL) {
            child = sk_at(children, 0);
        }
        if (child == NULL) {
            return unevaluable(SK_MISSING_OPERAND, NULL, op);
        }
        sk_predicate_outcome outcome = sk_predicate_evaluate_outcome(arena, child, context);
        /* Ignorance does not negate into knowledge, and this is the line that used to say it did. */
        if (outcome.outcome == SK_UNEVALUABLE) {
            return outcome;
        }
        return decided(outcome.outcome == SK_UNSATISFIED);
    }

    if (op_is(op_value, "exists")) {
        if (path == NULL) {
            return unevaluable(SK_MISSING_PATH, NULL, op);
        }
        /* The one operator that answers *about* resolution, so it never refuses for lack of it. */
        const sk_value *value = sk_read_path(context, path, path_len);
        if (sk_nullish(value)) {
            return decided(false);
        }
        if (sk_kind_of(value) == SK_ARRAY) {
            return decided(sk_len(value) > 0);
        }
        if (sk_kind_of(value) == SK_STRING) {
            size_t len = 0;
            const char *text = sk_as_string(value, &len, "");
            size_t trimmed_len = 0;
            (void)sk_js_trim(arena, text, len, &trimmed_len);
            return decided(trimmed_len != 0);
        }
        return decided(true);
    }

    bool equality = op_is(op_value, "eq") || op_is(op_value, "neq");
    bool membership = op_is(op_value, "in") || op_is(op_value, "includes");
    bool coverage = op_is(op_value, "includes_all") || op_is(op_value, "includes_any");
    bool numeric = op_is(op_value, "gte") || op_is(op_value, "lte") || op_is(op_value, "gt") ||
                   op_is(op_value, "lt");
    bool version_range = op_is(op_value, "semver_satisfies");

    if (equality || membership || coverage || numeric || version_range) {
        if (path == NULL) {
            return unevaluable(SK_MISSING_PATH, NULL, op);
        }
        operand left = from_path(context, path, path_len);
        operand right = from_value(arena, sk_get(predicate, "value"), context);

        /* The order the two operands are checked in is the order the reference passes them, and it
         * decides which path a refusal names when both are unresolved. `in` is the one operator whose
         * left-hand side is the needle rather than the collection. */
        operand operands[2];
        operands[0] = left;
        operands[1] = right;
        bool refused = false;
        sk_predicate_outcome refusal = first_unresolved(op, operands, 2, &refused);
        if (refused) {
            return refusal;
        }

        if (equality) {
            bool equal = identical(&left, &right);
            return decided(op_is(op_value, "eq") ? equal : !equal);
        }
        if (membership) {
            /* `in` reads the path as the needle and the value as the collection; `includes` is the
             * other way round. Two operators, one comparison. */
            if (op_is(op_value, "in")) {
                return decided(contains(arena, &right, &left));
            }
            return decided(contains(arena, &left, &right));
        }
        if (coverage) {
            size_t count = operand_length(right.value);
            bool any = false;
            bool all = true;
            for (size_t at = 0; at < count; at += 1) {
                operand wanted = element(arena, &right, operand_at(right.value, at), at);
                if (contains(arena, &left, &wanted)) {
                    any = true;
                } else {
                    all = false;
                }
            }
            /* An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from
             * a requirement that *is* empty, never from one nobody could read. */
            if (op_is(op_value, "includes_any")) {
                return decided(count == 0 || any);
            }
            return decided(all);
        }
        if (numeric) {
            double left_number = 0;
            if (!to_number(arena, left.value, &left_number)) {
                return unevaluable(SK_NOT_A_NUMBER, left.path, op);
            }
            double right_number = 0;
            if (!to_number(arena, right.value, &right_number)) {
                return unevaluable(SK_NOT_A_NUMBER, right.path, op);
            }
            if (op_is(op_value, "gte")) {
                return decided(left_number >= right_number);
            }
            if (op_is(op_value, "lte")) {
                return decided(left_number <= right_number);
            }
            if (op_is(op_value, "gt")) {
                return decided(left_number > right_number);
            }
            return decided(left_number < right_number);
        }

        /* A version this edition cannot parse is a fact about the declaration, not a version that fell
         * outside the window — so it is named rather than counted as a miss. */
        size_t version_len = 0;
        const char *version = sk_as_string(left.value, &version_len, "");
        if (sk_kind_of(left.value) != SK_STRING ||
            !sk_semver_valid_version(arena, version, version_len)) {
            return unevaluable(SK_NOT_A_VERSION, left.path, op);
        }
        size_t range_len = 0;
        const char *range = sk_as_string(right.value, &range_len, "");
        if (sk_kind_of(right.value) != SK_STRING ||
            !sk_semver_valid_range(arena, range, range_len)) {
            return unevaluable(SK_NOT_A_VERSION, right.path, op);
        }
        return decided(sk_semver_satisfies(arena, version, version_len, range, range_len));
    }

    return unevaluable(SK_UNKNOWN_OP, path, op);
}

bool sk_predicate_satisfied(const sk_predicate_outcome *outcome)
{
    return outcome->outcome == SK_SATISFIED;
}

bool sk_predicate_evaluate(sk_arena *arena, const sk_value *predicate, const sk_value *context)
{
    sk_predicate_outcome outcome = sk_predicate_evaluate_outcome(arena, predicate, context);
    return sk_predicate_satisfied(&outcome);
}

const sk_value *sk_predicate_evaluate_case(sk_arena *arena, const sk_value *input,
                                            const char **error)
{
    (void)error;
    const sk_value *predicate = sk_get(input, "predicate");
    const sk_value *context = sk_get(input, "context");
    sk_predicate_outcome outcome = sk_predicate_evaluate_outcome(arena, predicate, context);

    /*
     * The boolean and the outcome in one document, deliberately. `evaluatePredicate` is defined as
     * `satisfied(...)` of the outcome, so an edition that computed the two separately would be free to
     * disagree with itself — and the whole argument for the third value is that it refines the boolean
     * rather than offering a second opinion. Freezing both makes that checkable.
     */
    sk_value *out = sk_object(arena);
    sk_set(arena, out, "satisfied", sk_bool(sk_predicate_satisfied(&outcome)));
    sk_set(arena, out, "outcome", sk_string(arena, sk_outcome_name(outcome.outcome)));
    const char *reason = sk_reason_name(outcome.reason);
    sk_set(arena, out, "reason", reason == NULL ? sk_null() : sk_string(arena, reason));
    sk_set(arena, out, "path", outcome.path == NULL ? sk_null() : sk_string(arena, outcome.path));
    sk_set(arena, out, "op", outcome.op == NULL ? sk_null() : sk_string(arena, outcome.op));
    return out;
}
