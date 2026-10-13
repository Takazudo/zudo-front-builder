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
  declare readonly component: string;
  declare readonly parent: RuntimeScope | undefined;
  declare private readonly reporter: Reporter | undefined;
  declare private readonly path: string;
  declare private readonly protocol: string;
  declare private readonly build: string;
  private readonly children = new Set<RuntimeScope>();
  readonly cleanups: Cleanup[] = [];
  declare activations: Array<() => void | Cleanup>;
  declare effects: Array<{ start: () => void; dispose: () => void }>;
  declare expose?: (scope: RuntimeScope) => RuntimeScope;
  declare start?: () => void;
  declare stop?: () => void;
  declare recovery: ((scope: RuntimeScope, error: unknown) => (() => void) | undefined) | undefined;
  declare renderRecovery: ((error: unknown) => () => void) | undefined;
  declare controller: AbortController | undefined;
  live = true;
  activated = false;
  readonly jobReporter = (code: string, actual: string): void => {
    this.report(code, "update", actual);
  };
  root(): RuntimeScope {
    return this.parent?.root() ?? this;
  }

  constructor(options: ScopeOptions) {
    this.component = options.component;
    this.parent = options.parent;
    this.reporter = options.reporter ?? options.parent?.reporter;
    this.path = options.path ?? options.parent?.path ?? "";
    this.protocol = options.protocol ?? options.parent?.protocol ?? "zudo-react.v1";
    this.build = options.build ?? options.parent?.build ?? "";
    this.parent?.children.add(this);
  }
  get active(): boolean {
    return this.live;
  }
  get isActivated(): boolean {
    return this.activated;
  }
  declare readonly abortSignal: AbortSignal;
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
  declare onCleanup: Scope["onCleanup"];
  declare onActivate: Scope["onActivate"];
  declare effect: Scope["effect"];
  child(component: string): RuntimeScope {
    if (!this.live) throw new Error("ZR_SCOPE_DISPOSED");
    const child = new RuntimeScope({ component, parent: this });
    this.expose?.(child);
    return child;
  }
  // The optional service prepares under a catch; commit/activation stays outside it.
  recoverRender(error: unknown): boolean {
    const commit = this.root().recovery?.(this, error);
    commit?.();
    return !!commit;
  }
  activate(): void {
    if (!this.live || this.activated) return;
    try {
      for (const child of [...this.children]) child.activate();
    } catch (error) {
      this.dispose();
      throw error;
    }
    this.activated = true;
    this.start?.();
  }
  dispose(): void {
    if (!this.live) return;
    this.live = false;
    this.parent?.children.delete(this);
    this.renderRecovery = undefined;
    this.recovery = undefined;
    this.controller?.abort();
    for (const child of [...this.children].reverse()) child.dispose();
    this.children.clear();
    this.stop?.();
    for (const cleanup of [...this.cleanups].reverse()) {
      try {
        untracked(cleanup);
      } catch (error) {
        this.report("ZR_CLEANUP_ERROR", "cleanup", String(error));
      }
    }
    this.cleanups.length = 0;
  }
}

export function createScope(options: ScopeOptions): RuntimeScope {
  const scope = new RuntimeScope(options);
  return exposeScope(scope);
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
  return exposeScope(currentScope);
}

function effect(this: RuntimeScope, fn: () => void | Cleanup): void {
  ensureSetup(this);
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

// Public lifecycle capabilities are installed only when application code asks
// for a scope. Internal renderer scopes keep the same ownership lifecycle.
function exposeScope(scope: RuntimeScope): RuntimeScope {
  if (!scope.expose) {
    scope.expose = exposeScope;
    scope.effects = [];
    scope.activations = [];
    scope.stop = () => {
      for (const effect of scope.effects) effect.dispose();
      scope.effects.length = 0;
      scope.activations.length = 0;
    };
    scope.effect = effect;
    scope.onActivate = onActivate;
    scope.onCleanup = onCleanup;
    scope.start = activateCallbacks.bind(scope);
    Object.defineProperty(scope, "abortSignal", { get: abortSignal });
  }
  return scope;
}
function abortSignal(this: RuntimeScope): AbortSignal {
  this.controller ??= new AbortController();
  if (!this.live && !this.controller.signal.aborted) this.controller.abort();
  return this.controller.signal;
}

function onActivate(this: RuntimeScope, fn: () => void | Cleanup): void {
  ensureSetup(this);
  this.activations.push(fn);
}

function activateCallbacks(this: RuntimeScope): void {
  for (const fn of this.activations) {
    try {
      const result = fn();
      if (isPromise(result)) throw new Error("ZR_ASYNC_CLEANUP: activation returned a promise");
      if (typeof result === "function") registerCleanup(this, result);
    } catch (error) {
      this.report("ZR_ACTIVATION_ERROR", "activation", String(error));
      this.dispose();
      throw error;
    }
  }
  for (const effect of this.effects) effect.start();
}

function ensureSetup(scope: RuntimeScope): void {
  if (!scope.live || scope.activated || currentScope !== scope)
    throw new Error("ZR_SCOPE_SETUP: registration requires active setup");
}
function onCleanup(this: RuntimeScope, fn: Cleanup): void {
  ensureSetup(this);
  registerCleanup(this, fn);
}
function registerCleanup(scope: RuntimeScope, fn: Cleanup): void {
  scope.cleanups.push(() => {
    if (isPromise(fn())) scope.report("ZR_ASYNC_CLEANUP", "cleanup", "cleanup returned a promise");
  });
}
