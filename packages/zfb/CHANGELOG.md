# Changelog

## 0.1.0-next.4

### Bug fixes

**Binary executable bit + launcher EACCES** (#441, #444 §1):

- #441: The bundled `bin/zfb.mjs` launcher was missing its executable bit in the published tarball, causing `zfb: command not found` after `npm install -g`.
- #444 §1: Companion fix ensuring the per-platform native binary receives its executable bit correctly on POSIX systems.

**`--version` stamping** (#445, #444 §2):

- #445: `zfb --version` printed `0.0.0` instead of the actual release version; the binary is now stamped with `ZFB_RELEASE_VERSION` at build time.
- #444 §2: Ensures the version reported by `--version` matches the npm package version for all platforms.

**`paths()` worker / `zfb/content` snapshot flow** (#442):

- #442: Fixed a race in the content-snapshot flow where `paths()` could be invoked before the worker finished writing the snapshot, causing intermittent empty-route tables.

**`@/` tsconfig path-alias regression** (#443):

- #443: The `@/` TypeScript path alias was dropped during the build pipeline refactor in 0.1.0-next.3, breaking imports that relied on the alias in user projects.

**`create-zfb` scaffold dist-tag** (#343):

- #343: `npm create zfb@latest` was resolving the wrong dist-tag on the first install; scaffolded projects now pin to the exact CLI version (`=<ver>` rather than `^<ver>`) to prevent silent downgrade once the stable release lands.

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
