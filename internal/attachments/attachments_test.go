package attachments

import (
	"os"
	"path/filepath"
	"testing"
)

func TestStripsPathSeparatorsAndControlChars(t *testing.T) {
	if got := SafeFilename("../../etc/passwd", "application/octet-stream", 1); got != "passwd" {
		t.Fatal(got)
	}
	if got := SafeFilename(`a\b\report.pdf`, "application/pdf", 1); got != "report.pdf" {
		t.Fatal(got)
	}
	if got := SafeFilename("na\x07me.txt", "text/plain", 1); got != "name.txt" {
		t.Fatal(got)
	}
}

func TestRejectsLeadingDotsAndEmptyNames(t *testing.T) {
	if got := SafeFilename("..", "application/pdf", 7); got != "attachment-7.pdf" {
		t.Fatal(got)
	}
	if got := SafeFilename(".hidden", "", 7); got != "hidden" {
		t.Fatal(got)
	}
	if got := SafeFilename("   ", "application/octet-stream", 9); got != "attachment-9" {
		t.Fatal(got)
	}
	if got := SafeFilename("", "image/png", 3); got != "attachment-3.png" {
		t.Fatal(got)
	}
}

func TestKeepsOrdinaryNamesVerbatim(t *testing.T) {
	if got := SafeFilename("Quarterly Report.pdf", "application/pdf", 1); got != "Quarterly Report.pdf" {
		t.Fatal(got)
	}
}

func TestUniquePathSuffixesOnCollision(t *testing.T) {
	dir := t.TempDir()
	p1 := UniquePath(dir, "file.pdf")
	if p1 != filepath.Join(dir, "file.pdf") {
		t.Fatal(p1)
	}
	os.WriteFile(p1, []byte("a"), 0o644)
	p2 := UniquePath(dir, "file.pdf")
	if p2 != filepath.Join(dir, "file (1).pdf") {
		t.Fatal(p2)
	}
	os.WriteFile(p2, []byte("b"), 0o644)
	if p3 := UniquePath(dir, "file.pdf"); p3 != filepath.Join(dir, "file (2).pdf") {
		t.Fatal(p3)
	}
}

func TestUniquePathSuffixesExtensionlessNames(t *testing.T) {
	dir := t.TempDir()
	p1 := UniquePath(dir, "attachment-5")
	os.WriteFile(p1, []byte("a"), 0o644)
	if p2 := UniquePath(dir, "attachment-5"); p2 != filepath.Join(dir, "attachment-5 (1)") {
		t.Fatal(p2)
	}
}

func TestDownloadsDirHonoursXDG(t *testing.T) {
	cfg := t.TempDir()
	target := filepath.Join(t.TempDir(), "dl")
	t.Setenv("XDG_CONFIG_HOME", cfg)
	os.WriteFile(filepath.Join(cfg, "user-dirs.dirs"), []byte("XDG_DOWNLOAD_DIR=\""+target+"\"\n"), 0o644)
	dir, err := DownloadsDir()
	if err != nil || dir != target {
		t.Fatalf("%s %v", dir, err)
	}
	if st, err := os.Stat(target); err != nil || !st.IsDir() {
		t.Fatal("not created")
	}
}
