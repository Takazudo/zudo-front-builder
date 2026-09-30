import type { Listener, Ref } from "@takazudo/zfb/zudo-react";
import type { JSX } from "@takazudo/zfb/zudo-react/jsx-runtime";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

const inferred = (
  <>
    <form
      on:submit={(event) => {
        const exact: Equal<typeof event, SubmitEvent & { currentTarget: HTMLFormElement }> = true;
        event.submitter;
        event.currentTarget.reset();
        // @ts-expect-error A bubbling event may originate from another element.
        event.target.value;
        void exact;
      }}
    />
    <input
      on:keydown={(event) => {
        const exact: Equal<typeof event, KeyboardEvent & { currentTarget: HTMLInputElement }> =
          true;
        void exact;
        void (event.key + event.currentTarget.value);
      }}
    />
    <button
      on:click={(event) => {
        const exact: Equal<typeof event, PointerEvent & { currentTarget: HTMLButtonElement }> =
          true;
        event.currentTarget.disabled = true;
        void exact;
      }}
    />
    <svg>
      <path
        on:click={(event) => {
          const exact: Equal<typeof event, PointerEvent & { currentTarget: SVGPathElement }> = true;
          event.currentTarget.getTotalLength();
          void exact;
        }}
      />
    </svg>
  </>
);

const annotations = (
  <>
    <form on:submit={(event: SubmitEvent) => void event.submitter} />
    <input on:keydown={(event: KeyboardEvent) => void event.key} />
    <form
      on:submit={async (event: SubmitEvent) => {
        void event.submitter;
      }}
    />
    {/* @ts-expect-error Submit expects SubmitEvent, not KeyboardEvent. */}
    <form on:submit={(event: KeyboardEvent) => void event.key} />
    <div on:my-event={(event: CustomEvent<{ id: string }>) => void event.detail.id} />
    <my-widget
      on:change={(event) => {
        const exact: Equal<typeof event, Event & { currentTarget: HTMLElement }> = true;
        void exact;
      }}
    />
    {/* @ts-expect-error React-style onSubmit is not an intrinsic prop. */}
    <form onSubmit={() => {}} />
  </>
);

const captureProps: JSX.IntrinsicElements["button"] = {
  "on:click:capture": (event) => {
    const exact: Equal<typeof event, PointerEvent & { currentTarget: HTMLButtonElement }> = true;
    event.currentTarget.disabled = true;
    void exact;
  },
};
const capture = <button {...captureProps} />;
const badCaptureProps: JSX.IntrinsicElements["button"] = {
  // @ts-expect-error Capture click expects PointerEvent.
  "on:click:capture": (event: KeyboardEvent) => void event.key,
};

const broadRef: Ref<HTMLElement> = { current: null };
const inputRefType: Equal<
  JSX.IntrinsicElements["input"]["ref"],
  Ref<HTMLInputElement> | undefined
> = true;
const textareaRefType: Equal<
  JSX.IntrinsicElements["textarea"]["ref"],
  Ref<HTMLTextAreaElement> | undefined
> = true;
const selectRefType: Equal<
  JSX.IntrinsicElements["select"]["ref"],
  Ref<HTMLSelectElement> | undefined
> = true;
const refs = (
  <>
    <div ref={broadRef} />
    <form ref={broadRef} />
  </>
);

function ForwardSubmit(props: { onSave: Listener<SubmitEvent> }) {
  return <form on:submit={props.onSave} />;
}

void [
  inferred,
  annotations,
  capture,
  badCaptureProps,
  inputRefType,
  textareaRefType,
  selectRefType,
  refs,
  ForwardSubmit,
];
