// Thin stub — doc-route-paths moved to the package (docs-1.0-migration, S3).
// Re-exports the pure prop-builder helpers from @takazudo/zudo-doc/doc-route-paths.

export type { DocNavNode, PaginationOverrides } from "@takazudo/zudo-doc/doc-route-paths";

export {
  flattenTree,
  findNode,
  flattenSubtree,
  resolveDocPrevNext,
  rewriteNavHref,
  remapNavChildHrefs,
} from "@takazudo/zudo-doc/doc-route-paths";
