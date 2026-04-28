# two-islands fixture

A minimal page that imports two distinct `"use client"` components. Used by
the `zfb-islands` integration tests to verify the scanner + manifest
pipeline (sub-task 3 of epic #53):

- The scanner walks `pages/home.tsx`, traverses both imports, and finds
  two islands.
- The manifest reorders by component name and emits the
  `ComponentName → resolved.tsx` JSON contract that the
  islands-bundling-shim topic consumes.
