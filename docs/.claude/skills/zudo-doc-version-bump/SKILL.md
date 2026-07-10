---
name: zudo-doc-version-bump
description: >-
  Deprecated release helper. Do not run a release from this skill; redirect version bump and
  release requests to /l-make-release.
user-invocable: true
disable-model-invocation: true
argument-description: Ignored. Use /l-make-release instead.
---

# /zudo-doc-version-bump

This skill is retired.

Do not use it to bump versions, generate changelog docs, commit, tag, push, create GitHub releases, or publish packages. The script and workflow this document used to describe are no longer the source of truth and conflict with the current release process.

When a user asks for a version bump, changelog entry, or release:

1. Stop this skill.
2. Tell the user to invoke `/l-make-release`.
3. Do not infer or run any release command from this file.
