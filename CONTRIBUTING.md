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

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
