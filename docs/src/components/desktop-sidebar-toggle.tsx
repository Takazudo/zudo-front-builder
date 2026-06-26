"use client";

// Re-exports the package component, but adds the SPA-navigation flash guard
// at module scope. The package only wires AFTER_NAVIGATE_EVENT inside a
// useEffect, which is torn down by zfb-runtime's unmountIslands() BEFORE the
// swap event fires — so the restore never runs. The guard must live at module
// scope to survive island unmount/remount. (zudolab/zudo-doc#2198)
export {
  DesktopSidebarToggle,
  SIDEBAR_STORAGE_KEY,
} from "@takazudo/zudo-doc/desktop-sidebar-toggle-island";

import { BEFORE_NAVIGATE_EVENT, AFTER_NAVIGATE_EVENT } from "@takazudo/zudo-doc/transitions";

// Register once at module load (browser-only), before any navigation.
let spaNavGuardRegistered = false;
function registerSidebarSpaNavGuard() {
  if (typeof document === "undefined" || spaNavGuardRegistered) return;
  spaNavGuardRegistered = true;

  let wasHidden = false;
  let swapToken = 0;
  // Safety net for navs that never reach `restore` (aborted/superseded).
  const STUCK_MARKER_FALLBACK_MS = 1500;

  const capture = () => {
    wasHidden = document.documentElement.hasAttribute("data-sidebar-hidden");
    document.documentElement.setAttribute("data-sidebar-no-transition", "");
    const token = ++swapToken;
    window.setTimeout(() => {
      if (token === swapToken) {
        document.documentElement.removeAttribute("data-sidebar-no-transition");
      }
    }, STUCK_MARKER_FALLBACK_MS);
  };

  const restore = () => {
    // Re-apply the no-transition marker first so the hidden geometry snaps in
    // without animating (the swap already wiped the capture-time marker).
    document.documentElement.setAttribute("data-sidebar-no-transition", "");
    if (wasHidden) {
      document.documentElement.setAttribute("data-sidebar-hidden", "");
    }
    // Drop the marker after the restored state is committed — defer one extra
    // frame so the restored attribute doesn't animate in.
    const token = ++swapToken;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (token === swapToken) {
          document.documentElement.removeAttribute("data-sidebar-no-transition");
        }
      });
    });
  };

  document.addEventListener(BEFORE_NAVIGATE_EVENT, capture);
  document.addEventListener(AFTER_NAVIGATE_EVENT, restore);
}

registerSidebarSpaNavGuard();
