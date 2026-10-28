# zfb brand: C01 Twin Joint

The zfb mark is one rounded square assembled from two fitted pieces. Two
dovetail tails run left to right: narrow at the root, flared at the head, and
held in matching sockets. The seam is transparent negative space, so every
symbol SVG works as a CSS mask and as a fixed-color image.

The brand name is lowercase `zfb`. The full project name stays
`zudo-front-builder`. Product-name text is Jost SemiBold 600.

This is the identity of the official docs site and this repository. Do not add
it to `create-zfb` templates, examples or generated downstream projects.

## Standard and compact variants

| Variant | Seam | Use |
| --- | --- | --- |
| Standard | 8/512 units | Hero (`img/logo.svg`), README lockups, social card, large standalone marks |
| Compact | 32/512 units, head tips at y=220 and y=302 instead of 238 and 284 | Header icon and favicons (16–32 px) |

The standard seam disappears at header and favicon sizes. The compact variant
widens it to 1 px at 16 px and pulls back the two inward-facing head tips, so
the receiving piece stays one connected shape. The outer square (radius 48),
neck positions, tail direction and tail count are the same in both. All
numbers live in [`source/c01-geometry.json`](source/c01-geometry.json).

## Files

All paths are under `docs/public/`.

| File | Variant | Contents |
| --- | --- | --- |
| `img/logo.svg` | Standard | `currentColor`; home hero mask via `logo` in `zfb.config.ts` |
| `img/brand/zfb-symbol.svg` | Standard | `currentColor` master (same bytes as `img/logo.svg`) |
| `img/brand/zfb-symbol-small.svg` | Compact | `currentColor`; header mask via `headerBrand.icon` |
| `img/brand/zfb-symbol-black.svg` | Standard | Fixed `#000000` |
| `img/brand/zfb-symbol-white.svg` | Standard | Fixed `#ffffff` |
| `img/brand/zfb-lockup.svg` | Standard | Black mark and outlined `zfb`, transparent; README light mode |
| `img/brand/zfb-lockup-white.svg` | Standard | White mark and outlined `zfb`, transparent; README dark mode |
| `img/ogp.png` | Standard | 1200 × 630 social card, opaque white |
| `favicon.svg` | Compact | `#111111`, or `#ffffff` under `prefers-color-scheme: dark` |
| `favicon.ico`, `favicon-16x16.png`, `favicon-32x32.png` | Compact | Black mark on a white backing (ICO frames 16/32/48) |
| `fonts/jost-600.woff2`, `fonts/OFL.txt` | — | Brand font and its license |

Colors: monochrome `#000000` / `#ffffff`. The favicon SVG uses `#111111` on
light backgrounds. The white backing belongs to the raster favicons only; never
add a background or a painted seam to a mask SVG.

Live site text is never outlined. Only the README lockups and the social card
contain outlined lettering.

## Rebuilding

Normal docs builds use the committed files and need none of these tools.

`source/build-c01-svg.py` (Python standard library) rewrites the five symbol
SVGs and `favicon.svg` from `c01-geometry.json`:

```sh
python3 docs/brand/source/build-c01-svg.py
```

`source/build-c01-brand-assets.py` rewrites the lockups, the editable social
composition (`source/ogp.svg`) and `img/ogp.png`. Labels and spacing come from
`source/brand-layout.json`. It needs fontTools and Pillow, plus Inkscape for
PNG export. WOFF2 decoding uses Python Brotli or, failing that, Node's
built-in zlib.

```sh
python3 docs/brand/source/build-c01-brand-assets.py                  # SVGs and img/ogp.png
python3 docs/brand/source/build-c01-brand-assets.py --svg-only       # no Inkscape
python3 docs/brand/source/build-c01-brand-assets.py --preview-dir /tmp/zfb-brand  # plus review PNGs
```

Both scripts reproduce the committed files byte for byte. The raster favicons
have no generator here: re-export them from `favicon.svg`'s compact paths in
black on white at 16, 32 and 48 px, and pack the three PNGs into
`favicon.ico`.

## Font provenance and license

`jost-600.woff2` is the Jost SemiBold file from the zudo-doc branding rollout
([zudolab/zudo-doc#4635](https://github.com/zudolab/zudo-doc/pull/4635),
`public/fonts/` at `6f72ad36e7e8caf39050aa8d8e57e9f013620605`).

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `jost-600.woff2` | 19,900 | `fa418d4c46615b4e07e2011eed91a3e399f88bf67bd8bc39e3fee14a04e442b5` |
| `OFL.txt` | 4,385 | `b9c2ddb5e698b4287241ab26190c8e4c5e3a395d486ac80e96e8f979b44ed625` |

Jost is Copyright 2020 The Jost Project Authors, licensed under the SIL Open
Font License 1.1. Keep `OFL.txt` next to the font, unmodified.

The site loads the font through `src/styles/global.css` as `"ZFB Brand Jost"`.
Its `url("../fonts/jost-600.woff2")` is relative to the emitted
`assets/styles*.css`, so it works at `/` and under a subpath `base`.

## Design notes

C01 was chosen over a single-joint candidate partly to stay clear of a mark
used by a Microsoft developer tool. That was a design decision, not a
trademark clearance.
