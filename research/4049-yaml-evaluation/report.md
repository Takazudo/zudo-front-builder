# YAML 0.0.55 evaluation — evidence pending

No KEEP/MIGRATE verdict has been established. Manager observed the unchanged targeted harness pass at both pins: 4/4 including exact equality of all 18 observations. Broader Phase 1 and Phase 2 remain pending/blocked. Manager completes remaining statuses, toolchain details and final race evidence before any disposition. Source starts at `cb4b6dbd3af21980b302916c5d97d4f767c51574`; candidate pin was temporary and restored.

| Case | Category | Baseline 0.0.44 | P1 0.0.55 | P3 if needed |
| --- | --- | --- | --- | --- |
| `anchors-and-aliases` | anchors-aliases | match (baseline harness) | match (candidate harness) | NOT RUN |
| `merge-key-is-an-ordinary-json-key` | merge-keys | match (baseline harness) | match (candidate harness) | NOT RUN |
| `non-string-scalar-keys` | non-string-keys | match (baseline harness) | match (candidate harness) | NOT RUN |
| `non-string-composite-key` | non-string-keys | match (baseline harness) | match (candidate harness) | NOT RUN |
| `yaml-11-boolean-spellings` | scalar-edge-cases | match (baseline harness) | match (candidate harness) | NOT RUN |
| `octals-sexagesimals-and-numbers` | scalar-edge-cases | match (baseline harness) | match (candidate harness) | NOT RUN |
| `null-and-date-scalars` | scalar-edge-cases | match (baseline harness) | match (candidate harness) | NOT RUN |
| `unicode-crlf-and-emoji` | unicode-bom-crlf-emoji | match (baseline harness) | match (candidate harness) | NOT RUN |
| `malformed-unicode-location` | malformed-input | match (baseline harness) | match (candidate harness) | NOT RUN |
| `malformed-flow-sequence-at-eof` | malformed-input | match (baseline harness) | match (candidate harness) | NOT RUN |
| `malformed-indentation` | malformed-input | match (baseline harness) | match (candidate harness) | NOT RUN |
| `built-in-explicit-tags` | explicit-tags | match (baseline harness) | match (candidate harness) | NOT RUN |
| `custom-explicit-tag` | explicit-tags | match (baseline harness) | match (candidate harness) | NOT RUN |
| `duplicate-map-keys-last-wins` | duplicate-keys | match (baseline harness) | match (candidate harness) | NOT RUN |
| `non-finite-and-overflowing-numbers` | non-finite-overflowing-numbers | match (baseline harness) | match (candidate harness) | NOT RUN |
| `integer-boundaries` | non-finite-overflowing-numbers | match (baseline harness) | match (candidate harness) | NOT RUN |
| `integer-overflow` | non-finite-overflowing-numbers | match (baseline harness) | match (candidate harness) | NOT RUN |
| `alias-anchor-repetition-limit` | alias-anchor-resource-limits | match (baseline harness) | match (candidate harness) | NOT RUN |

Equality means full JSON equality or exact error Display and optional line/column/byte index, plus unchanged original-source diagnostic pins. Do not mark individual rows matched if the aggregate assertion failed without inspecting each observation.

| Check | Status | Exact command / log / count |
| --- | --- | --- |
| Start and final trigger provenance / skipped releases / release PRs | NOT RUN | pending |
| Current-base source SHA and protected hashes | NOT RUN | pending |
| Pinned toolchain / Node / pnpm / bindgen / optimizer / deny | NOT RUN | pending |
| P1 harness (4 tests, 18 observations) | PASS | manager baseline: `/workspace/scratch/zfb-sweep-261008/yaml-baseline-harness.log`, guard PASS 64s; candidate: `/workspace/scratch/zfb-sweep-261008/yaml-candidate.log` and `yaml-candidate/harness.log`, guard PASS 52s |
| zfb-content including error_messages | NOT RUN | pending |
| md-wasm api and parse_to_ast | NOT RUN | pending |
| CLI library diagnostics | NOT RUN | pending |
| Three separate native checks | NOT RUN | pending |
| Conditional P3 and abandon-rule accounting | NOT RUN | pending |
| wasm32 check | NOT RUN | pending |
| Composite pnpm test:md-wasm | NOT RUN | pending |
| Both timed four-artifact builds and sixteen fields | NOT RUN | pending |
| Budget assertion and temporary manifest restoration | NOT RUN | pending |
| Tarball / complete dist / timing / peak RSS / disk | NOT RUN | pending |
| Both dependency graphs and exact lock delta | NOT RUN | pending |
| Both license reports and cargo deny check | NOT RUN | pending |
| Final release race and watcher errors | NOT RUN | pending |
| All tracked experiment changes restored / clean scope | NOT RUN | pending |

| Artifact | Field | Same-toolchain 0.0.44 | 0.0.55 | Delta |
| --- | --- | --- | --- | --- |
| default | finalWasm | pending | pending | pending |
| default | gzip9 | pending | pending | pending |
| default | glue | pending | pending | pending |
| default | glueGzip9 | pending | pending | pending |
| highlight-only | finalWasm | pending | pending | pending |
| highlight-only | gzip9 | pending | pending | pending |
| highlight-only | glue | pending | pending | pending |
| highlight-only | glueGzip9 | pending | pending | pending |
| render-only | finalWasm | pending | pending | pending |
| render-only | gzip9 | pending | pending | pending |
| render-only | glue | pending | pending | pending |
| render-only | glueGzip9 | pending | pending | pending |
| parse-only | finalWasm | pending | pending | pending |
| parse-only | gzip9 | pending | pending | pending |
| parse-only | glue | pending | pending | pending |
| parse-only | glueGzip9 | pending | pending | pending |

| Artifact | Gzip-9 ceiling | Candidate headroom | Result |
| --- | --- | --- | --- |
| default | 1,600,000 B | pending | NOT RUN |
| highlight-only | 880,000 B | pending | NOT RUN |
| render-only | 1,100,000 B | pending | NOT RUN |
| parse-only | 325,000 B | pending | NOT RUN |

Tarball ceiling: 3,900,000 B. A baseline over ceiling cannot permit MIGRATE. Projected deltas and passed test halves cannot replace a green composite gate.

Final disposition: BLOCKED / PENDING until evidence exists. Record semantic or gate failures separately from unavailable checks. No shipping dependency switch, watcher refresh, probe merge, or release is authorized by this report.

Remaining checks: full protected tests, three separate native checks, dependency/license audit, all Phase 2 checks and final release race. Manager reports disk now below the unchanged 30 GiB gate, so Phase 2 is blocked. Targeted 0.0.55 execution resolves the Release-note contradiction for this consumed Value path: the last-wins assertion still passes. Manager reports temporary Cargo files and protected files restored byte-identically after the targeted harness. No broader compatibility or terminal verdict follows from targeted 4/4 alone.
