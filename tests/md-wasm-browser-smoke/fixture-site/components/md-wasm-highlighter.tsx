"use client";

import { useState } from "preact/hooks";

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

function HighlightMarkup({ id, html }: { id: string; html: string | undefined }) {
  return <div id={id} dangerouslySetInnerHTML={{ __html: html ?? "" }} />;
}

/**
 * The browser lane intentionally uses the public lazy root import. No package
 * code, glue module, or wasm byte can load before a user action reaches one of
 * these handlers.
 */
export function MdWasmHighlighter() {
  const [status, setStatus] = useState("Waiting for a user action.");
  const [highlights, setHighlights] = useState<RenderedHighlights>({});
  const [incompleteHighlights, setIncompleteHighlights] = useState<RenderedHighlights>({});
  const [fallbackDiagnostic, setFallbackDiagnostic] = useState("");
  const [incompleteDiagnostics, setIncompleteDiagnostics] = useState("");
  const [recoveryState, setRecoveryState] = useState("");
  const [recoveryMarkup, setRecoveryMarkup] = useState("");

  async function runHighlights(): Promise<void> {
    setStatus("Loading the packed browser package…");

    // This exact root import is the browser-condition contract. zfb's islands
    // bundler must emit the package's glue and wasm resources, but neither is
    // requested until this click invokes one of the public API calls.
    const wasm = await import("@takazudo/zfb-md-wasm");
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

    setHighlights(rendered);
    setIncompleteHighlights(incomplete);
    setIncompleteDiagnostics(JSON.stringify(diagnostics));
    setFallbackDiagnostic(
      JSON.stringify({ html: fallback.html, diagnostics: fallback.diagnostics }),
    );
    setStatus("Highlights complete.");
  }

  async function forceTrapAndRecover(): Promise<void> {
    setStatus("Forcing the test-only trap and waiting for recovery…");
    const wasm = await import("@takazudo/zfb-md-wasm");
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
    setRecoveryMarkup(recovered.html);
    setRecoveryState(JSON.stringify({ before, after, trapName }));
    setStatus("Trap recovery complete.");
  }

  return (
    <section aria-label="md-wasm semantic highlighter">
      <button type="button" id="run-highlights" onClick={runHighlights}>
        Run semantic highlights
      </button>
      <button type="button" id="force-trap" onClick={forceTrapAndRecover}>
        Force trap and recover
      </button>
      <p id="status">{status}</p>

      <section aria-label="Semantic highlight results">
        <h2>Semantic highlights</h2>
        <HighlightMarkup id="highlight-html" html={highlights.html} />
        <HighlightMarkup id="highlight-css" html={highlights.css} />
        <HighlightMarkup id="highlight-javascript" html={highlights.javascript} />
      </section>

      <section aria-label="Incomplete editor input results">
        <h2>Incomplete editor input</h2>
        <HighlightMarkup id="incomplete-html" html={incompleteHighlights.html} />
        <HighlightMarkup id="incomplete-css" html={incompleteHighlights.css} />
        <HighlightMarkup id="incomplete-javascript" html={incompleteHighlights.javascript} />
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
