import type { Diagnostic, HighlightDiagnostic } from "@takazudo/zfb-md-wasm";

export type { Diagnostic, HighlightDiagnostic } from "@takazudo/zfb-md-wasm";

/** Diagnostics emitted by any of the playground's WASM entry points. */
export type PlaygroundDiagnostic = Diagnostic | HighlightDiagnostic;

type NonEmptyReadonlyArray<T> = readonly [T, ...T[]];

/** A payload produced without diagnostics. */
export interface PlaygroundSuccess<T> {
  kind: "success";
  payload: T;
  diagnostics: readonly [];
}

/** A usable payload accompanied by non-fatal diagnostics. */
export interface PlaygroundSuccessWithDiagnostics<T, TDiagnostic> {
  kind: "success-with-diagnostics";
  payload: T;
  diagnostics: NonEmptyReadonlyArray<TDiagnostic>;
}

/** An expected input failure described by diagnostics, not an exception. */
export interface PlaygroundFailureWithDiagnostics<TDiagnostic> {
  kind: "failure-with-diagnostics";
  payload: null;
  diagnostics: NonEmptyReadonlyArray<TDiagnostic>;
}

/**
 * The three resolved outcomes shared by playground pages. Rejected promises
 * (WASM traps and loader failures) deliberately live outside this union.
 */
export type PlaygroundResult<T, TDiagnostic = PlaygroundDiagnostic> =
  | PlaygroundSuccess<T>
  | PlaygroundSuccessWithDiagnostics<T, TDiagnostic>
  | PlaygroundFailureWithDiagnostics<TDiagnostic>;

/**
 * Normalize a package result without inspecting or rewriting diagnostic
 * messages. Package contracts guarantee that a null payload has diagnostics.
 */
export function normalizePlaygroundResult<T, TDiagnostic>(
  payload: T | null,
  diagnostics: readonly TDiagnostic[],
): PlaygroundResult<T, TDiagnostic> {
  if (payload === null) {
    return {
      kind: "failure-with-diagnostics",
      payload: null,
      diagnostics: diagnostics as NonEmptyReadonlyArray<TDiagnostic>,
    };
  }

  if (diagnostics.length > 0) {
    return {
      kind: "success-with-diagnostics",
      payload,
      diagnostics: diagnostics as NonEmptyReadonlyArray<TDiagnostic>,
    };
  }

  return { kind: "success", payload, diagnostics: [] };
}
