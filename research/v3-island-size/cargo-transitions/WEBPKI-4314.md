# Exact webpki security replacement

This is the second exact security replacement under [#4314](https://github.com/Takazudo/zudo-front-builder/issues/4314), after [RUSTLS-4314.md](RUSTLS-4314.md). On 2026-10-21 Takazudo chose to fix it before the next release, and it was approved under POLICY-02 on the 4336 coordination issue. It authorizes only the lock pair in `webpki-security-4314.json`: rustls-webpki 0.103.15 → 0.103.16 and webpki-roots 1.0.7 → 1.0.10, version and checksum only. `scripts/security-lock-transitions.mjs` pins the decision bytes and both complete lock hashes for each approved replacement. The shared reviewed-transition entry point dispatches each identity there. Caller-written manifests cannot authorize other replacements.

| Lock | SHA-256 |
| --- | --- |
| Original, main `0cb911138a4153d759c61fec85ec67ca6789aac2` | `0cd091b7adcc7776895d9de119ac28ac8e8a931cdf28e46c676d5460627cc6f2` |
| Approved minimal replacement | `d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d` |

Both locks contain 697 package records. Neither package changes its dependency list.

| Crate | New checksum |
| --- | --- |
| rustls-webpki 0.103.16 | `4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281` |
| webpki-roots 1.0.10 | `1c1c8e20c8a38fc7d65abfe74eb7280e4aa667e8eec8bc84996d2874810748d9` |

Cargo 1.97 verified both archives against these checksums while downloading for `cargo metadata --locked`.

`cargo update -p rustls-webpki -p webpki-roots` also reselected `windows-sys 0.52.0` to `0.59.0` in six records, as the rustls update did. Those incidental edges are **not authorized**. The minimal version/checksum edit instead passes `cargo metadata --locked` and `cargo tree --locked -p zfb -e normal,build,features` for the default, `--no-default-features` and `--no-default-features --features direct-v8` selections, without rewriting the lock.

## Security evidence

[Security Audit run 39606942633](https://github.com/Takazudo/zudo-front-builder/actions/runs/39606942633) on main `0cb91113` failed `cargo deny` on the old lock. Preserve that failure.

- [RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382): rustls-webpki is patched in `>=0.103.16, <0.104.0-alpha.1`.
- [RUSTSEC-2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381): webpki-roots is patched in `>=1.0.10`.

Both crates sit under rustls → hyper-rustls → reqwest, which supplies TLS for CLI and build downloads and for host fetch.

## Application

`research/v3-island-size/README.md` records each applied platform transition. Each platform still needs its own native evidence:

- **Linux:** the Actions `health` artifact.
- **Darwin:** owner-attested local evidence through the `cargo-producer-lock` local profile, which accepts exactly the approved #4314 decisions.

Select this decision with `--reason cargo-producer-lock`. Pass:

- `--reviewed-cargo-transition research/v3-island-size/cargo-transitions/webpki-security-4314.json`
- `--added-packages rustls-webpki@0.103.16,webpki-roots@1.0.10`
- `--old-cargo-ref 0cb911138a4153d759c61fec85ec67ca6789aac2:Cargo.lock`
- the two lock hashes above

Ceilings, allowances and the dependency fingerprint do not move.
