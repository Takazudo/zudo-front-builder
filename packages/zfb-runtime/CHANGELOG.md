# @takazudo/zfb-runtime

## 0.2.0-migration.0

### Minor changes

BREAKING (semantic): `@takazudo/zfb-runtime` `<ViewTransitions />` no
longer injects a meta tag or inline router script. Consumers must
add `@view-transition { navigation: auto; }` to their top-level
stylesheet to opt in to cross-document View Transitions. The
previous injection was incompatible with the spec and produced no
visible transitions in any browser; switching is a strict
improvement. Mounts of `<ViewTransitions />` continue to compile.

The `ViewTransitionsElement` type export is preserved. The function
signature is unchanged (`() => readonly ViewTransitionsElement[]`),
so existing host code that calls `ViewTransitions()` and spreads the
return into JSX continues to typecheck — it now spreads `[]`, which
is a no-op in JSX.

See zudolab/zudo-doc#1491 for the bug report and browser-verification
proof, and zudolab/zudo-doc#1500 for the host-side migration shape.

## 0.1.0-migration.0

Initial migration release.
