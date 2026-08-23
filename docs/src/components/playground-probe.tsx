/** @jsxRuntime automatic */
/** @jsxImportSource preact */
"use client";

import { useState } from "preact/hooks";

export default function PlaygroundProbe() {
  const [isFlipped, setIsFlipped] = useState(false);
  const [wasmStatus, setWasmStatus] = useState("WASM has not been loaded.");
  const [isLoading, setIsLoading] = useState(false);

  async function loadWasmVersion(): Promise<void> {
    setIsLoading(true);
    setWasmStatus("Loading WASM…");

    try {
      // Keep the focused entry point behind the user action: no WASM-related
      // resource should be requested before this handler runs.
      const { version } = await import("@takazudo/zfb-md-wasm/parse");
      setWasmStatus(`Loaded @takazudo/zfb-md-wasm ${version()}.`);
    } catch {
      setWasmStatus("Could not load @takazudo/zfb-md-wasm.");
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <section
      aria-label="Playground island probe"
      className="flex flex-col gap-vsp-xs rounded-lg border border-muted bg-surface p-hsp-lg text-fg"
    >
      <p className="text-small">Hydration state: {isFlipped ? "flipped" : "initial"}</p>
      <div className="flex flex-wrap gap-hsp-sm">
        <button
          type="button"
          className="rounded border border-muted bg-bg px-hsp-md py-vsp-2xs text-small text-fg transition-colors hover:border-accent focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
          onClick={() => setIsFlipped((value) => !value)}
        >
          Flip local state
        </button>
        <button
          type="button"
          className="rounded bg-accent px-hsp-md py-vsp-2xs text-small font-semibold text-bg transition-colors hover:bg-accent-hover focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:pointer-events-none disabled:opacity-50"
          disabled={isLoading}
          onClick={loadWasmVersion}
        >
          {isLoading ? "Loading…" : "Load WASM version"}
        </button>
      </div>
      <p aria-live="polite" className="text-caption text-muted">
        {wasmStatus}
      </p>
    </section>
  );
}
