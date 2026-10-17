# Combined-source continuation — 2026-10-16

Measured source: `546fce7cd3efccb1a9f05e8a51a9d889a545c256`, clean before
acceptance and regression verification. It merges accepted main `151d9d97`
into preserved `d541a25e` without importing held infrastructure. This directory
is evidence, not an activation or phase-two CLI implementation.

The real combined-source no-V8 compiler regenerated and checked all 173 Wind
samples. Only the manifest compiler-source digest changed. Independent review
checked all 780 compiler inputs against the source commit. The six native and
producer executables were then rebuilt by the unmodified acceptance entry point.

`acceptance.json` records the full successful `accept.py --generated` retry:
45 native and 9 shared-service tests; 63 config, 27 service and 47 module/worker
cases per engine; 8 HTTP and 7 outer-config cases; 8 freshly generated packets
per engine and 7 exact parity rows. The frozen manifest digest is
`94e0cb7da1862e6ed7abf8f11173f59a3c0af3bf59d13578e9cf88b74dcfb9c6`.
Chromium 151.0.7922.173 observed Count 0→1 and `rgb(12, 34, 56)` with no page or
console errors. Both mapped the authored throw to `throw.ts:6:9`. This is a
warm-target Linux replay, not a cold-build benchmark or native size measurement.

`initial-acceptance.json` retains the first attempt's missing-rustup failure.
The standalone Rust 1.99.1 installation requires the documented explicit
`--rustc` argument; the retry ran the complete suite into a new directory.
`original-observations.json` also retains the compiler's pinned-esbuild download
failure, the successful checksum-verified retry, the premature reference-suite
failure while the manifest was unresolved, and the inherited budget failures.
None is relabeled as a first-attempt pass.

`verification.json` and its logs record the affected renderer and source checks.
`session.json` binds locks, manifests, contracts, source, tools and focused tests.
`review.json` records independent source and evidence inspection. JSON copies
may be formatted by the repository hook; `copy-integrity.json` records raw
receipt hashes and parsed-value equality, plus hashes of the committed copies.
The frozen-manifest digest above binds the original producer manifest, not a
format-normalized copy. Generated binaries and large outputs remain ignored.

The refreshed declaration and four closure receipts replace no historical files.
`prepared-default-producer-proof.json` uses held tooling `16bb9165` in a separate
detached checkout, without importing it into this branch. It validates immutable
old/new lock bytes, committed unpublished manifests and the declared rewire:
Linux 497→498 and Darwin 481→482, only the internal host row added. Those are
workspace-resolved default-producer metadata proofs. `standalone-graph.json`
separately selects the direct-V8 host and verifies 152 normal/build rows with no
Deno. A future actual CLI graph tied to its exact feature build is still unproved.
Neither prepared proof admits artifacts or establishes live predecessor approval.
The pure proof was rerun against the formatted durable declaration and closure
files before its output was committed; its input digests were not substituted
manually. Final remote refresh found newer held heads `35a68f17` (#4234, test-only)
and `9912ad75` (#4259, documentation-only). Neither changes the measured source
or the recorded proof-tooling identity, and neither was imported.

Two inherited Cargo-provenance budget assertions remain failing. No ceilings,
allowances, policy, live contract, package version or runtime selection was
changed to clear them. Required gates remain #4234 control/replay → #4234 then
#4259 integration, independent #4260 repair/integration, runtime refresh,
compatible Linux/Darwin saved-byte measurements, paired staging/review,
separately authorized live apply, and final enforced verification. All drafts
and delivery issues remain open. No default switch, Deno removal, phase-two
adoption, release, publication or deployment occurred.
