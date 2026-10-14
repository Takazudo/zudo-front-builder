import { computed, For, Show, type ReadonlySignal } from "@takazudo/zfb/zudo-react";

export interface PlaygroundDiagnostic {
  severity: "error" | "warning";
  source: string;
  message: string;
  line: number | null;
  column: number | null;
}

export interface DiagnosticsListProps {
  diagnostics: ReadonlySignal<readonly PlaygroundDiagnostic[]>;
  label?: string;
  emptyMessage?: ReadonlySignal<string | undefined>;
}

interface IndexedDiagnostic {
  index: number;
  diagnostic: PlaygroundDiagnostic;
}

function formatLocation({ line, column }: PlaygroundDiagnostic): string | null {
  if (line !== null && column !== null) return `line ${line}, column ${column}`;
  if (line !== null) return `line ${line}`;
  if (column !== null) return `column ${column}`;
  return null;
}

function DiagnosticItem({ item }: { item: ReadonlySignal<IndexedDiagnostic> }) {
  const diagnostic = computed(() => item.value.diagnostic);
  const location = computed(() => formatLocation(diagnostic.value) ?? "");
  const hasLocation = computed(() => location.value !== "");
  const severityClassName = computed(() =>
    diagnostic.value.severity === "warning" ? "text-warning" : "text-danger",
  );

  return (
    <li class="border-l-2 border-muted pl-hsp-md text-small">
      <p class="m-0 flex flex-wrap gap-hsp-sm text-caption">
        <strong class={severityClassName}>{computed(() => diagnostic.value.severity)}</strong>
        <span class="font-mono text-muted">{computed(() => diagnostic.value.source)}</span>
        <Show when={hasLocation}>{() => <span class="text-muted">{location}</span>}</Show>
      </p>
      <p class="m-0 mt-vsp-3xs whitespace-pre-wrap text-fg">
        {computed(() => diagnostic.value.message)}
      </p>
    </li>
  );
}

export default function DiagnosticsList({
  diagnostics,
  label = "Diagnostics",
  emptyMessage,
}: DiagnosticsListProps) {
  const visible = computed(() => diagnostics.value.length > 0 || !!emptyMessage?.value);
  const empty = computed(() => diagnostics.value.length === 0);
  const items = computed(() =>
    diagnostics.value.map((diagnostic, index): IndexedDiagnostic => ({ index, diagnostic })),
  );

  return (
    <Show when={visible}>
      {() => (
        <section
          aria-label={label}
          aria-live="polite"
          class="rounded-lg border border-muted bg-surface p-hsp-lg"
        >
          <Show
            when={empty}
            fallback={() => (
              <ul class="m-0 flex list-none flex-col gap-vsp-xs p-0">
                <For each={items} by={(item) => item.index}>
                  {(item) => <DiagnosticItem item={item} />}
                </For>
              </ul>
            )}
          >
            {() => <p class="text-small text-muted">{computed(() => emptyMessage?.value ?? "")}</p>}
          </Show>
        </section>
      )}
    </Show>
  );
}
