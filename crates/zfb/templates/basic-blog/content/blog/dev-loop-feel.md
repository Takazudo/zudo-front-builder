---
title: The Dev Loop Should Feel Instant
date: 2026-04-24
description: "Notes on why zfb's dev server cares so much about cold-start and reload latency."
tags:
  - tooling
  - perf
---

Most of a developer's day inside a site builder is spent in the inner
loop: edit a file, glance at the browser, edit again. If that loop has
even half a second of friction in it, the friction compounds across
hundreds of edits a day and the whole experience starts to feel laggy.

zfb treats the dev loop as a first-class product surface. The CLI starts
the dev server in well under a second on a cold cache. File changes are
detected by a native watcher rather than a polling loop, so the round
trip from save to live-reload is dominated by the browser, not by the
build tool. The orchestrator stays out of the way.

This is the other reason the orchestrator is a Rust binary. Every
millisecond of startup tax shows up in the inner loop, and a binary that
the OS can map straight into memory has approximately zero of that tax.
The renderer is allowed to be slow and clever; the orchestrator has to
be fast and dumb.
