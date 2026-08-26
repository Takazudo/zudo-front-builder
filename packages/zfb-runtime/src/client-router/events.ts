/// <reference lib="dom" />
import { safeReplaceState } from "./history-safe.js";
import { swap } from "./swap-functions.js";
import type { Direction, NavigationTypeString } from "./types.js";

export const TRANSITION_BEFORE_PREPARATION = "zfb:before-preparation";
export const TRANSITION_AFTER_PREPARATION = "zfb:after-preparation";
export const TRANSITION_BEFORE_SWAP = "zfb:before-swap";
export const TRANSITION_AFTER_SWAP = "zfb:after-swap";
export const TRANSITION_PAGE_LOAD = "zfb:page-load";
export const TRANSITION_NAVIGATION_ABORTED = "zfb:navigation-aborted";

type Events =
  | "zfb:after-preparation"
  | "zfb:after-swap"
  | "zfb:page-load"
  | "zfb:navigation-aborted";
export const triggerEvent = (name: Events) => document.dispatchEvent(new Event(name));
export const onPageLoad = () => triggerEvent("zfb:page-load");

/*
 * Common stuff
 */
class BeforeEvent extends Event {
  readonly from: URL;
  to: URL;
  direction: Direction | string;
  readonly navigationType: NavigationTypeString;
  readonly sourceElement: Element | undefined;
  readonly info: any;
  newDocument: Document;
  readonly signal: AbortSignal;

  constructor(
    type: string,
    eventInitDict: EventInit | undefined,
    from: URL,
    to: URL,
    direction: Direction | string,
    navigationType: NavigationTypeString,
    sourceElement: Element | undefined,
    info: any,
    newDocument: Document,
    signal: AbortSignal,
  ) {
    super(type, eventInitDict);
    this.from = from;
    this.to = to;
    this.direction = direction;
    this.navigationType = navigationType;
    this.sourceElement = sourceElement;
    this.info = info;
    this.newDocument = newDocument;
    this.signal = signal;

    Object.defineProperties(this, {
      from: { enumerable: true },
      to: { enumerable: true, writable: true },
      direction: { enumerable: true, writable: true },
      navigationType: { enumerable: true },
      sourceElement: { enumerable: true },
      info: { enumerable: true },
      newDocument: { enumerable: true, writable: true },
      signal: { enumerable: true },
    });
  }
}

/*
 * TransitionBeforePreparationEvent

 */
export const isTransitionBeforePreparationEvent = (
  value: any,
): value is TransitionBeforePreparationEvent =>
  typeof value === "object" && value !== null && value.type === TRANSITION_BEFORE_PREPARATION;
export class TransitionBeforePreparationEvent extends BeforeEvent {
  formData: FormData | undefined;
  loader: () => Promise<void>;
  constructor(
    from: URL,
    to: URL,
    direction: Direction | string,
    navigationType: NavigationTypeString,
    sourceElement: Element | undefined,
    info: any,
    newDocument: Document,
    signal: AbortSignal,
    formData: FormData | undefined,
    loader: (event: TransitionBeforePreparationEvent) => Promise<void>,
  ) {
    super(
      TRANSITION_BEFORE_PREPARATION,
      { cancelable: true },
      from,
      to,
      direction,
      navigationType,
      sourceElement,
      info,
      newDocument,
      signal,
    );
    this.formData = formData;
    this.loader = loader.bind(this, this);
    Object.defineProperties(this, {
      formData: { enumerable: true },
      loader: { enumerable: true, writable: true },
    });
  }
}

/*
 * TransitionBeforeSwapEvent
 */
export const isTransitionBeforeSwapEvent = (value: any): value is TransitionBeforeSwapEvent =>
  typeof value === "object" && value !== null && value.type === TRANSITION_BEFORE_SWAP;
export class TransitionBeforeSwapEvent extends BeforeEvent {
  override readonly direction: Direction | string;
  readonly viewTransition: ViewTransition;
  swap: () => void;

  constructor(afterPreparation: BeforeEvent, viewTransition: ViewTransition) {
    super(
      TRANSITION_BEFORE_SWAP,
      undefined,
      afterPreparation.from,
      afterPreparation.to,
      afterPreparation.direction,
      afterPreparation.navigationType,
      afterPreparation.sourceElement,
      afterPreparation.info,
      afterPreparation.newDocument,
      afterPreparation.signal,
    );
    this.direction = afterPreparation.direction;
    this.viewTransition = viewTransition;
    this.swap = () => swap(this.newDocument);

    Object.defineProperties(this, {
      direction: { enumerable: true },
      viewTransition: { enumerable: true },
      swap: { enumerable: true, writable: true },
    });
  }
}

export async function doPreparation(
  from: URL,
  to: URL,
  direction: Direction | string,
  navigationType: NavigationTypeString,
  sourceElement: Element | undefined,
  info: any,
  signal: AbortSignal,
  formData: FormData | undefined,
  defaultLoader: (event: TransitionBeforePreparationEvent) => Promise<void>,
) {
  const event = new TransitionBeforePreparationEvent(
    from,
    to,
    direction,
    navigationType,
    sourceElement,
    info,
    window.document,
    signal,
    formData,
    defaultLoader,
  );
  if (document.dispatchEvent(event)) {
    await event.loader();
    if (!event.defaultPrevented) {
      triggerEvent("zfb:after-preparation");
      if (event.navigationType !== "traverse") {
        // save the current scroll position before we change the DOM and transition to the new page
        updateScrollPosition({ scrollX, scrollY });
      }
    }
  }
  return event;
}

// only update history entries that are managed by us
// leave other entries alone and do not accidentally add state.
export const updateScrollPosition = (positions: { scrollX: number; scrollY: number }) => {
  if (history.state) {
    history.scrollRestoration = "manual";
    safeReplaceState({ ...history.state, ...positions }, "");
  }
};

export async function doSwap(
  afterPreparation: BeforeEvent,
  viewTransition: ViewTransition,
  afterDispatch?: () => Promise<void>,
  beforeSwap?: (event: TransitionBeforeSwapEvent) => void,
) {
  const event = new TransitionBeforeSwapEvent(afterPreparation, viewTransition);
  document.dispatchEvent(event);
  if (afterDispatch) {
    await afterDispatch();
  }
  if (event.signal.aborted) {
    return { swapped: false as const, event };
  }
  // This callback and event.swap() deliberately form one synchronous commit
  // section. Once teardown starts, the navigation must finish even if an
  // observer aborts its signal from inside an overridden swap().
  beforeSwap?.(event);
  event.swap();
  return { swapped: true as const, event };
}
