"use client";

import { useState } from "preact/hooks";

type SummaryResponse = {
  summary: string;
  fallback: boolean;
  model?: string;
  reason?: string;
};

const SAMPLE_TEXT =
  "Zudo Front Builder renders static pages by default, then lets individual routes opt into request-time execution with prerender = false. On Cloudflare, the adapter emits a Worker entry and static assets in the same dist directory, so API routes can read bindings while the rest of the site stays cacheable.";

export default function SummarizeIsland() {
  const [text, setText] = useState(SAMPLE_TEXT);
  const [result, setResult] = useState<SummaryResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  async function handleSubmit(event: Event) {
    event.preventDefault();
    const input = text.trim();

    if (!input) {
      setResult(null);
      setError("Enter text to summarize.");
      return;
    }

    setIsLoading(true);
    setError(null);
    setResult({ summary: "Summarizing...", fallback: false });

    try {
      const response = await fetch("/api/summarize", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ text: input }),
      });
      const payload = (await response.json()) as Partial<SummaryResponse> & { error?: string };

      if (!response.ok) {
        setResult(null);
        setError(payload.error ?? "The summarizer could not process that text.");
        return;
      }

      if (typeof payload.summary !== "string" || !payload.summary.trim()) {
        setResult(null);
        setError("The summarizer returned an empty response.");
        return;
      }

      setResult({
        summary: payload.summary,
        fallback: Boolean(payload.fallback),
        model: payload.model,
        reason: payload.reason,
      });
    } catch {
      setResult(null);
      setError("The summarizer is unreachable.");
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <form class="summarizer" onSubmit={handleSubmit}>
      <label class="field">
        <span>Source text</span>
        <textarea
          value={text}
          rows={10}
          maxLength={6000}
          onInput={(event) => setText(event.currentTarget.value)}
        />
      </label>
      <div class="actions">
        <button type="submit" disabled={isLoading}>
          {isLoading ? "Summarizing..." : "Summarize"}
        </button>
        <span class="count">{text.trim().split(/\s+/).filter(Boolean).length} words</span>
      </div>
      <output class="result" aria-live="polite">
        {error ? <p class="error">{error}</p> : null}
        {result ? (
          <section>
            <div class="result-header">
              <span>Summary</span>
              {result.fallback ? <span class="badge">Fallback</span> : null}
            </div>
            <pre>{result.summary}</pre>
            {result.reason ? <p class="hint">Reason: {result.reason}</p> : null}
          </section>
        ) : null}
      </output>
    </form>
  );
}
