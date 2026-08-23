/** @jsxRuntime automatic */
/** @jsxImportSource preact */

export interface PlaygroundDiagnostic {
  severity: "error" | "warning";
  source: string;
  message: string;
  line: number | null;
  column: number | null;
}

export interface DiagnosticsListProps {
  diagnostics: readonly PlaygroundDiagnostic[];
  label?: string;
  emptyMessage?: string;
}

function formatLocation({ line, column }: PlaygroundDiagnostic): string | null {
  if (line !== null && column !== null) return `line ${line}, column ${column}`;
  if (line !== null) return `line ${line}`;
  if (column !== null) return `column ${column}`;
  return null;
}

export default function DiagnosticsList({
  diagnostics,
  label = "Diagnostics",
  emptyMessage,
}: DiagnosticsListProps) {
  if (diagnostics.length === 0 && !emptyMessage) return null;

  return (
    <section
      aria-label={label}
      aria-live="polite"
      className="rounded-lg border border-muted bg-surface p-hsp-lg"
    >
      {diagnostics.length === 0 ? (
        <p className="text-small text-muted">{emptyMessage}</p>
      ) : (
        <ul className="m-0 flex list-none flex-col gap-vsp-xs p-0">
          {diagnostics.map((diagnostic, index) => {
            const location = formatLocation(diagnostic);
            const severityClassName =
              diagnostic.severity === "warning" ? "text-warning" : "text-danger";

            return (
              <li key={index} className="border-l-2 border-muted pl-hsp-md text-small">
                <p className="m-0 flex flex-wrap gap-hsp-sm text-caption">
                  <strong className={severityClassName}>{diagnostic.severity}</strong>
                  <span className="font-mono text-muted">{diagnostic.source}</span>
                  {location ? <span className="text-muted">{location}</span> : null}
                </p>
                <p className="m-0 mt-vsp-3xs whitespace-pre-wrap text-fg">{diagnostic.message}</p>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
