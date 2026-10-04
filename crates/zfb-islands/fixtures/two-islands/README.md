# two-islands fixture

A minimal page that places two distinct `"use client"` components inside SDK
`Island` boundaries. Used by
the `zfb-islands` integration tests to verify the scanner + manifest
pipeline (sub-task 3 of epic #53):

- The scanner walks `pages/home.tsx`, resolves both boundary children, and
  finds two islands.
- The manifest reorders by component name and emits the
  `ComponentName → resolved.tsx` JSON contract that the
  islands-bundling-shim topic consumes.
