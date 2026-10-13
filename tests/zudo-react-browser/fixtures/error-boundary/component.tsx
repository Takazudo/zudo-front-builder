import { ErrorBoundary, getScope, Show, signal } from "@takazudo/zfb/zudo-react";

type Lifetime = { setups: number; activations: number; cleanups: number };
type State = {
  fail: ReturnType<typeof signal<boolean>>;
  resetKey: ReturnType<typeof signal<number>>;
  count: ReturnType<typeof signal<number>>;
  good: Lifetime;
  attempt: Lifetime;
  working: Lifetime;
  failed: Lifetime;
  fallback: Lifetime;
};
const states: Record<string, State> = {};
const lifetime = (): Lifetime => ({ setups: 0, activations: 0, cleanups: 0 });

function track(value: Lifetime) {
  value.setups++;
  const scope = getScope();
  scope.onCleanup(() => value.cleanups++);
  scope.onActivate(() => {
    value.activations++;
  });
}

function Good(props: { id: string; state: State }) {
  track(props.state.good);
  return (
    <button
      id={`good-${props.id}`}
      on:click={() => {
        props.state.count.value++;
      }}
    >
      Good {props.state.count}
    </button>
  );
}

function FailedBranch(props: { state: State }): never {
  track(props.state.failed);
  throw new Error("boundary branch failed");
}

function WorkingBranch(props: { id: string; state: State }) {
  track(props.state.working);
  return <span id={`working-${props.id}`}>Working child</span>;
}

function Attempt(props: { id: string; state: State }) {
  track(props.state.attempt);
  return (
    <Show when={props.state.fail} fallback={() => <WorkingBranch {...props} />}>
      {() => <FailedBranch state={props.state} />}
    </Show>
  );
}

function Fallback(props: { id: string; state: State; error: unknown; reset: () => void }) {
  track(props.state.fallback);
  return (
    <section id={`fallback-${props.id}`}>
      <output>{props.error instanceof Error ? props.error.message : String(props.error)}</output>
      <button
        id={`retry-${props.id}`}
        on:click={() => {
          props.state.fail.value = false;
          props.reset();
        }}
      >
        Retry child
      </button>
    </section>
  );
}

export default function BoundaryDemo(props: { id: string; failOn?: "server" | "client" }) {
  const client = typeof window !== "undefined";
  const state: State = {
    fail: signal(props.failOn === (client ? "client" : "server")),
    resetKey: signal(0),
    count: signal(0),
    good: lifetime(),
    attempt: lifetime(),
    working: lifetime(),
    failed: lifetime(),
    fallback: lifetime(),
  };
  states[props.id] = state;
  return (
    <section id={`demo-${props.id}`}>
      <Good id={props.id} state={state} />
      <button
        id={`fail-${props.id}`}
        on:click={() => {
          state.fail.value = true;
        }}
      >
        Fail child
      </button>
      <button
        id={`key-reset-${props.id}`}
        on:click={() => {
          state.fail.value = false;
          state.resetKey.value++;
        }}
      >
        Change reset key
      </button>
      <ErrorBoundary
        resetKey={state.resetKey}
        fallback={(error, reset) => (
          <Fallback id={props.id} state={state} error={error} reset={reset} />
        )}
      >
        {() => <Attempt id={props.id} state={state} />}
      </ErrorBoundary>
    </section>
  );
}

export function run() {
  return states;
}
