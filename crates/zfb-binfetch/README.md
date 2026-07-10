# zfb-binfetch

Small blocking download helper used by zfb build scripts when they need to
fetch platform tool binaries.

The crate intentionally owns only the HTTP transfer and retry loop. Callers
remain responsible for choosing a temp path, verifying the downloaded bytes,
setting permissions, and atomically moving the file into its final slot.

## Public API

- **`FetchOpts`** - retry and timeout knobs:
  - `attempts` defaults to `3`.
  - `connect_timeout` defaults to 15 seconds per attempt.
  - `overall_timeout` defaults to `None`; this avoids aborting a large but
    healthy slow download. Callers may opt into a whole-request deadline.
  - `initial_backoff` defaults to 500 ms and doubles after each failed
    attempt.
- **`fetch_to_file(url, dest, opts)`** - streams `url` to `dest`, retrying
  transient send/status/body failures. On success, the full response body is
  written to `dest`. On failure, any partial `dest` file is removed.

## Usage

```rust,ignore
use std::path::Path;
use zfb_binfetch::{fetch_to_file, FetchOpts};

let tmp = Path::new("vendor/bin/esbuild.tmp");
fetch_to_file("https://example.invalid/esbuild", tmp, &FetchOpts::default())?;

// Caller-owned follow-up: hash, chmod, and rename `tmp` into place.
```

## Tests

```sh
cargo test -p zfb-binfetch
```

The tests use a tiny local HTTP server to cover retry-after-truncation,
HTTP-status failures, zero-attempt behavior, and cleanup of partial files.
