# Release preparation: proposed v4.5.4 (draft, not approved)

Prepared 2026-10-19 by the docs/release-prep lane (L5) from `v4.5.3..21ef7c4`
(`v4.5.3` = 2026-10-16; `main` = `21ef7c4678adba115a3607801ededeeaf6e5814e`).
Nothing here bumps a version, creates a tag, publishes a package or deploys docs. The
executable procedure stays `.claude/skills/l-make-release/SKILL.md`; it re-reads the range
and recomputes positions at release time.

## Recommendation: patch, v4.5.4

| Evidence | Finding |
| --- | --- |
| `git diff v4.5.3 21ef7c4 -- packages/` | Empty. No npm package source, manifest, `exports`, `bin` or `engines` changed |
| `git diff v4.5.3 21ef7c4 -- crates/zfb-md-wasm` | Empty |
| `Cargo.lock` | Only the new internal `zfb-js-host` crate (`publish = false`) and its edge from `zfb-render`; no third-party version moved |
| CLI flags, config keys, environment variables | None added or removed: no new `clap` argument or user-facing `ZFB_*` variable in the `crates/zfb/src` diff (the one new variable, `ZFB_DEV_TEST_DIGEST_RELEASE_FILE`, is a test seam), and `packages/` is unchanged |
| `BREAKING CHANGE` / `!:` commits | None |
| `feat:` commits | 8, all repo tooling or the direct-V8 prototype: island-size budget/re-pin tooling (`c97e3e1`, `c23cc0e`, `edfede0`, `97c2640`) and `zfb-js-host`'s `direct-v8` feature (`26e8729`, `8dcdfb3`, `6706ab1`, `4c6e619`). The `direct-v8` feature is "never selected by the CLI" (`crates/zfb-js-host/Cargo.toml`), so no released binary can reach it |
| Package-facing fixes | Islands build-identity stability (PR 4298) and symlink-alias graph paths (PR 4308) in the `zfb` lane |

The release skill's mechanical rule (`feat:` ⇒ minor) would propose **4.6.0** from commit
prefixes alone. The prefixes describe prototype and tooling work that ships nothing new to
users, so this lane proposes **4.5.4**. The owner chooses; see "Owner decisions" below.

## Breaking changes

None. Behavior changes a consumer can observe, none requiring action:

- `data-zfb-build` and the identity token inside shipped islands JavaScript now stay the same
  across edits outside the browser graph. Before, any content or title edit changed them.
  Anything that assumed "every build gets a new token" (none known) must instead compare emitted
  bytes. The attribute's presence and meaning are unchanged (`docs/.../api/island.mdx`).
- Island builds and dev ticks do more esbuild work (PR 4298's stated tradeoff). No timing
  budget or default changed.

## Migration guide (v4.5.3 → v4.5.4)

No configuration, code or dependency change is needed. Update `@takazudo/zfb` (and the
lockstep packages) or the standalone binary as usual. If you pin `@takazudo/zfb-md-wasm`
artifact digests, re-pin them: every release moves all four SHA-256 digests because each
`.wasm` embeds `ZFB_RELEASE_VERSION`, even with no code change in that package.

## Five-lane notes

Drafts, with `sidebar_position` from `scripts/next-changelog-sidebar-position.mjs` on main
`21ef7c4` (zfb 155; the four other lanes 41). `Released: YYYY-MM-DD` is a placeholder the
release run replaces.

| Lane | Draft | Content |
| --- | --- | --- |
| zfb | [changelog/zfb/v4.5.4.mdx](changelog/zfb/v4.5.4.mdx) | 2 bug fixes, 1 performance note |
| zfb-runtime | [changelog/zfb-runtime/v4.5.4.mdx](changelog/zfb-runtime/v4.5.4.mdx) | No package-specific changes |
| zfb-adapter-cloudflare | [changelog/zfb-adapter-cloudflare/v4.5.4.mdx](changelog/zfb-adapter-cloudflare/v4.5.4.mdx) | No package-specific changes |
| create-zfb | [changelog/create-zfb/v4.5.4.mdx](changelog/create-zfb/v4.5.4.mdx) | No package-specific changes |
| zfb-md-wasm | [changelog/zfb-md-wasm/v4.5.4.mdx](changelog/zfb-md-wasm/v4.5.4.mdx) | No package-specific changes (digests still move; stated in the migration guide, not the lane body, per the skill's exact-body rule) |

They live here, not under `docs/src/content/docs/changelog/`, because any `docs/**` push to
`main` runs `docs-deploy.yml` and would publish an unreleased version page. The release run
copies them into the five lane directories.

Omitted as having no package-facing effect: the shared host-services refactor (`4a1bc10`,
behavior-preserving; embedded host transport, Web Crypto and module reads moved into
`zfb-js-host`), the direct-V8 prototype, island-size admission tooling and contracts,
nextest evidence helpers, CI and docs changes.

## Conditional entry: if the security transition (PR 4315) is on `main` at the cut

Add this to the `zfb` lane only (`changelog/zfb/v4.5.4.mdx`), as a new section above
`## Bug Fixes`, with the commit hash that lands rustls 0.23.46 on `main`. The other four lanes
stay "No package-specific changes": `@takazudo/zfb-md-wasm`, the runtime, the adapter and
`create-zfb` do not link rustls.

```mdx
## Security

- The CLI's TLS stack, used for downloads and request-time `fetch` in the embedded host, now uses rustls 0.23.46, which fixes the four RustSec advisories that `cargo deny` reported against 0.23.45 (issue 4314). No other dependency version changed (<hash>).
```

It does not change the bump: a dependency security fix with no API change stays a patch.
Before using it, re-check the advisory count and the "no other dependency" claim against the
landed `Cargo.lock` diff, and confirm the rustls consumers listed in PR 4315 still match.

## What else is on `main` but not in the candidate's notes

| Work | State on `main` `21ef7c4` | Release consequence |
| --- | --- | --- |
| Direct V8 (4301/4307) | Prototype crate feature only; the opt-in CLI wiring is in draft PR 4307, not merged; its lock is not admitted | Not released, not mentioned as a feature |
| Security transition (4314/4315/4319/4326) | Integrated on `base/4314-security-darwin-evidence`, not on `main`; rustls stays 0.23.45 on `main` | If it reaches `main` before the cut, use the conditional entry above |
| nextest 0.9.140 unification (4324/4325) | Draft | CI-only; no note |
| 4231 store-vs-link acceptance test (4328) | Draft | Test-only; no note |

## Release preconditions still open

These are inputs to the owner's release decision, not lane-L5 actions:

1. Native qualification at the candidate SHA is not done. Last native Mac evidence is for the
   4308/4309 frozen heads (`92e25ad4`, `a76e4775`); no full native run exists after `21d6807e`
   (see `research/development-queue.md` "Native qualification status").
2. The weekly Security Audit last ran red at `21d6807e` (issue 4314). Whether 4315's rustls
   change must ship in this release is an owner call.
3. The release procedure's own gates: package/Rust focused checks, docs check, strict docs
   build, emitted-HTML validation, and the `release.yml` publication run.

## Owner decisions

1. Version: 4.5.4 (recommended) or 4.6.0 (commit-prefix rule).
2. Whether the security transition (rustls 0.23.46) must be on `main` before cutting.
3. Whether to cut before or after the native qualification pass at the candidate SHA.
