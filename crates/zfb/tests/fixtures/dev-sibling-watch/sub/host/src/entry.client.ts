// Client-script entry for a genuinely-CLAIMED pnpm-workspace sub-package host
// (issue #1678). It reaches a SIBLING workspace package's source through a
// `?raw` import, and registers a project-local module worker whose own graph
// reaches a second sibling `?raw`. Editing either sibling file must invalidate
// the dev bundle without a `zfb dev` restart — the dev watcher registers the
// sibling directory dynamically on the tick that discovers the import.
//
// It also imports a sibling PLAIN module (`../../shared/plain`, issue #1711
// Scenario D) — neither `?raw` nor a worker dependency — proving the
// `client_script_siblings` registry (issue #1710) closes the DEV loop for
// that third sibling shape the same way it does for the two above.
import panel from "../../shared/panel.frag?raw";
import { plainMarker } from "../../shared/plain";

console.info("ZFB_SIBLING_CLIENT_ENTRY", panel);
console.info("ZFB_SIBLING_PLAIN_ENTRY", plainMarker);

if (typeof globalThis.addEventListener === "function") {
  globalThis.addEventListener(
    "zfb-sibling-worker",
    () => new Worker(new URL("./entry.worker.ts", import.meta.url), { type: "module" }),
    { once: true },
  );
}
