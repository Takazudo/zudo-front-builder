# Empty-token Wind contract fixtures

Issue #3830 owns this finite set. `manifest.json` reuses all 15 canonical case IDs from `../profile.json`; the extra `native-*` IDs are Wind-only bounded guarantees drawn from current catalog entries. The Rust test independently authors declaration expectations and asserts generation with truly empty token maps. `configurations.json` is input data for repeat compilations, not an expected-output generator.

`index.html` supplies browser geometry with a styled target and unstyled control for each applicable case. Inject generated CSS after the fixture's `fixture-baseline` layer, and compile each case with its named configuration in an isolated document. The runner must compare the declared expected outcome even when CSS is absent. Browser observations remain pending until manager integration with #3829/#3831. In particular, `mx-auto` needs horizontal and vertical writing modes with ltr/rtl; physical Wind margins do not imply logical reference parity. `p-0-empty` is an intentional generation difference; `p-0-mapped` gives the reference a declared zero token.

The exact Wind layer-order prelude occurs once per nonempty stylesheet. Named tokens are `var(--zw-*)` writes with declarations in the `zw-tokens` layer; token changes and removals require new compilation output. Reset mode is recorded separately as `none` and is not a design preset.

The baseline styles live in a low-priority CSS layer. Wind utility rules are unlayered; the reference utility layer is introduced after the fixture layer. This lets the 24px baseline padding remain visible on controls while either engine’s generated `p-0` can override it on targets. The fixture does not add corrective utility CSS.
