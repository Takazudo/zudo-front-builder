"""Build the C01 symbol and favicon SVGs from the editable numeric geometry.

Python stdlib only. Run from any directory:

    python3 docs/brand/source/build-c01-svg.py

Outputs are written into docs/public/ (see docs/brand/README.md).
"""
from pathlib import Path
import json
import math

SOURCE = Path(__file__).resolve().parent
PUBLIC = SOURCE.parent.parent / 'public'
G = json.loads((SOURCE / 'c01-geometry.json').read_text())
POINTS = [tuple(p) for p in G['seamCenterline']]

def cross(a, b):
    return a[0] * b[1] - a[1] * b[0]

def offset_line(points, distance):
    segments = []
    for a, b in zip(points, points[1:]):
        v = (b[0] - a[0], b[1] - a[1])
        length = math.hypot(*v)
        n = (-v[1] / length, v[0] / length)
        segments.append(((a[0] + n[0] * distance, a[1] + n[1] * distance), v, n))
    out = [segments[0][0]]
    for previous, following in zip(segments, segments[1:]):
        p, r, _ = previous
        q, s, _ = following
        delta = (q[0] - p[0], q[1] - p[1])
        t = cross(delta, s) / cross(r, s)
        out.append((p[0] + t * r[0], p[1] + t * r[1]))
    n = segments[-1][2]
    out.append((points[-1][0] + n[0] * distance, points[-1][1] + n[1] * distance))
    return out

def number(n):
    return f'{n:.6f}'.rstrip('0').rstrip('.')

def line(points):
    return ' '.join(('M' if i == 0 else 'L') + number(x) + ' ' + number(y)
                    for i, (x, y) in enumerate(points))

def paths(width, points=POINTS):
    left = offset_line(points, width / 2)
    right = offset_line(points, -width / 2)
    radius = G['outerRadius']
    edge = G['viewBox'][2]
    left_path = line(left) + f' H{radius} A{radius} {radius} 0 0 1 0 {edge-radius} V{radius} A{radius} {radius} 0 0 1 {radius} 0 Z'
    right_path = line(right) + f' H{edge-radius} A{radius} {radius} 0 0 0 {edge} {edge-radius} V{radius} A{radius} {radius} 0 0 0 {edge-radius} 0 Z'
    return left_path, right_path

def svg(width, fills=('currentColor', 'currentColor'), adaptive=False, points=POINTS):
    pieces = paths(width, points)
    style = '<style>:root{color:#111111}@media(prefers-color-scheme:dark){:root{color:#ffffff}}</style>\n' if adaptive else ''
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" fill="none">\n'
            '<title>ZFB C01 Twin Joint</title>\n' + style +
            '\n'.join(f'<path fill="{fill}" d="{path}"/>' for path, fill in zip(pieces, fills)) + '\n</svg>\n')

def main():
    brand = PUBLIC / 'img/brand'
    brand.mkdir(parents=True, exist_ok=True)
    colors = G['colors']
    standard = svg(G['standardSeamWidth'])
    small_points = G['smallSeamCenterline']
    outputs = {
        brand / 'zfb-symbol.svg': standard,
        # The home hero mask reads the standard master through settings.logo.
        PUBLIC / 'img/logo.svg': standard,
        brand / 'zfb-symbol-small.svg': svg(G['smallSeamWidth'], points=small_points),
        brand / 'zfb-symbol-black.svg': svg(G['standardSeamWidth'], (colors['black'],) * 2),
        brand / 'zfb-symbol-white.svg': svg(G['standardSeamWidth'], (colors['white'],) * 2),
        PUBLIC / 'favicon.svg': svg(G['smallSeamWidth'], adaptive=True, points=small_points),
    }
    for path, text in outputs.items():
        path.write_text(text)
    print(f'Generated {len(outputs)} SVGs from c01-geometry.json')

if __name__ == '__main__':
    main()
