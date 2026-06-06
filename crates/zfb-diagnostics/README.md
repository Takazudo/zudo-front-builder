# zfb-diagnostics

Structured framed error display for the zfb framework — rustc/esbuild-style source-context panels with sourcemap-aware JS error decoding.

## Public API

### `Diagnostic` and `FramedError`

```rust,ignore
use zfb_diagnostics::{Diagnostic, FramedError, render_framed};

// Attach source line from disk automatically
let diag = Diagnostic::new("src/pages/index.tsx", 12, 5, "unexpected token")
    .try_attach_source_from_disk(Path::new("src/pages/index.tsx"));

// Or supply the source line directly
let diag = Diagnostic::with_source(
    "src/lib.rs", 42, 1,
    "missing field `title`",
    "pub struct Post { body: String }",
);

// Render to a framed string (for embedding in larger messages)
let text = render_framed(&diag);

// Wrap as an anyhow error (for `?` propagation)
let err: anyhow::Error = FramedError(diag).into_anyhow();
```

`FramedError` implements `fmt::Display` and `std::error::Error`; `Display`
calls `render_framed` internally so `println!("{}", framed_err)` produces the
full panel.

### Frame format

```
error: <message>
 --> file:line:col
   |
N  | <source line>
   |   ^
   |
```

Mirrors the rustc / clippy / esbuild diagnostic style so terminal output is
visually consistent with other tools in the dev workflow.

### Path helpers

```rust,ignore
// Strip a project root prefix so paths in error messages are relative
let label = project_relative(Path::new("/home/user/my-site/src/foo.tsx"),
                              Some(Path::new("/home/user/my-site")));
// → "src/foo.tsx"
```

### JS runtime error decoding

```rust,ignore
use zfb_diagnostics::{from_js_runtime_error_with_decoder, DecodedPosition};

let diag = from_js_runtime_error_with_decoder(
    bundle_path,
    bundle_source,
    bundled_line,
    bundled_col,
    message,
    sourcemap_json,
    project_root,
    decode_fn,  // impl Fn(...) -> Option<impl DecodedPosition>
);
```

The `DecodedPosition` trait (`file`, `line`, `col`, `source_content`) decouples
the decoder from any particular sourcemap library. Callers supply a closure that
wraps their chosen library.

### Export-ident locator

```rust,ignore
// Returns (line, col) of the identifier in a named export declaration
let pos = locate_export_ident(source, "MyComponent");
```

Used to produce precise diagnostics when a bundled export cannot be found at
the expected position.

## Tests

```sh
cargo test -p zfb-diagnostics
```

Inline tests cover the framed renderer, `project_relative`, and the
`from_js_runtime_error_with_decoder` path for both resolved and unresolved
sourcemap positions.
