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

## TypeScript / TSX — not included yet

TypeScript/TSX grammars are deliberately NOT in this directory yet. Every
`.sublime-syntax` source investigated for them is unusable with syntect as-is:

- Microsoft's own `TypeScript-Sublime-Plugin` and the community
  `braver/TypeScriptSyntax` (also the grammar `bat` vendors) ship only
  `.tmLanguage` (plist/XML), a format syntect's syntax loader cannot parse at
  all (`SyntaxSetBuilder::add_from_folder` only reads `.sublime-syntax` YAML;
  syntect's `plist-load` feature covers `.tmTheme` *themes*, not tmLanguage
  *syntax definitions*).
- `sublimehq/Packages` itself DOES ship genuine `.sublime-syntax` files —
  `JavaScript/TypeScript.sublime-syntax` and `JavaScript/TSX.sublime-syntax` —
  but both declare `extends: JavaScript.sublime-syntax` (Sublime Text 4's
  syntax-inheritance mechanism) and reference contexts (e.g. `script`) that
  only exist in the base `JavaScript.sublime-syntax` file. syntect has never
  implemented `extends:`/`version: 2` inheritance, so loading either file
  standalone fails to resolve those contexts.

See issue #1848's follow-up discussion for the sourcing decision on
TypeScript/TSX before adding them here.
