Captured 2026-10-07 with:

- `curl -fsSL -H 'Accept: application/vnd.npm.install-v1+json' https://registry.npmjs.org/tailwindcss -o /tmp/zfb-3897-packument.json`; kept the real envelope and selected versions, with only `name`, `version`, `dist.integrity`, `dist.tarball`, and `dist.shasum` fields.
- `curl -fsSL -H 'Accept: application/vnd.github+json' https://api.github.com/repos/tailwindlabs/tailwindcss/git/ref/tags/v4.3.3 -o tag-ref.json`.

Annotated tag and error responses in tests are synthesized from the GitHub REST Git reference schema because the sampled upstream tag is a lightweight commit ref.
