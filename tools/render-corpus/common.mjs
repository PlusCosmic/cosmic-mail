// Shared by the Chrome-side tools: where things live, and the app's fonts.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const TOOLS = path.dirname(fileURLToPath(import.meta.url));
export const REPO = path.resolve(TOOLS, '../..');
/** The working directory: `corpus/` (the mails) and `out/` (renders). Never in git. */
export const WORK = process.env.RENDER_CORPUS || path.join(REPO, 'target/render-corpus');
/** The width the reader lays mail out at in a 1440pt window (`mail_width` in examples/mail_render.rs). */
export const WIDTH = Number(process.env.MAIL_WIDTH || 712);

/** The foundation's font files: the checkout Cargo made of the rev Cargo.toml pins. */
function fontsDir() {
  if (process.env.FOUNDATION_FONTS) return process.env.FOUNDATION_FONTS;
  const toml = fs.readFileSync(path.join(REPO, 'Cargo.toml'), 'utf8');
  const rev = (toml.match(/rune-theme-foundation\s*=\s*\{[^}]*rev\s*=\s*"([0-9a-f]+)"/) || [])[1];
  const base = path.join(process.env.CARGO_HOME || path.join(os.homedir(), '.cargo'), 'git/checkouts');
  if (rev && fs.existsSync(base)) {
    for (const d of fs.readdirSync(base).filter((d) => d.startsWith('ui-foundation-'))) {
      const dir = path.join(base, d, rev.slice(0, 7), 'rune/theme/assets/fonts');
      if (fs.existsSync(dir)) return dir;
    }
  }
  throw new Error('foundation fonts not found: build the app once, or set FOUNDATION_FONTS to the directory holding InstrumentSans-*.ttf');
}

/** The elements the app sets in its code face (the UA rule in `mail_doc::layout_inner`). */
export const CODE =
  'code, pre, kbd, samp, tt, [style*=monospace i], [style*=courier i], [style*=consolas i], [style*=menlo i], font[face*=mono i], font[face*=courier i]';

function fontCss() {
  const dir = fontsDir();
  const face = (family, file, weight) =>
    `@font-face{font-family:"${family}";src:url(data:font/ttf;base64,${fs.readFileSync(path.join(dir, file)).toString('base64')}) format("truetype");font-weight:${weight};font-style:normal}`;
  return [
    face('AppSans', 'InstrumentSans-Regular.ttf', 400),
    face('AppSans', 'InstrumentSans-SemiBold.ttf', 600),
    face('AppMono', 'JetBrainsMonoNL-Regular.ttf', 400),
    face('AppMono', 'JetBrainsMonoNL-SemiBold.ttf', 600),
  ].join('\n');
}

/**
 * Set the page in the app's fonts, as the app does. Its rule is a UA `!important` one, which
 * beats the mail's own `!important` declarations; an inline `!important` on every element is
 * the closest an author can get. With the same fonts in both renderers, a difference is a
 * layout difference.
 */
export async function forceAppFonts(page) {
  await page.addStyleTag({ content: fontCss() });
  await page.evaluate((code) => {
    const mono = new Set(document.querySelectorAll(code + ', ' + code.split(', ').map((s) => s + ' *').join(', ')));
    for (const el of document.querySelectorAll('*')) {
      if (el.style) el.style.setProperty('font-family', mono.has(el) ? 'AppMono' : 'AppSans', 'important');
    }
  }, CODE);
  await page.evaluate(() => document.fonts.ready.then(() => true));
}
