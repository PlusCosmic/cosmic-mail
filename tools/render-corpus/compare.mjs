// Compare the app's render of each mail with Chrome's: structural metrics from the two
// dumps, a pixel score, and side-by-side pages for looking at.
//
// usage: node compare.mjs OUT_DIR [--no-images] [id ...]
// writes OUT_DIR/<id>.cmp.json, OUT_DIR/<id>.cmp.<n>.png (app left, Chrome right)
//
// What the numbers mean is in README.md. One known artefact: text inside a closed <details>
// is measured as visible in Chrome, so it counts as "missing" in the app, which is right to
// hide it.
import fs from 'node:fs';
import path from 'node:path';
import sharp from 'sharp';
import { diffArrays } from 'diff';

const args = process.argv.slice(2);
const out = args[0];
const noImages = args.includes('--no-images');
const only = args.slice(1).filter((a) => !a.startsWith('--'));
const PAGE_H = 1500;

const clean = (s) => s.replace(/[­​-‏‪-‮⁠-⁯﻿]/g, '');
const norm = (s) => clean(s).normalize('NFKC').toLowerCase();
const rgb = (c) => {
  if (Array.isArray(c)) return c;
  const m = String(c).match(/[\d.]+/g) || [0, 0, 0];
  return [Number(m[0]), Number(m[1]), Number(m[2]), m[3] === undefined ? 255 : Math.round(Number(m[3]) * 255)];
};
const cdist = (a, b) => Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) + Math.abs(a[2] - b[2]);
const hex = (c) => '#' + c.slice(0, 3).map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');
const median = (xs) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)] : 0);
const pct = (xs, p) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor(xs.length * p))] : 0);
const r1 = (n) => Math.round(n * 10) / 10;

function appWords(app) {
  const words = [];
  app.items.forEach((it, idx) => {
    if (it.t !== 'text') return;
    const marker = /^(?:[•◦▪▫‣·]|\d+[.)]|[a-z][.)])$/i.test(it.text.trim());
    const re = /\S+/g;
    let m;
    let first = true;
    // byte offsets -> the text is UTF-8 in spans; map char index to byte index
    const enc = new TextEncoder();
    while ((m = re.exec(it.text))) {
      const t = clean(m[0]);
      if (!t) continue;
      const byte = enc.encode(it.text.slice(0, m.index)).length;
      const span = it.spans.find((s) => byte >= s.r[0] && byte < s.r[1]) || it.spans[0] || {};
      words.push({
        t, n: norm(t), item: idx, first, marker, x: it.x, y: it.y, lh: it.lh, size: it.size, w: it.w,
        frac: it.text.length ? m.index / it.text.length : 0,
        weight: span.weight ?? 400, color: span.color || [0, 0, 0, 255], underline: !!span.underline,
        italic: !!span.italic, mono: !!span.mono, link: !!span.link,
      });
      first = false;
    }
  });
  return words;
}

function phrases(list, max = 8) {
  // consecutive words -> phrases
  const outp = [];
  let cur = null;
  for (const w of list) {
    if (cur && w.i === cur.last + 1) {
      cur.words.push(w.t);
      cur.last = w.i;
    } else {
      cur = { words: [w.t], last: w.i, y: Math.round(w.y) };
      outp.push(cur);
    }
  }
  return outp
    .sort((a, b) => b.words.length - a.words.length)
    .slice(0, max)
    .map((p) => ({ y: p.y, n: p.words.length, text: p.words.join(' ').slice(0, 140) }));
}

function compareOne(id) {
  const app = JSON.parse(fs.readFileSync(path.join(out, `${id}.app.json`), 'utf8'));
  const chr = JSON.parse(fs.readFileSync(path.join(out, `${id}.chrome.json`), 'utf8'));
  const res = { id, compat: chr.compatMode };
  res.height = { app: Math.round(app.height), chrome: chr.height, ratio: r1((app.height / Math.max(chr.height, 1)) * 100) / 100 };
  res.width = { app: app.width, appContent: Math.round(app.content_width), chrome: chr.width, fitScale: Math.round(app.fit_scale * 100) / 100 };
  res.background = { app: app.background ? hex(app.background) : null, chromeBody: chr.bodyBg, chromeHtml: chr.htmlBg };
  res.counts = {
    appItems: app.items.length,
    appText: app.items.filter((i) => i.t === 'text').length,
    appBoxes: app.items.filter((i) => i.t === 'box').length,
    appImgs: app.items.filter((i) => i.t === 'img').length,
    chromeWords: chr.words.length,
  };

  // --- text ---
  const aw = appWords(app);
  const cw = chr.words.map((w, i) => ({ ...w, i, t: clean(w.t), n: norm(w.t) })).filter((w) => w.n);
  aw.forEach((w, i) => (w.i = i));
  const d = diffArrays(cw.map((w) => w.n), aw.map((w) => w.n));
  let ci = 0, ai = 0;
  const pairs = [];
  const missing = [], extra = [];
  // A replaced block is often the same characters split into words differently (adjacent inline
  // elements with no space between them are two words in the DOM, one in an app line): walk
  // both sides by characters and pair the words that start together.
  const replaced = (rem, add) => {
    let i = 0, j = 0, ri = 0, aj = 0; // word index and char offset within it, per side
    const usedR = new Set(), usedA = new Set();
    while (i < rem.length && j < add.length) {
      const a = rem[i].n.slice(ri), b = add[j].n.slice(aj);
      const n = Math.min(a.length, b.length);
      if (a.slice(0, n) !== b.slice(0, n)) break;
      if (ri === 0 && aj === 0) pairs.push([rem[i], add[j]]);
      usedR.add(i); usedA.add(j);
      ri += n; aj += n;
      if (ri === rem[i].n.length) { i++; ri = 0; }
      if (aj === add[j].n.length) { j++; aj = 0; }
    }
    const clean = ri === 0 && aj === 0;
    rem.forEach((w, k) => { if (!usedR.has(k) || (!clean && k === i)) missing.push(w); });
    add.forEach((w, k) => { if (!usedA.has(k) || (!clean && k === j)) extra.push(w); });
  };
  for (let p = 0; p < d.length; p++) {
    const part = d[p], next = d[p + 1];
    if ((part.added && next && next.removed) || (part.removed && next && next.added)) {
      const remN = part.removed ? part.value.length : next.value.length;
      const addN = part.added ? part.value.length : next.value.length;
      replaced(cw.slice(ci, ci + remN), aw.slice(ai, ai + addN));
      ci += remN; ai += addN; p++;
    } else if (part.added) {
      for (let k = 0; k < part.value.length; k++) extra.push(aw[ai++]);
    } else if (part.removed) {
      for (let k = 0; k < part.value.length; k++) missing.push(cw[ci++]);
    } else {
      for (let k = 0; k < part.value.length; k++) pairs.push([cw[ci++], aw[ai++]]);
    }
  }
  pairs.sort((a, b) => a[0].i - b[0].i);
  const extraReal = extra.filter((w) => !w.marker);
  const hiddenNorm = norm(chr.hiddenText.map((h) => h.text).join(' '));
  const extraHidden = extraReal.filter((w) => hiddenNorm.includes(w.n));
  res.text = {
    matched: pairs.length,
    missingInApp: missing.length,
    extraInApp: extraReal.length,
    extraThatChromeHides: extraHidden.length,
    missingPhrases: phrases(missing),
    extraPhrases: phrases(extraReal),
    chromeHidden: chr.hiddenText.slice(0, 6),
  };

  // vertical drift down the page and where it jumps
  const dys = pairs.map(([c, a]) => a.y + a.lh / 2 - (c.y + c.h / 2));
  const jumps = [];
  for (let k = 1; k < pairs.length; k++) {
    const j = dys[k] - dys[k - 1];
    if (Math.abs(j) > 6) jumps.push({ by: r1(j), atChromeY: Math.round(pairs[k][0].y), before: pairs[k - 1][0].t, after: pairs[k][0].t });
  }
  res.dy = {
    median: r1(median(dys)), p10: r1(pct(dys, 0.1)), p90: r1(pct(dys, 0.9)), end: r1(dys[dys.length - 1] || 0),
    maxAbs: r1(Math.max(0, ...dys.map(Math.abs))), jumps: jumps.length,
    biggestJumps: jumps.sort((a, b) => Math.abs(b.by) - Math.abs(a.by)).slice(0, 6),
  };
  // horizontal: the first word of each app line against the same word in Chrome
  const firsts = pairs.filter(([, a]) => a.first);
  const dxs = firsts.map(([c, a]) => a.x - c.x);
  const offX = firsts.filter(([c, a]) => Math.abs(a.x - c.x) > 6);
  res.dx = {
    lines: firsts.length, median: r1(median(dxs)), off: offX.length,
    eg: offX.slice(0, 6).map(([c, a]) => ({ word: c.t, appX: Math.round(a.x), chromeX: Math.round(c.x), chromeY: Math.round(c.y) })),
  };
  // lines that wrap differently: words that share an app line but not a Chrome line, or vice versa
  let wrapDiff = 0;
  for (let k = 1; k < pairs.length; k++) {
    const sameApp = pairs[k][1].item === pairs[k - 1][1].item;
    const sameChr = Math.abs(pairs[k][0].y + pairs[k][0].h / 2 - (pairs[k - 1][0].y + pairs[k - 1][0].h / 2)) < 3 && pairs[k][0].x >= pairs[k - 1][0].x;
    const newAppLine = !sameApp && Math.abs(pairs[k][1].y + pairs[k][1].lh / 2 - (pairs[k - 1][1].y + pairs[k - 1][1].lh / 2)) > 3;
    if ((sameApp && !sameChr) || (newAppLine && sameChr)) wrapDiff++;
  }
  res.wrapDiffs = wrapDiff;

  const eg = (list, f) => list.slice(0, 5).map(([c, a]) => ({ word: c.t, y: Math.round(c.y), ...f(c, a) }));
  const sizeOff = pairs.filter(([c, a]) => Math.abs(a.size - c.fs) > 0.6);
  const weightOff = pairs.filter(([c, a]) => (a.weight >= 600) !== (c.fw >= 600));
  const colorOff = pairs.filter(([c, a]) => cdist(rgb(c.color), a.color) > 48);
  const underOff = pairs.filter(([c, a]) => /underline/.test(c.deco) !== a.underline);
  const italicOff = pairs.filter(([c, a]) => c.italic !== a.italic);
  const monoOff = pairs.filter(([c, a]) => c.mono !== a.mono);
  const strike = pairs.filter(([c]) => /line-through/.test(c.deco));
  const upper = pairs.filter(([c, a]) => c.tt === 'uppercase' && a.t !== a.t.toUpperCase());
  const capit = pairs.filter(([c, a]) => c.tt === 'capitalize' && a.first && a.t[0] !== a.t[0].toUpperCase());
  const spaced = pairs.filter(([c]) => c.ls !== 0);
  const lineH = pairs.filter(([c, a]) => a.first).filter(([c, a]) => Math.abs(a.lh - c.h) > 3);
  res.style = {
    fontSize: { off: sizeOff.length, eg: eg(sizeOff, (c, a) => ({ app: a.size, chrome: c.fs })) },
    weight: { off: weightOff.length, eg: eg(weightOff, (c, a) => ({ app: a.weight, chrome: c.fw })) },
    color: { off: colorOff.length, eg: eg(colorOff, (c, a) => ({ app: hex(a.color), chrome: hex(rgb(c.color)) })) },
    underline: { off: underOff.length, eg: eg(underOff, (c, a) => ({ app: a.underline, chrome: c.deco })) },
    italic: { off: italicOff.length }, mono: { off: monoOff.length },
    strikeInChrome: strike.length,
    uppercaseLost: { off: upper.length, eg: eg(upper, (c, a) => ({ app: a.t })) },
    capitalizeLost: capit.length,
    letterSpacedWords: spaced.length,
  };

  // --- images ---
  const visible = chr.images.filter((i) => !i.hidden && i.w > 2 && i.h > 2);
  const appImgs = app.items.filter((i) => i.t === 'img');
  const bySrc = {};
  for (const i of appImgs) (bySrc[i.src] ||= []).push(i);
  const short = (s) => (s && s.length > 300 ? s.slice(0, 120) + '…' : s);
  const decode = {};
  for (const i of app.images) decode[i.src] = i;
  const imgRes = { chromeVisible: visible.length, chromeBroken: visible.filter((i) => i.broken).length, app: appImgs.length, missingInApp: [], sizeOff: [], undecodable: [], formats: chr.formats };
  for (const ci2 of visible) {
    if (ci2.broken) continue;
    const key = short(ci2.src);
    const a = (bySrc[key] || []).shift();
    const name = (ci2.src || '').startsWith('data:') ? 'data:' + (ci2.src || '').slice(5, 30) : (ci2.src || '').split('?')[0].split('/').slice(-1)[0].slice(0, 50);
    if (!a) {
      imgRes.missingInApp.push({ name, y: Math.round(ci2.y), w: ci2.w, h: ci2.h });
      continue;
    }
    const dec = decode[key];
    if (dec && dec.error) imgRes.undecodable.push({ name, format: dec.format, w: ci2.w, h: ci2.h, y: Math.round(ci2.y), appW: Math.round(a.w), appH: Math.round(a.h) });
    else if (Math.abs(a.w - ci2.w) > 3 || Math.abs(a.h - ci2.h) > 3) imgRes.sizeOff.push({ name, y: Math.round(ci2.y), chrome: [ci2.w, ci2.h], app: [Math.round(a.w), Math.round(a.h)], natural: [ci2.nw, ci2.nh], objectFit: ci2.objectFit });
  }
  imgRes.missingN = imgRes.missingInApp.length;
  imgRes.sizeOffN = imgRes.sizeOff.length;
  imgRes.undecodableN = imgRes.undecodable.length;
  imgRes.missingInApp = imgRes.missingInApp.slice(0, 6);
  imgRes.sizeOff = imgRes.sizeOff.slice(0, 6);
  imgRes.undecodable = imgRes.undecodable.slice(0, 6);
  imgRes.decodeErrors = [...new Set(app.images.filter((i) => i.error).map((i) => `${i.format}: ${String(i.error).slice(0, 80)}`))].slice(0, 4);
  res.images = imgRes;

  res.features = Object.fromEntries(Object.entries(chr.feat).map(([k, v]) => [k, { n: v.n, area: v.area, eg: v.eg.slice(0, 2) }]));
  res.attrs = chr.attrs;
  res.mediaRules = chr.media;
  return res;
}

async function pixels(id, res) {
  const appPng = path.join(out, `${id}.app.png`);
  const chrPng = path.join(out, `${id}.chrome.png`);
  const am = await sharp(appPng).metadata();
  const cm = await sharp(chrPng).metadata();
  res.png = { app: [am.width, am.height], chrome: [cm.width, cm.height] };
  // pixel score: both at 1/4 scale, grey; rows only one has count as fully different
  const W = 178;
  const load = async (file, meta, cropW) => {
    const w = Math.min(meta.width, cropW);
    const h = Math.max(1, Math.round((meta.height * W) / w));
    const buf = await sharp(file).extract({ left: 0, top: 0, width: w, height: meta.height }).resize(W, h, { fit: 'fill' }).flatten({ background: '#fff' }).greyscale().raw().toBuffer();
    return { buf, h };
  };
  const a = await load(appPng, am, 712);
  const c = await load(chrPng, cm, 712);
  const H = Math.max(a.h, c.h), h = Math.min(a.h, c.h);
  let sum = 0;
  for (let i = 0; i < W * h; i++) sum += Math.abs(a.buf[i] - c.buf[i]);
  sum += (H - h) * W * 128;
  res.pixelDiff = Math.round((sum / (W * H * 255)) * 1000) / 1000;

  const gap = 16, colW = 728, label = 26;
  const total = Math.max(am.height, cm.height);
  const pages = Math.min(Math.ceil(total / PAGE_H), 8);
  res.pages = pages;
  res.truncatedPages = Math.ceil(total / PAGE_H) > 8;
  if (noImages) return;
  // side-by-side pages: app | chrome
  for (let p = 0; p < pages; p++) {
    const top = p * PAGE_H;
    const comps = [];
    for (const [file, meta, left] of [[appPng, am, 0], [chrPng, cm, colW + gap]]) {
      if (top >= meta.height) continue;
      const hh = Math.min(PAGE_H, meta.height - top);
      const ww = Math.min(meta.width, colW);
      const buf = await sharp(file).extract({ left: 0, top, width: ww, height: hh }).png().toBuffer();
      comps.push({ input: buf, left, top: label });
    }
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${colW * 2 + gap}" height="${label}"><rect width="100%" height="100%" fill="#222"/><text x="8" y="18" font-family="Helvetica" font-size="14" fill="#fff">APP  — mail ${id}, page ${p + 1}/${pages} (y ${top}…)</text><text x="${colW + gap + 8}" y="18" font-family="Helvetica" font-size="14" fill="#7fd">CHROME (reference)</text></svg>`;
    comps.push({ input: Buffer.from(svg), left: 0, top: 0 });
    const hgt = label + Math.min(PAGE_H, total - top);
    await sharp({ create: { width: colW * 2 + gap, height: hgt, channels: 3, background: '#ff00ff' } })
      .composite(comps)
      .png({ compressionLevel: 6 })
      .toFile(path.join(out, `${id}.cmp.${p + 1}.png`));
  }
}

const ids = only.length
  ? only
  : fs.readdirSync(out).filter((f) => f.endsWith('.app.json')).map((f) => f.replace('.app.json', '')).sort((a, b) => a - b);
for (const id of ids) {
  if (!fs.existsSync(path.join(out, `${id}.chrome.json`)) || !fs.existsSync(path.join(out, `${id}.app.json`))) continue;
  try {
    const res = compareOne(id);
    try {
      await pixels(id, res);
    } catch (e) {
      res.pixelError = String(e).slice(0, 200);
    }
    fs.writeFileSync(path.join(out, `${id}.cmp.json`), JSON.stringify(res, null, 1));
  } catch (e) {
    console.error(id, String(e).slice(0, 300));
  }
}
console.log('compared', ids.length);
