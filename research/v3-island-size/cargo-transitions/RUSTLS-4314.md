# Exact rustls security replacement; live admission remains pending

The owner approved this bounded security pathway on 2026-10-19 for [#4314](https://github.com/Takazudo/zudo-front-builder/issues/4314). It authorizes only the lock pair in `rustls-security-4314.json`: rustls 0.23.45 → 0.23.46, version and checksum only. The shared reviewed-transition entry point dispatches this identity to `scripts/rustls-security-transition.mjs`, which pins the exact decision bytes and both complete lock hashes. Caller-written manifests cannot authorize other replacements. The existing #318 exception and default additive proof are unchanged. This does not import the internal-rewire/native-evidence work in #4310/#4311.

| Lock | SHA-256 |
| --- | --- |
| Original, main `21d6807e442bbed61e578c165a91d64c4a7dc141` | `230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138` |
| Approved minimal replacement | `563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd` |

Both locks contain 697 package records. The new crate archive checksum is `48e13bd8c0e9365c43cfa5c9e8f9ad49d3c8444926c9aac819e0e4dc503c8fdf`, verified against the official registry index and downloaded archive. Upstream dependency and feature declarations are unchanged; the release is not yanked and retains Rust 1.71 as its minimum.

The initial `cargo update -p rustls --precise 0.23.46` also reselected `windows-sys 0.52.0` to `0.59.0` in errno, is-terminal, rustix, tempfile and winapi-util. That first candidate had SHA-256 `4db678eaf6048ec79acda0badcf7114ecc69c4da62a08058fcc43949abe16b38`. Those incidental changes are **not authorized**. The minimal version/checksum edit instead passed Cargo 1.99.1 full locked Linux metadata and locked default/no-V8 feature-tree resolution without rewriting the lock. The initial offline metadata attempt lacked cached `az 1.3.0`; the subsequent online check passed separately.

## Security evidence and reachability

[Security Audit run 39217854663](https://github.com/Takazudo/zudo-front-builder/actions/runs/39217854663/job/117945845815) failed cargo-deny on the old lock. Preserve that failure. RustSec lists 0.23.46 as patched for [0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html), [0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html), [0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html) and [0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html).

Reqwest supplies TLS for CLI/build downloads and host fetch, including the no-V8 CLI. Resolved rustls features are `ring`, `std`, and `tls12`. Source inspection found no custom session-store Debug logging or enabled ECH configuration. Rustls Brotli and hybrid key-exchange providers are not enabled. These observations are not an exploitation assessment and do not justify ignoring the advisories. The audit policy and its existing exceptions are unchanged.

## Wind provenance is regenerated from the candidate

The complete Wind lock closure, including dev and platform edges, remains byte-equivalent across all 164 reachable records; rustls is absent from that closure. The other 99 inputs to the 100-path Wind source digest are unchanged. The digest was recomputed from those exact bytes, and both English and Japanese canonical reference generators passed their checks. All 117 Wind library tests passed.

The workspace no-V8 CLI was rebuilt from the candidate with Cargo `--locked` under the heavy guard. The official `docs/scripts/wind-preview-assets.mjs` generator reproduced 173 samples, including diagnostic controls. Only the generated manifest's compiler source digest changed; all sample metadata and CSS/HTML bytes remained unchanged. No published or stale compiler was substituted.

## Application still requires separate authorization and native evidence

This PR performs **no live contract application**, size-ceiling change, platform qualification, or checkpoint integration. Both live island-size contracts retain the old Cargo hash, so their candidate-provenance assertions are expected to reject this lock until separately authorized application. That rejection must not be called a passing budget result.

A later approved application can select this decision with the existing `cargo-producer-lock` reason and `--reviewed-cargo-transition research/v3-island-size/cargo-transitions/rustls-security-4314.json`, using `--added-packages rustls@0.23.46` and the exact old/new hashes above. The old source remains an immutable Git blob. Every existing artifact/source/platform/toolchain, complete workspace/packed matrix, unchanged totals, ceiling, and allowance check still applies. This reason supports per-platform application; this PR does not introduce or claim paired-stage enforcement. Each platform still needs its own qualifying native evidence and explicit application authorization. No Mac workflow is dispatched here.

Synthetic CLI fixtures test the proof and refusal paths; they are not native measurements. Retain original failures and exact-head PR CI status in the PR receipt. Do not dispatch the scheduled audit against a candidate merely to obtain a green badge: its issue-closing job could close the unresolved main alert.
