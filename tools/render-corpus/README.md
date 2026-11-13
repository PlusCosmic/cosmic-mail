# Render corpus: the reader's HTML mail against Chrome

Renders every mail in your local cache twice — through the app's real reader (headless) and
through headless Chrome at the same width, in the same fonts — and diffs the two: page height,
which words each shows and where, text styles, image boxes, plus side-by-side pictures. It is
how the Blitz renderer's bugs were found and grouped, and it is the regression check for any
change to `src/ui/mail_doc.rs`, `src/ui/view/reader.rs` or the Blitz/Rune pins.

## Before you run it

- **The corpus is your private mail.** It lives in `target/render-corpus/` (or `$RENDER_CORPUS`),
  which git ignores. Never copy mail from it into the repo, a test, a commit message or a PR:
  reduce a bug to HTML you wrote yourself.
- **Chrome loads each mail's remote images**, so senders' tracking sees the mail opened (images a
  mail declares as 1–2px or hidden are skipped; per-recipient image URLs are not). Each mail is
  rendered once and kept, so this happens once per mail.
- Needs Node 20+, Python 3, `curl`, and the app's build dependencies. `npm install` here once
  (Puppeteer downloads its own Chrome; if it does not, `npx puppeteer browsers install chrome`).

## Running it

All from this directory. Everything is incremental and safe to re-run.

```sh
npm install
python3 corpus.py                 # cached HTML bodies -> corpus/<id>.html, index.json, uncached.tsv
# optional: bodies not cached yet, over IMAP, read-only (it prints this command with paths):
#   cargo run --example mail_corpus_fetch -- …/corpus/uncached.tsv …/corpus
node chrome_render.mjs            # the reference: out/<id>.chrome.png/.chrome.json/.manifest.json
python3 images.py                 # image bytes as the app's own fetch gets them
./run_app.sh                      # the app: out/<id>.app.png/.app.json, then the diff: .cmp.json/.cmp.N.png
python3 aggregate.py > ../../target/render-corpus/main.txt   # corpus totals; out/summary.tsv per mail
```

Checking a change: render the corpus with your branch into its own directory and put the totals
next to main's.

```sh
OUT=../../target/render-corpus/mine NO_IMAGES=1 ./run_app.sh
python3 aggregate.py ../../target/render-corpus/mine > ../../target/render-corpus/mine.txt
./cmp_agg.sh ../../target/render-corpus/main.txt ../../target/render-corpus/mine.txt
```

Then look at a few mails that moved: `./run_app.sh 200 258` (ids limit any step to those mails;
without `NO_IMAGES` you get the side-by-side pages) and `python3 show.py DIR 200 258`.

Reducing a difference to a repro: run the same snippet through both and compare the output.

```sh
cargo build --example mail_render
./lay.sh '<table width="300" align="center"><tr><td>Centred?</td></tr></table>'           # the app
node chrome_lay.mjs '<table width="300" align="center"><tr><td>Centred?</td></tr></table>' # Chrome
```

Both accept `@file.html`. Bisect the mail (delete halves until the difference goes) to get there.

Table column widths have their own fixed set of cases, written by hand (no mail): 23 table
patterns common in HTML mail (percentage, pixel and `auto` columns, widths on later rows,
`colspan` headers, fixed layout, nested tables), each through both renderers.

```sh
python3 tablecases.py             # every case: OK, or each cell's "x+width" in Chrome and the app
python3 tablecases.py 05 14       # only these cases
```

It exits non-zero if any case differs by more than 2px. Run it after any change to the Blitz
fork's table code or to `mail_doc`'s table passes (`tables.rs`, `hints.rs`); the cases still
differing are listed under "Blitz" in `docs/GOTCHAS.md`.

Line layout under each document mode has a matrix too: 28 hand-written cases (text in the block
or only in spans, nested and padded spans, `<br>` lines, images alone and beside text in cells,
inline blocks, mixed sizes, tables in a big or centred block, cell heights with padding,
unitless lengths, class case), each under `<!DOCTYPE html>` (`std`), XHTML 1.0 Transitional
(`lq`, limited-quirks) and no doctype (`q`, quirks), through both renderers.

```sh
python3 quirkcases.py             # every case and mode: OK, or each renderer's rows
python3 quirkcases.py 02 08 q,lq  # only these cases, in these modes
```

Rows are `t<y>` per text line (`@x<x>` when not 0), `i<y>+<h>` per image, `b<y>+<h>` per filled
box. It exits non-zero if any differs by more than 1px (x: 4px). Run it after any change to the
fork's inline layout (`layout/construct.rs`) or quirks-mode code; the cases still differing are
listed under "Blitz" in `docs/GOTCHAS.md`.

## What is compared, and how

- **The app side** is `examples/mail_render.rs`: the demo mailbox with the mail as its open
  message, drawn by the real view at scale 1 and cropped to the mail column, in a 1440pt window —
  which lays mail out 712px wide (`MAIL_WIDTH` here must match; the default 1280pt window gives
  616px, where mobile `@media` rules start to apply). It also dumps the display list
  (`.app.json`). Images come from the manifest; nothing is fetched and no consent notice shows.
- **The reference** is the mail as a document in Chrome (its own doctype, so quirks mode as the
  mail asks), JavaScript off, a 712×10 viewport (the app's is 1px tall), light colour scheme, and
  every element forced into the app's two font families — the app sets all mail in its own
  faces, so font choice is not a difference worth reporting.
- **Images**: Chrome is served AVIF/WebP by most CDNs because it asks for them; the app is not.
  `images.py` refetches those the way the app does, so "undecodable" means what the app would
  really have received.

## Reading the numbers (`aggregate.py`, `show.py`, `<id>.cmp.json`)

| metric | meaning |
|---|---|
| height ratio | the app's page height over Chrome's. `<=2px` is a collapsed page. |
| fit-scaled | the app found content wider than the pane and scales the page down (`fitScale`) |
| missing text | words Chrome shows that the app's display list lacks (dropped content) |
| extra text | words the app draws that Chrome hides or clips |
| x offset lines | app lines whose first word is more than 6px from Chrome's x |
| dy jumps | places where the app−Chrome vertical offset changes by more than 6px: where the layouts diverge. `show.py` prints the Chrome y and the words either side |
| wrap diffs | lines that break at different words |
| text drawn over text | mails where the app draws two lines of text on top of each other; needs no reference, so it also works at other widths (`mail_render`'s window-width argument) |
| font size / weight / colour / underline off | style mismatches on matched words |
| images missing / size off / undecodable | no image item; a box differing by more than 3px; bytes Rune cannot decode |

`CSS features` and `attrs` count the mails using each feature or legacy attribute as Chrome
computed it, with example rectangles in `<id>.chrome.json` (`feat`) — the way to find mails
that exercise something.

Known artefacts: text inside a closed `<details>` counts as missing (Chrome reports its
geometry; the app is right to hide it). Side-by-side pages are capped at 8 × 1500px per mail.
