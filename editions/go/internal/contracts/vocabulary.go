package contracts

import (
	"fmt"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// closedSets answers `vocabulary/closed-sets`: given a set's name, its members in declaration
// order.
func closedSets(input canonical.Value) (canonical.Value, error) {
	name, ok := input.Field("set").Str()
	if !ok {
		return canonical.Null(), fmt.Errorf("no `set` name in the input")
	}
	members, found := vocabulary.Set(name)
	if !found {
		// Loud, and specifically not a skip: the corpus naming a set this edition does not have is
		// the divergence the whole vocabulary corpus was added to catch.
		return canonical.Null(), fmt.Errorf("this edition has no vocabulary named %q", name)
	}
	return canonical.Strings(members), nil
}
