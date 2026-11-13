// What Chrome lays out for an HTML snippet: boxes, images and text lines, in the app's fonts
// at the reader's width, so the output is directly comparable with lay.sh's.
//
// usage: node chrome_lay.mjs '<html…>' | @file.html  [width]  [screenshot.png]
import fs from 'node:fs';
import puppeteer from 'puppeteer';
import { WIDTH as DEFAULT_WIDTH, forceAppFonts } from './common.mjs';

const html = process.argv[2].startsWith('@') ? fs.readFileSync(process.argv[2].slice(1), 'utf8') : process.argv[2];
const WIDTH = Number(process.argv[3] || DEFAULT_WIDTH);
const browser = await puppeteer.launch({ headless: true, args: ['--hide-scrollbars'] });
const page = await browser.newPage();
await page.setJavaScriptEnabled(false);
await page.setViewport({ width: WIDTH, height: 10, deviceScaleFactor: 1 });
await page.setRequestInterception(true);
page.on('request', (r) => (r.url() === 'https://mail.invalid/' ? r.respond({ status: 200, contentType: 'text/html; charset=utf-8', body: html }) : r.url().startsWith('data:') ? r.continue() : r.abort()));
await page.goto('https://mail.invalid/', { waitUntil: 'load' }).catch(() => {});
await forceAppFonts(page);
const d = await page.evaluate(() => {
  const out = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  let n, line = null;
  while ((n = walker.nextNode())) {
    const el = n.parentElement;
    if (!/\S/.test(n.nodeValue) || ['STYLE', 'SCRIPT', 'TITLE'].includes(el.tagName)) continue;
    const cs = getComputedStyle(el);
    const re = /\S+/g; let m;
    while ((m = re.exec(n.nodeValue))) {
      range.setStart(n, m.index); range.setEnd(n, m.index + m[0].length);
      const r = range.getBoundingClientRect();
      if (!r.width && !r.height) continue;
      if (line && Math.abs(line.y - r.top) < 2 && r.left >= line.right - 1 && r.left - line.right < 12) { line.text += ' ' + m[0]; line.right = r.right; }
      else { line = { text: m[0], x: r.left, y: r.top, right: r.right, h: r.height, size: parseFloat(cs.fontSize), weight: cs.fontWeight, color: cs.color }; out.push(line); }
    }
  }
  const imgs = [...document.querySelectorAll('img')].map((i) => { const r = i.getBoundingClientRect(); return `img  x=${r.left.toFixed(0)} y=${r.top.toFixed(0)} w=${r.width.toFixed(0)} h=${r.height.toFixed(0)} ${(i.getAttribute('src') || '').slice(0, 40)}`; });
  const boxes = [];
  for (const el of document.body.querySelectorAll('*')) {
    const cs = getComputedStyle(el), r = el.getBoundingClientRect();
    const bg = cs.backgroundColor !== 'rgba(0, 0, 0, 0)', bd = parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth) + parseFloat(cs.borderLeftWidth) + parseFloat(cs.borderRightWidth) > 0;
    if ((bg || bd) && r.width && r.height) boxes.push(`box  ${el.tagName.toLowerCase()} x=${r.left.toFixed(0)} y=${r.top.toFixed(0)} w=${r.width.toFixed(0)} h=${r.height.toFixed(0)} fill=${bg ? cs.backgroundColor : '-'} border=${bd ? [cs.borderTopWidth, cs.borderRightWidth, cs.borderBottomWidth, cs.borderLeftWidth].join(' ') + ' ' + cs.borderTopColor : '-'} r=${cs.borderTopLeftRadius}`);
  }
  return { h: document.documentElement.scrollHeight, w: document.documentElement.scrollWidth, mode: document.compatMode, lines: out, imgs, boxes };
});
console.log(`height ${d.h} width ${d.w} ${d.mode}`);
for (const b of d.boxes.slice(0, 40)) console.log(b);
for (const i of d.imgs) console.log(i);
for (const l of d.lines) console.log(`text x=${l.x.toFixed(0)} y=${l.y.toFixed(0)} w=${(l.right - l.x).toFixed(0)} h=${l.h.toFixed(1)} size=${l.size} w${l.weight} ${l.color} ${JSON.stringify(l.text.slice(0, 60))}`);
if (process.argv[4]) await page.screenshot({ path: process.argv[4], fullPage: true });
await browser.close();
