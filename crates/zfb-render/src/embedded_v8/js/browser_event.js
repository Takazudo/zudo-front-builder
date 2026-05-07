// Minimal browser Event / CustomEvent / EventTarget polyfills for the
// embedded V8 host.
//
// Why this exists
// ---------------
// The just-shipped client-router surface in `@takazudo/zfb-runtime`
// (`packages/zfb-runtime/src/client-router/events.ts`) declares three
// classes at module top level that extend the browser Event global:
//
//   class BeforeEvent extends Event { ... }
//   export class TransitionBeforePreparationEvent extends BeforeEvent { ... }
//   export class TransitionBeforeSwapEvent extends BeforeEvent { ... }
//
// `extends Event` is evaluated at module-load time. When a consumer
// bundle that imports the client-router surface is loaded by the
// embedded V8 host for SSG `paths()` evaluation, the bundle aborts at
// load time with:
//
//   ReferenceError: Event is not defined
//
// Surfaced by zudolab/zudo-doc#1521 (W5A pin bump). Upstream tests run
// in jsdom (which has Event); the embedded V8 host has no DOM and no
// browser globals, so this stub fills the gap.
//
// Scope
// -----
// SSG `paths()` evaluation walks module top level only — it never
// dispatches events, never touches an EventTarget instance method, and
// never reads back from a constructed Event. The stubs only need to be
// shaped enough that:
//
//   - `class X extends Event` is structurally valid (Event is a
//     constructable class with a prototype).
//   - `super(type, init)` from a subclass succeeds and produces an
//     object with the spec-shaped fields the subclass may read off
//     `this` (defaultPrevented, type, etc.).
//   - `new CustomEvent(type, { detail })` survives if any path
//     constructs one (none currently does at module top level, but
//     consumers may add more).
//
// We deliberately do NOT implement event propagation, listener
// management, or capture/bubble semantics — those would never fire on
// the SSG path, and a buggy implementation would mask real bugs.
//
// If the SSG path ever does need real dispatch (e.g. a future
// build-time-rendering surface that simulates a navigation), expand
// the EventTarget stub with a real listener registry. Until then, the
// methods are no-ops.

(function installBrowserEventGlobals(globalThis) {
  class Event {
    constructor(type, eventInitDict) {
      this.type = String(type ?? "");
      const init = eventInitDict || {};
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.composed = !!init.composed;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.isTrusted = false;
      this.timeStamp = 0;
    }
    preventDefault() {
      if (this.cancelable) {
        this.defaultPrevented = true;
      }
    }
    stopPropagation() {}
    stopImmediatePropagation() {}
    composedPath() {
      return [];
    }
  }
  // WHATWG-defined constants — some libraries probe these.
  Event.NONE = 0;
  Event.CAPTURING_PHASE = 1;
  Event.AT_TARGET = 2;
  Event.BUBBLING_PHASE = 3;

  class CustomEvent extends Event {
    constructor(type, eventInitDict) {
      super(type, eventInitDict);
      const init = eventInitDict || {};
      this.detail = init.detail === undefined ? null : init.detail;
    }
  }

  // Minimal EventTarget. Listeners are registered into an internal
  // map but never invoked — `dispatchEvent` is a no-op that returns
  // true (i.e. "default not prevented"). See header for why we don't
  // implement real dispatch.
  class EventTarget {
    constructor() {
      this._zfbListeners = new Map();
    }
    addEventListener(type, _listener, _options) {
      const t = String(type);
      if (!this._zfbListeners.has(t)) {
        this._zfbListeners.set(t, []);
      }
    }
    removeEventListener(_type, _listener, _options) {}
    dispatchEvent(event) {
      // Match the spec return contract: true unless preventDefault was
      // called on a cancelable event. We never invoke listeners, so
      // defaultPrevented stays whatever the caller set on the event.
      return !(event && event.defaultPrevented);
    }
  }

  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.EventTarget = EventTarget;
})(globalThis);
