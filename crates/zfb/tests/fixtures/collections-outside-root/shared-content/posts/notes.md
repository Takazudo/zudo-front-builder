---
title: Should Be Filtered Out By Include Glob
---

This entry has a `.md` extension. The collection's `include: ["**/*.mdx"]`
glob does not match it, so it must be excluded from both the content
snapshot and the shadow content tree even though `.md` is otherwise a
recognised collection-entry extension.
