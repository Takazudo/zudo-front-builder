"use client";

// Issue #1758 (Stage Audit Fix epic #1794, Wave 3): proves the ISLANDS call
// site of `remap_project_plugin_virtual_modules_to_shadow`
// (`crates/zfb/src/commands/build.rs`, reached via
// `build_default_islands_payload`) carries the same sibling `?raw` +
// nested-module-worker rewrites #1701 already proved for the two
// CLIENT-script call sites (reroot.client.ts / vmodule-host.ts). Importing
// the same registered virtual module `virtual:reroot-vmodule-host` the
// client entry already imports brings along both the sibling `?raw` payload
// and the nested `vmodule-worker.ts` registration from vmodule-host.ts's own
// graph — no separate worker wiring needed on the island itself.
import "virtual:reroot-vmodule-host";

export function RerootIsland() {
  return <button id="reroot-island">ZFB_REROOT_ISLAND</button>;
}
