# Changelog

## 0.1.0-next.4

Scaffolded projects now pin to the exact CLI version (`=<ver>` instead of `^<ver>`) in the generated `package.json`. This is a meaningful behavior change: previously `npm create zfb@latest` could silently resolve a compatible stable release once `0.1.0` lands; the exact pin prevents that. See #343.

## 0.1.0-next.1

Initial public prerelease on npm.

- npm-create scaffolding entry point for zfb: `npm create zfb@latest my-site` resolves this package and delegates to `zfb new my-site`, bootstrapping a new project from the built-in `basic-blog` template.
