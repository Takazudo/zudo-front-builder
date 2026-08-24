/** @jsxRuntime automatic */
/** @jsxImportSource preact */
"use client";

import { useMemo, useState } from "preact/hooks";

import type {
  FrontmatterPolicy,
  MdastRoot,
  ParseDialect,
  ParseToAstOptions,
  ParseToAstResult,
} from "@takazudo/zfb-md-wasm/parse";

import DiagnosticsList, { type PlaygroundDiagnostic } from "./diagnostics-list";
import OptionRow from "./option-row";
import PlaygroundShell from "./playground-shell";
import { createModuleLoader, useWasmModule } from "./use-wasm-module";

type ParseModule = typeof import("@takazudo/zfb-md-wasm/parse");

type GfmField =
  | "strikethrough"
  | "table"
  | "autolinkLiteral"
  | "taskListItem"
  | "footnoteDefinition";

interface ParseRunResult {
  ast: MdastRoot | null;
  displayAst: unknown;
  frontmatter: unknown;
  diagnostics: PlaygroundDiagnostic[];
}

const SAMPLE_SOURCE = `---
title: Raw AST sample
---
import Callout from "./Callout";

# Raw mdast

Hello **world** and [a link](https://example.com).

<Callout variant="note">JSX attributes have no positions.</Callout>

- one
- two
`;

const GFM_FIELDS: readonly { key: GfmField; label: string }[] = [
  { key: "strikethrough", label: "pipeline.gfm.strikethrough" },
  { key: "table", label: "pipeline.gfm.table" },
  { key: "autolinkLiteral", label: "pipeline.gfm.autolinkLiteral" },
  { key: "taskListItem", label: "pipeline.gfm.taskListItem" },
  { key: "footnoteDefinition", label: "pipeline.gfm.footnoteDefinition" },
];

const DEFAULT_GFM: Record<GfmField, boolean> = {
  strikethrough: true,
  table: true,
  autolinkLiteral: true,
  taskListItem: true,
  footnoteDefinition: true,
};

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

function ParseOutput({ state }: { state: ReturnType<typeof useParseModule>["state"] }) {
  if (state.status !== "ready") {
    return (
      <pre className="m-0 whitespace-pre-wrap">
        {state.status === "idle" ? "Run to inspect the raw mdast tree." : ""}
      </pre>
    );
  }

  const summary = summarizeAst(state.result.displayAst);
  return (
    <div className="flex min-w-0 flex-col gap-vsp-sm">
      <p className="m-0 text-caption">
        node count: {summary.nodeCount} · max depth: {summary.maxDepth}
      </p>
      <pre className="m-0 whitespace-pre">{formatJson(state.result.displayAst)}</pre>
    </div>
  );
}

function useParseModule() {
  const loadParseModule = useMemo(
    () => createModuleLoader<ParseModule>(() => import("@takazudo/zfb-md-wasm/parse")),
    [],
  );

  return useWasmModule<
    ParseModule,
    [source: string, options: ParseToAstOptions, adaptToMdast: boolean],
    ParseRunResult
  >({
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

function ParseOptions({
  dialect,
  filename,
  directives,
  frontmatter,
  gfm,
  adaptToMdast,
  onDialectChange,
  onFilenameChange,
  onDirectivesChange,
  onFrontmatterChange,
  onGfmChange,
  onAdapterChange,
}: {
  dialect: ParseDialect;
  filename: string;
  directives: boolean;
  frontmatter: FrontmatterPolicy;
  gfm: Record<GfmField, boolean>;
  adaptToMdast: boolean;
  onDialectChange: (value: ParseDialect) => void;
  onFilenameChange: (value: string) => void;
  onDirectivesChange: (value: boolean) => void;
  onFrontmatterChange: (value: FrontmatterPolicy) => void;
  onGfmChange: (field: GfmField, value: boolean) => void;
  onAdapterChange: (value: boolean) => void;
}) {
  return (
    <fieldset className="m-0 border-0 p-0">
      <legend className="mb-vsp-xs text-small font-semibold">ParseToAstOptions</legend>
      <div className="grid grid-cols-1 gap-hsp-md md:grid-cols-2">
        <label className="flex flex-col gap-vsp-2xs text-small">
          <span className="font-mono">dialect</span>
          <select
            className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            value={dialect}
            onChange={(event) => onDialectChange(event.currentTarget.value as ParseDialect)}
          >
            <option value="markdown">markdown</option>
            <option value="mdx">mdx</option>
          </select>
        </label>

        <label className="flex flex-col gap-vsp-2xs text-small">
          <span className="font-mono">filename</span>
          <input
            className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            type="text"
            value={filename}
            onInput={(event) => onFilenameChange(event.currentTarget.value)}
          />
        </label>

        <label className="flex items-center gap-hsp-sm text-small">
          <input
            type="checkbox"
            className="accent-accent focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            checked={directives}
            onChange={(event) => onDirectivesChange(event.currentTarget.checked)}
          />
          <span className="font-mono">directives</span>
        </label>

        <label className="flex flex-col gap-vsp-2xs text-small">
          <span className="font-mono">frontmatter</span>
          <select
            className="rounded border border-muted bg-surface px-hsp-md py-vsp-xs text-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            value={frontmatter}
            onChange={(event) =>
              onFrontmatterChange(event.currentTarget.value as FrontmatterPolicy)
            }
          >
            <option value="extract">extract</option>
            <option value="node">node</option>
            <option value="none">none</option>
          </select>
        </label>
      </div>

      <div className="mt-vsp-sm">
        {GFM_FIELDS.map(({ key, label }) => (
          <OptionRow
            key={key}
            label={label}
            checked={gfm[key]}
            onChange={(value) => onGfmChange(key, value)}
          />
        ))}
        <OptionRow label="toMdastRoot" checked={adaptToMdast} onChange={onAdapterChange} />
      </div>
    </fieldset>
  );
}

function ParsePlayground() {
  const [source, setSource] = useState(SAMPLE_SOURCE);
  const [dialect, setDialect] = useState<ParseDialect>("mdx");
  const [filename, setFilename] = useState("preview.mdx");
  const [directives, setDirectives] = useState(false);
  const [frontmatter, setFrontmatter] = useState<FrontmatterPolicy>("extract");
  const [gfm, setGfm] = useState(DEFAULT_GFM);
  const [adaptToMdast, setAdaptToMdast] = useState(false);
  const [hasRun, setHasRun] = useState(false);
  const { state, run } = useParseModule();

  function handleRun(): void {
    const options: ParseToAstOptions = {
      dialect,
      filename,
      directives,
      frontmatter,
      pipeline: { gfm },
    };
    setHasRun(true);
    void run(source, options, adaptToMdast).catch(() => undefined);
  }

  const diagnostics = state.status === "ready" ? state.result.diagnostics : [];
  const trapError = state.status === "error" ? formatTrap(state.error) : null;

  return (
    <PlaygroundShell
      value={source}
      onInput={setSource}
      onRun={handleRun}
      pending={state.status === "loading"}
      hasRun={hasRun}
      options={
        <ParseOptions
          dialect={dialect}
          filename={filename}
          directives={directives}
          frontmatter={frontmatter}
          gfm={gfm}
          adaptToMdast={adaptToMdast}
          onDialectChange={setDialect}
          onFilenameChange={setFilename}
          onDirectivesChange={setDirectives}
          onFrontmatterChange={setFrontmatter}
          onGfmChange={(field, value) => setGfm((current) => ({ ...current, [field]: value }))}
          onAdapterChange={setAdaptToMdast}
        />
      }
      output={<ParseOutput state={state} />}
      diagnostics={<DiagnosticsList diagnostics={diagnostics} label="Parse diagnostics" />}
      trapError={trapError}
      textareaProps={{ "aria-label": "Markdown or MDX source" }}
    />
  );
}

(ParsePlayground as { displayName?: string }).displayName = "ParsePlayground";

export default ParsePlayground;
