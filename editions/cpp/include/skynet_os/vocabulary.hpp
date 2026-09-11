// Every closed set in the system (§31.4's `vocabulary` corpus).
//
// A closed set is a vocabulary an edition may not extend on its own. `guards` has eight members
// and a C++ edition that grew a ninth would be a C++ edition computing a different verdict from
// the Rust one, silently, for every robot that used it. §31.2 puts it plainly: the corpus is the
// thing all editions are measured against, and a vocabulary that drifted per edition would make
// the measurement meaningless.
//
// # Where these values came from, and where they deliberately did not
//
// Transcribed from `reference/typescript/*.ts` — the reference edition's own exported constants —
// **not** from `corpus/vocabulary/closed-sets.json`, and **not** from the Python edition beside
// this one.
//
// That distinction is the whole point of §31.4. A corpus exists to catch a divergence between an
// edition and the reference; an edition whose vocabulary was generated from the corpus would agree
// with it by construction and would prove exactly nothing. Copying the Python edition would be the
// same mistake at one remove: it would prove that two editions were transcribed by the same hand,
// which is not what the corpus is asked to establish.
//
// # Order is part of the set
//
// Declared in the order the reference edition declares them, never sorted. `FAULT_RESPONSES` runs
// least to most severe and `OS_TIERS` runs in rank order; an edition that sorted them
// alphabetically would produce the same members and a different answer to "what is the next tier
// up". This is also why the table is a vector of pairs rather than a map: a map would sort the
// members' container by set name, which is harmless, and would tempt the next reader to sort the
// members too, which is not.

#ifndef SKYNET_OS_VOCABULARY_HPP
#define SKYNET_OS_VOCABULARY_HPP

#include <string_view>
#include <vector>

namespace skynet::vocabulary {

using Set = std::vector<std::string_view>;

/// The members of a closed set, by the name the corpus uses.
///
/// Throws on an unknown name rather than returning an empty set. An empty vocabulary serialises to
/// `[]`, which is a legitimate answer for some sets and would make a typo indistinguishable from a
/// set that really is empty.
[[nodiscard]] const Set& members(std::string_view name);

/// Whether `candidate` is a member of the named set. The membership question, asked in one place,
/// so no contract re-spells a set it wanted to test against.
[[nodiscard]] bool contains(std::string_view name, std::string_view candidate);

/// How many closed sets there are. Used by the edition's own check that it declares as many as the
/// corpus asks about.
[[nodiscard]] std::size_t count();

}  // namespace skynet::vocabulary

#endif  // SKYNET_OS_VOCABULARY_HPP
