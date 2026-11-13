// Reference renders: each corpus mail in headless Chrome at the reader's width, with the
// app's fonts forced (as the app forces them), JS off. Saves a full-page screenshot, a DOM
// dump (words, images, CSS feature census) and an image manifest for the app-side render.
//
// Chrome FETCHES THE MAIL'S REMOTE IMAGES (except ones the mail declares as 1-2px or hidden),
// which tells senders the mail was opened. See README.md.
//
// usage: node chrome_render.mjs [id ...]      (default: every mail not rendered yet)
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import puppeteer from 'puppeteer';
import { WORK, WIDTH, forceAppFonts } from './common.mjs';

const only = process.argv.slice(2);
const corpus = path.join(WORK, 'corpus');
const out = path.join(WORK, 'out');
const imgCache = path.join(out, 'imgcache');
fs.mkdirSync(imgCache, { recursive: true });

const decodeEntities = (s) =>
  s.replace(/&amp;/gi, '&').replace(/&#38;/g, '&').replace(/&#x26;/gi, '&').replace(/&quot;/gi, '"');

// URLs of images the mail itself declares as 1–2px or hidden: tracking beacons. Not fetched.
function beacons(html) {
  const urls = new Set();
  for (const m of html.matchAll(/<img\b[^>]*>/gi)) {
    const tag = m[0];
    const attr = (n) => (tag.match(new RegExp(`\\b${n}\\s*=\\s*["']?\\s*(\\d+)(?:px)?\\s*["'\\s>]`, 'i')) || [])[1];
    const w = attr('width'), h = attr('height');
    const style = (tag.match(/\bstyle\s*=\s*("([^"]*)"|'([^']*)')/i) || [])[2] || '';
    const tiny =
      (w !== undefined && Number(w) <= 2) || (h !== undefined && Number(h) <= 2) ||
      /display\s*:\s*none/i.test(style) || /(?:^|;)\s*(?:width|height)\s*:\s*[012]px/i.test(style);
    if (!tiny) continue;
    const src = (tag.match(/\bsrc\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))/i) || []);
    const u = src[2] ?? src[3] ?? src[4];
    if (u) urls.add(decodeEntities(u.trim()));
  }
  return urls;
}

const EXT = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp', 'image/svg+xml': 'svg', 'image/avif': 'avif', 'image/bmp': 'bmp', 'image/x-icon': 'ico' };

async function renderOne(browser, id) {
  const html = fs.readFileSync(path.join(corpus, `${id}.html`), 'utf8');
  const blocked = beacons(html);
  const base = 'https://mail.invalid/';
  const page = await browser.newPage();
  const urlToFile = {};
  const types = {};
  const pending = [];
  let blockedCount = 0;
  try {
    await page.setJavaScriptEnabled(false);
    await page.setViewport({ width: WIDTH, height: 10, deviceScaleFactor: 1 });
    await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: 'light' }]);
    await page.setRequestInterception(true);
    page.on('request', (req) => {
      const url = req.url();
      if (url === base) {
        return req.respond({ status: 200, contentType: 'text/html; charset=utf-8', body: html });
      }
      const type = req.resourceType();
      if (url.startsWith('data:')) return req.continue();
      if (type === 'image') {
        const raw = url.replace(/^https?:/, '');
        for (const b of blocked) {
          if (b === url || b.replace(/^https?:/, '') === raw) {
            blockedCount++;
            return req.abort();
          }
        }
        return req.continue();
      }
      if (type === 'stylesheet' || type === 'font') return req.continue();
      return req.abort();
    });
    page.on('response', (res) => {
      const req = res.request();
      if (req.resourceType() !== 'image' || req.url().startsWith('data:')) return;
      if (res.status() >= 300) return;
      const chain = req.redirectChain();
      const original = chain.length ? chain[0].url() : req.url();
      pending.push(
        res.buffer().then((buf) => {
          if (!buf.length) return;
          const ct = (res.headers()['content-type'] || '').split(';')[0].trim().toLowerCase();
          const name = crypto.createHash('sha1').update(original).digest('hex') + '.' + (EXT[ct] || 'bin');
          const file = path.join(imgCache, name);
          fs.writeFileSync(file, buf);
          urlToFile[original] = file;
          urlToFile[req.url()] = file;
          types[original] = ct;
        }).catch(() => {}),
      );
    });
    try {
      await page.goto(base, { waitUntil: 'networkidle0', timeout: 25000 });
    } catch (e) {
      // slow image hosts: take what loaded
    }
    await Promise.all(pending);
    await forceAppFonts(page);
    await new Promise((r) => setTimeout(r, 150));

    const dump = await page.evaluate(() => {
      const round = (n) => Math.round(n * 100) / 100;
      const de = document.documentElement;
      const clipAncestors = (el) => {
        const list = [];
        for (let a = el; a && a !== document.body; a = a.parentElement) {
          const cs = getComputedStyle(a);
          if (cs.overflowX !== 'visible' || cs.overflowY !== 'visible') list.push(a.getBoundingClientRect());
        }
        return list;
      };
      const hiddenBy = (el, r) => {
        for (let a = el; a; a = a.parentElement) {
          const cs = getComputedStyle(a);
          if (Number(cs.opacity) === 0) return 'opacity';
          if (cs.visibility !== 'visible') return 'visibility';
        }
        for (const c of clipAncestors(el)) {
          const ix = Math.min(r.right, c.right) - Math.max(r.left, c.left);
          const iy = Math.min(r.bottom, c.bottom) - Math.max(r.top, c.top);
          if (ix <= 0.5 || iy <= 0.5) return 'clipped';
        }
        return null;
      };
      const decoOf = (el) => {
        const lines = new Set();
        for (let a = el; a; a = a.parentElement) {
          const cs = getComputedStyle(a);
          for (const l of cs.textDecorationLine.split(' ')) if (l !== 'none') lines.add(l);
          if (cs.display !== 'inline' && cs.display !== 'contents') break;
        }
        return [...lines].sort().join(' ');
      };
      const words = [];
      const hiddenText = [];
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      let n;
      while ((n = walker.nextNode())) {
        const t = n.nodeValue;
        if (!/\S/.test(t)) continue;
        const el = n.parentElement;
        if (!el || ['STYLE', 'SCRIPT', 'TITLE', 'NOSCRIPT'].includes(el.tagName)) continue;
        const cs = getComputedStyle(el);
        const meta = {
          fs: round(parseFloat(cs.fontSize)), fw: Number(cs.fontWeight), color: cs.color,
          italic: cs.fontStyle !== 'normal', deco: decoOf(el), tt: cs.textTransform,
          ls: cs.letterSpacing === 'normal' ? 0 : round(parseFloat(cs.letterSpacing)),
          mono: /AppMono/.test(cs.fontFamily), link: !!el.closest('a[href]'),
          lh: cs.lineHeight, va: cs.verticalAlign, ws: cs.whiteSpace,
        };
        const re = /\S+/g;
        let m;
        let first = true;
        while ((m = re.exec(t))) {
          range.setStart(n, m.index);
          range.setEnd(n, m.index + m[0].length);
          const r = range.getBoundingClientRect();
          if (r.width === 0 && r.height === 0) continue;
          const hid = hiddenBy(el, r);
          const alpha = (cs.color.match(/rgba\([^)]*,\s*([\d.]+)\)/) || [])[1];
          if (hid || alpha === '0' || parseFloat(cs.fontSize) < 1) {
            if (first) hiddenText.push({ text: t.trim().slice(0, 80), why: hid || 'transparent/tiny' });
            first = false;
            continue;
          }
          first = false;
          words.push({ t: m[0], x: round(r.left + scrollX), y: round(r.top + scrollY), w: round(r.width), h: round(r.height), ...meta });
        }
      }

      const images = [];
      for (const img of document.querySelectorAll('img')) {
        const r = img.getBoundingClientRect();
        const cs = getComputedStyle(img);
        // The content box: where the pixels go, which is what the app's image item is.
        const px = (v) => parseFloat(v) || 0;
        const left = px(cs.borderLeftWidth) + px(cs.paddingLeft), top = px(cs.borderTopWidth) + px(cs.paddingTop);
        const right = px(cs.borderRightWidth) + px(cs.paddingRight), bottom = px(cs.borderBottomWidth) + px(cs.paddingBottom);
        images.push({
          src: img.getAttribute('src'), resolved: img.currentSrc || img.src,
          x: round(r.left + left + scrollX), y: round(r.top + top + scrollY),
          w: round(Math.max(0, r.width - left - right)), h: round(Math.max(0, r.height - top - bottom)),
          nw: img.naturalWidth, nh: img.naturalHeight, complete: img.complete,
          broken: img.complete && img.naturalWidth === 0, alt: img.getAttribute('alt') || '',
          hidden: cs.display === 'none' || !!hiddenBy(img, r), objectFit: cs.objectFit,
          radius: cs.borderTopLeftRadius,
        });
      }

      // CSS the display list cannot express (or might not): where and how much.
      const feat = {};
      const bump = (k, r, extra) => {
        const f = (feat[k] ||= { n: 0, area: 0, eg: [] });
        f.n++;
        f.area += Math.round(r.width * r.height);
        if (f.eg.length < 4) f.eg.push({ y: Math.round(r.top + scrollY), x: Math.round(r.left), w: Math.round(r.width), h: Math.round(r.height), ...(extra ? { v: String(extra).slice(0, 90) } : {}) });
      };
      const attrs = {};
      for (const el of document.body.querySelectorAll('*')) {
        for (const a of ['bgcolor', 'background', 'align', 'valign', 'cellpadding', 'cellspacing', 'border', 'hspace', 'vspace', 'nowrap', 'bordercolor']) {
          if (el.hasAttribute(a)) attrs[`${el.tagName.toLowerCase()}[${a}]`] = (attrs[`${el.tagName.toLowerCase()}[${a}]`] || 0) + 1;
        }
        const r = el.getBoundingClientRect();
        if (!r.width || !r.height) continue;
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility !== 'visible') continue;
        const tag = el.tagName.toLowerCase();
        if (['font', 'center', 'svg', 'video', 'picture', 'hr', 'u', 's', 'strike', 'del', 'sub', 'sup', 'small', 'big', 'marquee', 'button', 'input', 'select', 'textarea', 'iframe', 'object', 'canvas'].includes(tag)) bump('tag:' + tag, r);
        if (cs.backgroundImage !== 'none') bump(/gradient/.test(cs.backgroundImage) ? 'bg-gradient' : 'bg-image', r, cs.backgroundImage);
        if (cs.boxShadow !== 'none') bump('box-shadow', r, cs.boxShadow);
        const radii = [cs.borderTopLeftRadius, cs.borderTopRightRadius, cs.borderBottomRightRadius, cs.borderBottomLeftRadius];
        if (radii.some((x) => x !== '0px')) {
          if (new Set(radii).size > 1) bump('radius-nonuniform', r, radii.join(' '));
          else bump('radius', r, radii[0]);
          if (cs.overflow !== 'visible' && el.children.length) bump('radius-clips-children', r);
        }
        const sides = ['Top', 'Right', 'Bottom', 'Left'].map((s) => ({ w: parseFloat(cs[`border${s}Width`]), st: cs[`border${s}Style`], c: cs[`border${s}Color`] })).filter((s) => s.w > 0 && s.st !== 'none' && s.st !== 'hidden');
        if (sides.length) {
          if (new Set(sides.map((s) => s.c)).size > 1) bump('border-colors-differ', r, sides.map((s) => s.c).join(' | '));
          for (const s of sides) if (!['solid', 'inset', 'outset'].includes(s.st)) { bump('border-style:' + s.st, r); break; }
          if (sides.some((s) => s.st === 'inset' || s.st === 'outset')) bump('border-style:inset/outset', r);
        }
        if (cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) > 0) bump('outline', r);
        const op = Number(cs.opacity);
        if (op > 0 && op < 1) bump('opacity-partial', r, op);
        if (cs.position !== 'static') bump('position:' + cs.position, r);
        if (cs.float !== 'none') bump('float', r, cs.float);
        if ((cs.overflowX !== 'visible' || cs.overflowY !== 'visible') && (el.scrollHeight > el.clientHeight + 1 || el.scrollWidth > el.clientWidth + 1)) bump('overflow-clips', r, `${cs.overflowX}/${cs.overflowY} scroll ${el.scrollWidth}x${el.scrollHeight} client ${el.clientWidth}x${el.clientHeight}`);
        if (cs.transform !== 'none') bump('transform', r, cs.transform);
        if (cs.filter !== 'none') bump('filter', r, cs.filter);
        if (cs.textShadow !== 'none') bump('text-shadow', r);
        if (cs.textTransform !== 'none' && el.childNodes.length && [...el.childNodes].some((c) => c.nodeType === 3 && /\S/.test(c.nodeValue))) bump('text-transform:' + cs.textTransform, r);
        if (cs.letterSpacing !== 'normal' && parseFloat(cs.letterSpacing) !== 0 && [...el.childNodes].some((c) => c.nodeType === 3 && /\S/.test(c.nodeValue))) bump('letter-spacing', r, cs.letterSpacing);
        if (cs.textAlign === 'justify') bump('text-align:justify', r);
        if (cs.direction === 'rtl') bump('direction:rtl', r);
        if (tag === 'img' && cs.objectFit !== 'fill') bump('object-fit', r, cs.objectFit);
        if (cs.display === 'flex' || cs.display === 'inline-flex') bump('display:flex', r);
        if (cs.display === 'grid' || cs.display === 'inline-grid') bump('display:grid', r);
        if (cs.display === 'inline-block') bump('display:inline-block', r);
        if (cs.display === 'list-item') bump('list-item:' + cs.listStyleType, r);
        if (tag === 'table' && cs.borderCollapse === 'collapse') bump('border-collapse', r);
        if (cs.whiteSpace !== 'normal' && [...el.childNodes].some((c) => c.nodeType === 3 && /\S/.test(c.nodeValue))) bump('white-space:' + cs.whiteSpace, r);
        if (cs.textOverflow === 'ellipsis') bump('text-overflow:ellipsis', r);
        if (cs.boxSizing === 'border-box' && (parseFloat(cs.paddingLeft) || parseFloat(cs.paddingTop))) bump('box-sizing:border-box', r);
      }
      const bodyCs = getComputedStyle(document.body);
      return {
        compatMode: document.compatMode, doctype: document.doctype ? document.doctype.name + ' ' + document.doctype.publicId : null,
        width: de.scrollWidth, height: Math.max(de.scrollHeight, document.body.scrollHeight),
        viewport: innerWidth, bodyBg: bodyCs.backgroundColor, htmlBg: getComputedStyle(de).backgroundColor,
        bodyMargin: bodyCs.marginTop + ' ' + bodyCs.marginLeft,
        words, hiddenText, images, feat, attrs,
        media: [...document.styleSheets].reduce((n, s) => { try { return n + [...s.cssRules].filter((r) => r.type === 4).length; } catch { return n; } }, 0),
      };
    });

    // The manifest the app-side render reads: each <img src> as written -> the bytes Chrome got.
    const manifest = { images: {} };
    const formats = {};
    for (const img of dump.images) {
      if (!img.src || img.src.startsWith('data:')) continue;
      const file = urlToFile[img.resolved];
      const ct = types[img.resolved];
      if (ct) img.type = ct;
      if (file) {
        manifest.images[img.src] = { file, w: img.nw || undefined, h: img.nh || undefined };
        const ext = path.extname(file).slice(1);
        formats[ext] = (formats[ext] || 0) + 1;
      }
    }
    dump.formats = formats;
    dump.blockedBeacons = blockedCount;
    const h = Math.min(dump.height, 16000);
    await page.screenshot({ path: path.join(out, `${id}.chrome.png`), clip: { x: 0, y: 0, width: Math.max(dump.width, WIDTH), height: h }, captureBeyondViewport: true });
    fs.writeFileSync(path.join(out, `${id}.chrome.json`), JSON.stringify(dump));
    fs.writeFileSync(path.join(out, `${id}.manifest.json`), JSON.stringify(manifest));
    return { id, ok: true, h: dump.height, words: dump.words.length, imgs: dump.images.length };
  } catch (e) {
    return { id, ok: false, error: String(e).slice(0, 300) };
  } finally {
    await page.close().catch(() => {});
  }
}

const ids = only.length
  ? only
  : fs.readdirSync(corpus).filter((f) => f.endsWith('.html')).map((f) => f.replace(/\.html$/, '')).sort((a, b) => a - b);
const browser = await puppeteer.launch({ headless: true, args: ['--hide-scrollbars', '--font-render-hinting=none'] });
const queue = [...ids];
const results = [];
await Promise.all(
  Array.from({ length: 4 }, async () => {
    while (queue.length) {
      const id = queue.shift();
      if (!only.length && fs.existsSync(path.join(out, `${id}.chrome.json`))) continue;
      const r = await renderOne(browser, id);
      results.push(r);
      console.log(JSON.stringify(r));
    }
  }),
);
await browser.close();
