# TypeScript and TSX grammar provenance

The syntax definitions in this directory are copied verbatim from
`sharkdp/bat` commit `0acf9417cd6d9635d927dda9f8e6ab5e57176fa4`:

- `assets/syntaxes/02_Extra/TypeScript.sublime-syntax`
- `assets/syntaxes/02_Extra/TypsecriptReact.sublime-syntax` (renamed here to
  correct the filename typo; the YAML `name` is unchanged)

bat hand-converted these files from Microsoft's `TypeScript-Sublime-Plugin`
at submodule commit `ba45efd058df5111837e30fb9598cfc8cbd51095`.
The conversion history is recorded by bat commits `1d46eb8e`, `c97aa551`,
`3358b075`, and `d7b65194`.

Microsoft's upstream grammar is Apache-2.0 licensed. bat distributes its
derivative under MIT OR Apache-2.0; this directory uses the common
Apache-2.0 option. `LICENSE` is Microsoft's license copied verbatim.
