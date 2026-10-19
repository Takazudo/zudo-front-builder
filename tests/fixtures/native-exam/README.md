# Captured nextest protocol fixture

Captured on Linux x86_64 with Rust 1.99.1 and cargo-nextest 0.9.148 on 2026-10-18.
`Cargo.toml`, `lib.rs` and `nextest.toml` are the tiny producer sources. They have no
external dependencies. Copy them to a temporary package at `src/lib.rs` and
`.config/nextest.toml` to regenerate. `capture/shard-N` contains actual full/partition
JSON, JUnit and logs from the commands documented in `scripts/native-exam.md`.
Absolute `/tmp/native-probe` paths are the original capture paths, not local inputs.

`doc-list.txt` and `doc-run.txt` were captured from the same source before the final
12 unit tests were added; the doctest lines are identical. Synthetic metadata/receipts
are created by the regression tests so these Linux bytes cannot be mistaken for a
native macOS result. Mutations deliberately exercise missing evidence and false green
wrappers. These files are parser evidence, not workspace acceptance results.
