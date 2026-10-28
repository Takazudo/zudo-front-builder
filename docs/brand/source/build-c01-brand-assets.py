#!/usr/bin/env python3
"""Rebuild the C01 portable lockups, social card, and inspection sheet.

This is optional design tooling, not a project-build dependency. Run from any
directory with Python 3, fontTools, Pillow, and Inkscape installed. WOFF2 decoding
uses Python Brotli when available, or Node's built-in zlib Brotli decoder.

    python3 docs/brand/source/build-c01-brand-assets.py [--svg-only] [--preview-dir DIR]

--svg-only rewrites only the SVG outputs (no Inkscape needed). --preview-dir also
writes the lockup PNG previews and the inspection sheet to DIR, outside the repo.

The two approved C01 SVG inputs are read verbatim. This script never rewrites
them. Live official-site product text should remain live; only these portable
README/social-artwork exports contain outlined Jost 600 text. Edit the strings
and layout in brand-layout.json to regenerate them.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess
import types
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
import fontTools.ttLib.woff2 as woff2
from PIL import Image, ImageDraw, ImageFont


SOURCE = Path(__file__).resolve().parent
PUBLIC = SOURCE.parent.parent / "public"
BRAND = PUBLIC / "img/brand"
LAYOUT = json.loads((SOURCE / "brand-layout.json").read_text())
SVG_NS = "http://www.w3.org/2000/svg"


def decimal(value):
    return f"{value:.6f}".rstrip("0").rstrip(".")


def ensure_brotli_decoder():
    """Provide only the decompression callback FontTools needs for reading."""
    if woff2.haveBrotli:
        return "Python Brotli"
    node = shutil.which("node")
    if not node:
        raise RuntimeError("Install Brotli for Python or provide Node on PATH.")

    def decompress(data):
        command = (
            "const fs=require('node:fs'); const z=require('node:zlib'); "
            "process.stdout.write(z.brotliDecompressSync(fs.readFileSync(0)));"
        )
        return subprocess.run(
            [node, "-e", command], input=data, stdout=subprocess.PIPE,
            check=True,
        ).stdout

    woff2.brotli = types.SimpleNamespace(decompress=decompress)
    woff2.haveBrotli = True
    return "Node built-in zlib Brotli"


def kerning_lookups(font):
    if "GPOS" not in font:
        return []
    table = font["GPOS"].table
    indices = []
    for record in table.FeatureList.FeatureRecord:
        if record.FeatureTag == "kern":
            for index in record.Feature.LookupListIndex:
                if index not in indices:
                    indices.append(index)
    return [table.LookupList.Lookup[index] for index in indices]


def pair_values(subtable, first, second):
    """Resolve the standard GPOS pair-positioning used by Latin Jost."""
    coverage = subtable.Coverage.glyphs
    if first not in coverage:
        return None
    if subtable.Format == 1:
        pair_set = subtable.PairSet[coverage.index(first)]
        for pair in pair_set.PairValueRecord:
            if pair.SecondGlyph == second:
                return pair.Value1, pair.Value2
    elif subtable.Format == 2:
        class1 = subtable.ClassDef1.classDefs.get(first, 0)
        class2 = subtable.ClassDef2.classDefs.get(second, 0)
        pair = subtable.Class1Record[class1].Class2Record[class2]
        return pair.Value1, pair.Value2
    return None


def outline_text(font, text):
    cmap = font.getBestCmap()
    glyph_set = font.getGlyphSet()
    names = [cmap[ord(character)] for character in text]
    adjustments = [dict(XPlacement=0, YPlacement=0, XAdvance=0) for _ in names]
    for lookup in kerning_lookups(font):
        for index in range(len(names) - 1):
            for original in lookup.SubTable:
                lookup_type = lookup.LookupType
                subtable = original
                if lookup_type == 9:
                    lookup_type = original.ExtensionLookupType
                    subtable = original.ExtSubTable
                if lookup_type != 2:
                    continue
                pair = pair_values(subtable, names[index], names[index + 1])
                if pair is None:
                    continue
                for offset, value in enumerate(pair):
                    if value is not None:
                        for field in adjustments[index + offset]:
                            adjustments[index + offset][field] += getattr(value, field, 0)
                break

    svg_pen = SVGPathPen(glyph_set, ntos=decimal)
    bounds_pen = BoundsPen(glyph_set)
    cursor = 0
    for name, adjustment in zip(names, adjustments):
        transform = (
            1, 0, 0, 1,
            cursor + adjustment["XPlacement"], adjustment["YPlacement"],
        )
        glyph_set[name].draw(TransformPen(svg_pen, transform))
        glyph_set[name].draw(TransformPen(bounds_pen, transform))
        cursor += font["hmtx"][name][0] + adjustment["XAdvance"]
    return svg_pen.getCommands(), bounds_pen.bounds


def read_symbol(path):
    root = ET.fromstring(path.read_text())
    if root.attrib.get("viewBox") != "0 0 512 512":
        raise ValueError("Unexpected approved symbol viewBox")
    paths = root.findall(f"{{{SVG_NS}}}path")
    if len(paths) != 2:
        raise ValueError("C01 must contain exactly two source piece paths")
    return [(item.attrib["d"], item.attrib["fill"]) for item in paths]


def symbol_group(paths, size, x=0, y=0, group_id="c01-symbol"):
    return (
        f'<g id="{group_id}" transform="translate({decimal(x)} {decimal(y)}) '
        f'scale({decimal(size / 512)})">\n'
        + "\n".join(f'<path fill="{fill}" d="{path}"/>' for path, fill in paths)
        + "\n</g>"
    )


def outline_group(path, bounds, x, y, scale, fill, group_id, label):
    xmin, _, _, ymax = bounds
    return (
        f'<g id="{group_id}" aria-label="{escape(label)}" '
        f'transform="translate({decimal(x - xmin * scale)} '
        f'{decimal(y + ymax * scale)}) scale({decimal(scale)} '
        f'{decimal(-scale)})" fill="{fill}">'
        f'<path d="{path}"/></g>'
    )


def svg_document(width, height, title, contents, description):
    return (
        f'<svg xmlns="{SVG_NS}" width="{decimal(width)}" '
        f'height="{decimal(height)}" viewBox="0 0 {decimal(width)} {decimal(height)}" '
        f'role="img" aria-labelledby="title desc">\n'
        f'<title id="title">{escape(title)}</title>\n'
        f'<desc id="desc">{escape(description)}</desc>\n'
        + contents + "\n</svg>\n"
    )


def lockup(font, paths, fill):
    config = LAYOUT["lockup"]
    size = config["symbolSize"]
    word_height = config["wordmarkInkHeight"]
    path, bounds = outline_text(font, LAYOUT["productName"])
    xmin, ymin, xmax, ymax = bounds
    scale = word_height / (ymax - ymin)
    text_x = size + config["symbolTextGap"]
    width = math.ceil(text_x + (xmax - xmin) * scale)
    contents = symbol_group(paths, size)
    contents += "\n" + outline_group(
        path, bounds, text_x, (size - word_height) / 2,
        scale, fill, "jost-600-wordmark", LAYOUT["productName"],
    )
    return width, size, contents


def export_png(svg, output, width=None):
    inkscape = shutil.which("inkscape")
    if not inkscape:
        raise RuntimeError("Inkscape must be available on PATH for PNG export")
    command = [inkscape, str(svg), "--export-type=png", f"--export-filename={output}"]
    if width is not None:
        command.append(f"--export-width={width}")
    subprocess.run(command, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def make_inspection(black_png, white_png, social_png, output):
    sheet = Image.new("RGB", (960, 758), "#f0f0f0")
    draw = ImageDraw.Draw(sheet)
    label_font = ImageFont.truetype("DejaVuSans.ttf", 17)
    draw.text((32, 20), "C01 Twin Joint / portable brand exports", fill="#222222", font=label_font)
    draw.rounded_rectangle((32, 60, 464, 244), radius=10, fill="#ffffff")
    draw.rounded_rectangle((496, 60, 928, 244), radius=10, fill="#111111")
    for source, left in [(black_png, 32), (white_png, 496)]:
        logo = Image.open(source).convert("RGBA")
        logo.thumbnail((344, 124), Image.Resampling.LANCZOS)
        sheet.paste(logo, (left + (432 - logo.width) // 2, 60 + (184 - logo.height) // 2), logo)
    draw.text((32, 263), "Social card / 1200 × 630", fill="#333333", font=label_font)
    social = Image.open(social_png).convert("RGB").resize((864, 454), Image.Resampling.LANCZOS)
    sheet.paste(social, (48, 294))
    sheet.save(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--svg-only", action="store_true", help="skip Inkscape PNG exports")
    parser.add_argument("--preview-dir", type=Path, help="write review PNGs here")
    args = parser.parse_args()
    if args.svg_only and args.preview_dir:
        parser.error("--preview-dir needs PNG export; drop --svg-only")

    decoder = ensure_brotli_decoder()
    font_path = (SOURCE / LAYOUT["font"]).resolve()
    font = TTFont(font_path)
    if font["OS/2"].usWeightClass != 600:
        raise ValueError("The selected font must be Jost 600")
    if args.preview_dir:
        args.preview_dir.mkdir(parents=True, exist_ok=True)
    black_source = BRAND / "zfb-symbol-black.svg"
    white_source = BRAND / "zfb-symbol-white.svg"
    black = read_symbol(black_source)
    white = read_symbol(white_source)
    outputs = []

    # Relative to the lockups in docs/public/img/brand/.
    description = (
        "Approved C01 Twin Joint symbol with lowercase zfb in Jost SemiBold 600. "
        "Original symbol paths are unchanged; lettering is outlined for portability. "
        "The font is licensed under SIL OFL 1.1; see ../../fonts/OFL.txt."
    )
    for suffix, paths, fill in [("", black, "#000000"), ("-white", white, "#ffffff")]:
        width, height, contents = lockup(font, paths, fill)
        output = BRAND / f"zfb-lockup{suffix}.svg"
        output.write_text(svg_document(width, height, "zfb", contents, description))
        outputs.append(output)
        if args.preview_dir:
            export_png(output, args.preview_dir / f"zfb-lockup{suffix}.png", width=714)

    social = LAYOUT["social"]
    width, height, lockup_contents = lockup(font, black, social["ink"])
    scale = social["symbolSize"] / height
    social_x = (social["width"] - width * scale) / 2
    contents = (
        f'<rect id="background" width="{social["width"]}" height="{social["height"]}" '
        f'fill="{social["background"]}"/>\n'
        f'<g id="brand-lockup" transform="translate({decimal(social_x)} '
        f'{decimal(social["lockupTop"])}) scale({decimal(scale)})">\n'
        + lockup_contents + "\n</g>\n"
    )
    path, bounds = outline_text(font, LAYOUT["projectName"])
    text_scale = social["projectNameSize"] / font["head"].unitsPerEm
    text_width = (bounds[2] - bounds[0]) * text_scale
    contents += outline_group(
        path, bounds, (social["width"] - text_width) / 2,
        social["projectNameTop"], text_scale,
        social["secondaryInk"], "project-name", LAYOUT["projectName"],
    )
    social_svg = SOURCE / "ogp.svg"
    social_svg.write_text(svg_document(
        social["width"], social["height"], "zfb — zudo-front-builder", contents,
        "Editable vector social composition. C01 Twin Joint and outlined Jost 600 lettering. "
        "White background; no slogan. Edit brand-layout.json to change text or spacing.",
    ))
    outputs.append(social_svg)
    if not args.svg_only:
        social_png = PUBLIC / "img/ogp.png"
        export_png(social_svg, social_png)
        # The social image has a deliberate opaque white background.
        with Image.open(social_png) as rendered:
            rendered.convert("RGB").save(social_png)
        outputs.append(social_png)
        if args.preview_dir:
            make_inspection(
                args.preview_dir / "zfb-lockup.png", args.preview_dir / "zfb-lockup-white.png",
                social_png, args.preview_dir / "c01-brand-exports-inspection.png",
            )
    print(json.dumps({
        "font": str(font_path), "fontWeight": font["OS/2"].usWeightClass,
        "woff2Decoder": decoder,
        "inputSymbolSha256": {
            item.name: hashlib.sha256(item.read_bytes()).hexdigest()
            for item in [black_source, white_source]
        },
        "outputs": [str(item) for item in outputs],
    }, indent=2))


if __name__ == "__main__":
    main()
