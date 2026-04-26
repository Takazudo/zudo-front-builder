---
title: SSR and Islands
date: 2026-04-22
description: How zfb plans to render pages on the server and hydrate only what needs to be interactive.
tags:
  - framework
  - ssr
---

zfb pages are server-rendered by default. That means every route in
`pages/` is turned into a real HTML file at build time, with the markup
already in place when the browser receives it. Search engines, RSS
readers, and link previews all see the same content the user does.

For interactive bits — a counter, a theme toggle, a search box — zfb uses
the **island** pattern. A component file marked `"use client"` becomes a
hydration boundary: the server renders its initial HTML, and the client
loads just enough JavaScript to wake up that one widget. Everything else
on the page stays static.

This trade is the whole point of the architecture. You get the SEO and
performance of a static site, plus the interactivity of a SPA exactly
where you ask for it, and nowhere else. The `<ThemeToggle />` in this
example is the smallest possible demonstration of the pattern.
