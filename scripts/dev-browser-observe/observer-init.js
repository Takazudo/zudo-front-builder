// dev-browser-observe v1 observer. Installed with context.addInitScript so it
// runs in every document, including after a native full reload. Read-only
// with respect to the page DOM: it only reads, keeps its own state in
// sessionStorage, and reports through Playwright bindings.
(function () {
  "use strict";

  var STORAGE_KEY = "__dbo_expect_v1";
  var BINDINGS = { notify: "__dboNotify", docInit: "__dboDocInit", ping: "__dboPing" };

  function has(obj, key) {
    return Object.prototype.hasOwnProperty.call(obj, key);
  }

  function createPredicateEngine() {
    var STRING_OPS = ["equals", "notEquals", "includes", "excludes"];
    var KINDS = ["text", "attribute", "count", "absent", "computed-style", "document-uuid-not"];

    function stringOp(pred) {
      for (var i = 0; i < STRING_OPS.length; i++)
        if (has(pred, STRING_OPS[i])) return STRING_OPS[i];
      return null;
    }

    function matchString(actual, pred) {
      if (actual === null || actual === undefined) return false;
      var s = String(actual);
      switch (stringOp(pred)) {
        case "equals":
          return s === pred.equals;
        case "notEquals":
          return s !== pred.notEquals;
        case "includes":
          return s.indexOf(pred.includes) !== -1;
        case "excludes":
          return s.indexOf(pred.excludes) === -1;
        default:
          return false;
      }
    }

    function validate(pred) {
      var errors = [];
      if (!pred || typeof pred !== "object") return ["predicate must be an object"];
      if (KINDS.indexOf(pred.kind) === -1) errors.push("unknown kind: " + pred.kind);
      if (pred.kind !== "document-uuid-not" && typeof pred.selector !== "string")
        errors.push("selector required");
      if (
        pred.kind === "text" ||
        pred.kind === "computed-style" ||
        (pred.kind === "attribute" && !pred.present)
      ) {
        if (stringOp(pred) === null)
          errors.push("one of equals/notEquals/includes/excludes required");
      }
      if (pred.kind === "attribute" && typeof pred.name !== "string")
        errors.push("attribute name required");
      if (pred.kind === "computed-style" && typeof pred.property !== "string")
        errors.push("property required");
      if (pred.kind === "count" && typeof pred.equals !== "number")
        errors.push("count equals must be a number");
      if (pred.kind === "document-uuid-not" && typeof pred.value !== "string")
        errors.push("value required");
      return errors;
    }

    // env: { querySelectorAll(selector) -> array-like, getComputedStyle(el) -> { getPropertyValue }, docUUID }
    function evaluate(pred, env) {
      var errors = validate(pred);
      if (errors.length) return { matched: false, error: errors.join("; ") };
      if (pred.kind === "document-uuid-not")
        return { matched: env.docUUID !== pred.value, actual: env.docUUID };
      var els = env.querySelectorAll(pred.selector);
      var n = els ? els.length : 0;
      if (pred.kind === "count") return { matched: n === pred.equals, actual: n };
      if (pred.kind === "absent") return { matched: n === 0, actual: n };
      if (n === 0) return { matched: false, actual: null, reason: "no-element" };
      var el = els[0];
      if (pred.kind === "text") {
        var text =
          el.textContent === null || el.textContent === undefined ? "" : String(el.textContent);
        if (pred.trim !== false) text = text.trim();
        return { matched: matchString(text, pred), actual: text };
      }
      if (pred.kind === "attribute") {
        var value = el.getAttribute(pred.name);
        if (pred.present === true && stringOp(pred) === null)
          return { matched: value !== null, actual: value };
        return { matched: value !== null && matchString(value, pred), actual: value };
      }
      var cs = env.getComputedStyle(el).getPropertyValue(pred.property);
      var v = cs === null || cs === undefined ? "" : String(cs).trim();
      return { matched: matchString(v, pred), actual: v };
    }

    return { evaluate: evaluate, validate: validate, kinds: KINDS.slice() };
  }

  var engine = createPredicateEngine();
  globalThis.__dboPredicateEngine = engine;

  if (typeof window === "undefined" || typeof document === "undefined") return;
  if (window.top !== window) return;
  if (window.__dbo) return;

  function uuid() {
    if (window.crypto && typeof window.crypto.randomUUID === "function")
      return window.crypto.randomUUID();
    var b = new Uint8Array(16);
    window.crypto.getRandomValues(b);
    return Array.prototype.map
      .call(b, function (x) {
        return (x + 256).toString(16).slice(1);
      })
      .join("");
  }

  var navEntry = performance.getEntriesByType && performance.getEntriesByType("navigation")[0];
  var docUUID = uuid();
  var navigationType = navEntry ? navEntry.type : "unknown";
  var outbox = [];
  var confirming = null;
  // Token whose notification was sent from THIS document and not yet
  // acknowledged. `delivered` is persisted only on the controller's ack, so a
  // document replaced mid-delivery leaves the expectation armed for the next one.
  var inFlight = null;
  var rafScheduled = false;
  var checkQueued = false;

  function env() {
    return {
      docUUID: docUUID,
      querySelectorAll: function (s) {
        return document.querySelectorAll(s);
      },
      getComputedStyle: function (el) {
        return window.getComputedStyle(el);
      },
    };
  }

  function readArmed() {
    try {
      var raw = window.sessionStorage.getItem(STORAGE_KEY);
      return raw ? JSON.parse(raw) : null;
    } catch (e) {
      return null;
    }
  }

  function writeArmed(value) {
    if (value === null) window.sessionStorage.removeItem(STORAGE_KEY);
    else window.sessionStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  }

  function flush() {
    while (outbox.length) {
      var item = outbox[0];
      var fn = window[item.name];
      if (typeof fn !== "function") {
        requestAnimationFrame(flush);
        return;
      }
      outbox.shift();
      var ret;
      try {
        ret = fn(item.payload);
      } catch (e) {
        ret = null;
        if (item.onFail) item.onFail();
      }
      if (ret && typeof ret.then === "function") ret.then(item.onAck || null, item.onFail || null);
    }
  }

  function send(name, payload, onAck, onFail) {
    outbox.push({ name: name, payload: payload, onAck: onAck, onFail: onFail });
    flush();
  }

  function scheduleFrameLoop() {
    if (rafScheduled) return;
    rafScheduled = true;
    requestAnimationFrame(function () {
      rafScheduled = false;
      check();
      var armed = readArmed();
      if (armed && !armed.delivered) scheduleFrameLoop();
    });
  }

  function queueCheck() {
    if (checkQueued) return;
    checkQueued = true;
    Promise.resolve().then(function () {
      checkQueued = false;
      check();
    });
  }

  function check() {
    var armed = readArmed();
    if (!armed || armed.delivered) return;
    if (inFlight === armed.token) return;
    if (!document.documentElement) return;
    var r = engine.evaluate(armed.predicate, env());
    if (r.matched) confirm(armed.token);
  }

  function confirm(token) {
    if (confirming === token) return;
    confirming = token;
    requestAnimationFrame(function () {
      requestAnimationFrame(function () {
        confirming = null;
        var cur = readArmed();
        if (!cur || cur.token !== token || cur.delivered) return;
        var r = engine.evaluate(cur.predicate, env());
        if (!r.matched) {
          scheduleFrameLoop();
          return;
        }
        inFlight = token;
        var active = document.activeElement;
        send(
          BINDINGS.notify,
          {
            token: token,
            docUUID: docUUID,
            navigationType: navigationType,
            armedDocUUID: cur.armedDocUUID,
            url: location.href,
            scroll: { x: window.scrollX, y: window.scrollY },
            activeElementId: active && active.id ? active.id : null,
            activeElementTag: active ? active.tagName : null,
            actual: r.actual === undefined ? null : r.actual,
          },
          function () {
            var now = readArmed();
            if (now && now.token === token) {
              now.delivered = true;
              writeArmed(now);
            }
          },
          function () {
            if (inFlight === token) inFlight = null;
            scheduleFrameLoop();
          },
        );
      });
    });
  }

  new MutationObserver(function () {
    var armed = readArmed();
    if (armed && !armed.delivered) queueCheck();
  }).observe(document, { subtree: true, childList: true, characterData: true, attributes: true });

  window.__dbo = {
    version: "dev-browser-observe.observer.v1",
    state: function () {
      return {
        docUUID: docUUID,
        navigationType: navigationType,
        url: location.href,
        armed: readArmed(),
      };
    },
    evaluate: function (predicate) {
      return engine.evaluate(predicate, env());
    },
    arm: function (token, predicate) {
      var errors = engine.validate(predicate);
      if (errors.length) return { armed: false, error: errors.join("; "), docUUID: docUUID };
      var pre = engine.evaluate(predicate, env());
      if (pre.matched)
        return {
          armed: false,
          precondition: "true-before-write",
          actual: pre.actual === undefined ? null : pre.actual,
          docUUID: docUUID,
        };
      writeArmed({ token: token, predicate: predicate, armedDocUUID: docUUID, delivered: false });
      scheduleFrameLoop();
      return {
        armed: true,
        docUUID: docUUID,
        navigationType: navigationType,
        actual: pre.actual === undefined ? null : pre.actual,
      };
    },
    disarm: function () {
      var prev = readArmed();
      writeArmed(null);
      return prev;
    },
    ping: function (id) {
      send(BINDINGS.ping, { id: id, mode: "direct", docUUID: docUUID });
      return true;
    },
    pingAfterTwoRaf: function (id) {
      requestAnimationFrame(function () {
        requestAnimationFrame(function () {
          send(BINDINGS.ping, { id: id, mode: "two-raf", docUUID: docUUID });
        });
      });
      return true;
    },
  };

  send(BINDINGS.docInit, {
    docUUID: docUUID,
    navigationType: navigationType,
    url: location.href,
    armedToken: (readArmed() || {}).token || null,
  });

  var pending = readArmed();
  if (pending && !pending.delivered) scheduleFrameLoop();
})();
