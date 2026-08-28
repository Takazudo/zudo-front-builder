const MARK = "globalThis.__zfbBackRaceMark?.";

function replaceOnce(source, before, after, label) {
  const first = source.indexOf(before);
  if (first === -1) throw new Error(`back-race diagnostics: missing ${label} anchor`);
  if (source.indexOf(before, first + before.length) !== -1) {
    throw new Error(`back-race diagnostics: ambiguous ${label} anchor`);
  }
  return source.slice(0, first) + after + source.slice(first + before.length);
}

export function instrumentBackRaceRouter(source) {
  let instrumented = source;

  instrumented = replaceOnce(
    instrumented,
    `const finishAbortedUpdate = () => {\n        notifyNavigationAborted(currentNavigation);`,
    `const finishAbortedUpdate = () => {\n        notifyNavigationAborted(currentNavigation, "finishAbortedUpdate");`,
    "finishAbortedUpdate notify call",
  );
  instrumented = replaceOnce(
    instrumented,
    `notifyNavigationAborted(previousNavigation);`,
    `notifyNavigationAborted(previousNavigation, "abortAndRecreateMostRecentNavigation");`,
    "abortAndRecreateMostRecentNavigation notify call",
  );
  instrumented = replaceOnce(
    instrumented,
    `function notifyNavigationAborted(navigation) {\n    // From island teardown onward the navigation is committed and must finish,\n    // even if an overridden event.swap() synchronously starts a newer one.\n    if (navigation.domCommitStarted || navigation.abortEventEmitted)\n        return;`,
    `function notifyNavigationAborted(navigation, diagnosticCallSite = "other") {\n    // From island teardown onward the navigation is committed and must finish,\n    // even if an overridden event.swap() synchronously starts a newer one.\n    const diagnosticOutcome = navigation.domCommitStarted\n        ? "early-return:domCommitStarted"\n        : navigation.abortEventEmitted\n          ? "early-return:abortEventEmitted"\n          : "emit";\n    ${MARK}("notify-navigation-aborted", {\n        callSite: diagnosticCallSite,\n        outcome: diagnosticOutcome,\n        domCommitStarted: navigation.domCommitStarted,\n        abortEventEmitted: navigation.abortEventEmitted,\n    });\n    if (navigation.domCommitStarted || navigation.abortEventEmitted)\n        return;`,
    "notifyNavigationAborted decision",
  );
  instrumented = replaceOnce(
    instrumented,
    `safePushState({ ...options.state, index: ++currentHistoryIndex, scrollX: 0, scrollY: 0 }, "", prepEvent.to.href);`,
    `safePushState({ ...options.state, index: ++currentHistoryIndex, scrollX: 0, scrollY: 0 }, "", prepEvent.to.href);\n            ${MARK}("early-history-commit", { historyIndex: currentHistoryIndex });`,
    "early safePushState commit",
  );
  instrumented = replaceOnce(
    instrumented,
    `currentTransition.viewTransition = document.startViewTransition(async () => {\n            domUpdateOutcome = await updateDOM`,
    `currentTransition.viewTransition = document.startViewTransition(async () => {\n            ${MARK}("native-view-transition-callback-entry");\n            domUpdateOutcome = await updateDOM`,
    "native startViewTransition callback",
  );
  instrumented = replaceOnce(
    instrumented,
    `currentNavigation.domCommitStarted = true;\n        cancelPendingIslands();`,
    `currentNavigation.domCommitStarted = true;\n        ${MARK}("dom-commit-started");\n        cancelPendingIslands();`,
    "domCommitStarted assignment",
  );
  instrumented = replaceOnce(
    instrumented,
    `function onPopState(ev) {\n    if (!transitionEnabledOnThisPage() && ev.state) {`,
    `function onPopState(ev) {\n    ${MARK}("router-popstate-entry", { stateIndex: ev.state?.index ?? null });\n    if (!transitionEnabledOnThisPage() && ev.state) {`,
    "router popstate entry",
  );

  return instrumented;
}
