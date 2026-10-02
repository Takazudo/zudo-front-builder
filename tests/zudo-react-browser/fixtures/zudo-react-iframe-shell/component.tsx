import { Show, signal, type Ref } from "@takazudo/zfb/zudo-react";

type State = {
  src: ReturnType<typeof signal<string>>;
  srcdoc: ReturnType<typeof signal<string | null>>;
  visible: ReturnType<typeof signal<boolean>>;
  loads: ReturnType<typeof signal<number>>;
  ref: Ref<HTMLIFrameElement>;
};

const ROOT_IDS = ["frame-hydrate", "frame-mount", "frame-markers"];
const states = new Map<string, State>();

function initialDocument(id: string) {
  return `<!doctype html><html><body data-generation="${id}-srcdoc-one"><p id="foreign-content">${id} foreign document</p><!--zr:1:77:t-->ZFB_FOREIGN_DOCUMENT_MARKER<!--/zr:1:77--></body></html>`;
}

export default function IframeShellProbe(props: { id: string }) {
  const state: State = {
    src: signal("/fixtures/zudo-react-iframe-shell/src-one.html"),
    srcdoc: signal(initialDocument(props.id)),
    visible: signal(true),
    loads: signal(0),
    ref: { current: null },
  };
  states.set(props.id, state);

  return (
    <section>
      <output id={`${props.id}-load-count`}>{state.loads}</output>
      {props.id === "frame-hydrate" ? (
        <iframe
          id="frame-static-src"
          title="Static source"
          src="/fixtures/zudo-react-iframe-shell/src-one.html"
          sandbox="allow-same-origin"
          allow="fullscreen"
        />
      ) : null}
      <Show when={state.visible}>
        {() => (
          <iframe
            id={props.id}
            title={`Preview ${props.id}`}
            src={state.src}
            srcdoc={state.srcdoc}
            sandbox="allow-same-origin"
            allow="fullscreen"
            ref={state.ref}
            on:load={() => {
              state.loads.value++;
            }}
          />
        )}
      </Show>
    </section>
  );
}

export function run(index: number) {
  return states.get(ROOT_IDS[index]);
}
