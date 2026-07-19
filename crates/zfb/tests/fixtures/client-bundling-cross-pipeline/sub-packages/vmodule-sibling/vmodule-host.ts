// Issue #1701 (Wave 2 confirm pass): this file is reached ONLY through the
// registered virtual module `virtual:reroot-vmodule-host` (see
// ../reroot-host/preset.mjs), whose own source ABSOLUTE-imports this path —
// not a relative project import like reroot.client.ts/reroot.worker.ts
// above. It exercises the sibling rewrite kinds the CLIENT-script pipeline
// supports — a `?raw` import and a nested module worker — so the workspace
// tier of `remap_virtual_module_project_imports_to_shadow` (#1699/#1700) is
// proven to carry those preprocessing passes through a virtual-module edge,
// not just a plain relative sibling import.
//
// `import.meta.glob` is deliberately NOT exercised here: client scripts do
// not expand `import.meta.glob` at all (issue #1404 — the client-script
// preprocessing mirror handles `?raw`/worker but never a glob call), so a
// glob reached through this CLIENT virtual-module edge would ship unexpanded
// regardless of the #1699 remap. Glob expansion for a virtual-module sibling
// belongs to the SSR/bundler path (where `run_esbuild`'s materialise expands
// it), not this client path.
import panel from "./vmodule-panel.frag?raw";

console.info("ZFB_VMODULE_SIBLING_ENTRY", panel);

// Browser-only worker registration, capability-guarded so importing this
// module in the SSR V8 pass never constructs the Worker.
if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener(
    "zfb-vmodule-worker",
    () => new Worker(new URL("./vmodule-worker.ts", import.meta.url), { type: "module" }),
    { once: true },
  );
}
