"use client";

import {
  computed,
  getScope,
  Show,
  signal,
  type ReadonlySignal,
  type Ref,
} from "@takazudo/zfb/zudo-react";

import DiagnosticsList from "./diagnostics-list";
import PlaygroundShell from "./playground-shell";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import { createModuleLoader, wasmModule } from "./use-wasm-module";

/** The direct highlighter accepts the full role names as override keys. */
type HighlightRole = "keyword" | "string";

type HighlightDiagnostic = {
  severity: "error" | "warning";
  source: "options" | "highlight" | "internal";
  message: string;
  line: number | null;
  column: number | null;
};

interface HighlightCodeOptions {
  language: string;
  classPrefix?: string;
  roleClasses?: Partial<Record<HighlightRole, string>>;
}

interface HighlightCodeResult {
  html: string | null;
  diagnostics: HighlightDiagnostic[];
}

interface HighlightModule {
  highlightCode: (code: string, options: HighlightCodeOptions) => Promise<HighlightCodeResult>;
}

interface HighlightSample extends PlaygroundSample {
  language: string;
  classPrefix?: string;
  roleClasses?: Partial<Record<HighlightRole, string>>;
}

const LANGUAGE_OPTIONS = [
  "html",
  "css",
  "javascript",
  "typescript",
  "json",
  "markdown",
  "rust",
  "python",
  "bash",
  "sql",
] as const;

const SAMPLES: readonly HighlightSample[] = [
  {
    id: "html",
    label: "HTML",
    language: "html",
    value: '<main data-x="a & b">hello</main>',
  },
  {
    id: "css",
    label: "CSS",
    language: "css",
    value: ".button { color: red; }",
  },
  {
    id: "javascript",
    label: "JavaScript",
    language: "javascript",
    value: 'const answer = 42;\nconsole.log("hello");',
  },
  {
    id: "incomplete",
    label: "Incomplete HTML",
    language: "html",
    value: "<article><span",
  },
  {
    id: "unknown-language",
    label: "Unknown language",
    language: "not-a-bundled-syntax",
    value: "<tag>&",
  },
];

type HighlightOutcome = "success" | "warning" | "options-error" | "error";

function getOutcome(result: HighlightCodeResult | null): HighlightOutcome | null {
  if (result === null) return null;
  if (result.html === null) {
    return result.diagnostics.some((diagnostic) => diagnostic.source === "options")
      ? "options-error"
      : "error";
  }
  return result.diagnostics.some((diagnostic) => diagnostic.severity === "warning")
    ? "warning"
    : "success";
}

function outcomeLabel(outcome: HighlightOutcome): string {
  switch (outcome) {
    case "success":
      return "Success";
    case "warning":
      return "Success with warning";
    case "options-error":
      return "Options error";
    case "error":
      return "Highlight error";
  }
}

function outcomeClassName(outcome: HighlightOutcome): string {
  if (outcome === "success") return "text-success";
  if (outcome === "warning") return "text-warning";
  return "text-danger";
}

function buildRoleClasses(
  keywordClass: string,
  stringClass: string,
): Partial<Record<HighlightRole, string>> | undefined {
  const roleClasses: Partial<Record<HighlightRole, string>> = {};
  const keyword = keywordClass.trim();
  const string = stringClass.trim();

  if (keyword !== "") roleClasses.keyword = keyword;
  if (string !== "") roleClasses.string = string;

  return Object.keys(roleClasses).length > 0 ? roleClasses : undefined;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const loadHighlightModule = createModuleLoader<HighlightModule>(async () => {
  const module = await import("@takazudo/zfb-md-wasm/highlight");
  return { highlightCode: module.highlightCode };
});

const inputClass =
  "rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2";

function HighlightOutput({ result }: { result: ReadonlySignal<HighlightCodeResult | null> }) {
  const hasResult = computed(() => result.value !== null);
  const hasHtml = computed(() => result.value?.html != null);
  const html = computed(() => result.value?.html ?? "");
  const failureMessage = computed(() =>
    result.value?.diagnostics.some((diagnostic) => diagnostic.source === "options")
      ? "No HTML output: the supplied options were rejected."
      : "No HTML output was returned.",
  );

  return (
    <Show
      when={hasResult}
      fallback={() => (
        <p class="m-0 text-small text-muted">Run the highlighter to see semantic HTML.</p>
      )}
    >
      {() => (
        <Show
          when={hasHtml}
          fallback={() => <p class="m-0 text-small text-danger">{failureMessage}</p>}
        >
          {() => (
            <div class="flex flex-col gap-vsp-sm">
              <section aria-label="Rendered semantic HTML">
                <h3 class="mb-vsp-2xs text-small font-semibold">Rendered semantic HTML</h3>
                <div class="zd-html-preview-code" rawHtml={html} />
              </section>
              <section aria-label="Raw HTML source">
                <h3 class="mb-vsp-2xs text-small font-semibold">Raw HTML source</h3>
                <pre class="m-0 overflow-auto whitespace-pre-wrap break-words">{html}</pre>
              </section>
            </div>
          )}
        </Show>
      )}
    </Show>
  );
}

export default function HighlightPlayground() {
  const scope = getScope();
  const initialSample = SAMPLES[2];
  const source = signal(initialSample.value);
  const activeSampleId = signal<string | undefined>(initialSample.id);
  const language = signal(initialSample.language);
  const classPrefix = signal("hi-");
  const keywordClass = signal("");
  const stringClass = signal("");
  const displayedResult = signal<HighlightCodeResult | null>(null);
  const languageInput: Ref<HTMLInputElement> = { current: null };

  // A run promise still resolves after the island is disposed by a
  // soft-navigation swap. Invalidate the local result guard so its callback
  // cannot publish into the old island instance.
  let runSequence = 0;
  scope.onCleanup(() => {
    runSequence += 1;
  });

  // zudo-react's v1 attribute vocabulary has no `list`, so the datalist
  // association is applied to the owned input after activation.
  scope.onActivate(() => {
    languageInput.current?.setAttribute("list", "highlight-languages");
  });

  const { state, run } = wasmModule<
    HighlightModule,
    [string, HighlightCodeOptions],
    HighlightCodeResult
  >(scope, {
    loadModule: loadHighlightModule,
    execute: (module, input, options) => module.highlightCode(input, options),
  });

  const outcome = computed(() => getOutcome(displayedResult.value));
  const hasOutcome = computed(() => outcome.value !== null);
  const diagnostics = computed(() => displayedResult.value?.diagnostics ?? []);
  const emptyMessage = computed(() => (displayedResult.value ? "No diagnostics." : undefined));
  const trapError = computed(() => {
    const current = state.value;
    return current.status === "error" ? errorMessage(current.error) : null;
  });
  const hasTrapError = computed(() => trapError.value !== null);

  function clearDisplayedResult() {
    runSequence += 1;
    displayedResult.value = null;
  }

  function pickSample(sample: PlaygroundSample) {
    const selected = SAMPLES.find((candidate) => candidate.id === sample.id);
    if (selected === undefined) return;

    clearDisplayedResult();
    source.value = selected.value;
    activeSampleId.value = selected.id;
    language.value = selected.language;
    classPrefix.value = selected.classPrefix ?? "hi-";
    keywordClass.value = selected.roleClasses?.keyword ?? "";
    stringClass.value = selected.roleClasses?.string ?? "";
  }

  function runHighlight() {
    const currentRun = ++runSequence;
    displayedResult.value = null;

    const roleClasses = buildRoleClasses(keywordClass.value, stringClass.value);
    const options: HighlightCodeOptions = {
      language: language.value,
      classPrefix: classPrefix.value,
      ...(roleClasses ? { roleClasses } : {}),
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
        clearDisplayedResult();
        activeSampleId.value = undefined;
      }}
      onRun={runHighlight}
      pending={computed(() => state.value.status === "loading")}
      hasRun={computed(() => state.value.status !== "idle")}
      labels={{
        input: "Source code",
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
              <span class="font-mono">language</span>
              <input
                ref={languageInput}
                class={inputClass}
                modelValue={language}
                on:input={() => {
                  clearDisplayedResult();
                  activeSampleId.value = undefined;
                }}
              />
              <datalist id="highlight-languages">
                {LANGUAGE_OPTIONS.map((option) => (
                  <option value={option} />
                ))}
              </datalist>
            </label>

            <label class="flex flex-col gap-vsp-3xs text-small text-fg">
              <span class="font-mono">classPrefix</span>
              <input class={inputClass} modelValue={classPrefix} on:input={clearDisplayedResult} />
            </label>
          </div>

          <fieldset class="m-0 flex flex-col gap-vsp-2xs border-0 p-0">
            <legend class="font-mono text-small font-semibold text-fg">roleClasses</legend>
            <p class="m-0 text-caption text-muted">
              Optional full-role class overrides; clear a field to restore its default semantic
              class.
            </p>
            <div class="grid grid-cols-1 gap-vsp-sm md:grid-cols-2">
              <label class="flex flex-col gap-vsp-3xs text-small text-fg">
                <span class="font-mono">roleClasses.keyword</span>
                <input
                  class={inputClass}
                  modelValue={keywordClass}
                  placeholder="default: hi-kw"
                  on:input={clearDisplayedResult}
                />
              </label>
              <label class="flex flex-col gap-vsp-3xs text-small text-fg">
                <span class="font-mono">roleClasses.string</span>
                <input
                  class={inputClass}
                  modelValue={stringClass}
                  placeholder="default: hi-str"
                  on:input={clearDisplayedResult}
                />
              </label>
            </div>
          </fieldset>
        </div>
      }
      output={
        <div class="flex flex-col gap-vsp-sm">
          <Show when={hasOutcome}>
            {() => (
              <p
                role="status"
                class={computed(() =>
                  outcome.value
                    ? `m-0 text-small font-semibold ${outcomeClassName(outcome.value)}`
                    : "m-0 text-small font-semibold",
                )}
              >
                {computed(() => (outcome.value ? outcomeLabel(outcome.value) : ""))}
              </p>
            )}
          </Show>
          <HighlightOutput result={displayedResult} />
        </div>
      }
      diagnostics={
        <DiagnosticsList
          diagnostics={diagnostics}
          label="Highlight diagnostics"
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
