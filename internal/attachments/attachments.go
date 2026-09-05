// Package attachments holds the filesystem side of saving attachments: the
// downloads directory, filename sanitisation, and non-overwriting collision
// naming. Extraction from parsed messages lives in package mailparse; this
// is deliberately pure and unit-tested (no IMAP, no DB).
package attachments

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"unicode"

	"cosmicmail/internal/xdg"
)

// DownloadsDir resolves the user's downloads directory, creating it if
// necessary (the XDG download dir, falling back to ~/Downloads).
func DownloadsDir() (string, error) {
	dir, ok := xdg.DownloadDir()
	if !ok {
		return "", errors.New("could not determine a downloads directory")
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", fmt.Errorf("creating downloads directory %s: %w", dir, err)
	}
	return dir, nil
}

// SafeFilename produces a safe base filename for an attachment: strips path
// components and control characters, refuses leading dots (so a name can't
// become a dotfile or `..`), and falls back to `attachment-<id>` — with an
// extension guessed from the MIME type when reasonable — for empty or
// fully-stripped names.
func SafeFilename(raw, mimeType string, attachmentID int64) string {
	base := raw
	if i := strings.LastIndexAny(base, `/\`); i >= 0 {
		base = base[i+1:]
	}
	base = strings.ReplaceAll(base, "\x00", "")
	var cleaned strings.Builder
	for _, c := range base {
		if !unicode.IsControl(c) {
			cleaned.WriteRune(c)
		}
	}
	name := strings.TrimSpace(strings.TrimLeft(strings.TrimSpace(cleaned.String()), "."))
	if name == "" {
		name = fmt.Sprintf("attachment-%d", attachmentID)
		if ext := extensionForMime(mimeType); ext != "" {
			name += "." + ext
		}
	}
	return name
}

func extensionForMime(mimeType string) string {
	switch strings.ToLower(strings.TrimSpace(mimeType)) {
	case "application/pdf":
		return "pdf"
	case "application/zip":
		return "zip"
	case "application/json":
		return "json"
	case "application/msword":
		return "doc"
	case "text/plain":
		return "txt"
	case "text/html":
		return "html"
	case "text/csv":
		return "csv"
	case "image/png":
		return "png"
	case "image/jpeg":
		return "jpg"
	case "image/gif":
		return "gif"
	case "image/webp":
		return "webp"
	case "image/svg+xml":
		return "svg"
	}
	return ""
}

// UniquePath returns a path in dir for name that does not overwrite an
// existing file, suffixing the stem on collision: `name (1).ext`, `name (2).ext`, …
func UniquePath(dir, name string) string {
	candidate := filepath.Join(dir, name)
	if !exists(candidate) {
		return candidate
	}
	stem, ext := splitName(name)
	for n := 1; ; n++ {
		alt := fmt.Sprintf("%s (%d)", stem, n)
		if ext != "" {
			alt += "." + ext
		}
		path := filepath.Join(dir, alt)
		if !exists(path) {
			return path
		}
	}
}

func exists(path string) bool {
	_, err := os.Lstat(path)
	return err == nil
}

// splitName splits a filename into (stem, extension); a leading dot is part
// of the stem, not an extension separator.
func splitName(name string) (string, string) {
	i := strings.LastIndex(name, ".")
	if i <= 0 || i == len(name)-1 {
		return name, ""
	}
	return name[:i], name[i+1:]
}
