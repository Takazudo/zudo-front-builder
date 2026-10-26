# Exact tokio security replacement

This is the reviewed `Cargo.lock` replacement for [#4569](https://github.com/Takazudo/zudo-front-builder/issues/4569) (PR #4571). It was approved under POLICY-02 on the 4336 coordination issue, which lets agents run admission, apply and attestation steps. It authorizes only the lock pair in `tokio-security-4569.json`: tokio 1.52.3 → 1.54.1, version and checksum only. `scripts/security-lock-transitions.mjs` pins the decision bytes and both complete lock hashes next to the #4314 and #4268 entries. The shared reviewed-transition entry point dispatches it from there. Caller-written manifests cannot authorize other replacements.

| Lock | SHA-256 |
| --- | --- |
| Original, main `c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283` (the #4429 oxc edge's new lock) | `e79303ad33f123966d23efb32e309f4263afc00e587ea14bab3a2c872273893d` |
| Approved minimal replacement | `216d99d595688e68db223517fe52048f3618d73d36abbb71ea652c25fec97810` |

The old lock is preserved verbatim in [tokio-security-4569.old.lock](tokio-security-4569.old.lock). Both locks contain 676 package records. tokio 1.54.1 keeps 1.52.3's dependency list (`bytes`, `libc`, `mio`, `pin-project-lite`, `signal-hook-registry`, `socket2`, `tokio-macros`, `windows-sys 0.61.2`), and the locked versions satisfy its requirements.

| Crate | New checksum |
| --- | --- |
| tokio 1.54.1 | `b52fd3255258d26fac7264ab5a76b53c13367136dc011d3d6ad4f8ed070396a3` |

Cargo 1.94.0 verified the archive against this checksum while downloading for `cargo metadata --locked`.

`cargo update -p tokio --precise 1.54.1` also reselected the `windows-sys` edges of nine other records (anstyle-query, anstyle-wincon, errno, is-terminal, quinn-udp, rustix, socket2, tempfile, winapi-util). Those incidental edges are **not authorized**. The minimal version/checksum edit instead passes `cargo metadata --locked` and `cargo tree --locked -p zfb -e normal,build,features` for the default, `--no-default-features` and `--no-default-features --features direct-v8` selections, without rewriting the lock.

## Security evidence

[Security Audit run 40287711771](https://github.com/Takazudo/zudo-front-builder/actions/runs/40287711771) on main `2ecde51d9917b905ad22842e0966fdbf372dd133` failed `cargo deny` on the old lock. Preserve that failure.

- [RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386): the safe `tokio::io::unix::AsyncFd` constructors violate I/O safety (unsound). tokio is patched in `>=1.51.6, <1.52.0` or `>=1.53.3`.

tokio is a direct dependency of `zfb`, `zfb-build` and other workspace crates, and also reaches them through axum, hyper, hyper-util, reqwest and tower. No zfb crate calls `AsyncFd`, so the fix closes the advisory rather than a known zfb exploit path.

## Application

`research/v3-island-size/README.md` records each applied platform transition. Each platform still needs its own native evidence:

- **Linux:** the Actions `health` artifact.
- **Darwin:** owner-attested local evidence through the `cargo-producer-lock` local profile, which accepts exactly the approved decisions in `security-lock-transitions.mjs`.

Select this decision with `--reason cargo-producer-lock`. Pass:

- `--reviewed-cargo-transition research/v3-island-size/cargo-transitions/tokio-security-4569.json`
- `--added-packages tokio@1.54.1`
- `--old-cargo-ref c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283:Cargo.lock`
- the two lock hashes above

Ceilings, allowances and the dependency fingerprint do not move.
