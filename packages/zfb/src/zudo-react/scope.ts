import { subscribe, untracked, type Subscription } from "./reactive.js";
import { enqueue, type Job } from "./scheduler.js";
import type { Cleanup, Diagnostic, Reporter, Scope } from "./index.js";

let currentScope: RuntimeScope | null = null;
let nextScopeId = 0;

function isPromise(value: unknown): value is PromiseLike<unknown> {
  return (
    value !== null &&
    (typeof value === "object" || typeof value === "function") &&
    typeof (value as { then?: unknown }).then === "function"
  );
}

export interface ScopeOptions {
  component: string;
  reporter?: Reporter;
  parent?: RuntimeScope;
  path?: string;
  protocol?: string;
  build?: string;
}

export class RuntimeScope implements Scope {
  readonly id = ++nextScopeId;
  readonly component: string;
  readonly parent: RuntimeScope | undefined;
  private readonly reporter: Reporter | undefined;
  private readonly path: string;
  private readonly protocol: string;
  private readonly build: string;
  private readonly children: RuntimeScope[] = [];
  private readonly cleanups: Cleanup[] = [];
  private readonly activations: Array<() => void | Cleanup> = [];
  private readonly effects: Array<{ start: () => void; dispose: () => void }> = [];
  private controller: AbortController | undefined;
  private live = true;
  private activated = false;
  private readonly jobReporter = (code: string, actual: string): void => {
    this.report(code, "update", actual);
  };
  private root(): RuntimeScope {
    return this.parent?.root() ?? this;
  }

  constructor(options: ScopeOptions) {
    this.component = options.component;
    this.parent = options.parent;
    this.reporter = options.reporter ?? options.parent?.reporter;
    this.path = options.path ?? options.parent?.path ?? "";
    this.protocol = options.protocol ?? options.parent?.protocol ?? "zudo-react.v1";
    this.build = options.build ?? options.parent?.build ?? "";
    this.parent?.children.push(this);
  }
  get active(): boolean {
    return this.live;
  }
  get isActivated(): boolean {
    return this.activated;
  }
  get abortSignal(): AbortSignal {
    this.controller ??= new AbortController();
    if (!this.live && !this.controller.signal.aborted) this.controller.abort();
    return this.controller.signal;
  }
  private ensureSetup(): void {
    if (!this.live || this.activated || currentScope !== this)
      throw new Error("ZR_SCOPE_SETUP: registration requires active setup");
  }
  report(code: string, phase: Diagnostic["phase"], actual: string): void {
    const stack: string[] = [];
    for (let scope: RuntimeScope | undefined = this; scope; scope = scope.parent)
      stack.unshift(scope.component);
    try {
      this.reporter?.({
        code,
        phase,
        component: this.component,
        componentStack: stack,
        path: this.path,
        expected: "",
        actual,
        protocol: this.protocol,
        build: this.build,
      });
    } catch {
      /* Reporter failures must not stop resource cleanup. */
    }
  }
  onCleanup(fn: Cleanup): void {
    this.ensureSetup();
    this.cleanups.push(fn);
  }
  onActivate(fn: () => void | Cleanup): void {
    this.ensureSetup();
    this.activations.push(fn);
  }
  effect(fn: () => void | Cleanup): void {
    this.ensureSetup();
    let cleanup: Cleanup | undefined;
    const subscription: Subscription = subscribe(
      () => {
        if (cleanup) {
          try {
            const result: unknown = untracked(cleanup);
            if (isPromise(result))
              this.report("ZR_ASYNC_CLEANUP", "cleanup", "effect cleanup returned a promise");
          } catch (error) {
            this.report("ZR_EFFECT_CLEANUP", "cleanup", String(error));
          }
          cleanup = undefined;
        }
        try {
          const result = fn();
          if (isPromise(result))
            this.report("ZR_ASYNC_CLEANUP", "update", "effect returned a promise");
          else if (typeof result === "function") cleanup = result;
        } catch (error) {
          this.report("ZR_EFFECT_ERROR", "update", String(error));
        }
      },
      {
        phase: "effect",
        id: `${this.component}:effect:${this.id}:${this.effects.length}`,
        active: () => this.live && this.activated,
        report: this.root().jobReporter,
      },
    );
    const job: Job = {
      id: subscription.id,
      phase: "effect",
      active: () => this.live && this.activated,
      run: () => subscription.run(),
      report: this.root().jobReporter,
    };
    this.effects.push({
      start: () => enqueue(job),
      dispose: () => {
        subscription.dispose();
        if (cleanup) {
          try {
            const result: unknown = untracked(cleanup);
            if (isPromise(result))
              this.report("ZR_ASYNC_CLEANUP", "cleanup", "effect cleanup returned a promise");
          } catch (error) {
            this.report("ZR_EFFECT_CLEANUP", "cleanup", String(error));
          }
          cleanup = undefined;
        }
      },
    });
  }
  child(component: string): RuntimeScope {
    if (!this.live) throw new Error("ZR_SCOPE_DISPOSED");
    return new RuntimeScope({ component, parent: this });
  }
  activate(): void {
    if (!this.live || this.activated) return;
    try {
      for (const child of this.children) child.activate();
    } catch (error) {
      this.dispose();
      throw error;
    }
    this.activated = true;
    for (const fn of this.activations) {
      try {
        const result = fn();
        if (isPromise(result)) throw new Error("ZR_ASYNC_CLEANUP: activation returned a promise");
        if (typeof result === "function") this.cleanups.push(result);
      } catch (error) {
        this.report("ZR_ACTIVATION_ERROR", "activation", String(error));
        this.dispose();
        throw error;
      }
    }
    for (const effect of this.effects) effect.start();
  }
  dispose(): void {
    if (!this.live) return;
    this.live = false;
    this.controller?.abort();
    for (const child of [...this.children].reverse()) child.dispose();
    this.children.length = 0;
    for (const effect of this.effects) effect.dispose();
    this.effects.length = 0;
    for (const cleanup of [...this.cleanups].reverse()) {
      try {
        const result: unknown = untracked(cleanup);
        if (isPromise(result))
          this.report("ZR_ASYNC_CLEANUP", "cleanup", "cleanup returned a promise");
      } catch (error) {
        this.report("ZR_CLEANUP_ERROR", "cleanup", String(error));
      }
    }
    this.cleanups.length = 0;
    this.activations.length = 0;
  }
}

export function createScope(options: ScopeOptions): RuntimeScope {
  return new RuntimeScope(options);
}
export function withScope<T>(scope: RuntimeScope, fn: () => T): T {
  if (!scope.active) throw new Error("ZR_SCOPE_DISPOSED");
  const previous = currentScope;
  currentScope = scope;
  try {
    return fn();
  } finally {
    currentScope = previous;
  }
}
export function getScope(): Scope {
  if (!currentScope || !currentScope.active || currentScope.isActivated)
    throw new Error("ZR_NO_SCOPE: getScope requires synchronous setup");
  return currentScope;
}
