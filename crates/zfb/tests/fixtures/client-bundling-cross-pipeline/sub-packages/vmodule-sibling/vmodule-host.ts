// Issue #1701 (Wave 2 confirm pass): this file is reached ONLY through the
// registered virtual module `virtual:reroot-vmodule-host` (see
// ../reroot-host/preset.mjs), whose own source ABSOLUTE-imports this path —
// not a relative project import like reroot.client.ts/reroot.worker.ts
// above. It exercises all three sibling rewrite kinds in one file so the
// workspace tier of `remap_virtual_module_project_imports_to_shadow`
// (#1699/#1700) is proven to carry every preprocessing pass through a
// virtual-module edge, not just `?raw`.
import panel from "./vmodule-panel.frag?raw";

console.info("ZFB_VMODULE_SIBLING_ENTRY", panel);

const globModules = import.meta.glob("./vmodule-glob/*.ts", { eager: true });
console.info("ZFB_VMODULE_SIBLING_GLOB", globModules);

// Browser-only worker registration, capability-guarded so importing this
// module in the SSR V8 pass never constructs the Worker.
if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener(
    "zfb-vmodule-worker",
    () => new Worker(new URL("./vmodule-worker.ts", import.meta.url), { type: "module" }),
    { once: true },
  );
}
