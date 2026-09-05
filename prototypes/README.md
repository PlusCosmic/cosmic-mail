# Cosmic Mail — UI Design Prototypes

Static, self-contained HTML mockups exploring three layouts for **Cosmic Mail**, a native
Linux mail client for the [omarchy](https://omarchy.org) desktop (Hyprland + mako, minimal
keyboard-driven aesthetic). No build step, no external assets — all CSS is inline and the mail
data is hardcoded fake content across two accounts (`harry@pluscosmic.dev` IMAP + a Gmail
account).

## How to view

Just open any file in a browser:

```
xdg-open prototypes/01-classic-three-pane.html
xdg-open prototypes/02-keyboard-minimal.html
xdg-open prototypes/03-hybrid.html
```

Each file has a **theme switcher** in the top-right corner (plain `<select>`) that swaps the
`:root` CSS custom properties between three omarchy themes — **kanagawa**, **tokyo-night**, and
**everforest** — so you can confirm the UI is fully theme-adaptive. Every color in every
prototype is derived from the `OmarchyTheme` variables (`--bg`, `--fg`, `--accent`, `--cursor`,
`--sel-bg`, `--sel-fg`, `--c0`..`--c15`) via `color-mix()`; no other colors are hardcoded, which
mirrors how the real app maps `~/.local/state/omarchy/current/theme/colors.toml` onto CSS variables.

For best fidelity install a Nerd Font (`CaskaydiaMono Nerd Font` or `JetBrainsMono Nerd Font`) so
the glyphs render; the layout is otherwise font-agnostic monospace.

## The three concepts

### 01 · Classic Three-Pane
The familiar desktop-mail layout (Thunderbird / Betterbird / Evolution): a left sidebar with both
accounts and their folders (each carrying an unread badge), a middle message list, and a right
reading pane, topped by a persistent toolbar (compose / reply / archive / delete / sync) and a
bottom status bar showing live per-account sync state.

- **Trade-offs:** Zero learning curve and maximum information density on a wide monitor — every
  action is visible and clickable. But it is the most chrome-heavy option (toolbar + status bar
  cost vertical space), and its mouse-first, tree-driven model is the weakest fit for a
  keyboard-centric Hyprland workflow.

### 02 · Keyboard-Minimal (omarchy-native)
A modal, keyboard-first client with almost no chrome that feels at home beside walker and a foot
terminal. A dense single-pane message list is the home screen; pressing Enter swaps the whole
pane to a focused reader. Actions are single keystrokes (`j`/`k`, `enter`, `a`, `r`, `/`) and a
walker/telescope-style command palette (`Ctrl+K`) fuzzy-runs everything else. Because the file is
static, the list and reader states are shown side-by-side across a labelled divider, with the
`Ctrl+K` palette rendered open in the reader state.

- **Trade-offs:** The highest signal-to-chrome ratio — content owns the screen and it feels fast
  and native for vim/terminal users. But the affordances are hidden until learned (steep for
  mouse-first users), and a single pane means no simultaneous list-plus-preview glance.

### 03 · Hybrid Two-Pane
A pragmatic middle ground: a collapsed **icon-rail** on the far left switches between the unified
view and each account (hover reveals the label), while the main area is a two-pane split of a
**unified inbox** message list beside a persistent reader. A slim header carries a single search
input and the sync indicator; a footer keeps the vim key hints discoverable.

- **Trade-offs:** Reclaims the horizontal space the classic layout spends on a folder tree while
  keeping list + preview visible at once, and the unified inbox matches real cross-account triage.
  It stays fully keyboard-drivable while also working with a mouse. The cost is that the folder
  tree is one click away (rail → account) rather than always-on, which power users with deeply
  nested folders may miss.

## Recommendation

**03 · Hybrid** is the strongest default: it keeps the discoverability that makes 01 approachable
while adopting the minimal chrome, unified inbox, and keyboard bindings that make Cosmic Mail feel
omarchy-native — with 02's command palette and modal reader as an optional power-user mode to
grow into.
