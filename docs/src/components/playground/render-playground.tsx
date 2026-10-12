"use client";

import { computed, getScope, Show, signal } from "@takazudo/zfb/zudo-react";

import {
  isNoteDirectiveSample,
  NOTE_DIRECTIVE_SAMPLE,
  noteDirectiveFeatures,
  type NoteDirectiveFeatures,
} from "./admonition-sample";
import DiagnosticsList from "./diagnostics-list";
import OptionRow from "./option-row";
import PlaygroundShell from "./playground-shell";
import { createGfmSignals, snapshotGfm } from "./gfm-signals";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import { createModuleLoader, wasmModule } from "./use-wasm-module";
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
    features?: NoteDirectiveFeatures;
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

const SAMPLES = [SAMPLE, NOTE_DIRECTIVE_SAMPLE] as const;

function isValidFilename(filename: string): boolean {
  return filename.endsWith(".md") || filename.endsWith(".mdx");
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const loadRenderModule = createModuleLoader<RenderModule>(async () => {
  const module = await import("@takazudo/zfb-md-wasm/render");
  return { renderHtml: module.renderHtml };
});

const inputClass =
  "rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2";

export default function RenderPlayground() {
  const scope = getScope();
  const source = signal(SAMPLE.value);
  const activeSampleId = signal<string | undefined>(SAMPLE.id);
  const dialect = signal<string>("markdown");
  const filename = signal("preview.md");
  const gfm = createGfmSignals();
  const cjkFriendly = signal(false);
  const hardBreaks = signal(false);
  const noteDirectiveEnabled = signal(false);
  const theme = signal("");
  const codeHighlightMode = signal<string>("inline");
  const classPrefix = signal("hi-");
  const displayedResult = signal<RenderResult | null>(null);

  // A run promise still resolves after the island is disposed by a
  // soft-navigation swap. Invalidate the local result guard so its callback
  // cannot publish into the old island instance.
  let runSequence = 0;
  scope.onCleanup(() => {
    runSequence += 1;
  });

  const { state, run } = wasmModule<RenderModule, [string, RenderOptions], RenderResult>(scope, {
    loadModule: loadRenderModule,
    execute: (module, input, options) => module.renderHtml(input, options),
  });

  const invalidFilename = computed(() => !isValidFilename(filename.value));
  const classMode = computed(() => codeHighlightMode.value === "class");
  const pending = computed(() => state.value.status === "loading");
  const hasRun = computed(() => state.value.status !== "idle");
  const html = computed(() => displayedResult.value?.html ?? "");
  const diagnostics = computed(() => displayedResult.value?.diagnostics ?? []);
  const emptyMessage = computed(() =>
    state.value.status === "ready" ? "No diagnostics." : undefined,
  );
  const trapError = computed(() => {
    const current = state.value;
    return current.status === "error" ? errorMessage(current.error) : null;
  });
  const hasTrapError = computed(() => trapError.value !== null);

  function pickSample(sample: PlaygroundSample) {
    source.value = sample.value;
    activeSampleId.value = sample.id;
    noteDirectiveEnabled.value = isNoteDirectiveSample(sample);
  }

  function runRender() {
    const currentRun = ++runSequence;
    displayedResult.value = null;

    if (invalidFilename.value) return;

    const mode = codeHighlightMode.value as CodeHighlightMode;
    const options: RenderOptions = {
      filename: filename.value,
      dialect: dialect.value as Dialect,
      pipeline: {
        gfm: snapshotGfm(gfm),
        cjkFriendly: cjkFriendly.value,
        hardBreaks: hardBreaks.value,
        ...(noteDirectiveEnabled.value ? { features: noteDirectiveFeatures(true) } : {}),
        ...(mode === "class" ? {} : theme.value ? { theme: theme.value } : {}),
        codeHighlight: {
          mode,
          ...(mode === "class" ? { classPrefix: classPrefix.value } : {}),
        },
      },
    };

    void run(source.value, options)
      .then((nextResult) => {
        if (currentRun === runSequence) displayedResult.value = nextResult;
      })
      .catch(() => undefined);
  }

  return (
    <PlaygroundShell
      value={source}
      onInput={() => {
        activeSampleId.value = undefined;
      }}
      onRun={runRender}
      pending={pending}
      hasRun={hasRun}
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
        <div class="flex flex-col gap-vsp-sm">
          <SamplePicker
            samples={SAMPLES}
            activeSampleId={activeSampleId}
            onPick={pickSample}
            label="Samples"
          />

          <div class="grid grid-cols-1 gap-vsp-sm md:grid-cols-2">
            <label class="flex flex-col gap-vsp-3xs text-small text-fg">
              <span class="font-mono">dialect</span>
              <select class={inputClass} modelValue={dialect}>
                <option value="markdown">markdown</option>
                <option value="mdx">mdx</option>
              </select>
            </label>

            <label class="flex flex-col gap-vsp-3xs text-small text-fg">
              <span class="font-mono">filename</span>
              <input class={inputClass} modelValue={filename} aria-invalid={invalidFilename} />
              <Show when={invalidFilename}>
                {() => (
                  <span class="text-caption text-danger">
                    Use a filename ending in .md or .mdx.
                  </span>
                )}
              </Show>
            </label>
          </div>

          <fieldset class="m-0 flex flex-col gap-vsp-2xs border-0 p-0">
            <legend class="font-mono text-small font-semibold text-fg">pipeline.gfm</legend>
            <OptionRow label="pipeline.gfm.strikethrough" checked={gfm.strikethrough} />
            <OptionRow label="pipeline.gfm.table" checked={gfm.table} />
            <OptionRow label="pipeline.gfm.autolinkLiteral" checked={gfm.autolinkLiteral} />
            <OptionRow label="pipeline.gfm.taskListItem" checked={gfm.taskListItem} />
            <OptionRow label="pipeline.gfm.footnoteDefinition" checked={gfm.footnoteDefinition} />
          </fieldset>

          <div class="grid grid-cols-1 gap-vsp-2xs md:grid-cols-2">
            <OptionRow label="pipeline.cjkFriendly" checked={cjkFriendly} />
            <OptionRow label="pipeline.hardBreaks" checked={hardBreaks} />
          </div>

          <OptionRow
            label={'pipeline.features.directives.note = "Note"'}
            checked={noteDirectiveEnabled}
          />

          <label class="flex flex-col gap-vsp-3xs text-small text-fg">
            <span class="font-mono">pipeline.theme</span>
            <input
              class={`${inputClass} disabled:opacity-50`}
              modelValue={theme}
              placeholder="built-in default"
              disabled={classMode}
            />
          </label>

          <div class="grid grid-cols-1 gap-vsp-2xs md:grid-cols-2">
            <label class="flex flex-col gap-vsp-3xs text-small text-fg">
              <span class="font-mono">pipeline.codeHighlight.mode</span>
              <select class={inputClass} modelValue={codeHighlightMode}>
                <option value="inline">inline</option>
                <option value="class">class</option>
              </select>
            </label>

            <Show when={classMode}>
              {() => (
                <label class="flex flex-col gap-vsp-3xs text-small text-fg">
                  <span class="font-mono">pipeline.codeHighlight.classPrefix</span>
                  <input class={inputClass} modelValue={classPrefix} />
                </label>
              )}
            </Show>
          </div>
        </div>
      }
      output={
        <div class="flex flex-col gap-vsp-sm">
          <section aria-label="HTML source">
            <h3 class="mb-vsp-2xs text-small font-semibold">HTML source</h3>
            <pre class="m-0 overflow-auto whitespace-pre-wrap break-words">{html}</pre>
          </section>
          <section aria-label="HTML preview">
            <h3 class="mb-vsp-2xs text-small font-semibold">Preview</h3>
            <iframe
              title="Rendered HTML preview"
              srcdoc={html}
              // Keep one harmless, non-empty token so the iframe stays sandboxed
              // even if a renderer drops an empty attribute; this list
              // intentionally omits the script permission.
              sandbox="allow-forms"
              // Chromium moves sequential focus into the child browsing
              // context without exposing :focus or :focus-visible on this
              // element. The preview is passive output and its full source is
              // adjacent, so omit that otherwise invisible keyboard stop.
              tabindex={-1}
              class="block w-full border-0 bg-surface"
            />
          </section>
        </div>
      }
      diagnostics={
        <DiagnosticsList
          diagnostics={diagnostics}
          label="Render diagnostics"
          emptyMessage={emptyMessage}
        />
      }
      trapError={
        <Show when={hasTrapError}>
          {() => <span role="alert">{computed(() => trapError.value ?? "")}</span>}
        </Show>
      }
    />
  );
}
