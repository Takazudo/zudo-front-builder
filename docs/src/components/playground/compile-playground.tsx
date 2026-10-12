"use client";

import {
  computed,
  getScope,
  Show,
  signal,
  type ReadonlySignal,
  type Signal,
} from "@takazudo/zfb/zudo-react";

import {
  isNoteDirectiveSample,
  NOTE_DIRECTIVE_SAMPLE,
  noteDirectiveFeatures,
} from "./admonition-sample";
import DiagnosticsList from "./diagnostics-list";
import OptionRow from "./option-row";
import PlaygroundShell from "./playground-shell";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import type { ZfbMdWasmOptions } from "@takazudo/zfb-md-wasm";

import type { PlaygroundDiagnostic } from "./result-types";
import { createModuleLoader, wasmModule } from "./use-wasm-module";

type GfmKey = "strikethrough" | "table" | "autolinkLiteral" | "taskListItem" | "footnoteDefinition";

interface GfmOptions {
  strikethrough: boolean;
  table: boolean;
  autolinkLiteral: boolean;
  taskListItem: boolean;
  footnoteDefinition: boolean;
}

type CompileOptions = ZfbMdWasmOptions;

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

const COMPILE_SAMPLES = [COMPILE_SAMPLE, NOTE_DIRECTIVE_SAMPLE] as const;

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

type GfmSignals = Record<GfmKey, Signal<boolean>>;

const loadCompileModule = createModuleLoader(() => import("@takazudo/zfb-md-wasm"));

function CompileOptionsPanel({
  development,
  gfm,
  filename,
  cjkFriendly,
  hardBreaks,
  noteDirectiveEnabled,
  activeSampleId,
  onPickSample,
}: {
  development: Signal<boolean>;
  gfm: GfmSignals;
  filename: Signal<string>;
  cjkFriendly: Signal<boolean>;
  hardBreaks: Signal<boolean>;
  noteDirectiveEnabled: Signal<boolean>;
  activeSampleId: ReadonlySignal<string | undefined>;
  onPickSample: (sample: PlaygroundSample) => void;
}) {
  const filenameIsInvalid = computed(() => !isValidFilename(filename.value));

  return (
    <div class="flex flex-col gap-vsp-sm">
      <SamplePicker
        samples={COMPILE_SAMPLES}
        activeSampleId={activeSampleId}
        onPick={onPickSample}
      />

      <div class="grid grid-cols-1 gap-hsp-lg md:grid-cols-2">
        <label class="flex flex-col gap-vsp-2xs text-small font-semibold text-fg">
          filename
          <input
            type="text"
            class="rounded border border-muted bg-surface px-hsp-md py-vsp-2xs font-mono text-small font-normal text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            modelValue={filename}
            aria-invalid={filenameIsInvalid}
          />
          <Show when={filenameIsInvalid}>
            {() => (
              <span class="text-caption font-normal text-danger">Use a .md or .mdx filename.</span>
            )}
          </Show>
        </label>
      </div>

      <OptionRow label="development" checked={development} />

      <div class="grid grid-cols-1 gap-x-hsp-lg md:grid-cols-2">
        {GFM_OPTIONS.map(({ key, label }) => (
          <OptionRow label={label} checked={gfm[key]} />
        ))}
      </div>

      <div class="grid grid-cols-1 gap-x-hsp-lg md:grid-cols-2">
        <OptionRow label="pipeline.cjkFriendly" checked={cjkFriendly} />
        <OptionRow label="pipeline.hardBreaks" checked={hardBreaks} />
      </div>

      <OptionRow
        label={'pipeline.features.directives.note = "Note"'}
        checked={noteDirectiveEnabled}
      />
    </div>
  );
}

function CompileOutput({ result }: { result: ReadonlySignal<CompileResult | null> }) {
  const hasResult = computed(() => result.value !== null);
  const hasCode = computed(() => result.value?.code != null);

  return (
    <Show
      when={hasResult}
      fallback={() => <p class="m-0 text-small text-muted">Press Run to emit an ES module.</p>}
    >
      {() => (
        <div class="flex min-w-0 flex-col gap-vsp-md">
          <section class="flex min-w-0 flex-col gap-vsp-2xs" aria-label="code">
            <h3 class="m-0 text-small font-semibold text-fg">code</h3>
            <Show
              when={hasCode}
              fallback={() => (
                <p class="m-0 text-small text-muted">No module source was emitted.</p>
              )}
            >
              {() => (
                <pre class="m-0 max-w-full overflow-auto whitespace-pre font-mono text-caption text-code-fg">
                  {computed(() => result.value?.code ?? "")}
                </pre>
              )}
            </Show>
          </section>

          <section class="flex min-w-0 flex-col gap-vsp-2xs" aria-label="frontmatter">
            <h3 class="m-0 text-small font-semibold text-fg">frontmatter</h3>
            <pre class="m-0 max-w-full overflow-auto whitespace-pre-wrap font-mono text-caption text-code-fg">
              {computed(() => formatFrontmatter(result.value?.frontmatter ?? null))}
            </pre>
          </section>
        </div>
      )}
    </Show>
  );
}

export default function CompilePlayground() {
  const scope = getScope();
  const source = signal(COMPILE_SAMPLE.value);
  const activeSampleId = signal<string | undefined>(COMPILE_SAMPLE.id);
  const development = signal(false);
  const filename = signal("preview.mdx");
  const gfm: GfmSignals = {
    strikethrough: signal(DEFAULT_GFM.strikethrough),
    table: signal(DEFAULT_GFM.table),
    autolinkLiteral: signal(DEFAULT_GFM.autolinkLiteral),
    taskListItem: signal(DEFAULT_GFM.taskListItem),
    footnoteDefinition: signal(DEFAULT_GFM.footnoteDefinition),
  };
  const cjkFriendly = signal(false);
  const hardBreaks = signal(false);
  const noteDirectiveEnabled = signal(false);
  const validationDiagnostics = signal<readonly PlaygroundDiagnostic[]>([]);

  const { state, run } = wasmModule(scope, {
    loadModule: loadCompileModule,
    execute: async (module, input: string, options: CompileOptions): Promise<CompileResult> =>
      module.compile(input, options),
  });

  function handleRun(): void {
    if (!isValidFilename(filename.value)) {
      validationDiagnostics.value = [filenameDiagnostic()];
      return;
    }

    validationDiagnostics.value = [];
    void run(source.value, {
      filename: filename.value,
      development: development.value,
      pipeline: {
        gfm: {
          strikethrough: gfm.strikethrough.value,
          table: gfm.table.value,
          autolinkLiteral: gfm.autolinkLiteral.value,
          taskListItem: gfm.taskListItem.value,
          footnoteDefinition: gfm.footnoteDefinition.value,
        },
        cjkFriendly: cjkFriendly.value,
        hardBreaks: hardBreaks.value,
        ...(noteDirectiveEnabled.value ? { features: noteDirectiveFeatures(true) } : {}),
      },
    }).catch(() => {
      // The runner publishes rejected calls in its error state. Keeping the
      // rejection handled here prevents an expected trap from becoming an
      // unhandled promise while the shell displays that state.
    });
  }

  const result = computed(() => {
    const current = state.value;
    return current.status === "ready" && validationDiagnostics.value.length === 0
      ? current.result
      : null;
  });
  const diagnostics = computed(() => {
    const current = state.value;
    if (validationDiagnostics.value.length > 0) return validationDiagnostics.value;
    return current.status === "ready" ? current.result.diagnostics : [];
  });
  const emptyMessage = computed(() =>
    state.value.status === "ready" ? "No diagnostics." : undefined,
  );
  const rejectedError = computed(() => {
    const current = state.value;
    return current.status === "error" ? formatRejectedError(current.error) : "";
  });

  return (
    <PlaygroundShell
      value={source}
      onInput={() => {
        activeSampleId.value = undefined;
      }}
      onRun={handleRun}
      pending={computed(() => state.value.status === "loading")}
      hasRun={computed(
        () => state.value.status !== "idle" || validationDiagnostics.value.length > 0,
      )}
      options={
        <CompileOptionsPanel
          development={development}
          gfm={gfm}
          filename={filename}
          cjkFriendly={cjkFriendly}
          hardBreaks={hardBreaks}
          noteDirectiveEnabled={noteDirectiveEnabled}
          activeSampleId={activeSampleId}
          onPickSample={(sample) => {
            source.value = sample.value;
            activeSampleId.value = sample.id;
            noteDirectiveEnabled.value = isNoteDirectiveSample(sample);
          }}
        />
      }
      output={<CompileOutput result={result} />}
      diagnostics={
        <DiagnosticsList
          diagnostics={diagnostics}
          label="Diagnostics"
          emptyMessage={emptyMessage}
        />
      }
      trapError={rejectedError}
      textareaLabel="MDX source"
    />
  );
}
