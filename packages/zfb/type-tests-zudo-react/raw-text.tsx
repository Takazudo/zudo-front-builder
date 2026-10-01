import { computed, signal } from "@takazudo/zfb/zudo-react";
import type { Ref } from "@takazudo/zfb/zudo-react";
import type { JSX } from "@takazudo/zfb/zudo-react/jsx-runtime";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

const scriptText: string = `window.label = "A & B <tag>";`;
const styleText: string = `body::before { content: "A & B <tag>"; }`;
const dynamicText = signal(scriptText);
const computedText = computed(() => styleText);

const scriptProps = {
  src: "/app.js",
  rawHtml: scriptText,
} satisfies JSX.IntrinsicElements["script"];
const styleProps = { media: "screen", rawHtml: styleText } satisfies JSX.IntrinsicElements["style"];

const accepted = (
  <>
    <script />
    <script src="/external.js" />
    <script rawHtml="window.app = true;" />
    <script rawHtml={scriptText} />
    <script {...scriptProps} />
    <style />
    <style rawHtml="body { color: red; }" />
    <style rawHtml={styleText} />
    <style {...styleProps} />
    <div rawHtml={dynamicText} />
    <title>Page title</title>
    <textarea defaultValue="text" />
    <textarea>text</textarea>
  </>
);

// @ts-expect-error Script children are forbidden.
const scriptChild = <script>window.app = true;</script>;
// @ts-expect-error Style children are forbidden.
const styleChild = <style>stylesheet text</style>;
// @ts-expect-error The explicit children prop is forbidden for scripts.
const scriptChildrenOnly = <script children="window.app = true;" />;
// @ts-expect-error The explicit children prop is forbidden for styles.
const styleChildrenOnly = <style children="body { color: red; }" />;
// @ts-expect-error Script children cannot accompany rawHtml.
const scriptChildrenAndRaw = <script rawHtml={scriptText} children="fallback" />;
// @ts-expect-error Style children cannot accompany rawHtml.
const styleChildrenAndRaw = <style rawHtml={styleText} children="fallback" />;

// @ts-expect-error Script rawHtml cannot be a signal.
const scriptSignal = <script rawHtml={dynamicText} />;
// @ts-expect-error Script rawHtml cannot be computed.
const scriptComputed = <script rawHtml={computedText} />;
// @ts-expect-error Script rawHtml cannot be null.
const scriptNull = <script rawHtml={null} />;
// @ts-expect-error Style rawHtml cannot be a signal.
const styleSignal = <style rawHtml={dynamicText} />;
// @ts-expect-error Style rawHtml cannot be computed.
const styleComputed = <style rawHtml={computedText} />;
// @ts-expect-error Style rawHtml cannot be null.
const styleNull = <style rawHtml={null} />;

const scriptChildSpread = { children: "window.app = true;" } as const;
const styleChildSpread = { children: "body { color: red; }" } as const;
const scriptRawChildSpread = { rawHtml: scriptText, children: "fallback" } as const;
const styleRawChildSpread = { rawHtml: styleText, children: "fallback" } as const;
const scriptReactiveSpread = { rawHtml: dynamicText } as const;
const styleReactiveSpread = { rawHtml: computedText } as const;
// @ts-expect-error A typed script spread cannot supply children.
const rejectedScriptChildSpread = <script {...scriptChildSpread} />;
// @ts-expect-error A typed style spread cannot supply children.
const rejectedStyleChildSpread = <style {...styleChildSpread} />;
// @ts-expect-error A typed script spread cannot combine children and rawHtml.
const rejectedScriptRawChildSpread = <script {...scriptRawChildSpread} />;
// @ts-expect-error A typed style spread cannot combine children and rawHtml.
const rejectedStyleRawChildSpread = <style {...styleRawChildSpread} />;
// @ts-expect-error A typed script spread cannot supply reactive rawHtml.
const rejectedScriptReactiveSpread = <script {...scriptReactiveSpread} />;
// @ts-expect-error A typed style spread cannot supply reactive rawHtml.
const rejectedStyleReactiveSpread = <style {...styleReactiveSpread} />;

// exactOptionalPropertyTypes is disabled here, so TypeScript accepts children={undefined}.
// The runtime still observes the own children property and rejects it with rawHtml.
const undefinedChildren = (
  <>
    <script rawHtml={scriptText} children={undefined} />
    <style rawHtml={styleText} children={undefined} />
  </>
);

// `any` bypasses these JSX checks; runtime validation remains necessary for untyped inputs.
const untypedProps: any = { children: "unsafe", rawHtml: scriptText };
const uncheckedSpread = <script {...untypedProps} />;

const scriptRefType: Equal<JSX.IntrinsicElements["script"]["ref"], Ref<HTMLElement> | undefined> =
  true;
const styleRefType: Equal<JSX.IntrinsicElements["style"]["ref"], Ref<HTMLElement> | undefined> =
  true;
const scriptEvent = (
  <script
    on:click={(event) => {
      const exact: Equal<typeof event, PointerEvent & { currentTarget: HTMLScriptElement }> = true;
      void event.currentTarget.src;
      void exact;
    }}
  />
);
const styleEvent = (
  <style
    on:click={(event) => {
      const exact: Equal<typeof event, PointerEvent & { currentTarget: HTMLStyleElement }> = true;
      void event.currentTarget.media;
      void exact;
    }}
  />
);

void [
  accepted,
  scriptChild,
  styleChild,
  scriptChildrenOnly,
  styleChildrenOnly,
  scriptChildrenAndRaw,
  styleChildrenAndRaw,
  scriptSignal,
  scriptComputed,
  scriptNull,
  styleSignal,
  styleComputed,
  styleNull,
  rejectedScriptChildSpread,
  rejectedStyleChildSpread,
  rejectedScriptRawChildSpread,
  rejectedStyleRawChildSpread,
  rejectedScriptReactiveSpread,
  rejectedStyleReactiveSpread,
  undefinedChildren,
  uncheckedSpread,
  scriptRefType,
  styleRefType,
  scriptEvent,
  styleEvent,
];
