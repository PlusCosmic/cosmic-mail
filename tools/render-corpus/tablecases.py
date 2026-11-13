#!/usr/bin/env python3
"""Table column widths through the app and Chrome: each cell's x and width in both.

usage: tablecases.py [NN ...]      (build first: cargo build --example mail_render)

Runs small hand-written tables (no mail) through lay.sh and chrome_lay.mjs. Prints OK when every
cell is within 2px of Chrome's, else both renderers' "x+width" per cell, and exits 1 if any case
differs. Every cell has a bgcolor: the app side reads the filled boxes, Chrome's the <td>s.
"""
import base64
import os
import re
import struct
import subprocess
import sys
import zlib

TOOLS = os.path.dirname(os.path.abspath(__file__))


def png(w, h):
    """A valid w×h grey PNG as a data: URI (the app decodes it; Chrome sizes by it)."""

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))

    raw = b"".join(b"\x00" + b"\x99" * w for _ in range(h))
    body = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )
    return "data:image/png;base64," + base64.b64encode(body).decode()


L = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore"
COLOURS = ["#fdd", "#dfd", "#ddf", "#ffd", "#dff", "#fdf", "#eee", "#ccc"]
IMG = '<img src="%s" style="display:block">' % png(300, 20)


def cells(*specs):
    """One <td> per (attributes, contents), each its own colour."""
    return "".join(
        '<td bgcolor="%s" %s>%s</td>' % (COLOURS[i % len(COLOURS)], attrs, text)
        for i, (attrs, text) in enumerate(specs)
    )


def table(attrs, *rows):
    return '<body style="margin:0"><table cellpadding="0" cellspacing="0" %s>%s</table>' % (
        attrs,
        "".join("<tr>%s</tr>" % r for r in rows),
    )


NESTED_100 = '<table width="100%" cellpadding="0" cellspacing="0"><tr><td>' + IMG + "</td></tr></table>"
HALVES = (
    '<body style="margin:0"><div style="display:table;width:100%">'
    + "".join(
        '<div style="display:table-cell;width:50%%"><table cellpadding="0" cellspacing="0"><tr>'
        '<td bgcolor="%s" style="width:600px">%s</td></tr></table></div>' % (c, t)
        for c, t in (("#fdd", "A"), ("#dfd", "B"))
    )
    + "</div>"
)
FIXED = 'width="600" style="table-layout:fixed"'

CASES = {
    "01 auto cols, short vs long text": table('width="600"', cells(("", "Short"), ("", L))),
    "02 auto cols, two long texts 1:3": table('width="600"', cells(("", L[:40]), ("", L + " " + L))),
    "03 50%/50% unequal content": table('width="600"', cells(('width="50%"', "Short"), ('width="50%"', L))),
    "04 35% + auto": table('width="600"', cells(('width="35%"', "Short"), ("", L))),
    "05 px widths sum < table (100+100 in 600)": table('width="600"', cells(('width="100"', "A"), ('width="100"', "B"))),
    "06 px widths sum > table (400+400 in 600)": table('width="600"', cells(('width="400"', "A"), ('width="400"', "B"))),
    "07 px width + auto (200 + auto)": table('width="600"', cells(('width="200"', "A"), ("", L))),
    "08 width smaller than content (100, 300px img)": table('width="600"', cells(('width="100"', IMG), ("", "B"))),
    "09 widths on 2nd row only": table('width="600"', cells(("", "A"), ("", "B")), cells(('width="150"', "C"), ("", "D"))),
    "10 colspan header row, widths below": table(
        'width="600"', '<td colspan="2" bgcolor="#eee">Header</td>', cells(('width="150"', "C"), ("", L))
    ),
    "11 colspan=2 width=600 header, cells below": table(
        'width="600"', '<td colspan="2" width="600" bgcolor="#eee">Header</td>', cells(("", "C"), ("", L))
    ),
    "12 no table width (shrink to fit)": table("", cells(("", "Short"), ("", "A bit longer text"))),
    "13 100% + nowrap (label + price)": table('width="600"', cells(('width="100%"', "Item name"), ("nowrap", "&pound;12.99"))),
    "14 24px, 100%, auto (header row)": table(
        'width="600"', cells(('style="width:24px"', "Status"), ('style="width:100%"', "Job"), ("", "Annotations"))
    ),
    "15 nested 100% table in width=1 cell": table('width="600"', cells(('width="1"', NESTED_100), ("", "B"))),
    "16 fixed layout, 30% + padding": table(FIXED, cells(('style="width:30%;padding-right:60px"', "A"), ("", "B"))),
    "17 3 auto cols, unequal text": table('width="600"', cells(("", "One"), ("", "Two words here"), ("", L[:60]))),
    "18 percent widths sum < 100 (20%+30%+auto)": table(
        'width="600"', cells(('width="20%"', "A"), ('width="30%"', "B"), ("", "C"))
    ),
    "19 100% table, 4 auto cols": table(
        'width="100%"', cells(("", "Ordered"), ("", "Dispatched"), ("", "Out for delivery"), ("", "Delivered"))
    ),
    "20 cell min-width / max-width": table('width="600"', cells(('style="min-width:300px"', "A"), ('style="max-width:50px"', L))),
    "21 fixed layout, widths sum < table, no auto col": table(FIXED, cells(('width="100"', "A"), ('width="200"', "B"))),
    "22 px cell wider than its 100% table allows": table('width="100%"', cells(('width="900"', L), ("", "B"))),
    "23 two 600px cells in 50% display:table-cell halves": HALVES,
}


def boxes(pattern, out):
    return [(float(m.group(1)), float(m.group(2))) for m in re.finditer(pattern, out, re.M)]


def app(html):
    out = subprocess.run([os.path.join(TOOLS, "lay.sh"), html], capture_output=True, text=True).stdout
    return boxes(r"^box\s+x=(-?[\d.]+) y=-?[\d.]+ w=([\d.]+)", out)


def chrome(html):
    out = subprocess.run(["node", "chrome_lay.mjs", html], capture_output=True, text=True, cwd=TOOLS).stdout
    return boxes(r"^box\s+td x=(-?[\d.]+) y=-?[\d.]+ w=([\d.]+)", out)


def fmt(cells):
    return " ".join("%d+%d" % (x, w) for x, w in cells)


def main(only):
    run = [name for name in CASES if not only or name[:2] in only]
    bad = 0
    for name in run:
        a, c = app(CASES[name]), chrome(CASES[name])
        ok = len(a) == len(c) and all(abs(x1 - x2) <= 2 and abs(w1 - w2) <= 2 for (x1, w1), (x2, w2) in zip(a, c))
        print(("OK   " if ok else "DIFF ") + name)
        if not ok:
            bad += 1
            print("       chrome: %s\n       app:    %s" % (fmt(c), fmt(a)))
    print("%d of %d differ" % (bad, len(run)))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
