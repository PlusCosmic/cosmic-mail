#!/usr/bin/env python3
"""Corpus totals from the per-mail comparisons: how many mails (and senders) show each kind of
difference, the CSS features and legacy attributes in use, and OUT/summary.tsv (one row per mail).

usage: aggregate.py [OUT_DIR]        (default: the reference dir)
"""
import json,glob,os,sys,collections
from common import CORPUS, OUT
out=sys.argv[1] if len(sys.argv)>1 else OUT
idx=json.load(open(os.path.join(os.environ.get("CORPUS", CORPUS),"index.json")))
rows=[]
for f in glob.glob(out+"/*.cmp.json"):
    r=json.load(open(f)); m=idx.get(str(r["id"]),{}); r["from"]=m.get("from_addr","?"); r["subject"]=m.get("subject","")[:50]; rows.append(r)
rows.sort(key=lambda r:int(r["id"]))
N=len(rows); print("mails",N)
def cnt(name,pred):
    hit=[r for r in rows if pred(r)]
    senders=collections.Counter(r["from"].split("@")[-1] for r in hit)
    print(f"{name:55s} {len(hit):4d} mails  {len(senders):3d} senders   e.g. {' '.join(str(r['id']) for r in hit[:12])}")
    return hit
cnt("height ratio <0.9 (app shorter)",lambda r:r["height"]["ratio"]<0.9)
cnt("height ratio <0.5",lambda r:r["height"]["ratio"]<0.5)
cnt("app height <=2px (collapsed page)",lambda r:r["height"]["app"]<=2)
cnt("height ratio >1.1 (app taller)",lambda r:r["height"]["ratio"]>1.1)
cnt("height ratio >1.5",lambda r:r["height"]["ratio"]>1.5)
cnt("height within 2%",lambda r:0.98<=r["height"]["ratio"]<=1.02)
cnt("fit-scaled (content wider than pane)",lambda r:r["width"]["fitScale"]<0.999)
cnt("  chrome also overflows",lambda r:r["width"]["chrome"]>714)
cnt("  app overflows, chrome does not",lambda r:r["width"]["fitScale"]<0.999 and r["width"]["chrome"]<=714)
cnt("missing text >=3 words",lambda r:r["text"]["missingInApp"]>=3)
cnt("missing text >=20 words",lambda r:r["text"]["missingInApp"]>=20)
cnt("extra text >=3 words",lambda r:r["text"]["extraInApp"]>=3)
cnt("  extra text that chrome hides >=3",lambda r:r["text"]["extraThatChromeHides"]>=3)
cnt("no text matched at all but chrome has words",lambda r:r["text"]["matched"]==0 and r["counts"]["chromeWords"]>0)
cnt("x offset lines >=3",lambda r:r["dx"]["off"]>=3)
cnt("x offset lines >=25% of lines",lambda r:r["dx"]["lines"]>0 and r["dx"]["off"]/r["dx"]["lines"]>=.25)
cnt("dy jumps >=3",lambda r:r["dy"]["jumps"]>=3)
cnt("wrap diffs >=3",lambda r:r["wrapDiffs"]>=3)
cnt("font size off >=3 words",lambda r:r["style"]["fontSize"]["off"]>=3)
cnt("weight off >=3 words",lambda r:r["style"]["weight"]["off"]>=3)
cnt("color off >=3 words",lambda r:r["style"]["color"]["off"]>=3)
cnt("underline off >=3 words",lambda r:r["style"]["underline"]["off"]>=3)
cnt("italic off",lambda r:r["style"]["italic"]["off"]>=1)
cnt("strike-through text in chrome",lambda r:r["style"]["strikeInChrome"]>=1)
cnt("uppercase lost",lambda r:r["style"]["uppercaseLost"]["off"]>=1)
cnt("capitalize lost",lambda r:r["style"]["capitalizeLost"]>=1)
cnt("letter-spaced words",lambda r:r["style"]["letterSpacedWords"]>=1)
cnt("images missing in app",lambda r:r["images"]["missingN"]>=1)
cnt("images size off",lambda r:r["images"]["sizeOffN"]>=1)
cnt("images undecodable",lambda r:r["images"]["undecodableN"]>=1)
fm=collections.Counter(); 
for r in rows:
    for k,v in (r["images"].get("formats") or {}).items(): fm[k]+=v
print("image formats fetched:",dict(fm))
errs=collections.Counter()
for r in rows:
    for e in r["images"]["decodeErrors"]: errs[e.split(":")[0]]+=1
print("mails with decode errors by format:",dict(errs))
feat=collections.Counter(); farea=collections.Counter()
for r in rows:
    for k,v in r["features"].items(): feat[k]+=1; farea[k]+=v["area"]
print("\nCSS features (mails using):")
for k,v in feat.most_common(): print(f"  {k:32s} {v:4d}")
att=collections.Counter()
for r in rows:
    for k in r["attrs"]: att[k]+=1
print("\nattrs (mails using):", dict(att.most_common(30)))
print("compat:",collections.Counter(r["compat"] for r in rows))
print("pixelDiff >0.15:",len([r for r in rows if r.get("pixelDiff",0)>0.15]), " >0.08:",len([r for r in rows if r.get("pixelDiff",0)>0.08]), " <0.03:",len([r for r in rows if r.get("pixelDiff",1)<0.03]))
# table
with open(out+"/summary.tsv","w") as f:
    f.write("id\tfrom\tsubject\tcompat\thA\thC\tratio\tfit\tmiss\textra\textraHid\tdxOff\tlines\tjumps\twrap\tsize\tweight\tcolor\tunder\timgMiss\timgSize\timgUndec\tpix\tpages\n")
    for r in rows:
        f.write("\t".join(str(x) for x in [r["id"],r["from"],r["subject"].replace("\t"," "),r["compat"][:4],r["height"]["app"],r["height"]["chrome"],r["height"]["ratio"],r["width"]["fitScale"],r["text"]["missingInApp"],r["text"]["extraInApp"],r["text"]["extraThatChromeHides"],r["dx"]["off"],r["dx"]["lines"],r["dy"]["jumps"],r["wrapDiffs"],r["style"]["fontSize"]["off"],r["style"]["weight"]["off"],r["style"]["color"]["off"],r["style"]["underline"]["off"],r["images"]["missingN"],r["images"]["sizeOffN"],r["images"]["undecodableN"],r.get("pixelDiff"),r.get("pages")])+"\n")
