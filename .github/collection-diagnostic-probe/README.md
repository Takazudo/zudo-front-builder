# Temporary collection diagnosis

Source issue: #3823; task #3852; epic #3851.

Pinned product/test source:09c3c0e812f95ee271f8f9a14c6c8723bef105c9.

This branch is evidence-only and must close unmerged. It executes the exact source-faithful three-case fixture with fail-fast disabled and retries0, then applies the retained logging-only patch and repeats all cases. Both JUnit files must contain the same three unskipped cases. A failing reproduction leaves this workflow red; captured logs are evidence, not a passing verification claim. No product fix is included.
