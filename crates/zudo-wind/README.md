# zudo-wind

`zudo-wind` is the owned, versioned utility CSS language engine for zfb. This
first crate slice defines candidate syntax, variant vocabulary, structured
diagnostics, and compile input/output types. It does not compile CSS yet.

The public `structural_split` function separates one class token without a
configured vocabulary. `parse_candidate` additionally checks configured
breakpoints, dark mode, supported variants, and canonical variant order.
Utility catalog resolution and CSS value validation follow in later slices.

The language contract is `research/3242-zudo-wind-v1-spec.md` in the workspace.
