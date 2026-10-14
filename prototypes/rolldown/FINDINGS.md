# Rolldown findings — 2026-10-15

**Continue the existing opt-in prototype; do not migrate the live plugin caller yet.** Fresh Linux execution verifies the selected existing client/browser/SSR paths after the main merge. The generated-data repairs are committed. The plugin option probe has one reproduced resolution failure, and plugin cancellation/cleanup parity remains unverified. Overall integration and adoption are not complete.

The previous report is preserved byte-for-byte in [the historical findings](FINDINGS-HISTORY-20261012.md). Its older platform and next-step statements describe their named source revisions, not the current state. Historical benchmark files are unchanged; this continuation makes no timing or size-advantage claim.

## Sources and completed repairs

- Main `fbb7921d8fb3f28d859bbd4cf5737759f547866a` was merged into the existing prototype as `557f65e3c5febc651e05bafc8974b8bbb91737bd`.
- Fresh native/default verification used `21863e45fa920292e9bf48746f667f00ac6980a1`, which adds only its temporary verification workflow over that merge.
- Repair commit `9b47cfe5d1f17b182ff3b0f2be1b0830eadc0ec9` updates Wind source/preview provenance and the actual CI-measured WASM rows plus their generated documentation. It leaves both locks, island-size contracts, all ceilings and assertions unchanged.
- Exact Rolldown revision remains `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`. The default esbuild backend, production native adapter, configuration loader and execution hosts are unchanged by this continuation.

[POST-SYNC.md](POST-SYNC.md) records the exact source, toolchain, artifact hashes and repair procedure. The 117 Wind library tests and 19 preview-pipeline tests passed; regenerating all 173 actual previews changed only the compiler fingerprint in the manifest, not CSS bytes. The four refreshed WASM measurements come from the original hosted CI job, not a local build. The eleven-file repair was replayed from pristine inputs and then idempotently; tampered Cargo and manifest inputs were rejected without output mutation.

## Existing native/default verification

[Run 38624226907, job 116059183063](https://github.com/Takazudo/zudo-front-builder/actions/runs/38624226907/job/116059183063) completed successfully on Linux x86_64 with Rust 1.99.0, Node 24.14.0, esbuild 0.25.12, Playwright 1.58.2 and Chromium 143.0.7499.4. The build used the debug profile with two Cargo jobs and disabled incremental/debug-symbol output. These are environment facts, not benchmark conditions supporting a performance claim.

| Selection | Actual outcome |
| --- | --- |
| Client-script library tests | 14 passed |
| Output/readback library tests | 5 passed |
| Ignored islands client-script integration | 5 esbuild + 5 native passed |
| Exact ignored cross-pipeline test | 1 esbuild + 1 native passed |
| `gate.mjs` | Native graph, code, maps and unresolved-import checks passed |
| `client-scripts.mjs` | Both backends passed production/dev/edit/negative cases; native dispatch 3/3 jobs versus esbuild 0/0; zero browser errors |
| `verify.mjs` | Both backends passed SSR and real Chromium hydration/split/worker/WASM/glue cases |
| `production.mjs` | Both actual CLI scratch and adapter outputs executed; semantic comparison and second-pass determinism passed |

The production evidence retains WASM values 29/43, CommonJS value 31, MDX/CSS behavior and the scratch throw mapped to `pages/throw.tsx:3:9`. Unsupported SSR copied-resource URLs and the adapter's source-map stripping remain limitations. No zero-test selection is counted as a pass. Earlier WSL and Darwin results remain separate historical evidence; this run does not satisfy Intel or native-Darwin obligations.

## Plugin prebundle capability result

The new Rust example and Node harness are isolated verification artifacts:

- `crates/zfb-rolldown-prototype/examples/plugin_prebundle.rs`
- `prototypes/rolldown/plugin-prebundle.mjs`

They do not call or modify production `bundle_plugin_entry`. Emitted modules are executed in Node with fixed literal expectations; this is not an actual CLI-to-plugin-runner acceptance test.

At source `9b47cfe5d1f17b182ff3b0f2be1b0830eadc0ec9`, [run 38627815980, job 116071362417](https://github.com/Takazudo/zudo-front-builder/actions/runs/38627815980/job/116071362417) successfully built the native example and executed all sixteen rows. Esbuild passed eight cases; native passed seven and failed one. The job correctly failed overall.

| Case | Esbuild | Native candidate |
| --- | --- | --- |
| TypeScript entry returns 17 | PASS | PASS |
| MTS package subpath returns 31 | PASS | PASS |
| CTS/CommonJS built-in require returns `b` | PASS | PASS |
| Plugin's own TSX factory configuration | PASS | PASS |
| Installed plugin's nested dependency returns 73 | PASS | PASS |
| `tsconfig` paths alias returns 59 | PASS | **FAIL: unresolved external alias** |
| Deliberate throw maps to authored line 3 | PASS | PASS |
| Missing relative import fails with its source named | PASS | PASS |

The downloaded artifact `11925320296` was verified against ZIP SHA-256 `67454d600f9da66010a8676062168bcd2559d14e64d92148d24768e9a2bef84c`. Its reports, logs, fixture sources and emitted modules were inspected. Native example SHA-256: `24333d4e9e16db5ba674f0f0418b899eb6a210d9c78f480ed27aa0bca94cb158`. Esbuild SHA-256: `bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014`. The artifact expires on 2026-10-21; the reproduction and substantive outcome remain here.

### Reproduced alias gap

The fixture maps `@local/*` to `./src/*` in its own `tsconfig.json` and exports 59 from `src/value.ts`. Its entry imports `@local/value`. The candidate currently classifies every bare specifier as external before alias resolution. Its emitted module still contains `import { value } from "@local/value"`; Node fails with `ERR_MODULE_NOT_FOUND`. The esbuild control resolves the alias and returns 59.

This is a defect in the candidate external-package mapping, not proof that Rolldown cannot support aliases. The correction must use the real resolver's alias/package semantics while retaining genuinely external packages and their entry-adjacent runtime resolution. Do not hardcode the fixture alias, bundle every package, scan imports with a second resolver, or change the failing expectation to obtain a pass.

Reproduce from the repository root with the recorded toolchain and existing heavy-command guard:

```sh
npm install --prefix "$PWD/target/rolldown-plugin-tools" --no-audit --no-fund @esbuild/linux-x64@0.25.12
export ZFB_ESBUILD_BIN="$PWD/target/rolldown-plugin-tools/node_modules/@esbuild/linux-x64/bin/esbuild"
cargo build --locked -p zfb-rolldown-prototype --features native --example plugin_prebundle
node prototypes/rolldown/plugin-prebundle.mjs
```

The current final command intentionally returns a failure because the supported alias assertion is unsatisfied. Reports are saved under `target/rolldown-plugin-probe/`. Its external-process watchdogs bound the diagnostic run; they are not proof of cancellation inside the production process.

### Lifecycle and rollout boundary

The production plugin path has a subprocess deadline, termination behavior, entry-adjacent staging and file-ownership/sweeping protections. Successful native `close()` after a completed build does not establish cancellation of an active build, bounded quiescence or absence of writes after temporary-file cleanup. The pinned `write`/`close` APIs require a mutable bundler, and the current production adapter synchronously joins its scoped thread. No equivalent cancellation/ownership implementation was produced or verified in this continuation.

Therefore the live plugin caller remains on esbuild. Fix the resolver mapping and independently prove the lifecycle contract before any actual caller integration; retain all positive and negative fixtures. A future CLI-to-plugin-host round trip, failure cleanup and repeated-start tests are still required. There is no new public backend selector or change to the Node plugin host.

## Failure history and remaining integration gate

The first isolated plugin workflow attempt in run `38627387252` failed before compilation: Node setup automatically requested pnpm for caching, but that npm-only job had no pnpm installation. The job was corrected to disable package-manager auto-caching. Its successor compiled and produced the semantic result above; the original setup failure is not reclassified as a runtime result.

The original normal Health run `38621899886` rejected Wind provenance and two island-size Cargo-provenance assertions before later broad workspace verification. The Wind repair addresses its specific mismatch. The island contracts still name a different Cargo producer input; this is not evidence of a measured size overflow. The actual Rolldown dependency delta is not admitted by the existing additive-only Cargo, internal-only rewire or pnpm-only transition reasons. Preserve that rejection until a reviewed transition and qualifying platform evidence are available. No live paired re-pin or Mac dispatch was performed.

Normal CI for repair source `9b47cfe5` passed PR Checks, actionlint and Wind computed styles. At the recorded inspection, the EN/JA reference checks, docs build and docs type-check also passed, while the full Docs, binary/scaffold and Health workflows were still running. Those partial successes are not a final integrated green result; the PR conversation records later observations. No final-source WASM remeasurement or broad workspace success is claimed here.

The one-shot full-native collector, completed data-export job and finite plugin-probe workflow are retired after collecting their named evidence. Existing required workflows, assertions and budgets remain enabled and unchanged. The plugin harness retains its failing oracle for explicit replay; the failed run remains linked, not hidden by a passing replacement gate.

**Disposition:** generated-data repairs implemented and integrated into the draft; selected existing Linux runtime contracts verified at their named source; plugin probe implemented with a reproduced gap; real plugin caller integration and overall adoption not complete. Keep PR #4120 and #318/#4098/#4123/#4124/#4133 open. No merge into main, ready transition, default switch, release, publication or deployment is included.
