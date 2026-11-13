#!/usr/bin/env python3
"""Line layout under each document mode, through the app and Chrome.

usage: quirkcases.py [NN ...] [std,lq,q]     (build first: cargo build --example mail_render)

Runs small hand-written snippets (no mail) through lay.sh and chrome_lay.mjs, each under three
doctypes: `<!DOCTYPE html>` (std: no-quirks), XHTML 1.0 Transitional (lq: limited-quirks) and
none (q: quirks). Prints, per case and mode, every text line's y, every image's y+height and every
filled box's y+height, in both renderers, and each text line's x; OK when they agree within 1px
(x within 4px), else both rows. Exits 1 if any case differs.
"""
import os
import re
import subprocess
import sys

from tablecases import png

TOOLS = os.path.dirname(os.path.abspath(__file__))

DOCTYPES = {
    "std": "<!DOCTYPE html>",
    "lq": '<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" '
    '"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">',
    "q": "",
}

B = "One<br>Two<br>Three"
T = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore"
IMG = '<img src="%s">' % png(60, 20)
DIV = '<div style="width:300px;font-size:16px;line-height:16px">%s</div>'
CELL = '<table cellpadding="0" cellspacing="0"><tr><td bgcolor="#ddd">%s</td></tr></table>'
AFTER = '<div style="font-size:16px;line-height:16px">After</div>'

CASES = {
    "01 text directly in the block": DIV % T,
    "02 text only in a 12px span": DIV % ('<span style="font-size:12px">%s</span>' % T),
    "03 12px span, line-height normal": '<div style="width:300px"><span style="font-size:12px">%s</span></div>' % T,
    "04 span with padding-top": DIV % ('<span style="font-size:12px;padding-top:1px">%s</span>' % B),
    "05 span with a border": DIV % ('<span style="font-size:12px;border:1px solid #888">%s</span>' % B),
    "06 span with horizontal padding": DIV % ('<span style="font-size:12px;padding:0 4px">%s</span>' % B),
    "07 <br> lines in a span": '<div style="line-height:30px"><span style="font-size:12px">One<br>Two<br><br>Four</span></div>',
    "08 image alone in a cell": CELL % IMG + AFTER,
    "09 image alone, whitespace round it": CELL % ("\n  " + IMG + "\n  ") + AFTER,
    "10 image beside text": CELL % (IMG + " Text") + AFTER,
    "11 image in a span in a cell": CELL % ('<span style="font-size:12px">%s</span>' % IMG) + AFTER,
    "12 text, <br>, image (per line)": CELL % ("Label<br>" + IMG) + AFTER,
    "13 empty inline-block": '<div><span style="display:inline-block;width:20px;height:20px;background:#ccc"></span></div>'
    + AFTER,
    "14 list item, text in a span": '<ul style="margin:0;font-size:16px;line-height:16px"><li><span style="font-size:12px">%s</span></li></ul>'
    % T,
    "15 two sizes on one line": '<div style="font-size:16px"><span style="font-size:12px">small</span> <span style="font-size:24px">big</span></div>'
    + AFTER,
    "16 nested spans, text in the inner": DIV
    % ('<span style="font-size:24px;line-height:40px"><span style="font-size:12px">%s</span></span>' % T),
    "17 text in the block on line 1 only": '<div style="width:300px;font-size:16px;line-height:30px">A <span style="font-size:12px;line-height:12px">%s</span></div>'
    % T,
    "18 <font> in a cell": CELL % ('<font size="2" face="Arial">%s</font>' % T[:60]) + AFTER,
    "19 empty cell with whitespace": '<table cellpadding="0" cellspacing="0"><tr><td bgcolor="#ddd" width="50"> </td><td>X</td></tr></table>'
    + AFTER,
    "20 image block in a link in a cell": CELL % ('<a href="#">%s</a>' % IMG) + AFTER,
    "21 space between two images": CELL % (IMG + " " + IMG) + AFTER,
    "22 outer span with padding-left, text in inner": DIV
    % ('<span style="font-size:24px;line-height:40px;padding-left:4px"><span style="font-size:12px">%s</span></span>' % B),
    "23 table in a big centred bold block": '<div style="font-size:24px;line-height:40px;font-weight:bold;text-align:center">'
    '<table width="300"><tr><td>Table text</td></tr></table></div>' + AFTER,
    "24 table in <center>": '<center><table width="300"><tr><td>Centred?</td></tr></table></center>' + AFTER,
    "25 cell height with cellpadding": '<table cellpadding="10" cellspacing="0"><tr><td bgcolor="#ddd" height="50">X</td></tr></table>'
    + AFTER,
    "26 cell height below its content": '<table cellpadding="0" cellspacing="0"><tr><td bgcolor="#ddd" height="10">X<br>Y</td></tr></table>'
    + AFTER,
    "27 unitless length, hashless colour": '<div style="width:100;height:30;background-color:ff0000">X</div>' + AFTER,
    "28 class name in another case": "<style>.Big{font-size:30px}</style><div class=\"big\">Case</div>" + AFTER,
}


def body(case, mode):
    return DOCTYPES[mode] + '<html><body style="margin:0">' + CASES[case] + "</body></html>"


def rows(out):
    """'t y' per text line, 'i y+h' per image, 'b y+h' per filled box: comparable rows."""
    r = []
    # The app draws list bullets as text; Chrome's walk sees no ::marker text.
    for m in re.finditer(r"^text x=(-?\d+) y=(-?\d+)(?!.*'•')", out, re.M):
        r.append(("t", int(m.group(2)), 0, int(m.group(1))))
    for m in re.finditer(r"^img\s+x=-?\d+ y=(-?\d+) w=\d+ h=(\d+)", out, re.M):
        r.append(("i", int(m.group(1)), int(m.group(2)), 0))
    for m in re.finditer(r"^box\s+(?:\w+ )?x=-?\d+ y=(-?\d+) w=\d+ h=(\d+) fill=(?!-|None)", out, re.M):
        r.append(("b", int(m.group(1)), int(m.group(2)), 0))
    return r


def app(html):
    return rows(subprocess.run([os.path.join(TOOLS, "lay.sh"), html], capture_output=True, text=True).stdout)


def chrome(html):
    out = subprocess.run(["node", "chrome_lay.mjs", html], capture_output=True, text=True, cwd=TOOLS).stdout
    return rows(out)


def fmt(rs):
    return " ".join(
        k + str(y) + ("+%d" % h if h else "") + ("@x%d" % x if x else "")
        for k, y, h, x in sorted(rs, key=lambda r: (r[0], r[1]))
    )


def same(a, c):
    a, c = sorted(a), sorted(c)
    return len(a) == len(c) and all(
        k1 == k2 and abs(y1 - y2) <= 1 and abs(h1 - h2) <= 1 and abs(x1 - x2) <= 4
        for (k1, y1, h1, x1), (k2, y2, h2, x2) in zip(a, c)
    )


def main(args):
    modes = list(DOCTYPES)
    only = []
    for a in args:
        if "," in a or a in DOCTYPES:
            modes = a.split(",")
        else:
            only.append(a)
    bad = total = 0
    for name in (n for n in CASES if not only or n[:2] in only):
        for mode in modes:
            html = body(name, mode)
            a, c = app(html), chrome(html)
            total += 1
            ok = same(a, c)
            print("%s %-3s %s" % ("OK  " if ok else "DIFF", mode, name))
            if not ok:
                bad += 1
                print("         chrome: %s\n         app:    %s" % (fmt(c), fmt(a)))
    print("%d of %d differ" % (bad, total))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
