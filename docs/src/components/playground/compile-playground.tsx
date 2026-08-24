/** @jsxRuntime automatic */
/** @jsxImportSource preact */
"use client";

import { useMemo, useState } from "preact/hooks";

import DiagnosticsList from "./diagnostics-list";
import OptionRow from "./option-row";
import PlaygroundShell from "./playground-shell";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import type { PlaygroundDiagnostic } from "./result-types";
import { createModuleLoader, useWasmModule } from "./use-wasm-module";

type JsxRuntime = "preact" | "react";
type GfmKey = "strikethrough" | "table" | "autolinkLiteral" | "taskListItem" | "footnoteDefinition";

interface GfmOptions {
  strikethrough: boolean;
  table: boolean;
  autolinkLiteral: boolean;
  taskListItem: boolean;
  footnoteDefinition: boolean;
}

interface CompileOptions {
  filename: string;
  jsxRuntime: JsxRuntime;
  development: boolean;
  pipeline: {
    gfm: GfmOptions;
    cjkFriendly: boolean;
    hardBreaks: boolean;
  };
}

interface CompileResult {
  code: string | null;
  frontmatter: unknown;
  diagnostics: readonly PlaygroundDiagnostic[];
}

const DEFAULT_GFM: GfmOptions = {
  strikethrough: true,
  table: true,
  autolinkLiteral: true,
  taskListItem: true,
  footnoteDefinition: true,
};

const GFM_OPTIONS: readonly { key: GfmKey; label: string }[] = [
  { key: "strikethrough", label: "gfm.strikethrough" },
  { key: "table", label: "gfm.table" },
  { key: "autolinkLiteral", label: "gfm.autolinkLiteral" },
  { key: "taskListItem", label: "gfm.taskListItem" },
  { key: "footnoteDefinition", label: "gfm.footnoteDefinition" },
];

const COMPILE_SAMPLE: PlaygroundSample = {
  id: "compile-sample",
  label: "MDX sample",
  value: `---
title: Compile sample
description: An MDX document for the compile playground.
---

# Hello from MDX

<Callout kind="info">This JSX component tag stays in the emitted module.</Callout>

The expression {1 + 2} is emitted as source, alongside ordinary **Markdown**.
`,
};

function isValidFilename(filename: string): boolean {
  return /\.mdx?$/.test(filename);
}

function filenameDiagnostic(): PlaygroundDiagnostic {
  return {
    severity: "error",
    source: "options",
    message: "filename must end in .md or .mdx.",
    line: null,
    column: null,
  };
}

function formatFrontmatter(frontmatter: unknown): string {
  const formatted = JSON.stringify(frontmatter, null, 2);
  return formatted === undefined ? "null" : formatted;
}

function formatRejectedError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function CompileOptionsPanel({
  development,
  gfm,
  jsxRuntime,
  filename,
  cjkFriendly,
  hardBreaks,
  onDevelopmentChange,
  onGfmChange,
  onJsxRuntimeChange,
  onFilenameChange,
  onCjkFriendlyChange,
  onHardBreaksChange,
  onPickSample,
}: {
  development: boolean;
  gfm: GfmOptions;
  jsxRuntime: JsxRuntime;
  filename: string;
  cjkFriendly: boolean;
  hardBreaks: boolean;
  onDevelopmentChange: (checked: boolean) => void;
  onGfmChange: (key: GfmKey, checked: boolean) => void;
  onJsxRuntimeChange: (runtime: JsxRuntime) => void;
  onFilenameChange: (value: string) => void;
  onCjkFriendlyChange: (checked: boolean) => void;
  onHardBreaksChange: (checked: boolean) => void;
  onPickSample: (sample: PlaygroundSample) => void;
}) {
  const filenameIsValid = isValidFilename(filename);

  return (
    <div className="flex flex-col gap-vsp-sm">
      <SamplePicker
        samples={[COMPILE_SAMPLE]}
        activeSampleId={COMPILE_SAMPLE.id}
        onPick={onPickSample}
      />

      <div className="grid grid-cols-1 gap-hsp-lg md:grid-cols-2">
        <label className="flex flex-col gap-vsp-2xs text-small font-semibold text-fg">
          jsxRuntime
          <select
            className="rounded border border-muted bg-surface px-hsp-md py-vsp-2xs font-normal text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            value={jsxRuntime}
            onChange={(event) => onJsxRuntimeChange(event.currentTarget.value as JsxRuntime)}
          >
            <option value="preact">preact</option>
            <option value="react">react</option>
          </select>
        </label>

        <label className="flex flex-col gap-vsp-2xs text-small font-semibold text-fg">
          filename
          <input
            type="text"
            className="rounded border border-muted bg-surface px-hsp-md py-vsp-2xs font-mono text-small font-normal text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            value={filename}
            aria-invalid={!filenameIsValid}
            onInput={(event) => onFilenameChange(event.currentTarget.value)}
          />
          {!filenameIsValid ? (
            <span className="text-caption font-normal text-danger">
              Use a .md or .mdx filename.
            </span>
          ) : null}
        </label>
      </div>

      <OptionRow label="development" checked={development} onChange={onDevelopmentChange} />

      <div className="grid grid-cols-1 gap-x-hsp-lg md:grid-cols-2">
        {GFM_OPTIONS.map(({ key, label }) => (
          <OptionRow
            key={key}
            label={label}
            checked={gfm[key]}
            onChange={(checked) => onGfmChange(key, checked)}
          />
        ))}
      </div>

      <div className="grid grid-cols-1 gap-x-hsp-lg md:grid-cols-2">
        <OptionRow
          label="pipeline.cjkFriendly"
          checked={cjkFriendly}
          onChange={onCjkFriendlyChange}
        />
        <OptionRow label="pipeline.hardBreaks" checked={hardBreaks} onChange={onHardBreaksChange} />
      </div>
    </div>
  );
}

function CompileOutput({ result }: { result: CompileResult | null }) {
  if (result === null) {
    return <p className="m-0 text-small text-muted">Press Run to emit an ES module.</p>;
  }

  return (
    <div className="flex min-w-0 flex-col gap-vsp-md">
      <section className="flex min-w-0 flex-col gap-vsp-2xs" aria-label="code">
        <h3 className="m-0 text-small font-semibold text-fg">code</h3>
        {result.code === null ? (
          <p className="m-0 text-small text-muted">No module source was emitted.</p>
        ) : (
          <pre className="m-0 max-w-full overflow-auto whitespace-pre font-mono text-caption text-code-fg">
            {result.code}
          </pre>
        )}
      </section>

      <section className="flex min-w-0 flex-col gap-vsp-2xs" aria-label="frontmatter">
        <h3 className="m-0 text-small font-semibold text-fg">frontmatter</h3>
        <pre className="m-0 max-w-full overflow-auto whitespace-pre-wrap font-mono text-caption text-code-fg">
          {formatFrontmatter(result.frontmatter)}
        </pre>
      </section>
    </div>
  );
}

function CompilePlayground() {
  const [source, setSource] = useState(COMPILE_SAMPLE.value);
  const [jsxRuntime, setJsxRuntime] = useState<JsxRuntime>("preact");
  const [development, setDevelopment] = useState(false);
  const [filename, setFilename] = useState("preview.mdx");
  const [gfm, setGfm] = useState<GfmOptions>({ ...DEFAULT_GFM });
  const [cjkFriendly, setCjkFriendly] = useState(false);
  const [hardBreaks, setHardBreaks] = useState(false);
  const [validationDiagnostics, setValidationDiagnostics] = useState<
    readonly PlaygroundDiagnostic[]
  >([]);

  const loadCompileModule = useMemo(
    () => createModuleLoader(() => import("@takazudo/zfb-md-wasm")),
    [],
  );
  const { state, run } = useWasmModule({
    loadModule: loadCompileModule,
    execute: async (module, input: string, options: CompileOptions): Promise<CompileResult> =>
      module.compile(input, options),
  });

  function handleRun(): void {
    if (!isValidFilename(filename)) {
      setValidationDiagnostics([filenameDiagnostic()]);
      return;
    }

    setValidationDiagnostics([]);
    void run(source, {
      filename,
      jsxRuntime,
      development,
      pipeline: {
        gfm: { ...gfm },
        cjkFriendly,
        hardBreaks,
      },
    }).catch(() => {
      // The runner publishes rejected calls in its error state. Keeping the
      // rejection handled here prevents an expected trap from becoming an
      // unhandled promise while the shell displays that state.
    });
  }

  const result =
    state.status === "ready" && validationDiagnostics.length === 0 ? state.result : null;
  const diagnostics =
    validationDiagnostics.length > 0
      ? validationDiagnostics
      : state.status === "ready"
        ? state.result.diagnostics
        : [];
  const rejectedError = state.status === "error" ? formatRejectedError(state.error) : null;

  return (
    <PlaygroundShell
      value={source}
      onInput={setSource}
      onRun={handleRun}
      pending={state.status === "loading"}
      hasRun={state.status !== "idle" || validationDiagnostics.length > 0}
      options={
        <CompileOptionsPanel
          development={development}
          gfm={gfm}
          jsxRuntime={jsxRuntime}
          filename={filename}
          cjkFriendly={cjkFriendly}
          hardBreaks={hardBreaks}
          onDevelopmentChange={setDevelopment}
          onGfmChange={(key, checked) => setGfm((value) => ({ ...value, [key]: checked }))}
          onJsxRuntimeChange={setJsxRuntime}
          onFilenameChange={setFilename}
          onCjkFriendlyChange={setCjkFriendly}
          onHardBreaksChange={setHardBreaks}
          onPickSample={(sample) => setSource(sample.value)}
        />
      }
      output={<CompileOutput result={result} />}
      diagnostics={
        <DiagnosticsList
          diagnostics={diagnostics}
          label="Diagnostics"
          emptyMessage={state.status === "ready" ? "No diagnostics." : undefined}
        />
      }
      trapError={rejectedError}
      textareaProps={{ "aria-label": "MDX source" }}
    />
  );
}

(CompilePlayground as { displayName?: string }).displayName = "CompilePlayground";

export default CompilePlayground;
