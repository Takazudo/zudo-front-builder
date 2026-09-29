import { signal } from "@takazudo/zfb/zudo-react";

type State = {
  left: ReturnType<typeof signal<string>>;
  empty: ReturnType<typeof signal<string>>;
  right: ReturnType<typeof signal<string>>;
};
const states: Record<string, State> = {};

export default function AdoptionProbe(props: { id: string }) {
  const state = {
    left: signal(`left-${props.id}`),
    empty: signal(""),
    right: signal(`right-${props.id}`),
  };
  states[props.id] = state;
  return (
    <section id={`adoption-${props.id}`}>
      <p id={`adjacent-${props.id}`}>
        lead{state.left}
        {state.empty}
        {state.right}tail
      </p>
      <>
        <strong id={`fragment-a-${props.id}`}>fragment A</strong>
        <em id={`fragment-b-${props.id}`}>fragment B</em>
      </>
    </section>
  );
}

export function run() {
  return states;
}
