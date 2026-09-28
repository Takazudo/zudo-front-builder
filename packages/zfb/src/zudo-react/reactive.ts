import { enqueue, type Job, type Phase } from "./scheduler.js";
import type { ReadonlySignal, Signal } from "./reactive-types.js";

interface Source {
  readonly subscribers: Set<Observer>;
}
interface Observer {
  readonly id: string;
  readonly dependencies: Set<Source>;
  invalidate(): void;
}
let currentObserver: Observer | null = null;
let evaluating = 0;
const evaluationStack: Computed<unknown>[] = [];
let nextId = 0;

// The quoted own key is the retained runtime definition witness.
const core = { "@takazudo/zfb/zudo-react/runtime-definition-v1": true, nextId: () => ++nextId };

function track(source: Source): void {
  if (!currentObserver) return;
  source.subscribers.add(currentObserver);
  currentObserver.dependencies.add(source);
}
function detach(observer: Observer): void {
  for (const source of observer.dependencies) source.subscribers.delete(observer);
  observer.dependencies.clear();
}
function untracked<T>(fn: () => T): T {
  const previous = currentObserver;
  currentObserver = null;
  try {
    return fn();
  } finally {
    currentObserver = previous;
  }
}
export { untracked };

class Writable<T> implements Signal<T>, Source {
  readonly $$zudoReactive = "zudo-react.reactive.v1" as const;
  readonly subscribers = new Set<Observer>();
  constructor(private current: T) {
    if (!core["@takazudo/zfb/zudo-react/runtime-definition-v1"])
      throw new Error("ZR_RUNTIME_DEFINITION");
  }
  get value(): T {
    track(this);
    return this.current;
  }
  set value(value: T) {
    if (evaluating > 0) throw new Error("ZR_COMPUTED_WRITE: write during computed evaluation");
    if (Object.is(value, this.current)) return;
    this.current = value;
    for (const observer of [...this.subscribers]) observer.invalidate();
  }
}

class Computed<T> implements ReadonlySignal<T>, Source, Observer {
  readonly $$zudoReactive = "zudo-react.reactive.v1" as const;
  readonly id = `computed:${core.nextId()}`;
  readonly subscribers = new Set<Observer>();
  readonly dependencies = new Set<Source>();
  private cached!: T;
  private dirty = true;
  private evaluatingNow = false;
  constructor(private readonly read: () => T) {}
  get value(): T {
    if (this.evaluatingNow) {
      throw new Error(
        `ZR_COMPUTED_CYCLE: ${[...evaluationStack.map((entry) => entry.id), this.id].join(" -> ")}`,
      );
    }
    if (this.dirty) this.evaluate();
    track(this);
    if (this.subscribers.size === 0) {
      detachAndRelease(this);
      this.dirty = true;
    }
    return this.cached;
  }
  private evaluate(): void {
    detachAndRelease(this);
    const previous = currentObserver;
    currentObserver = this;
    this.evaluatingNow = true;
    evaluationStack.push(this as Computed<unknown>);
    evaluating++;
    try {
      this.cached = this.read();
      this.dirty = false;
    } catch (error) {
      detachAndRelease(this);
      throw error;
    } finally {
      evaluating--;
      evaluationStack.pop();
      this.evaluatingNow = false;
      currentObserver = previous;
    }
  }
  invalidate(): void {
    if (this.dirty) return;
    this.dirty = true;
    for (const observer of [...this.subscribers]) observer.invalidate();
  }
  release(): void {
    if (this.subscribers.size === 0) {
      detachAndRelease(this);
      this.dirty = true;
    }
  }
}

function release(source: Source): void {
  if (source instanceof Computed) source.release();
}
function detachAndRelease(observer: Observer): void {
  const sources = [...observer.dependencies];
  detach(observer);
  for (const source of sources) release(source);
}

export function signal<T>(initial: T): Signal<T> {
  return new Writable(initial);
}
export function computed<T>(read: () => T): ReadonlySignal<T> {
  return new Computed(read);
}
export function readSnapshot<T>(value: ReadonlySignal<T>): T {
  return untracked(() => value.value);
}

export interface Subscription {
  readonly id: string;
  run(): void;
  dispose(): void;
}
export function subscribe(
  callback: () => void,
  options: { phase?: Phase; id?: string; active?: () => boolean; report?: Job["report"] } = {},
): Subscription {
  let disposed = false;
  const observer: Observer = {
    id: options.id ?? `subscriber:${core.nextId()}`,
    dependencies: new Set(),
    invalidate() {
      enqueue(job);
    },
  };
  const job: Job = {
    id: observer.id,
    phase: options.phase ?? "commit",
    active: () => !disposed && (options.active?.() ?? true),
    run: () => subscription.run(),
    ...(options.report ? { report: options.report } : {}),
  };
  const subscription: Subscription = {
    id: observer.id,
    run() {
      if (!job.active()) return;
      detachAndRelease(observer);
      const previous = currentObserver;
      currentObserver = observer;
      try {
        callback();
      } finally {
        currentObserver = previous;
      }
    },
    dispose() {
      disposed = true;
      detachAndRelease(observer);
    },
  };
  return subscription;
}

/** Internal test introspection; absent from every public entry. */
export function subscriberCount(value: ReadonlySignal<unknown>): number {
  return value instanceof Writable || value instanceof Computed ? value.subscribers.size : 0;
}
