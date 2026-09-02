# Custom syntax extras (issue #1848)

syntect's bundled `SyntaxSet` (`SyntaxSet::load_defaults_newlines()`) ships a
fixed set of grammars and cannot be extended at runtime — `SyntaxSet` is
immutable after `SyntaxSetBuilder::build()`. To add grammars beyond the
bundled set, zfb generates its own replacement dump: `load_defaults_newlines()`
plus the `.sublime-syntax` files under this directory, loaded via
`SyntaxSetBuilder::add_from_folder`. See `tests/bin/generate_syntax_dump.rs`
for the generator and `src/syntect_highlight.rs` for where the resulting dump
is loaded.

Each subdirectory below holds one language's `.sublime-syntax` source plus the
upstream license covering it.

## toml/

- Source: [`TOML/TOML.sublime-syntax`](https://github.com/sublimehq/Packages/blob/master/TOML/TOML.sublime-syntax)
  from `sublimehq/Packages` — the same repository Sublime Text itself bundles
  as its default package set (and the source syntect's own `load_defaults_newlines()`
  grammars, including `JavaScript.sublime-syntax`, are drawn from).
- Fetched at commit `e4e988e202322a04bea88e6d991f3dc537600731`.
- License: `toml/LICENSE`, copied verbatim from the repository root
  (`sublimehq/Packages`' default permissive license; no `TOML`-specific
  `-license` sidecar file exists in that repo, so the root license applies).
- Self-contained: no `extends:` dependency on another grammar file.

## typescript/

- Source: bat's hand-converted `TypeScript.sublime-syntax` and
  `TypsecriptReact.sublime-syntax` at commit
  `0acf9417cd6d9635d927dda9f8e6ab5e57176fa4` (the latter is stored here with
  its filename typo corrected). bat vendors Microsoft's
  `TypeScript-Sublime-Plugin`, not `braver/TypeScriptSyntax`; its submodule pin
  is `ba45efd058df5111837e30fb9598cfc8cbd51095`, and the conversion history is
  recorded by bat commits `1d46eb8e`, `c97aa551`, `3358b075`, and `d7b65194`.
- License: Apache-2.0. `typescript/LICENSE` is copied verbatim from Microsoft;
  bat's derivative is MIT OR Apache-2.0. See `typescript/PROVENANCE.md`.
- Self-contained: neither grammar uses `extends:`.

The current `sublimehq/Packages` TypeScript/TSX grammars cannot be shimmed into
syntect 5.3.0: they depend on Sublime Text syntax inheritance and newer
`branch_point`/`branch`/`fail` keys that the 5.3.0 YAML loader does not
implement. syntect master's `load_defaults_newlines()` already contains
TypeScript, TSX, and TOML (192 syntaxes), so these extras can be removed when
the trigger-based syntect 6 migration recorded in `DEPENDENCIES.md` proceeds.
