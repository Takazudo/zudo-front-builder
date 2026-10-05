/** Locale-neutral, literal source shared by compiler input and generated MDX. */
export interface WindExampleSource {
  id: string;
  html: string;
  utilities: string[];
  authoredClasses: string[];
  scaffoldCss: string;
  /** Recursive wind-only overrides. A null value removes the inherited key. */
  config?: { wind: Record<string, unknown> };
}
export interface WindExpectedDiagnostic {
  code: `ZW${string}`;
  severity: "error" | "warning";
}
export interface WindPositiveExample extends WindExampleSource {
  kind: "positive";
  /** Keep fragment links within the isolated preview document. */
  head?: '<base href="about:srcdoc">';
  candidateOrigin?: never;
  expectedDiagnostics?: never;
  diagnosticStylesheet?: never;
  sourceExclusion?: never;
}
export interface WindDiagnosticExample extends WindExampleSource {
  kind: "expected-diagnostic";
  head?: never;
  candidateOrigin?: "source" | "safelist";
  expectedDiagnostics: WindExpectedDiagnostic[];
  /** Separate rejected CSS entry, checked for ZW009; never rendered in an iframe. */
  diagnosticStylesheet?: string;
  /** Narrow explicit source-exclusion lessons checked for ZW010. */
  sourceExclusion?: "all" | "partial";
}
export type WindExample = WindPositiveExample | WindDiagnosticExample;
export interface WindExampleFamily {
  schemaVersion: 1;
  family: string;
  examples: WindExample[];
}
