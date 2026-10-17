# Baseline binary-page producer limitation

The first real CLI preparation at source
`8dcdfb35b5f97c32eac5684d1535e82fbf6e024c` failed with exit 1:

```text
production asset pipeline (hash + URL rewrite) failed
prod orchestrator: failed to read rendered page from disk at .../dist/bytes.bin
stream did not contain valid UTF-8
```

The original full site, rendered bytes, producer commands and stderr remain
under ignored `target/direct-v8-generated-1/`. This directory preserves the
original `pages/bytes.bin.tsx` byte-for-byte with a minimal config. The isolated
reduction is authored but has not itself been executed; the observed failure
came from the original full fixture. Stage this directory into a new ignored
directory and run the same normal `zfb build` through the manager's heavy lane
to reproduce the boundary independently.

`crates/zfb-build/src/pipeline/orchestrator.rs` reads rendered page output with
`std::fs::read_to_string` before production URL rewriting. The page's opaque
bytes include 128 and 255, so that read fails. No pipeline code changes here.

`worker-expected.json` retains the original literal Worker response expectation;
it is not a claim that the production build succeeds. The positive generated
site now has five routes. A separate prepared Worker tests binary requests and
an exact binary response with duplicate ordered headers, without this asset
rewrite step. Binary transport parity and successful production emission are
distinct observations.
