# Contributing

Thanks for your interest in `zudo-front-builder`. The project is in early scaffolding, so the workflow is intentionally minimal for now.

## Toolchain

- **Rust**: stable channel, pinned via `rust-toolchain.toml` at the repo root. With `rustup` installed, the correct toolchain is selected automatically.
- **Node / pnpm**: pnpm will be pinned via [Corepack](https://nodejs.org/api/corepack.html) once Sub 2 lands. Until then, no Node-side tooling is required.

## Workspace layout

This is a Cargo workspace. Crates live under `crates/`. The placeholder bin crate is `crates/zfb/`.

Common commands:

```sh
cargo build --workspace
cargo run -p zfb
```

## Workflow basics

- Branch off `main` (or the relevant base branch for an in-flight epic).
- Keep commits focused; conventional commit-style messages are appreciated but not strictly enforced.
- `lefthook`, `rustfmt`, `clippy`, and JS/CSS formatters will be wired up in Subs 3–5. Until then, please run `cargo fmt` and `cargo clippy` manually before opening a PR.
- Open a PR against `main` (or the relevant epic base branch). CI will be enabled later in the foundation work.

## Formatting

JavaScript, TypeScript, JSON, and YAML are formatted with **Prettier**. Markdown and MDX use a separate formatter wired up in Sub 5.

Run formatters across the repo:

```sh
pnpm format         # autoformat all (TS/JS/JSON/YAML + MD/MDX)
pnpm format:check   # check formatting (CI-friendly, exits non-zero on diffs)
```

Targeted variants:

- `pnpm format:ts` / `pnpm format:check:ts` — Prettier over JS/TS/JSON/YAML
- `pnpm format:mdx` / `pnpm format:check:mdx` — Markdown/MDX (see Sub 5)

The pre-commit hook (lefthook) runs Prettier on staged matching files and re-stages the fixes.

### Why Prettier (and not Oxfmt) for now

[Oxfmt](https://oxc.rs/docs/guide/usage/formatter.html) is the formatter from the Oxc project and is the long-term direction the JS ecosystem is moving toward (30x faster than Prettier, 100% Prettier JS/TS conformance, supports JSON and YAML). At evaluation time (2026-04-26) it is published as `oxfmt` v0.46.0 and is officially in **Beta** ([announcement](https://oxc.rs/blog/2026-02-24-oxfmt-beta)) — sub-1.0 with no formal stable release yet, comparable to where Oxlint sat before its v1.0 stable announcement.

To keep the foundation conservative we ship Prettier today and revisit the swap once Oxfmt cuts a 1.0 / "stable" release. Tracker: [oxc-project/oxc milestone 19](https://github.com/oxc-project/oxc/milestone/19).

## CI secrets

The Cloudflare Pages deploy workflow (wired up in a later sub-task) expects the following GitHub Actions secrets to be present on the repository:

- **`CLOUDFLARE_API_TOKEN`** (required) — API token with `Pages:Edit` permission, used by `wrangler pages deploy`.
- **`CLOUDFLARE_ACCOUNT_ID`** (required) — the Cloudflare account ID that owns the Pages project.
- **`IFTTT_PROD_NOTIFY`** (optional) — IFTTT webhook key used to push a notification when a production deploy lands. Omit to skip notifications.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
