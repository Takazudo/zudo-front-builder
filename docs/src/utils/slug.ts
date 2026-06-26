// Thin re-export shim. The canonical slug rules moved into the package
// (`@takazudo/zudo-doc/slug`) as part of the package-first migration (#1251).
// Host code keeps importing from `@/utils/slug`; the implementation —
// including the canonical-root rule — lives once in the package.
export { toRouteSlug, toHistorySlug, toSlugParams, toTitleCase } from "@takazudo/zudo-doc/slug";
