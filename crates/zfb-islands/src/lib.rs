//! zfb-islands: islands runtime support crate.
//!
//! Module slots:
//! - [`hydration`] — server-side HTML rewrite that turns each rendered island's
//!   marker-bracketed output into the `<div data-zfb-island="…" data-props="…">`
//!   wrapper the client-side hydration runtime walks. Owned by Sub 3.
//!
//! Sub 1 is expected to add an AST scanner module here (e.g. `scanner`) and a
//! `ClientBundler` trait module. Sub 4 will add the build-time `<Island>`
//! wrapper transform. Each sub-task lives in its own module under this crate
//! so the surface stays orthogonal.

pub mod hydration;

pub use hydration::{hydration_script_tag, rewrite_islands, IslandDescriptor, IslandRewriteError};
