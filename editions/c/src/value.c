#include "skynet_os/value.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * `null`, `true` and `false` are three values with no fields, so there are three of them for the
 * whole process rather than one per construction. They are the only objects in this edition not
 * owned by an arena, which is why they are `const` and why nothing writes through a value pointer.
 */
static const sk_value SK_TRUE = {.kind = SK_BOOL, .truth = true};
static const sk_value SK_FALSE = {.kind = SK_BOOL, .truth = false};
static const sk_value SK_NULL_VALUE = {.kind = SK_NULL};

const sk_value *sk_null(void)
{
    return &SK_NULL_VALUE;
}

const sk_value *sk_bool(bool truth)
{
    return truth ? &SK_TRUE : &SK_FALSE;
}

const sk_value *sk_number(sk_arena *arena, double number)
{
    sk_value *value = sk_arena_alloc(arena, sizeof *value);
    value->kind = SK_NUMBER;
    value->number = number;
    return value;
}

const sk_value *sk_string_len(sk_arena *arena, const char *text, size_t len)
{
    sk_value *value = sk_arena_alloc(arena, sizeof *value);
    value->kind = SK_STRING;
    value->text = sk_arena_copy(arena, text, len);
    value->text_len = len;
    return value;
}

const sk_value *sk_string(sk_arena *arena, const char *text)
{
    return sk_string_len(arena, text, strlen(text));
}

sk_value *sk_array(sk_arena *arena)
{
    sk_value *value = sk_arena_alloc(arena, sizeof *value);
    value->kind = SK_ARRAY;
    return value;
}

void sk_push(sk_arena *arena, sk_value *array, const sk_value *item)
{
    if (item == NULL) {
        (void)fprintf(stderr, "skynet-corpus-c: sk_push of a NULL item\n");
        abort();
    }
    if (array->count == array->capacity) {
        /* Doubling from four. The arena does not reclaim the old block, which is the cost of not
         * having a free: an array grown to a thousand elements has left ~1k pointers behind it in
         * the region. Measured against the alternative — a per-array free list in a value model
         * whose whole point is that nothing here frees — that is the cheaper trade, and the region
         * is reset per corpus case. */
        size_t capacity = array->capacity == 0 ? 4 : array->capacity * 2;
        const sk_value **items = sk_arena_alloc_array(arena, capacity, sizeof *items);
        if (array->count != 0) {
            memcpy(items, array->items, array->count * sizeof *items);
        }
        array->items = items;
        array->capacity = capacity;
    }
    array->items[array->count] = item;
    array->count += 1;
}

sk_value *sk_object(sk_arena *arena)
{
    sk_value *value = sk_arena_alloc(arena, sizeof *value);
    value->kind = SK_OBJECT;
    return value;
}

static sk_member *find(const sk_value *object, const char *key, size_t key_len)
{
    if (object == NULL || object->kind != SK_OBJECT) {
        return NULL;
    }
    for (sk_member *member = object->members; member != NULL; member = member->next) {
        if (member->key_len == key_len && memcmp(member->key, key, key_len) == 0) {
            return member;
        }
    }
    return NULL;
}

void sk_set(sk_arena *arena, sk_value *object, const char *key, const sk_value *value)
{
    if (value == NULL) {
        (void)fprintf(stderr, "skynet-corpus-c: sk_set of a NULL value for %s\n", key);
        abort();
    }
    size_t key_len = strlen(key);
    sk_member *existing = find(object, key, key_len);
    if (existing != NULL) {
        existing->value = value;
        return;
    }
    sk_member *member = sk_arena_alloc(arena, sizeof *member);
    member->key = sk_arena_copy(arena, key, key_len);
    member->key_len = key_len;
    member->value = value;
    member->next = NULL;
    if (object->last == NULL) {
        object->members = member;
    } else {
        object->last->next = member;
    }
    object->last = member;
    object->count += 1;
}

void sk_set_optional(sk_arena *arena, sk_value *object, const char *key, const sk_value *value)
{
    if (value == NULL) {
        return;
    }
    sk_set(arena, object, key, value);
}

sk_kind sk_kind_of(const sk_value *value)
{
    return value == NULL ? SK_NULL : value->kind;
}

bool sk_is_null(const sk_value *value)
{
    return value != NULL && value->kind == SK_NULL;
}

bool sk_nullish(const sk_value *value)
{
    return value == NULL || value->kind == SK_NULL;
}

bool sk_as_bool(const sk_value *value, bool fallback)
{
    if (value == NULL || value->kind != SK_BOOL) {
        return fallback;
    }
    return value->truth;
}

double sk_as_number(const sk_value *value, double fallback)
{
    if (value == NULL || value->kind != SK_NUMBER) {
        return fallback;
    }
    return value->number;
}

const char *sk_as_string(const sk_value *value, size_t *len, const char *fallback)
{
    if (value == NULL || value->kind != SK_STRING) {
        if (len != NULL) {
            *len = strlen(fallback);
        }
        return fallback;
    }
    if (len != NULL) {
        *len = value->text_len;
    }
    return value->text;
}

bool sk_string_is(const sk_value *value, const char *literal)
{
    if (value == NULL || value->kind != SK_STRING) {
        return false;
    }
    size_t len = strlen(literal);
    return value->text_len == len && memcmp(value->text, literal, len) == 0;
}

size_t sk_len(const sk_value *value)
{
    if (value == NULL) {
        return 0;
    }
    if (value->kind == SK_ARRAY || value->kind == SK_OBJECT) {
        return value->count;
    }
    return 0;
}

const sk_value *sk_at(const sk_value *value, size_t index)
{
    if (value == NULL || value->kind != SK_ARRAY || index >= value->count) {
        return NULL;
    }
    return value->items[index];
}

const sk_value *sk_get(const sk_value *value, const char *key)
{
    sk_member *member = find(value, key, strlen(key));
    return member == NULL ? NULL : member->value;
}

const sk_member *sk_members(const sk_value *value)
{
    if (value == NULL || value->kind != SK_OBJECT) {
        return NULL;
    }
    return value->members;
}

bool sk_has(const sk_value *value, const char *key)
{
    return find(value, key, strlen(key)) != NULL;
}
