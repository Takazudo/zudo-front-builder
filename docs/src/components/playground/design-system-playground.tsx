/** @jsxRuntime automatic */
/** @jsxImportSource preact */
"use client";

import { useEffect, useRef } from "preact/hooks";
import type { WorkshopSnapshot } from "./design-workshop/model.js";
import { mountDesignWorkshop } from "./design-workshop/workshop.js";

// Host-owned memory survives native route changes even if island modules reload.
// A hard reload intentionally starts a fresh browser session.
type WorkshopWindow = Window & { __zfbDesignWorkshopSnapshot?: WorkshopSnapshot };

export default function DesignSystemPlayground() {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!root.current) return;
    const host = window as WorkshopWindow;
    const workshop = mountDesignWorkshop(root.current, {
      initialState: host.__zfbDesignWorkshopSnapshot,
    });
    return () => {
      host.__zfbDesignWorkshopSnapshot = workshop.getSnapshot();
      workshop.dispose();
    };
  }, []);

  return <div id="design-workshop" ref={root} />;
}

DesignSystemPlayground.displayName = "DesignSystemPlayground";
