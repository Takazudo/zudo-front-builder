import { getScope, signal } from "@takazudo/zfb/zudo-react";

type State = { activations: number; clicks: ReturnType<typeof signal<number>> };
const states: Record<string, State> = {};

export default function MismatchProbe(props: { variant: string }) {
  const state = { activations: 0, clicks: signal(0) };
  states[props.variant] = state;
  getScope().onActivate(() => {
    state.activations++;
  });
  const preservedInput = <input id={`preserve-${props.variant}`} defaultValue="before hydration" />;

  if (props.variant === "wrong-tag")
    return (
      <section>
        <button>stable button</button>
        {preservedInput}
      </section>
    );
  if (props.variant === "missing-marker") {
    const text = signal("dynamic slot");
    return (
      <section>
        <p>{text}</p>
        {preservedInput}
      </section>
    );
  }
  if (props.variant === "wrong-text")
    return (
      <section>
        <span>server text expected</span>
        {preservedInput}
      </section>
    );
  if (props.variant === "healthy")
    return (
      <section>
        <button
          id="healthy-button"
          on:click={() => {
            state.clicks.value++;
          }}
        >
          Healthy {state.clicks}
        </button>
        {preservedInput}
      </section>
    );
  return (
    <section>
      <button id={`identity-${props.variant}`}>Identity probe</button>
      {preservedInput}
    </section>
  );
}

export function run() {
  return states;
}
