# Plugin alias verification — 2026-10-15

The isolated native plugin prebundle example now passes the original alias oracle and an expanded 18-contract differential matrix. This is not a migration of the production plugin caller. The Node host, default backend, shared adapter and staging/lifecycle implementation are unchanged.

## Resolution correction

The previous before-resolution bare-specifier predicate externalized `@local/value` without consulting its tsconfig mapping. The original failing run remains [38627815980](https://github.com/Takazudo/zudo-front-builder/actions/runs/38627815980): esbuild 8/8, native 7/8, with the alias left in emitted code and rejected by Node.

The probe now uses a native resolve hook plus `ResolveOptions.modules = []`. Package-directory lookup is disabled while the same Rolldown resolver still handles local files, tsconfig paths/baseUrl, package imports and import kind. Successful results preserve native resolution metadata. Only an ordinary unresolved bare package request is left external; configuration errors and missing package imports remain build failures. The helper is confined to `examples/plugin_prebundle/packages.rs`.

This does not assume that every file inside node_modules is an external package: a tsconfig alias can intentionally target source there. Nor does it require an external package to be installed during bundling. The rooted-path classifier uses `Path::has_root`, avoiding the Windows distinction between rooted and fully absolute paths. Windows-specific assertions are committed but not counted as executed on Linux.

## Executed contracts

Every row below passed on both real esbuild 0.25.12 and the pinned native Rolldown example. The harness executes emitted modules in fresh Node processes, compares fixed literal expectations and preserves failing build/runtime oracles. No mock backend or fallback is used.

| Contract | Required observable result |
| --- | --- |
| TypeScript entry | 17 |
| MTS external package subpath | 31, then 37 after editing the dependency without rebuilding |
| CTS built-in require | `b` |
| Entry-owned tsconfig JSX factory | `probe` |
| Installed plugin's nested external dependency | 73, then 83 after editing that dependency without rebuilding |
| Original tsconfig paths alias | 59 |
| Deliberate throw with inline source map | Error maps to authored `throw.ts:3` |
| Missing relative import | Build fails and names the missing source |
| Inherited tsconfig paths | 59 |
| baseUrl-only local import | 61 |
| Alias shadows an installed package | 67, not the installed package's value |
| Alias targets source inside node_modules | 79 |
| Dynamic import of an alias | 59 |
| CTS require of an alias | 59 |
| Package installed only after bundling | 89 at runtime |
| Local package import `#local` | 59 |
| Missing package import `#absent` | Build fails and names the request |
| Malformed tsconfig | Build fails and names tsconfig.json |

Result: 36/36 backend rows passed, zero failures. Post-build package edits are essential: matching the initial output alone would not distinguish correct externalization from accidental dependency inlining. The original eight assertions were retained, not weakened to make the new implementation pass. This matrix is bounded evidence, not universal equivalence for every esbuild resolution case.

## Source and execution identity

[Run 38634288237, job 116093358176](https://github.com/Takazudo/zudo-front-builder/actions/runs/38634288237/job/116093358176) completed successfully. It checked out candidate `d0492c1edbb4e1c3415d5c2b7df3bcd417d5d080`, applied the audited root lock edge, formatted the Rust example/helper and refreshed reviewed Wind provenance. The exported, reviewed Git tree is `13a0cdab0c5d71d09427e1bc67984117630ab949`. The export created Git objects only; branch integration was a separate reviewed fast-forward.

The environment was Linux x86_64, Rust 1.99.0 (`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`), Node 24.14.0 and pnpm 12.8.2. Builds used the debug profile, two Cargo jobs and disabled incremental compilation/debug symbols. Emission targeted node22; execution here was Node 24.14.0, not a claim of separately testing Node 22.

Artifact [11927432668](https://github.com/Takazudo/zudo-front-builder/actions/runs/38634288237/artifacts/11927432668), `rolldown-plugin-alias-38634288237`, has ZIP SHA-256 `9773b0d7b09ab604126d445203489ddd59b1468740862fd94423f4182c4d8bc4`. The archive was downloaded, its digest checked, all 36 report rows inspected, and each exported file checked against its SHA-256 and Git blob identity. It contains exact fixture sources, emitted modules, stdout/stderr, test logs, source delta, the 100-path Wind audit and object identities. Its retention ends on 2026-10-21; the executable assertions and substantive results remain in the repository.

- Native example SHA-256: `203b90c9d042a6a9c158ae9ba70be25d395bc9c9ccfde8fcefceef9dab40ce9b`.
- Esbuild executable SHA-256: `bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014`.
- Cargo.lock SHA-256: `13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37`.
- Unchanged pnpm-lock.yaml SHA-256: `ee4accdbbbe22851c41ac529d492fbb6c03b007061ed0042068b2551731b5817`.
- Exact Rolldown revision: `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`.

The native example's 2 Linux unit tests and the prototype library's 4 tests passed, with zero failures or ignored tests. Locked native compilation and the feature-off all-target check passed. The temporary collector is removed after evidence integration; no existing required workflow is disabled or weakened.

## Audited lock and Wind provenance repair

Cargo.lock adds only `rolldown_resolver` to `zfb-rolldown-prototype`'s root dependencies. The resolver already exists at the same pinned version in the lock. The manifest exposes it only through the optional native feature so the hook can distinguish typed resolution errors. Parsed before/after lockfiles differ in exactly that one package record; no version, checksum or source pin changes.

The reviewed Wind source closure includes all 98 files under crates/zudo-wind plus root Cargo.toml and Cargo.lock. Its original 100-path digest was first validated, then all before/after file identities were compared. Only Cargo.lock changed. Wind sources, catalog, support pin and locked package records are unchanged; both actual Wind and ordinary no-V8 dependency graphs contain no Rolldown.

The exact reviewed closure digest moves from `5a08ea9a15e45de6afd6d7adfc6887e217e04d8249290a5ce0a1974b39aaee4c` to `c11e6436e2d7760fe88f70f0caaa6676b841c78256914106464d50ba2fc37f69`. Only that recorded digest and its explanatory comment change in wind-compatibility-pages.mjs. The support pin, inventory/catalog checks, path-count assertion and mismatch rejection are unchanged. Deliberately tampered Cargo input and an unreviewed support pin were both rejected after the refresh, and the original bytes restored.

All 117 Wind library tests and 19 preview-pipeline tests passed. A freshly built locked no-V8 zfb regenerated all 173 real previews and passed freshness. The no-V8 build used normal SHA-verified esbuild vendoring, not the probe's executable override. English and Japanese reference freshness both passed. The generated manifest changes only compiler.sourceSha256 to `95f1ec4a6062ac5adfd51c568bf403b2300bdaffa46709daf0a21e7ab7d21c44`; no preview CSS bytes, diagnostics or other manifest data changed.

Earlier collector run 38632460764 passed its 36 plugin rows but failed overall because its English reference invocation omitted `--locale en`. Run 38633707464 corrected that invocation and again passed plugin execution and preview freshness, then correctly rejected the changed lock's old Wind source digest. Neither failed job is called green. The final run audits the exact change and independently replays all of the checks above; it does not suppress the reference failure or loosen an assertion.

## Reproduction and boundaries

Run from the integrated PR checkout with the recorded toolchain, using the repository's existing heavy-command guard where configured:

```sh
npm install --prefix "$PWD/target/rolldown-plugin-tools" --no-audit --no-fund @esbuild/linux-x64@0.25.12
cargo build --locked -p zfb-rolldown-prototype --features native --example plugin_prebundle
cargo test --locked -p zfb-rolldown-prototype --features native --example plugin_prebundle
cargo test --locked -p zfb-rolldown-prototype --features native --lib
cargo check --locked -p zfb-rolldown-prototype --no-default-features --all-targets
ZFB_PROBE_SOURCE="$(git rev-parse HEAD)" ZFB_ESBUILD_BIN="$PWD/target/rolldown-plugin-tools/node_modules/@esbuild/linux-x64/bin/esbuild" node prototypes/rolldown/plugin-prebundle.mjs
cargo test --locked -p zudo-wind --lib
env -u ZFB_ESBUILD_BIN cargo build --locked -p zfb --no-default-features
node --test docs/scripts/__tests__/wind-preview-assets.test.mjs
node docs/scripts/wind-preview-assets.mjs --compiler "$PWD/target/debug/zfb" --check
node docs/scripts/generate-wind-reference.mjs --locale en --check
node docs/scripts/generate-wind-reference.mjs --locale ja --check
```

Reports and diagnostic fixtures are written under target/rolldown-plugin-probe. Watchdogs bound this diagnostic subprocess run; they do not prove active-build cancellation or quiescence inside the production process. A successful close after completion likewise does not prove no writes after cleanup.

The real plugin caller stays on esbuild until lifecycle ownership, cancellation, cleanup and actual CLI-to-Node-host acceptance are independently implemented and tested. Full final-head CI, island-size Cargo-provenance admission, native Darwin/Intel obligations and broader resolution coverage remain separate gates. No island contract, size ceiling, benchmark, default selector, production caller, release or deployment is changed by this increment.
