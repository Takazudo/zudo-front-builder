// Thin re-export shim. The sidebar-resizer moved into the package
// (`@takazudo/zudo-doc/sidebar-resizer`) as part of the package-first
// migration (#1251). The package ships the same idempotent
// initSidebarResizer() function that was previously local.
export { initSidebarResizer } from "@takazudo/zudo-doc/sidebar-resizer";
