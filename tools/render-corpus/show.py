#!/usr/bin/env python3
"""The comparison of one or more mails, readably: heights, missing/extra phrases, where the
vertical offset jumps (where the layouts diverge), x-offset examples, image mismatches.

usage: show.py OUT_DIR id [id ...]
"""
import json,sys
out=sys.argv[1]
for i in sys.argv[2:]:
    r=json.load(open(f"{out}/{i}.cmp.json"))
    print("== %s h %s/%s (%.2f) fit %s cw %s/%s miss %d extra %d(hid %d) dx %d/%d jumps %d wrap %d"%(i,r["height"]["app"],r["height"]["chrome"],r["height"]["ratio"],r["width"]["fitScale"],r["width"]["appContent"],r["width"]["chrome"],r["text"]["missingInApp"],r["text"]["extraInApp"],r["text"]["extraThatChromeHides"],r["dx"]["off"],r["dx"]["lines"],r["dy"]["jumps"],r["wrapDiffs"]))
    for p in r["text"]["missingPhrases"][:3]: print("   MISSING y=%d n=%d %r"%(p["y"],p["n"],p["text"][:80]))
    for p in r["text"]["extraPhrases"][:2]: print("   EXTRA   y=%d n=%d %r"%(p["y"],p["n"],p["text"][:80]))
    for h in r["text"]["chromeHidden"][:1]: print("   chrome hides:",str(h)[:100])
    for e in r["dx"]["eg"][:3]: print("   dx",e)
    for e in r["dy"]["biggestJumps"][:4]: print("   jump",e)
    im=r["images"]; 
    if im["missingInApp"]: print("   img miss",im["missingInApp"][:2])
    if im["sizeOff"]: print("   img sizeOff",im["sizeOff"][:3])
    if im["undecodable"]: print("   img undec",[(u["format"],u["w"],u["h"],u["appW"],u["appH"]) for u in im["undecodable"][:3]])
