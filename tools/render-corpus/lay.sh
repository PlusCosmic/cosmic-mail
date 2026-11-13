#!/bin/bash
# The app's display list for an HTML snippet: one line per box, image and line of text, with
# positions, at the reader's width. Compare with `node chrome_lay.mjs` on the same snippet.
#
# usage: lay.sh '<html…>' | @file.html        (build first: cargo build --example mail_render)
TOOLS="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$TOOLS/../.." && pwd)"
T=$(mktemp -d)
case "$1" in @*) cp "${1#@}" "$T/m.html" ;; *) printf '%s' "$1" > "$T/m.html" ;; esac
"${CARGO_TARGET_DIR:-$REPO/target}/debug/examples/mail_render" "$T/m.html" /nonexistent "$T/m" > "$T/log" 2>&1
grep -h -A1 panicked "$T/log"
python3 - "$T/m.app.json" <<'P'
import json,sys
d=json.load(open(sys.argv[1]))
print("height",d["height"],"content_width",d["content_width"],"bg",d["background"])
for i in d["items"]:
    if i["t"]=="text": print("text x=%.0f y=%.0f w=%.0f lh=%.1f size=%.1f %r %s"%(i["x"],i["y"],i["w"],i["lh"],i["size"],i["text"][:60], [(s["weight"],s["color"][:3],"u" if s["underline"] else "") for s in i["spans"]][:2]))
    elif i["t"]=="img": print("img  x=%.0f y=%.0f w=%.0f h=%.0f %s"%(i["x"],i["y"],i["w"],i["h"],i["src"][:40]))
    elif i["t"]=="box": print("box  x=%.0f y=%.0f w=%.0f h=%.0f fill=%s border=%s r=%s"%(i["x"],i["y"],i["w"],i["h"],i["fill"],i["border"],i["radius"]))
    else: print("other", i.get("debug","")[:150])
P
rm -rf "$T"
