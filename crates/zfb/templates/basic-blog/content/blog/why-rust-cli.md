---
title: Why a Rust CLI?
date: 2026-04-21
description: "The reasoning behind shipping zfb's developer tooling as a single Rust binary."
tags:
  - intro
  - tooling
---

The zfb CLI is written in Rust and shipped as a single static binary. That
choice is deliberate, and it is _not_ a religious one.

A site builder spends most of its wall-clock time on three things: file
system traversal, watching files for changes, and shelling out to small
helper processes. All three benefit from a runtime with predictable
startup, no install step, and no dependency on the user's local Node
version. A Rust binary gets all of that for free, and the user does not
need to think about which package manager pulled it down.

The flip side is the rendering pipeline itself, which is decidedly _not_
Rust. Pages render through a JavaScript runtime so authors can keep using
Preact, JSX, and the rest of the npm ecosystem. The split is intentional:
the orchestrator is Rust, the renderer is JS, and the seam between them
is small enough to keep honest.
