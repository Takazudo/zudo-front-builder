/** @jsxRuntime automatic */
/** @jsxImportSource preact */
"use client";

import { useEffect, useRef, useState } from "preact/hooks";

import DiagnosticsList from "./diagnostics-list";
import OptionRow from "./option-row";
import PlaygroundShell from "./playground-shell";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import { createModuleLoader, useWasmModule } from "./use-wasm-module";
import type { PlaygroundDiagnostic } from "./result-types";

type Dialect = "markdown" | "mdx";
type CodeHighlightMode = "inline" | "class";

interface RenderOptions {
  filename: string;
  dialect: Dialect;
  pipeline: {
    gfm: {
      strikethrough: boolean;
      table: boolean;
      autolinkLiteral: boolean;
      taskListItem: boolean;
      footnoteDefinition: boolean;
    };
    cjkFriendly: boolean;
    hardBreaks: boolean;
    theme?: string;
    codeHighlight: {
      mode: CodeHighlightMode;
      classPrefix?: string;
    };
  };
}

interface RenderResult {
  html: string | null;
  frontmatter: unknown;
  diagnostics: PlaygroundDiagnostic[];
}

interface RenderModule {
  renderHtml: (source: string, options: RenderOptions) => Promise<RenderResult>;
}

const SAMPLE: PlaygroundSample = {
  id: "render-basics",
  label: "GFM + HTML",
  value: `# Render preview

This sample exercises the controls below.

| Feature | Enabled |
| --- | --- |
| GFM table | yes |
| Task list | yes |

- [x] Render a table
- [ ] Try another dialect

> The same Rust pipeline powers this preview and \`zfb build\`.

~~~js
const message = "hello from zfb-md-wasm";
console.log(message);
~~~

<div data-preview-note="raw-html">Raw HTML passes through unchanged.</div>
`,
};

const SAMPLES = [SAMPLE] as const;

function isValidFilename(filename: string): boolean {
  return filename.endsWith(".md") || filename.endsWith(".mdx");
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function RenderPlayground() {
  const [source, setSource] = useState(SAMPLE.value);
  const [activeSampleId, setActiveSampleId] = useState<string | undefined>(SAMPLE.id);
  const [dialect, setDialect] = useState<Dialect>("markdown");
  const [filename, setFilename] = useState("preview.md");
  const [gfm, setGfm] = useState({
    strikethrough: true,
    table: true,
    autolinkLiteral: true,
    taskListItem: true,
    footnoteDefinition: true,
  });
  const [cjkFriendly, setCjkFriendly] = useState(false);
  const [hardBreaks, setHardBreaks] = useState(false);
  const [theme, setTheme] = useState("");
  const [codeHighlightMode, setCodeHighlightMode] = useState<CodeHighlightMode>("inline");
  const [classPrefix, setClassPrefix] = useState("hi-");
  const [displayedResult, setDisplayedResult] = useState<RenderResult | null>(null);

  const renderLoaderRef = useRef<(() => Promise<RenderModule>) | null>(null);
  const runSequenceRef = useRef(0);
  const loadRenderModule = () => {
    if (renderLoaderRef.current === null) {
      renderLoaderRef.current = createModuleLoader(async () => {
        const module = await import("@takazudo/zfb-md-wasm/render");
        return { renderHtml: module.renderHtml };
      });
    }

    return renderLoaderRef.current();
  };

  const { state, run } = useWasmModule<RenderModule, [string, RenderOptions], RenderResult>({
    loadModule: loadRenderModule,
    execute: (module, input, options) => module.renderHtml(input, options),
  });

  useEffect(
    () => () => {
      // A run promise still resolves after the island is unmounted by a
      // soft-navigation swap. Invalidate the local result guard so its callback
      // cannot publish into the old island instance.
      runSequenceRef.current += 1;
    },
    [],
  );

  const invalidFilename = !isValidFilename(filename);
  const pending = state.status === "loading";
  const html = displayedResult?.html ?? "";
  const diagnostics = displayedResult?.diagnostics ?? [];
  const trapError = state.status === "error" ? errorMessage(state.error) : null;

  function pickSample(sample: PlaygroundSample) {
    setSource(sample.value);
    setActiveSampleId(sample.id);
  }

  function runRender() {
    const runSequence = ++runSequenceRef.current;
    setDisplayedResult(null);

    if (invalidFilename) return;

    const options: RenderOptions = {
      filename,
      dialect,
      pipeline: {
        gfm,
        cjkFriendly,
        hardBreaks,
        ...(codeHighlightMode === "class" ? {} : theme ? { theme } : {}),
        codeHighlight: {
          mode: codeHighlightMode,
          ...(codeHighlightMode === "class" ? { classPrefix } : {}),
        },
      },
    };

    void run(source, options)
      .then((nextResult) => {
        if (runSequence === runSequenceRef.current) setDisplayedResult(nextResult);
      })
      .catch(() => undefined);
  }

  return (
    <PlaygroundShell
      value={source}
      onInput={(value) => {
        setSource(value);
        setActiveSampleId(undefined);
      }}
      onRun={runRender}
      pending={pending}
      hasRun={state.status !== "idle"}
      labels={{
        input: "Markdown / MDX",
        output: "HTML output",
        run: "Run",
        pending: "Running…",
        firstRunNotice: "The first Run downloads a WebAssembly module.",
        apiReference: "API reference",
      }}
      apiReferenceHref="/docs/api/md-wasm/"
      options={
        <div className="flex flex-col gap-vsp-sm">
          <SamplePicker
            samples={SAMPLES}
            activeSampleId={activeSampleId}
            onPick={pickSample}
            label="Samples"
          />

          <div className="grid grid-cols-1 gap-vsp-sm md:grid-cols-2">
            <label className="flex flex-col gap-vsp-3xs text-small text-fg">
              <span className="font-mono">dialect</span>
              <select
                className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                value={dialect}
                onChange={(event) => setDialect(event.currentTarget.value as Dialect)}
              >
                <option value="markdown">markdown</option>
                <option value="mdx">mdx</option>
              </select>
            </label>

            <label className="flex flex-col gap-vsp-3xs text-small text-fg">
              <span className="font-mono">filename</span>
              <input
                className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                value={filename}
                aria-invalid={invalidFilename}
                onInput={(event) => setFilename(event.currentTarget.value)}
              />
              {invalidFilename ? (
                <span className="text-caption text-danger">
                  Use a filename ending in .md or .mdx.
                </span>
              ) : null}
            </label>
          </div>

          <fieldset className="m-0 flex flex-col gap-vsp-2xs border-0 p-0">
            <legend className="font-mono text-small font-semibold text-fg">pipeline.gfm</legend>
            <OptionRow
              label="pipeline.gfm.strikethrough"
              checked={gfm.strikethrough}
              onChange={(checked) => setGfm((current) => ({ ...current, strikethrough: checked }))}
            />
            <OptionRow
              label="pipeline.gfm.table"
              checked={gfm.table}
              onChange={(checked) => setGfm((current) => ({ ...current, table: checked }))}
            />
            <OptionRow
              label="pipeline.gfm.autolinkLiteral"
              checked={gfm.autolinkLiteral}
              onChange={(checked) =>
                setGfm((current) => ({ ...current, autolinkLiteral: checked }))
              }
            />
            <OptionRow
              label="pipeline.gfm.taskListItem"
              checked={gfm.taskListItem}
              onChange={(checked) => setGfm((current) => ({ ...current, taskListItem: checked }))}
            />
            <OptionRow
              label="pipeline.gfm.footnoteDefinition"
              checked={gfm.footnoteDefinition}
              onChange={(checked) =>
                setGfm((current) => ({ ...current, footnoteDefinition: checked }))
              }
            />
          </fieldset>

          <div className="grid grid-cols-1 gap-vsp-2xs md:grid-cols-2">
            <OptionRow
              label="pipeline.cjkFriendly"
              checked={cjkFriendly}
              onChange={setCjkFriendly}
            />
            <OptionRow label="pipeline.hardBreaks" checked={hardBreaks} onChange={setHardBreaks} />
          </div>

          <label className="flex flex-col gap-vsp-3xs text-small text-fg">
            <span className="font-mono">pipeline.theme</span>
            <input
              className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:opacity-50"
              value={theme}
              placeholder="built-in default"
              disabled={codeHighlightMode === "class"}
              onInput={(event) => setTheme(event.currentTarget.value)}
            />
          </label>

          <div className="grid grid-cols-1 gap-vsp-2xs md:grid-cols-2">
            <label className="flex flex-col gap-vsp-3xs text-small text-fg">
              <span className="font-mono">pipeline.codeHighlight.mode</span>
              <select
                className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                value={codeHighlightMode}
                onChange={(event) =>
                  setCodeHighlightMode(event.currentTarget.value as CodeHighlightMode)
                }
              >
                <option value="inline">inline</option>
                <option value="class">class</option>
              </select>
            </label>

            {codeHighlightMode === "class" ? (
              <label className="flex flex-col gap-vsp-3xs text-small text-fg">
                <span className="font-mono">pipeline.codeHighlight.classPrefix</span>
                <input
                  className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                  value={classPrefix}
                  onInput={(event) => setClassPrefix(event.currentTarget.value)}
                />
              </label>
            ) : null}
          </div>
        </div>
      }
      output={
        <div className="flex flex-col gap-vsp-sm">
          <section aria-label="HTML source">
            <h3 className="mb-vsp-2xs text-small font-semibold">HTML source</h3>
            <pre className="m-0 overflow-auto whitespace-pre-wrap break-words">{html}</pre>
          </section>
          <section aria-label="HTML preview">
            <h3 className="mb-vsp-2xs text-small font-semibold">Preview</h3>
            <iframe
              title="Rendered HTML preview"
              srcDoc={html}
              // Preact removes an empty sandbox attribute during hydration. Keep
              // one harmless, non-empty token so the iframe remains sandboxed;
              // notably, this list intentionally omits the script permission.
              sandbox="allow-forms"
              className="block w-full border-0 bg-surface"
            />
          </section>
        </div>
      }
      diagnostics={
        <DiagnosticsList
          diagnostics={diagnostics}
          label="Render diagnostics"
          emptyMessage={state.status === "ready" ? "No diagnostics." : undefined}
        />
      }
      trapError={trapError ? <span role="alert">{trapError}</span> : null}
    />
  );
}

(RenderPlayground as { displayName?: string }).displayName = "RenderPlayground";

export default RenderPlayground;
