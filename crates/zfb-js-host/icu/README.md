# Vendored ICU data

`icudtl.dat` is the ICU 77 common data that the direct V8 host passes to
`v8::icu::set_common_data_77` (`src/process.rs`). It is vendored so every
direct-V8 build, including `cross` and release builds, embeds the same bytes
without an environment variable, a download, or a Deno crate.

| Field | Value |
| --- | --- |
| Size | 10,822,192 bytes |
| SHA-256 | `1cf67874b5a87a8363a86fb3f81e3cbbed54d389062dab8fb52308d5cf8c8612` |
| Source | `deno_core_icudata-0.77.0/src/icudtl.dat`, crate archive SHA-256 `a9efff8990a82c1ae664292507e1a5c6749ddd2312898cdf9cd7cb1fd4bc64c6` (the bytes the deno_core host embeds) |
| Matches | `v8` 147.4.0 (ICU 77) |
| License | Unicode License v3, see [LICENSE](LICENSE) |

Verify offline with `python3 prototypes/direct-v8/prepare-icu.py --verify`; the
`embedded_icu_data_matches_the_pin` unit test checks the embedded bytes too.

Refresh only together with a V8 upgrade that changes the ICU version: update
the pins in `prototypes/direct-v8/prepare-icu.py`, run it without `--verify`,
and update the length and digest in `src/process.rs` and this file.
