import { computed, getScope, signal, type Ref } from "@takazudo/zfb/zudo-react";

declare global {
  interface Window {
    __zudoReactAbortDuringSetup?: () => void;
  }
}

type WidgetState = {
  value: ReturnType<typeof signal<number>>;
  clicks: ReturnType<typeof signal<number>>;
  effectRuns: number;
  effectCleanups: number;
  observerCleanups: number;
  listenerCleanups: number;
  observedChanges: number;
};
type FailureState = { activations: number; clicks: number };
const widget: WidgetState = {
  value: signal(0),
  clicks: signal(0),
  effectRuns: 0,
  effectCleanups: 0,
  observerCleanups: 0,
  listenerCleanups: 0,
  observedChanges: 0,
};
const failed: Record<string, FailureState> = {};

export default function DisposalProbe(props: { variant?: string }) {
  if (props.variant === "aborted" || props.variant === "preflight") {
    const variant = props.variant;
    failed[variant] ??= { activations: 0, clicks: 0 };
    const state = failed[variant];
    if (variant === "aborted" && typeof window !== "undefined")
      window.__zudoReactAbortDuringSetup?.();
    getScope().onActivate(() => {
      state.activations++;
    });
    return (
      <button
        id={`${variant}-button`}
        on:click={() => {
          state.clicks++;
        }}
      >
        {variant === "preflight" ? "preflight expected" : "aborted root"}
      </button>
    );
  }

  const scope = getScope();
  const doubled = computed(() => widget.value.value * 2);
  const ref: Ref<HTMLButtonElement> = { current: null };
  scope.effect(() => {
    doubled.value;
    widget.effectRuns++;
    return () => widget.effectCleanups++;
  });
  scope.onActivate(() => {
    const button = ref.current!;
    const observer = new MutationObserver(() => widget.observedChanges++);
    observer.observe(button, { childList: true, subtree: true, characterData: true });
    return () => {
      observer.disconnect();
      widget.observerCleanups++;
    };
  });
  scope.onActivate(() => {
    const listener = () => widget.value.value++;
    window.addEventListener("zudo-disposal-probe", listener);
    return () => {
      window.removeEventListener("zudo-disposal-probe", listener);
      widget.listenerCleanups++;
    };
  });

  return (
    <section>
      <button
        id="disposal-button"
        ref={ref}
        on:click={() => {
          widget.clicks.value++;
        }}
      >
        Keep this widget
      </button>
      <output id="disposal-value">{doubled}</output>
    </section>
  );
}

export function run() {
  return { widget, failed };
}
