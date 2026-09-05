// Package omarchy reads the active omarchy theme's colors and watches for
// theme switches.
//
// The active theme lives at ~/.config/omarchy/current/theme, a symlink that
// omarchy-theme-set swaps atomically. We watch the parent directory
// ~/.config/omarchy/current (non-recursively), debounce, re-read, and emit.
package omarchy

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/fsnotify/fsnotify"
	toml "github.com/pelletier/go-toml/v2"

	"cosmicmail/internal/models"
	"cosmicmail/internal/xdg"
)

// Debounce is how long the watcher waits for a burst of events to settle
// before re-reading the theme.
const Debounce = 300 * time.Millisecond

type colorsToml struct {
	Accent              *string `toml:"accent"`
	Foreground          *string `toml:"foreground"`
	Background          *string `toml:"background"`
	Cursor              *string `toml:"cursor"`
	SelectionForeground *string `toml:"selection_foreground"`
	SelectionBackground *string `toml:"selection_background"`
	Color0              *string `toml:"color0"`
	Color1              *string `toml:"color1"`
	Color2              *string `toml:"color2"`
	Color3              *string `toml:"color3"`
	Color4              *string `toml:"color4"`
	Color5              *string `toml:"color5"`
	Color6              *string `toml:"color6"`
	Color7              *string `toml:"color7"`
	Color8              *string `toml:"color8"`
	Color9              *string `toml:"color9"`
	Color10             *string `toml:"color10"`
	Color11             *string `toml:"color11"`
	Color12             *string `toml:"color12"`
	Color13             *string `toml:"color13"`
	Color14             *string `toml:"color14"`
	Color15             *string `toml:"color15"`
}

// CurrentDir is ~/.config/omarchy/current.
func CurrentDir() (string, error) {
	cfg, err := xdg.ConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(cfg, "omarchy", "current"), nil
}

// KanagawaFallback is the built-in theme used when the omarchy files are
// missing.
func KanagawaFallback() models.OmarchyTheme {
	return models.OmarchyTheme{
		Name: "kanagawa", Accent: "#7e9cd8", Foreground: "#dcd7ba", Background: "#1f1f28", Cursor: "#c8c093",
		SelectionForeground: "#c8c093", SelectionBackground: "#2d4f67",
		Palette: []string{
			"#090618", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093",
			"#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba",
		},
	}
}

// ReadTheme reads the active omarchy theme, falling back to kanagawa on any
// problem.
func ReadTheme() models.OmarchyTheme {
	dir, err := CurrentDir()
	if err != nil {
		return KanagawaFallback()
	}
	theme, err := ReadThemeFrom(dir)
	if err != nil {
		slog.Debug("falling back to kanagawa theme", "error", err)
		return KanagawaFallback()
	}
	return theme
}

// ReadThemeFrom reads `<dir>/theme/colors.toml` and `<dir>/theme.name`.
func ReadThemeFrom(dir string) (models.OmarchyTheme, error) {
	colorsPath := filepath.Join(dir, "theme", "colors.toml")
	raw, err := os.ReadFile(colorsPath)
	if err != nil {
		return models.OmarchyTheme{}, fmt.Errorf("reading %s: %w", colorsPath, err)
	}
	var colors colorsToml
	if err := toml.Unmarshal(raw, &colors); err != nil {
		return models.OmarchyTheme{}, fmt.Errorf("parsing %s: %w", colorsPath, err)
	}
	name := "unknown"
	if b, err := os.ReadFile(filepath.Join(dir, "theme.name")); err == nil {
		name = strings.TrimSpace(string(b))
	}
	fb := KanagawaFallback()
	pick := func(v *string, fallback string) string {
		if v != nil {
			return *v
		}
		return fallback
	}
	raws := []*string{colors.Color0, colors.Color1, colors.Color2, colors.Color3, colors.Color4, colors.Color5, colors.Color6, colors.Color7,
		colors.Color8, colors.Color9, colors.Color10, colors.Color11, colors.Color12, colors.Color13, colors.Color14, colors.Color15}
	palette := make([]string, 16)
	for i, c := range raws {
		palette[i] = pick(c, fb.Palette[i])
	}
	return models.OmarchyTheme{
		Name:                name,
		Accent:              pick(colors.Accent, fb.Accent),
		Foreground:          pick(colors.Foreground, fb.Foreground),
		Background:          pick(colors.Background, fb.Background),
		Cursor:              pick(colors.Cursor, fb.Cursor),
		SelectionForeground: pick(colors.SelectionForeground, fb.SelectionForeground),
		SelectionBackground: pick(colors.SelectionBackground, fb.SelectionBackground),
		Palette:             palette,
	}, nil
}

// Equal compares two themes.
func Equal(a, b models.OmarchyTheme) bool {
	if a.Name != b.Name || a.Accent != b.Accent || a.Foreground != b.Foreground || a.Background != b.Background || a.Cursor != b.Cursor ||
		a.SelectionForeground != b.SelectionForeground || a.SelectionBackground != b.SelectionBackground || len(a.Palette) != len(b.Palette) {
		return false
	}
	for i := range a.Palette {
		if a.Palette[i] != b.Palette[i] {
			return false
		}
	}
	return true
}

// Watch blocks, watching for omarchy theme changes and calling emit with the
// new theme until ctx is done. It returns nil immediately if the omarchy
// directory is absent.
func Watch(ctx context.Context, emit func(models.OmarchyTheme)) error {
	dir, err := CurrentDir()
	if err != nil {
		return err
	}
	if st, err := os.Stat(dir); err != nil || !st.IsDir() {
		slog.Info("omarchy current dir not present; theme watcher inactive")
		return nil
	}
	return WatchDir(ctx, dir, emit)
}

// WatchDir watches dir (non-recursively) for any event, debounces, re-reads
// the theme, and emits only when it actually changed: the watcher fires for
// unrelated churn in the dir (e.g. background rotation touching symlinks).
// fsnotify does not report access events, so re-reading the theme files from
// inside the handler cannot re-trigger it (the loop the Rust build had to
// filter out).
func WatchDir(ctx context.Context, dir string, emit func(models.OmarchyTheme)) error {
	watcher, err := fsnotify.NewWatcher()
	if err != nil {
		return fmt.Errorf("creating fs watcher: %w", err)
	}
	defer watcher.Close()
	if err := watcher.Add(dir); err != nil {
		return fmt.Errorf("watching %s: %w", dir, err)
	}
	slog.Info("watching omarchy theme dir", "dir", dir)
	read := func() models.OmarchyTheme {
		theme, err := ReadThemeFrom(dir)
		if err != nil {
			return KanagawaFallback()
		}
		return theme
	}
	last := read()
	for {
		select {
		case <-ctx.Done():
			return nil
		case err, ok := <-watcher.Errors:
			if !ok {
				return errors.New("fs watcher closed")
			}
			slog.Warn("omarchy watcher error", "error", err)
		case _, ok := <-watcher.Events:
			if !ok {
				return errors.New("fs watcher closed")
			}
			// Debounce: drain events that arrive within the settle window.
			timer := time.NewTimer(Debounce)
		drain:
			for {
				select {
				case <-watcher.Events:
					if !timer.Stop() {
						<-timer.C
					}
					timer.Reset(Debounce)
				case <-timer.C:
					break drain
				case <-ctx.Done():
					timer.Stop()
					return nil
				}
			}
			theme := read()
			if Equal(theme, last) {
				continue
			}
			last = theme
			emit(theme)
			slog.Info("emitted omarchy theme change", "theme", theme.Name)
		}
	}
}
