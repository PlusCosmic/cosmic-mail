#!/usr/bin/env python3
"""Give the app-side render the image bytes the app itself would get.

chrome_render.mjs saves what Chrome was served, but Chrome advertises AVIF/WebP in its
Accept header and CDNs oblige; the app's fetch (`Accept: */*`, no User-Agent) usually gets
JPEG/PNG for the same URL. So every image Chrome got as WebP/AVIF/unknown, and every image
it showed whose bytes were not captured, is fetched again the way the app does, and the
manifests are pointed at those bytes. Run after chrome_render.mjs.

usage: images.py
"""
import collections, concurrent.futures, glob, hashlib, json, os, subprocess
from common import OUT

CACHE = os.path.join(OUT, "imgcache")


def sniff(b):
    if b[:4] == b"\x89PNG": return "png"
    if b[:2] == b"\xff\xd8": return "jpg"
    if b[:4] == b"GIF8": return "gif"
    if b[:4] == b"RIFF" and b[8:12] == b"WEBP": return "webp"
    if b[4:8] == b"ftyp": return "avif"
    if b"<svg" in b[:600].lower(): return "svg"
    return "bin"


def kind_of(file):
    # By content: servers label AVIF bodies as image/jpeg often enough.
    try:
        with open(file, "rb") as f:
            return sniff(f.read(600))
    except OSError:
        return "bin"


def fetch(url):
    try:
        cmd = ["curl", "-sL", "--max-time", "25", "-A", "", "-H", "Accept: */*", url]
        return url, subprocess.run(cmd, capture_output=True, timeout=40).stdout
    except Exception:
        return url, None


jobs = collections.defaultdict(list)  # url -> [(manifest, src, natural w, natural h)]
for dump in glob.glob(os.path.join(OUT, "*.chrome.json")):
    manifest = dump.replace(".chrome.json", ".manifest.json")
    have = json.load(open(manifest))["images"]
    for img in json.load(open(dump))["images"]:
        src = img.get("src") or ""
        if not src or src.startswith("data:") or img["broken"] or img["nw"] <= 2:
            continue
        file = have.get(src, {}).get("file", "")
        if file and (".app." in os.path.basename(file) or kind_of(file) not in ("webp", "avif", "bin")):
            continue
        if img["resolved"].startswith("http"):
            jobs[img["resolved"]].append((manifest, src, img["nw"], img["nh"]))

kinds, patches = collections.Counter(), collections.defaultdict(dict)
with concurrent.futures.ThreadPoolExecutor(8) as pool:
    for url, body in pool.map(fetch, list(jobs)):
        if not body:
            kinds["failed"] += 1
            continue
        kind = sniff(body)
        kinds[kind] += 1
        file = os.path.join(CACHE, hashlib.sha1(url.encode()).hexdigest() + ".app." + kind)
        with open(file, "wb") as f:
            f.write(body)
        for manifest, src, w, h in jobs[url]:
            patches[manifest][src] = {"file": file, "w": w, "h": h}
for manifest, patch in patches.items():
    m = json.load(open(manifest))
    m["images"].update(patch)
    json.dump(m, open(manifest, "w"))
print(f"{len(jobs)} images fetched as the app would: {dict(kinds)}; {len(patches)} manifests updated")
