// The canonical JSON value model — this edition's half of `reference/typescript/canonical.ts`.
//
// §31.4 requires byte-identical output, and corpus/README.md states the form: no insignificant whitespace,
// object keys sorted by their UTF-8 bytes, numbers finite and positional, only the quote, the backslash and the
// C0 controls escaped, absent fields omitted and `null` written.
//
// A `string` here is UTF-16, exactly as a JavaScript string is, so there is no decision to make about
// representing the reference's strings. Rule 2 is where that stops being true: keys sort by their UTF-8
// **bytes**, and `string.CompareOrdinal` compares UTF-16 code units. The two orders agree for every key in the
// basic multilingual plane and disagree above it.
//
// Absent and null are different facts. A field never set is the reference's `undefined`, which rule 5 omits; a
// field set to `Value.Null` is JSON null, which rule 5 writes. `Set` refuses a C# `null` outright so the third
// state cannot exist.

using System;
using System.Collections.Generic;

namespace Skynet.Os;

internal enum Kind
{
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

internal readonly record struct Member(string Key, Value Value);

internal sealed class Value
{
    internal static readonly Value Null = new(Kind.Null);
    internal static readonly Value True = new(Kind.Bool);
    internal static readonly Value False = new(Kind.Bool);

    private readonly double number;
    private readonly string text;
    private readonly List<Value>? items;
    private readonly List<Member>? members;

    private Value(Kind kind)
    {
        ValueKind = kind;
        text = string.Empty;
        if (kind == Kind.Array)
        {
            items = new List<Value>();
        }
        else if (kind == Kind.Object)
        {
            members = new List<Member>();
        }
    }

    private Value(double held)
    {
        ValueKind = Kind.Number;
        number = held;
        text = string.Empty;
    }

    private Value(string held)
    {
        ValueKind = Kind.String;
        text = held;
    }

    internal Kind ValueKind { get; }

    internal double Number => number;

    internal string Text => text;

    internal List<Value> Items => items ?? throw new InvalidOperationException("not an array");

    internal List<Member> Members => members ?? throw new InvalidOperationException("not an object");

    internal static Value Of(bool truth) => truth ? True : False;

    internal static Value Of(double held) => new(held);

    internal static Value Of(string held) => new(held);

    internal static Value Array() => new(Kind.Array);

    internal static Value Object() => new(Kind.Object);

    internal Value Push(Value item)
    {
        ArgumentNullException.ThrowIfNull(item);
        Items.Add(item);
        return this;
    }

    /// <summary>
    /// Append <c>key</c>, replacing a key already present <b>in place</b> so it keeps its original position —
    /// which is what assigning to a field twice does in the reference, and matters because the order a contract
    /// inserted in is what a failure prints.
    /// </summary>
    internal Value Set(string key, Value value)
    {
        if (value is null)
        {
            throw new ArgumentNullException(
                nameof(value),
                "Value.Null is JSON null and omitting the call is the reference's undefined. A third state "
                    + "would make rule 5 undecidable.");
        }
        List<Member> held = Members;
        for (int at = 0; at < held.Count; at++)
        {
            if (string.Equals(held[at].Key, key, StringComparison.Ordinal))
            {
                held[at] = new Member(key, value);
                return this;
            }
        }
        held.Add(new Member(key, value));
        return this;
    }

    /// <summary>
    /// <c>Set</c> unless <c>value</c> is null — the reference's <c>undefined</c>, which rule 5 omits. Spelled
    /// differently from <c>Set</c> so that omitting a field is a decision at the call site.
    /// </summary>
    internal Value SetOptional(string key, Value? value) => value is null ? this : Set(key, value);

    // ── Reading ──────────────────────────────────────────────────────────────
    //
    // Every accessor is total: a kind mismatch answers the fallback rather than throwing, because a contract
    // reading a document reads whatever the document holds — exactly as the reference does when a field's
    // declared type and its actual value disagree.

    /// <summary>
    /// True when the field is absent (a C# null) or present and JSON null — the reference's <c>x == null</c>,
    /// which is the loose comparison it uses throughout and which is true of both.
    /// </summary>
    internal static bool Nullish(Value? value) => value is null || value.ValueKind == Kind.Null;

    internal static bool AsBool(Value? value, bool fallback) =>
        value is null || value.ValueKind != Kind.Bool ? fallback : ReferenceEquals(value, True);

    internal static double AsNumber(Value? value, double fallback) =>
        value is null || value.ValueKind != Kind.Number ? fallback : value.Number;

    internal static string AsString(Value? value, string fallback) =>
        value is null || value.ValueKind != Kind.String ? fallback : value.Text;

    internal static bool StringIs(Value? value, string literal) =>
        value is not null && value.ValueKind == Kind.String
            && string.Equals(value.Text, literal, StringComparison.Ordinal);

    internal static int Length(Value? value) => value?.ValueKind switch
    {
        Kind.Array => value.Items.Count,
        Kind.Object => value.Members.Count,
        _ => 0,
    };

    /// <summary>Element <c>index</c>, or null past the end — which <c>Nullish</c> reads as undefined.</summary>
    internal static Value? At(Value? value, int index)
    {
        if (value is null || value.ValueKind != Kind.Array || index < 0 || index >= value.Items.Count)
        {
            return null;
        }
        return value.Items[index];
    }

    /// <summary>Member <c>key</c>, or null when absent.</summary>
    internal static Value? Get(Value? value, string key)
    {
        if (value is null || value.ValueKind != Kind.Object)
        {
            return null;
        }
        foreach (Member member in value.Members)
        {
            if (string.Equals(member.Key, key, StringComparison.Ordinal))
            {
                return member.Value;
            }
        }
        return null;
    }

    /// <summary>
    /// True when <c>value</c> is an object with a member <c>key</c>, present even if null — the reference's
    /// <c>key in object</c>, which is a different question from <c>object[key] != null</c>.
    /// </summary>
    internal static bool Has(Value? value, string key)
    {
        if (value is null || value.ValueKind != Kind.Object)
        {
            return false;
        }
        foreach (Member member in value.Members)
        {
            if (string.Equals(member.Key, key, StringComparison.Ordinal))
            {
                return true;
            }
        }
        return false;
    }
}
