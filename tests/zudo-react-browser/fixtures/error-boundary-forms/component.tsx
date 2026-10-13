import { ErrorBoundary, getScope, Show, signal } from "@takazudo/zfb/zudo-react";

type State = {
  model: ReturnType<typeof signal<string | null>>;
  resetKey: ReturnType<typeof signal<number>>;
  fail: ReturnType<typeof signal<boolean>>;
  activations: number;
  cleanups: number;
};
const states: Record<string, State> = {};

function RadioAttempt(props: { id: string; formId?: string; state: State }) {
  getScope().onActivate(() => {
    props.state.activations++;
  });
  getScope().onCleanup(() => {
    props.state.cleanups++;
  });
  return (
    <>
      <label>
        <input
          id={`inside-${props.id}`}
          type="radio"
          name={`choice-${props.id}`}
          form={props.formId}
          value="inside"
          modelValue={props.state.model}
        />
        Inside boundary
      </label>
      <Show when={props.state.fail}>
        {() => {
          throw new Error("nested form child failed");
        }}
      </Show>
    </>
  );
}

function RadioBoundary(props: { id: string; formId?: string; state: State }) {
  return (
    <ErrorBoundary
      resetKey={props.state.resetKey}
      fallback={() => <p id={`outer-fallback-${props.id}`}>Outer fallback</p>}
    >
      {() => (
        <ErrorBoundary fallback={() => <p id={`inner-fallback-${props.id}`}>Inner fallback</p>}>
          {() => <RadioAttempt {...props} />}
        </ErrorBoundary>
      )}
    </ErrorBoundary>
  );
}

function Controls(props: { id: string; formId?: string; state: State }) {
  return (
    <>
      <RadioBoundary {...props} />
      <label>
        <input
          id={`outside-${props.id}`}
          type="radio"
          name={`choice-${props.id}`}
          form={props.formId}
          value="outside"
          modelValue={props.state.model}
        />
        Retained outside sibling
      </label>
      <output id={`model-${props.id}`}>{props.state.model}</output>
      <button
        type="button"
        id={`reset-${props.id}`}
        on:click={() => {
          props.state.resetKey.value++;
        }}
      >
        Reset boundary key
      </button>
      <button
        type="button"
        id={`fail-${props.id}`}
        on:click={() => {
          props.state.fail.value = true;
        }}
      >
        Fail nested child
      </button>
    </>
  );
}

export default function BoundaryForms(props: { association: "ancestor" | "explicit" }) {
  const id = props.association;
  const state: State = {
    model: signal<string | null>("inside"),
    resetKey: signal(0),
    fail: signal(false),
    activations: 0,
    cleanups: 0,
  };
  states[id] = state;
  if (id === "ancestor") {
    return (
      <form id={`form-${id}`}>
        <Controls id={id} state={state} />
      </form>
    );
  }
  return (
    <section>
      <form id={`form-${id}`} />
      <Controls id={id} formId={`form-${id}`} state={state} />
    </section>
  );
}

export function run() {
  return states;
}
