# Changelog

## 0.1.0-next.1

Initial public prerelease on npm.

- Rust-built static-site engine, distributed per-platform via npm optional-deps.
- TypeScript SDK with subpath exports for `runtime`, `content`, `paginate`, `config`, `plugins`, `frontmatter`.
- Bundled `basic-blog` template via `zfb new my-site` / `npm create zfb@latest my-site`.

## Behavior change

**Extra-dirs pass now honors `.gitignore`** (Fix B for #428, closes #433):

- Gitignored top-level directories (e.g. `worktrees/`) are no longer copied into the shadow build tree. Previously the bundler would unconditionally materialise every non-infrastructure top-level directory.
- Global git ignore (`~/.config/git/ignore`) and hidden-directory rules now apply at the top level in addition to `.gitignore`.
- **Negation caveat:** if your `.gitignore` contains a pattern like `!worktrees/keep/` to opt a sub-path back in, the negation is silently ignored by this pass. The extra-dirs walk operates whole-directory-or-nothing at `max_depth=1`; the parent directory is excluded before the negation can apply. Consumers relying on negated sub-path opt-ins in an otherwise-excluded directory will need an alternative arrangement (e.g. move the directory outside the gitignored subtree).
