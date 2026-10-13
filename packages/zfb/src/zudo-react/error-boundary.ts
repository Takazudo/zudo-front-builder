import { createDescription, type Child, type Description } from "./description.js";
import { boundaryType } from "./error-boundary-type.js";
import { renderErrorBoundary } from "./error-boundary-client.js";
import type { ReadonlySignal } from "./reactive-types.js";

export interface ErrorBoundaryProps {
  children: () => Child;
  fallback: (error: unknown, reset: () => void) => Child;
  resetKey?: ReadonlySignal<unknown> | undefined;
}

// The renderer hook belongs to the opt-in value. A barrel re-export alone must
// not retain boundary attempts, resets, or local hydration replacement code.
export const ErrorBoundary: (props: ErrorBoundaryProps) => Description = /* @__PURE__ */ (() => {
  function ErrorBoundary(props: ErrorBoundaryProps): Description {
    return createDescription(ErrorBoundary, props as unknown as Record<string, unknown>);
  }
  Object.defineProperty(ErrorBoundary, boundaryType, { value: renderErrorBoundary });
  return ErrorBoundary;
})();
