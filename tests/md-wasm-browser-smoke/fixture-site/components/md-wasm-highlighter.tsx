"use client";

import { computed, signal } from "@takazudo/zfb/zudo-react";
import type { ReadonlySignal } from "@takazudo/zfb/zudo-react";

const loadMdWasm = () => import("@takazudo/zfb-md-wasm");

const EXAMPLES = [
  ["html", '<main data-x="a & b">hello</main>'],
  ["css", ".button { color: red; }"],
  ["javascript", "const value = '<tag>';"],
] as const;

const INCOMPLETE_EXAMPLES = [
  ["html", "<article><span"],
  ["css", ".card { color:"],
  ["javascript", "const answer ="],
] as const;

type RenderedHighlights = Record<string, string>;

function HighlightMarkup({ id, html }: { id: string; html: ReadonlySignal<string> }) {
  // The highlighter produces trusted markup; rawHtml owns this reactive region.
  return <div id={id} rawHtml={html} />;
}

/**
 * The browser lane intentionally uses the public lazy root import. No package
 * code, glue module, or wasm byte can load before a user action reaches one of
 * these handlers.
 */
export function MdWasmHighlighter() {
  const status = signal("Waiting for a user action.");
  const highlights = signal<RenderedHighlights>({});
  const incompleteHighlights = signal<RenderedHighlights>({});
  const fallbackDiagnostic = signal("");
  const incompleteDiagnostics = signal("");
  const recoveryState = signal("");
  const recoveryMarkup = signal("");

  async function runHighlights(): Promise<void> {
    status.value = "Loading the packed browser package…";

    // This first-party module performs the public package-root lazy import.
    // zfb's islands bundler must emit the package's glue and wasm resources,
    // but neither is requested until this click invokes it.
    const wasm = await loadMdWasm();
    const rendered: RenderedHighlights = {};
    for (const [language, code] of EXAMPLES) {
      const result = await wasm.highlightCode(code, { language });
      if (result.html === null || result.diagnostics.length !== 0) {
        throw new Error(`expected a semantic ${language} result without diagnostics`);
      }
      rendered[language] = result.html;
    }

    const incomplete: RenderedHighlights = {};
    const diagnostics: Record<string, unknown> = {};
    for (const [language, code] of INCOMPLETE_EXAMPLES) {
      const result = await wasm.highlightCode(code, { language });
      if (result.html === null) {
        throw new Error(`expected usable incomplete ${language} markup`);
      }
      incomplete[language] = result.html;
      diagnostics[language] = result.diagnostics;
    }

    const fallback = await wasm.highlightCode("<tag>&", { language: "not-a-bundled-syntax" });
    if (fallback.html === null) {
      throw new Error("expected unknown-language fallback markup");
    }

    highlights.value = rendered;
    incompleteHighlights.value = incomplete;
    incompleteDiagnostics.value = JSON.stringify(diagnostics);
    fallbackDiagnostic.value = JSON.stringify({
      html: fallback.html,
      diagnostics: fallback.diagnostics,
    });
    status.value = "Highlights complete.";
  }

  async function forceTrapAndRecover(): Promise<void> {
    status.value = "Forcing the test-only trap and waiting for recovery…";
    const wasm = await loadMdWasm();
    const before = wasm.__getTrapRecoveryStateForTests();
    let trapName = "";
    try {
      await wasm.__forceTrapForTests();
    } catch (error) {
      trapName = error instanceof Error ? error.name : String(error);
    }

    const recovered = await wasm.highlightCode("const recovered = true;", {
      language: "javascript",
    });
    if (recovered.html === null || recovered.diagnostics.length !== 0) {
      throw new Error("highlightCode did not recover after the forced trap");
    }

    const after = wasm.__getTrapRecoveryStateForTests();
    recoveryMarkup.value = recovered.html;
    recoveryState.value = JSON.stringify({ before, after, trapName });
    status.value = "Trap recovery complete.";
  }

  return (
    <section aria-label="md-wasm semantic highlighter">
      <button type="button" id="run-highlights" on:click={runHighlights}>
        Run semantic highlights
      </button>
      <button type="button" id="force-trap" on:click={forceTrapAndRecover}>
        Force trap and recover
      </button>
      <p id="status">{status}</p>

      <section aria-label="Semantic highlight results">
        <h2>Semantic highlights</h2>
        <HighlightMarkup id="highlight-html" html={computed(() => highlights.value.html ?? "")} />
        <HighlightMarkup id="highlight-css" html={computed(() => highlights.value.css ?? "")} />
        <HighlightMarkup
          id="highlight-javascript"
          html={computed(() => highlights.value.javascript ?? "")}
        />
      </section>

      <section aria-label="Incomplete editor input results">
        <h2>Incomplete editor input</h2>
        <HighlightMarkup
          id="incomplete-html"
          html={computed(() => incompleteHighlights.value.html ?? "")}
        />
        <HighlightMarkup
          id="incomplete-css"
          html={computed(() => incompleteHighlights.value.css ?? "")}
        />
        <HighlightMarkup
          id="incomplete-javascript"
          html={computed(() => incompleteHighlights.value.javascript ?? "")}
        />
        <output id="incomplete-diagnostics">{incompleteDiagnostics}</output>
      </section>

      <section aria-label="Unknown language fallback">
        <h2>Unknown language fallback</h2>
        <output id="fallback-diagnostic">{fallbackDiagnostic}</output>
      </section>

      <section aria-label="Trap recovery result">
        <h2>Trap recovery</h2>
        <HighlightMarkup id="recovery-highlight" html={recoveryMarkup} />
        <output id="recovery-state">{recoveryState}</output>
      </section>
    </section>
  );
}
