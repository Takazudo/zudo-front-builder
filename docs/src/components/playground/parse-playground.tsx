"use client";

import {
  computed,
  getScope,
  Show,
  signal,
  type ReadonlySignal,
  type Scope,
  type Signal,
} from "@takazudo/zfb/zudo-react";

import type {
  FrontmatterPolicy,
  MdastRoot,
  ParseDialect,
  ParseToAstOptions,
  ParseToAstResult,
} from "@takazudo/zfb-md-wasm/parse";

import { isNoteDirectiveSample, NOTE_DIRECTIVE_SAMPLE } from "./admonition-sample";
import DiagnosticsList, { type PlaygroundDiagnostic } from "./diagnostics-list";
import { createGfmSignals, snapshotGfm, type GfmKey, type GfmSignals } from "./gfm-signals";
import OptionRow from "./option-row";
import PlaygroundShell from "./playground-shell";
import SamplePicker, { type PlaygroundSample } from "./sample-picker";
import { createModuleLoader, wasmModule, type WasmModuleState } from "./use-wasm-module";

type ParseModule = typeof import("@takazudo/zfb-md-wasm/parse");

interface ParseRunResult {
  ast: MdastRoot | null;
  displayAst: unknown;
  frontmatter: unknown;
  diagnostics: PlaygroundDiagnostic[];
}

const PARSE_SAMPLE: PlaygroundSample = {
  id: "raw-ast",
  label: "Raw MDX",
  value: `---
title: Raw AST sample
---
import Callout from "./Callout";

# Raw mdast

Hello **world** and [a link](https://example.com).

<Callout variant="note">JSX attributes have no positions.</Callout>

- one
- two
`,
};

const PARSE_SAMPLES = [PARSE_SAMPLE, NOTE_DIRECTIVE_SAMPLE] as const;

const GFM_FIELDS: readonly { key: GfmKey; label: string }[] = [
  { key: "strikethrough", label: "pipeline.gfm.strikethrough" },
  { key: "table", label: "pipeline.gfm.table" },
  { key: "autolinkLiteral", label: "pipeline.gfm.autolinkLiteral" },
  { key: "taskListItem", label: "pipeline.gfm.taskListItem" },
  { key: "footnoteDefinition", label: "pipeline.gfm.footnoteDefinition" },
];

function isAstNode(value: unknown): value is { children?: unknown[]; type: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    "type" in value &&
    typeof value.type === "string"
  );
}

function summarizeAst(value: unknown, depth = 1): { nodeCount: number; maxDepth: number } {
  if (!isAstNode(value)) return { nodeCount: 0, maxDepth: 0 };

  let nodeCount = 1;
  let maxDepth = depth;
  if (Array.isArray(value.children)) {
    for (const child of value.children) {
      const summary = summarizeAst(child, depth + 1);
      nodeCount += summary.nodeCount;
      maxDepth = Math.max(maxDepth, summary.maxDepth);
    }
  }
  return { nodeCount, maxDepth };
}

function formatJson(value: unknown): string {
  return JSON.stringify(value, null, 2) ?? "null";
}

function formatTrap(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const loadParseModule = createModuleLoader<ParseModule>(
  () => import("@takazudo/zfb-md-wasm/parse"),
);

function ParseOutput({ state }: { state: ReadonlySignal<WasmModuleState<ParseRunResult>> }) {
  const ready = computed(() => state.value.status === "ready");
  const summary = computed(() => {
    const current = state.value;
    return current.status === "ready"
      ? summarizeAst(current.result.displayAst)
      : { nodeCount: 0, maxDepth: 0 };
  });

  return (
    <Show
      when={ready}
      fallback={() => (
        <pre class="m-0 whitespace-pre-wrap">
          {computed(() =>
            state.value.status === "idle" ? "Run to inspect the raw mdast tree." : "",
          )}
        </pre>
      )}
    >
      {() => (
        <div class="flex min-w-0 flex-col gap-vsp-sm">
          <p class="m-0 text-caption">
            node count: {computed(() => summary.value.nodeCount)} · max depth:{" "}
            {computed(() => summary.value.maxDepth)}
          </p>
          <pre class="m-0 whitespace-pre">
            {computed(() => {
              const current = state.value;
              return current.status === "ready" ? formatJson(current.result.displayAst) : "";
            })}
          </pre>
        </div>
      )}
    </Show>
  );
}

function parseModule(scope: Scope) {
  return wasmModule<
    ParseModule,
    [source: string, options: ParseToAstOptions, adaptToMdast: boolean],
    ParseRunResult
  >(scope, {
    loadModule: loadParseModule,
    execute: async (module, source, options, adaptToMdast) => {
      const parsed: ParseToAstResult = await module.parseToAst(source, options);
      const diagnostics: PlaygroundDiagnostic[] = [...parsed.diagnostics];
      let displayAst: unknown = parsed.ast;

      // toMdastRoot throws for malformed/unsupported nodes. Parse diagnostics
      // and a null AST must be handled first so ordinary input failures stay
      // in the rendered diagnostics channel.
      if (adaptToMdast && parsed.diagnostics.length === 0 && parsed.ast !== null) {
        try {
          displayAst = module.toMdastRoot(parsed.ast);
        } catch (error: unknown) {
          if (!(error instanceof module.MdastAdapterError)) throw error;
          diagnostics.push({
            severity: "error",
            source: "toMdastRoot",
            message: `toMdastRoot failed at path ${error.path} for nodeType ${error.nodeType ?? "null"}: ${error.message}`,
            line: null,
            column: null,
          });
        }
      }

      return {
        ast: parsed.ast,
        displayAst,
        frontmatter: parsed.frontmatter,
        diagnostics,
      };
    },
  });
}

const selectClass =
  "rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2";

function ParseOptions({
  dialect,
  filename,
  directives,
  frontmatter,
  gfm,
  adaptToMdast,
}: {
  dialect: Signal<string>;
  filename: Signal<string>;
  directives: Signal<boolean>;
  frontmatter: Signal<string>;
  gfm: GfmSignals;
  adaptToMdast: Signal<boolean>;
}) {
  return (
    <fieldset class="m-0 border-0 p-0">
      <legend class="mb-vsp-xs text-small font-semibold">ParseToAstOptions</legend>
      <div class="grid grid-cols-1 gap-hsp-md md:grid-cols-2">
        <label class="flex flex-col gap-vsp-2xs text-small">
          <span class="font-mono">dialect</span>
          <select class={selectClass} modelValue={dialect}>
            <option value="markdown">markdown</option>
            <option value="mdx">mdx</option>
          </select>
        </label>

        <label class="flex flex-col gap-vsp-2xs text-small">
          <span class="font-mono">filename</span>
          <input class={selectClass} type="text" modelValue={filename} />
        </label>

        <label class="flex items-center gap-hsp-sm text-small">
          <input
            type="checkbox"
            class="accent-accent focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            modelChecked={directives}
          />
          <span class="font-mono">directives</span>
        </label>

        <label class="flex flex-col gap-vsp-2xs text-small">
          <span class="font-mono">frontmatter</span>
          <select class={selectClass} modelValue={frontmatter}>
            <option value="extract">extract</option>
            <option value="node">node</option>
            <option value="none">none</option>
          </select>
        </label>
      </div>

      <div class="mt-vsp-sm">
        {GFM_FIELDS.map(({ key, label }) => (
          <OptionRow label={label} checked={gfm[key]} />
        ))}
        <OptionRow label="toMdastRoot" checked={adaptToMdast} />
      </div>
    </fieldset>
  );
}

export default function ParsePlayground() {
  const scope = getScope();
  const source = signal(PARSE_SAMPLE.value);
  const activeSampleId = signal<string | undefined>(PARSE_SAMPLE.id);
  const dialect = signal<string>("mdx");
  const filename = signal("preview.mdx");
  const directives = signal(false);
  const frontmatter = signal<string>("extract");
  const gfm = createGfmSignals();
  const adaptToMdast = signal(false);
  const hasRun = signal(false);
  const { state, run } = parseModule(scope);

  function handleRun(): void {
    const options: ParseToAstOptions = {
      dialect: dialect.value as ParseDialect,
      filename: filename.value,
      directives: directives.value,
      frontmatter: frontmatter.value as FrontmatterPolicy,
      pipeline: {
        gfm: snapshotGfm(gfm),
      },
    };
    hasRun.value = true;
    void run(source.value, options, adaptToMdast.value).catch(() => undefined);
  }

  function pickSample(sample: PlaygroundSample): void {
    source.value = sample.value;
    activeSampleId.value = sample.id;
    directives.value = isNoteDirectiveSample(sample);
  }

  const diagnostics = computed(() => {
    const current = state.value;
    return current.status === "ready" ? current.result.diagnostics : [];
  });
  const trapError = computed(() => {
    const current = state.value;
    return current.status === "error" ? formatTrap(current.error) : "";
  });

  return (
    <PlaygroundShell
      value={source}
      onInput={() => {
        activeSampleId.value = undefined;
      }}
      onRun={handleRun}
      pending={computed(() => state.value.status === "loading")}
      hasRun={hasRun}
      options={
        <div class="flex flex-col gap-vsp-sm">
          <SamplePicker
            samples={PARSE_SAMPLES}
            activeSampleId={activeSampleId}
            onPick={pickSample}
          />
          <ParseOptions
            dialect={dialect}
            filename={filename}
            directives={directives}
            frontmatter={frontmatter}
            gfm={gfm}
            adaptToMdast={adaptToMdast}
          />
        </div>
      }
      output={<ParseOutput state={state} />}
      diagnostics={<DiagnosticsList diagnostics={diagnostics} label="Parse diagnostics" />}
      trapError={trapError}
      textareaLabel="Markdown or MDX source"
    />
  );
}
