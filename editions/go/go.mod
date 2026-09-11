// The Go edition of the Skynet OS contracts — §42's L2.
//
// No dependencies, for the reason `Cargo.toml` gives for the Rust workspace having none: §31.4
// requires byte-identical output, so the JSON number-formatting rule has to be ours rather than
// whatever a dependency does this year. `go build ./...` works with no network, forever.
module github.com/antonihabek/Skynet/editions/go

go 1.24
