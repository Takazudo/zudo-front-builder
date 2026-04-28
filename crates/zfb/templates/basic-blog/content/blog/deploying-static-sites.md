---
title: Deploying Static Sites
date: 2026-04-23
description: Why a folder of HTML is still the most boring and reliable way to ship a site.
tags:
  - deploy
  - perf
---

A zfb build emits a directory of static HTML, JavaScript, and assets.
That output is intentionally boring: there is no server runtime to
operate, no Lambda to wire up, and no edge worker to debug. You can
upload the `dist/` folder to any host that knows how to serve files
and the site will work.

That boringness is also where most of the perf wins come from. A static
HTML response is the fastest possible thing a CDN can do. There is no
cold start, no template engine, and no database round-trip. The browser
gets bytes, parses them, and paints. The only JavaScript that runs is
the tiny island bundle for whatever components asked to be interactive.

The practical upshot is that you can host a zfb site on Cloudflare
Pages, Netlify, GitHub Pages, an S3 bucket, or even a USB stick plugged
into a router, and the experience is the same. Pick whichever one is
cheapest and stop thinking about it.
