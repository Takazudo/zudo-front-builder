/** @jsxRuntime automatic */
/** @jsxImportSource preact */
"use client";

import { useEffect, useRef, useState } from "preact/hooks";

import DiagnosticsList from "./diagnostics-list";
import PlaygroundShell from "./playground-shell";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import { createModuleLoader, useWasmModule } from "./use-wasm-module";

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

function HighlightOutput({ result }: { result: HighlightCodeResult | null }) {
  if (result === null) {
    return <p className="m-0 text-small text-muted">Run the highlighter to see semantic HTML.</p>;
  }

  if (result.html === null) {
    const optionsFailure = result.diagnostics.some((diagnostic) => diagnostic.source === "options");
    return (
      <p className="m-0 text-small text-danger">
        {optionsFailure
          ? "No HTML output: the supplied options were rejected."
          : "No HTML output was returned."}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-vsp-sm">
      <section aria-label="Rendered semantic HTML">
        <h3 className="mb-vsp-2xs text-small font-semibold">Rendered semantic HTML</h3>
        <div className="zd-html-preview-code" dangerouslySetInnerHTML={{ __html: result.html }} />
      </section>
      <section aria-label="Raw HTML source">
        <h3 className="mb-vsp-2xs text-small font-semibold">Raw HTML source</h3>
        <pre className="m-0 overflow-auto whitespace-pre-wrap break-words">{result.html}</pre>
      </section>
    </div>
  );
}

function HighlightPlayground() {
  const [source, setSource] = useState(SAMPLES[2].value);
  const [activeSampleId, setActiveSampleId] = useState<string | undefined>(SAMPLES[2].id);
  const [language, setLanguage] = useState(SAMPLES[2].language);
  const [classPrefix, setClassPrefix] = useState("hi-");
  const [keywordClass, setKeywordClass] = useState("");
  const [stringClass, setStringClass] = useState("");
  const [displayedResult, setDisplayedResult] = useState<HighlightCodeResult | null>(null);

  const highlightLoaderRef = useRef<(() => Promise<HighlightModule>) | null>(null);
  const runSequenceRef = useRef(0);
  const loadHighlightModule = () => {
    if (highlightLoaderRef.current === null) {
      highlightLoaderRef.current = createModuleLoader(async () => {
        const module = await import("@takazudo/zfb-md-wasm/highlight");
        return { highlightCode: module.highlightCode };
      });
    }

    return highlightLoaderRef.current();
  };

  const { state, run } = useWasmModule<
    HighlightModule,
    [string, HighlightCodeOptions],
    HighlightCodeResult
  >({
    loadModule: loadHighlightModule,
    execute: (module, input, options) => module.highlightCode(input, options),
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

  const outcome = getOutcome(displayedResult);
  const diagnostics = displayedResult?.diagnostics ?? [];
  const trapError = state.status === "error" ? errorMessage(state.error) : null;

  function clearDisplayedResult() {
    runSequenceRef.current += 1;
    setDisplayedResult(null);
  }

  function pickSample(sample: PlaygroundSample) {
    const selected = SAMPLES.find((candidate) => candidate.id === sample.id);
    if (selected === undefined) return;

    clearDisplayedResult();
    setSource(selected.value);
    setActiveSampleId(selected.id);
    setLanguage(selected.language);
    setClassPrefix(selected.classPrefix ?? "hi-");
    setKeywordClass(selected.roleClasses?.keyword ?? "");
    setStringClass(selected.roleClasses?.string ?? "");
  }

  function runHighlight() {
    const runSequence = ++runSequenceRef.current;
    setDisplayedResult(null);

    const roleClasses = buildRoleClasses(keywordClass, stringClass);
    const options: HighlightCodeOptions = {
      language,
      classPrefix,
      ...(roleClasses ? { roleClasses } : {}),
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
        clearDisplayedResult();
        setSource(value);
        setActiveSampleId(undefined);
      }}
      onRun={runHighlight}
      pending={state.status === "loading"}
      hasRun={state.status !== "idle"}
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
        <div className="flex flex-col gap-vsp-sm">
          <SamplePicker
            samples={SAMPLES}
            activeSampleId={activeSampleId}
            onPick={pickSample}
            label="Samples"
          />

          <div className="grid grid-cols-1 gap-vsp-sm md:grid-cols-2">
            <label className="flex flex-col gap-vsp-3xs text-small text-fg">
              <span className="font-mono">language</span>
              <input
                className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                value={language}
                list="highlight-languages"
                onInput={(event) => {
                  clearDisplayedResult();
                  setLanguage(event.currentTarget.value);
                  setActiveSampleId(undefined);
                }}
              />
              <datalist id="highlight-languages">
                {LANGUAGE_OPTIONS.map((option) => (
                  <option key={option} value={option} />
                ))}
              </datalist>
            </label>

            <label className="flex flex-col gap-vsp-3xs text-small text-fg">
              <span className="font-mono">classPrefix</span>
              <input
                className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                value={classPrefix}
                onInput={(event) => {
                  clearDisplayedResult();
                  setClassPrefix(event.currentTarget.value);
                }}
              />
            </label>
          </div>

          <fieldset className="m-0 flex flex-col gap-vsp-2xs border-0 p-0">
            <legend className="font-mono text-small font-semibold text-fg">roleClasses</legend>
            <p className="m-0 text-caption text-muted">
              Optional full-role class overrides; clear a field to restore its default semantic
              class.
            </p>
            <div className="grid grid-cols-1 gap-vsp-sm md:grid-cols-2">
              <label className="flex flex-col gap-vsp-3xs text-small text-fg">
                <span className="font-mono">roleClasses.keyword</span>
                <input
                  className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                  value={keywordClass}
                  placeholder="default: hi-kw"
                  onInput={(event) => {
                    clearDisplayedResult();
                    setKeywordClass(event.currentTarget.value);
                  }}
                />
              </label>
              <label className="flex flex-col gap-vsp-3xs text-small text-fg">
                <span className="font-mono">roleClasses.string</span>
                <input
                  className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
                  value={stringClass}
                  placeholder="default: hi-str"
                  onInput={(event) => {
                    clearDisplayedResult();
                    setStringClass(event.currentTarget.value);
                  }}
                />
              </label>
            </div>
          </fieldset>
        </div>
      }
      output={
        <div className="flex flex-col gap-vsp-sm">
          {outcome ? (
            <p
              role="status"
              className={`m-0 text-small font-semibold ${outcomeClassName(outcome)}`}
            >
              {outcomeLabel(outcome)}
            </p>
          ) : null}
          <HighlightOutput result={displayedResult} />
        </div>
      }
      diagnostics={
        <DiagnosticsList
          diagnostics={diagnostics}
          label="Highlight diagnostics"
          emptyMessage={displayedResult ? "No diagnostics." : undefined}
        />
      }
      trapError={trapError ? <span role="alert">{trapError}</span> : null}
    />
  );
}

(HighlightPlayground as { displayName?: string }).displayName = "HighlightPlayground";

export default HighlightPlayground;
