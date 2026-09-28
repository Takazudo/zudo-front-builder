import { getScope, signal, type Ref } from "@takazudo/zfb/zudo-react";

type State = {
  clicks: ReturnType<typeof signal<number>>;
  asyncSettled: ReturnType<typeof signal<number>>;
  events: string[];
  childRef: Ref<HTMLButtonElement>;
  parentRef: Ref<HTMLDivElement>;
  settleAsync: () => void;
};
let state: State;

function EventChild(props: { state: State; pending: Promise<void> }) {
  const scope = getScope();
  scope.onActivate(() => {
    props.state.events.push(
      `child:${props.state.childRef.current?.id}:${props.state.parentRef.current?.id}`,
    );
  });
  return (
    <button
      id="event-button"
      ref={props.state.childRef}
      on:click={async () => {
        props.state.clicks.value++;
        const abortSignal = scope.abortSignal;
        await props.pending;
        if (!abortSignal.aborted) props.state.asyncSettled.value++;
      }}
    >
      Clicked {props.state.clicks}
    </button>
  );
}

export default function EventsRefsProbe() {
  const events: string[] = [];
  const childRef: Ref<HTMLButtonElement> = { current: null };
  const parentRef: Ref<HTMLDivElement> = { current: null };
  const clicks = signal(0);
  const asyncSettled = signal(0);
  let settleAsync!: () => void;
  const pending = new Promise<void>((resolve) => {
    settleAsync = resolve;
  });
  state = { clicks, asyncSettled, events, childRef, parentRef, settleAsync };
  getScope().onActivate(() => {
    events.push(`parent:${childRef.current?.id}:${parentRef.current?.id}`);
  });

  return (
    <div id="event-parent" ref={parentRef}>
      <EventChild state={state} pending={pending} />
      <output id="event-count">{clicks}</output>
      <output id="async-settled">{asyncSettled}</output>
    </div>
  );
}

export function run() {
  return state;
}
