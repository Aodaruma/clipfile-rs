# Contributing

Thank you for helping improve `clipfile`.

## Development

The minimum supported Rust version is declared by `package.rust-version` in
`Cargo.toml`. Before opening a pull request, run:

```console
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo doc --all-features --no-deps
cargo package
```

Parser changes should include a synthetic regression test. Real `.clip` files
must not be committed unless their redistribution terms and provenance are
documented explicitly. Prefer tiny, purpose-built fixtures without personal or
copyrighted artwork.

## Reverse-engineering notes

Document whether each claim is directly observed, inferred from comparisons,
or borrowed from another implementation. Include the producing application
version when known. Do not present reverse-engineered behavior as an official
CELSYS specification.
