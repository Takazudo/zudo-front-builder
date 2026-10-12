import {
  computed,
  Show,
  type Child,
  type ReadonlySignal,
  type Signal,
} from "@takazudo/zfb/zudo-react";

export interface PlaygroundShellLabels {
  input: string;
  output: string;
  run: string;
  pending: string;
  firstRunNotice: string;
  apiReference: string;
}

export interface PlaygroundShellProps {
  value: Signal<string>;
  onInput?: () => void;
  onRun: () => void;
  pending: ReadonlySignal<boolean>;
  hasRun: ReadonlySignal<boolean>;
  labels?: Partial<PlaygroundShellLabels>;
  apiReferenceHref?: string;
  options?: Child;
  output?: Child;
  diagnostics?: Child;
  trapError?: Child;
  textareaLabel?: string;
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
  pending,
  hasRun,
  labels: labelsProp,
  apiReferenceHref = "/docs/api/md-wasm/",
  options,
  output,
  diagnostics,
  trapError,
  textareaLabel,
}: PlaygroundShellProps) {
  const labels = { ...defaultLabels, ...labelsProp };
  const runLabel = computed(() => (pending.value ? labels.pending : labels.run));
  const showFirstRunNotice = computed(() => !hasRun.value);

  return (
    <section class="flex flex-col gap-vsp-sm text-fg">
      {options ? (
        <div class="rounded-lg border border-muted bg-surface p-hsp-lg">{options}</div>
      ) : null}

      <div class="grid grid-cols-1 gap-vsp-sm lg:grid-cols-2">
        <label class="flex min-w-0 flex-col gap-vsp-2xs text-small font-semibold">
          {labels.input}
          <textarea
            aria-label={textareaLabel}
            modelValue={value}
            rows={18}
            spellcheck={false}
            class="w-full resize-y rounded-lg border border-muted bg-code-bg p-hsp-lg font-mono text-small font-normal text-code-fg focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            on:input={() => onInput?.()}
          />
        </label>

        <section class="flex min-w-0 flex-col gap-vsp-2xs" aria-label={labels.output}>
          <h2 class="text-small font-semibold">{labels.output}</h2>
          <div class="flex-1 overflow-auto rounded-lg border border-muted bg-code-bg p-hsp-lg font-mono text-small text-code-fg">
            {output}
          </div>
        </section>
      </div>

      <div class="flex flex-wrap items-center gap-hsp-md">
        <button
          type="button"
          class="rounded bg-accent px-hsp-lg py-vsp-xs text-small font-semibold text-bg transition-colors hover:bg-accent-hover focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
          aria-busy={pending}
          on:click={() => onRun()}
        >
          {runLabel}
        </button>
        <div aria-live="assertive" class="min-w-0 text-small text-danger">
          {trapError}
        </div>
      </div>

      <Show when={showFirstRunNotice}>
        {() => (
          <p class="text-caption text-muted">
            {labels.firstRunNotice}{" "}
            <a class="text-accent hover:underline focus-visible:underline" href={apiReferenceHref}>
              {labels.apiReference}
            </a>
          </p>
        )}
      </Show>

      {diagnostics}
    </section>
  );
}
