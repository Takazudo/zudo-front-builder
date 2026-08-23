/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import type { ComponentChildren, JSX } from "preact";

export interface PlaygroundShellLabels {
  input: string;
  output: string;
  run: string;
  pending: string;
  firstRunNotice: string;
  apiReference: string;
}

export interface PlaygroundShellProps {
  value: string;
  onInput: (value: string) => void;
  onRun: () => void;
  pending?: boolean;
  hasRun?: boolean;
  labels?: Partial<PlaygroundShellLabels>;
  apiReferenceHref?: string;
  options?: ComponentChildren;
  output?: ComponentChildren;
  diagnostics?: ComponentChildren;
  trapError?: ComponentChildren;
  textareaProps?: Omit<
    JSX.TextareaHTMLAttributes<HTMLTextAreaElement>,
    "children" | "className" | "onInput" | "spellcheck" | "value"
  >;
  textareaClassName?: string;
}

const defaultLabels: PlaygroundShellLabels = {
  input: "Input",
  output: "Output",
  run: "Run",
  pending: "Running…",
  firstRunNotice: "The first Run downloads a WebAssembly module.",
  apiReference: "API reference",
};

export default function PlaygroundShell({
  value,
  onInput,
  onRun,
  pending = false,
  hasRun = false,
  labels: labelsProp,
  apiReferenceHref = "/docs/api/md-wasm/",
  options,
  output,
  diagnostics,
  trapError,
  textareaProps,
  textareaClassName = "",
}: PlaygroundShellProps) {
  const labels = { ...defaultLabels, ...labelsProp };

  return (
    <section className="flex flex-col gap-vsp-sm text-fg">
      {options ? (
        <div className="rounded-lg border border-muted bg-surface p-hsp-lg">{options}</div>
      ) : null}

      <div className="grid grid-cols-1 gap-vsp-sm lg:grid-cols-2">
        <label className="flex min-w-0 flex-col gap-vsp-2xs text-small font-semibold">
          {labels.input}
          <textarea
            {...textareaProps}
            value={value}
            rows={18}
            spellcheck={false}
            className={`w-full resize-y rounded-lg border border-muted bg-code-bg p-hsp-lg font-mono text-small font-normal text-code-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 ${textareaClassName}`}
            onInput={(event) => onInput(event.currentTarget.value)}
          />
        </label>

        <section className="flex min-w-0 flex-col gap-vsp-2xs" aria-label={labels.output}>
          <h2 className="text-small font-semibold">{labels.output}</h2>
          <div className="flex-1 overflow-auto rounded-lg border border-muted bg-code-bg p-hsp-lg font-mono text-small text-code-fg">
            {output}
          </div>
        </section>
      </div>

      <div className="flex flex-wrap items-center gap-hsp-md">
        <button
          type="button"
          className="rounded bg-accent px-hsp-lg py-vsp-xs text-small font-semibold text-bg transition-colors hover:bg-accent-hover focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:pointer-events-none disabled:opacity-50"
          disabled={pending}
          aria-busy={pending}
          onClick={onRun}
        >
          {pending ? labels.pending : labels.run}
        </button>
        <div aria-live="assertive" className="min-w-0 text-small text-danger">
          {trapError}
        </div>
      </div>

      {!hasRun ? (
        <p className="text-caption text-muted">
          {labels.firstRunNotice}{" "}
          <a
            className="text-accent hover:underline focus-visible:underline"
            href={apiReferenceHref}
          >
            {labels.apiReference}
          </a>
        </p>
      ) : null}

      {diagnostics}
    </section>
  );
}
