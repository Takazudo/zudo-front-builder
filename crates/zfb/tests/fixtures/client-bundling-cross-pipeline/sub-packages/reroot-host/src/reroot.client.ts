// A genuinely-CLAIMED pnpm-workspace sub-package host (issue #1674): the
// client entry lives under `sub-packages/reroot-host`, and its graph reaches
// SIBLING workspace source through a `?raw` import plus a project-local module
// worker whose own graph also reaches a sibling `?raw`. Before #1674 the
// client-script preprocessing stage rooted at the single package and rejected
// the sibling as "outside the mirrorable project tree".
import panel from "../../reroot-shared/panel.frag?raw";

// Issue #1701 (Wave 2): pulls in a registered virtual module whose OWN
// source absolute-imports a THIRD sibling package — proving the workspace
// tier of `remap_virtual_module_project_imports_to_shadow` (#1699/#1700)
// carries a virtual-module edge through `?raw`/glob/nested-worker
// preprocessing, not just the plain relative sibling import above.
import "virtual:reroot-vmodule-host";

console.info("ZFB_REROOT_CLIENT_ENTRY", panel);

// Browser-only worker registration, capability-guarded so importing this
// module in the SSR V8 pass never constructs the Worker.
if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener(
    "zfb-reroot-worker",
    () => new Worker(new URL("./reroot.worker.ts", import.meta.url), { type: "module" }),
    { once: true },
  );
}
