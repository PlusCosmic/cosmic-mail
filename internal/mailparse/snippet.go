package mailparse

import (
	"mime"
	"strings"
	"unicode"

	"github.com/emersion/go-message/charset"
)

// SnippetMaxChars is the max length (in runes) of a generated preview snippet.
const SnippetMaxChars = 160

// WordDecoder decodes RFC 2047 encoded-words with go-message's charset table.
var WordDecoder = &mime.WordDecoder{CharsetReader: charset.Reader}

// DecodeMIMEWords decodes RFC 2047 encoded-words in a raw header value.
func DecodeMIMEWords(raw string) string {
	decoded, err := WordDecoder.DecodeHeader(raw)
	if err != nil {
		return strings.TrimSpace(raw)
	}
	return strings.TrimSpace(decoded)
}

// UnquoteDisplayName strips lingering RFC 5322 quoted-string syntax from a
// decoded display name.
//
// The IMAP ENVELOPE personal-name is sometimes handed back still carrying the
// quoted-string form from the original From: header: either fully wrapped in
// a pair of literal `"` quotes, or (more commonly, e.g. some marketing
// senders) missing the real outer quotes but still containing escaped `\"`
// sequences. In both cases the backslash-escapes need undoing so the name
// renders as plain text instead of `\"Amazon.co.uk\"`.
func UnquoteDisplayName(name string) string {
	unescape := func(s string) string {
		var out strings.Builder
		out.Grow(len(s))
		rs := []rune(s)
		for i := 0; i < len(rs); i++ {
			if rs[i] == '\\' {
				if i+1 < len(rs) {
					i++
					out.WriteRune(rs[i])
				}
				// A trailing lone backslash has nothing to escape; drop it.
				continue
			}
			out.WriteRune(rs[i])
		}
		return out.String()
	}
	if len(name) >= 2 && strings.HasPrefix(name, `"`) && strings.HasSuffix(name, `"`) {
		return unescape(name[1 : len(name)-1])
	}
	if strings.Contains(name, `\"`) {
		u := unescape(name)
		if len(u) >= 2 && strings.HasPrefix(u, `"`) && strings.HasSuffix(u, `"`) {
			return u[1 : len(u)-1]
		}
		return u
	}
	return name
}

// SnippetForBody computes a single-line preview snippet from a message body.
//
// Prefers text (the text/plain part); when that yields nothing usable, falls
// back to html converted to plain text via HTMLToText before the same
// cleanup. Returns nil when there is nothing to show, so callers can leave a
// prior snippet untouched (COALESCE) or store an empty snippet.
//
// Also falls back to html (when present) when text *looks* machine-glued —
// some senders' text/plain parts are tag-stripped HTML with no separators
// between cells (Uber receipts: "15 Jul 202620:44Refunded…"). Text-only
// messages always keep the text-derived snippet. Whichever source is chosen,
// if it is itself machine-glued it is run through deglue first; deglue is
// only ever applied to sources looksMachineGlued flagged, since it would
// split brand names like "PayPal" in normal prose.
func SnippetForBody(text, html *string) *string {
	if text != nil {
		glued := looksMachineGlued(*text)
		preferText := html == nil || !glued
		if preferText {
			var snippet string
			if glued {
				snippet = SnippetFromText(deglue(*text))
			} else {
				snippet = SnippetFromText(*text)
			}
			if snippet != "" {
				return &snippet
			}
		}
	}
	if html == nil {
		return nil
	}
	converted := HTMLToText(*html)
	source := converted
	if looksMachineGlued(converted) {
		source = deglue(converted)
	}
	snippet := SnippetFromText(source)
	if snippet == "" {
		return nil
	}
	return &snippet
}

const (
	gluedTextSampleChars              = 200
	gluedTextMinSampleChars           = 40
	gluedTextWhitespaceRatioThreshold = 0.08
	gluedTextBoundaryThreshold        = 3
)

// looksMachineGlued reports whether the start of text looks like
// machine-glued, tag-stripped HTML rather than prose: either a very low
// whitespace ratio, or several digit→letter / letter→digit / lower→UPPER
// adjacencies over the first 200 characters. Short text never qualifies.
func looksMachineGlued(text string) bool {
	sample := []rune(strings.TrimLeftFunc(text, unicode.IsSpace))
	if len(sample) > gluedTextSampleChars {
		sample = sample[:gluedTextSampleChars]
	}
	if len(sample) < gluedTextMinSampleChars {
		return false
	}
	whitespace := 0
	for _, c := range sample {
		if unicode.IsSpace(c) {
			whitespace++
		}
	}
	ratio := float64(whitespace) / float64(len(sample))
	return ratio < gluedTextWhitespaceRatioThreshold || glueBoundaryCount(sample) >= gluedTextBoundaryThreshold
}

func isASCIIDigit(c rune) bool  { return c >= '0' && c <= '9' }
func isASCIIUpper(c rune) bool  { return c >= 'A' && c <= 'Z' }
func isASCIILower(c rune) bool  { return c >= 'a' && c <= 'z' }
func isASCIILetter(c rune) bool { return isASCIIUpper(c) || isASCIILower(c) }

func glueBoundaryCount(sample []rune) int {
	n := 0
	for i := 0; i+1 < len(sample); i++ {
		a, b := sample[i], sample[i+1]
		if (isASCIIDigit(a) && isASCIILetter(b)) || (isASCIILetter(a) && isASCIIDigit(b)) || (isASCIILower(a) && isASCIIUpper(b)) {
			n++
		}
	}
	return n
}

// deglue inserts a single space at cell-seam boundaries in machine-glued
// text: digit→Upper, lower→Upper, letter→digit, and `.,!?;:`→Upper. It
// deliberately leaves digit→lower ("3pm"), digit→digit and punctuation→lower
// ("example.com") alone.
func deglue(text string) string {
	var out strings.Builder
	out.Grow(len(text) + len(text)/8)
	var prev rune
	hasPrev := false
	for _, c := range text {
		if hasPrev {
			split := (isASCIIDigit(prev) && isASCIIUpper(c)) ||
				(isASCIILower(prev) && isASCIIUpper(c)) ||
				(isASCIILetter(prev) && isASCIIDigit(c)) ||
				(strings.ContainsRune(".,!?;:", prev) && isASCIIUpper(c))
			if split {
				out.WriteByte(' ')
			}
		}
		out.WriteRune(c)
		prev, hasPrev = c, true
	}
	return out.String()
}

// SnippetFromText computes a single-line snippet from a plain-text body
// (~160 chars): strips invisible preheader padding, bare and bracket-wrapped
// URLs and raw HTML tags, collapses whitespace, and trims punctuation debris
// at the edges.
func SnippetFromText(text string) string {
	depadded := stripInvisiblePadding(text)
	stripped := stripURLs(depadded)
	collapsed := strings.Join(strings.Fields(stripped), " ")
	trimmed := strings.TrimFunc(collapsed, func(c rune) bool {
		return unicode.IsSpace(c) || strings.ContainsRune("-–—|*><()[]•·:;,", c)
	})
	rs := []rune(trimmed)
	if len(rs) > SnippetMaxChars {
		rs = rs[:SnippetMaxChars]
	}
	return string(rs)
}

// paddingEntities are named entities some senders abuse as invisible
// preheader padding, with their plain-text replacement. `&nbsp;` becomes a
// space so the words either side of it don't glue together.
var paddingEntities = [][2]string{
	{"&zwnj;", ""},
	{"&zwj;", ""},
	{"&shy;", ""},
	{"&nbsp;", " "},
}

func isInvisiblePaddingChar(c rune) bool {
	switch {
	case c >= 0x200B && c <= 0x200D: // zero-width space/non-joiner/joiner
		return true
	case c == 0xFEFF, c == 0x034F, c == 0x00AD: // BOM, combining grapheme joiner, soft hyphen
		return true
	}
	return false
}

func stripInvisiblePadding(text string) string {
	result := text
	for _, e := range paddingEntities {
		result = ReplaceASCIICaseInsensitive(result, e[0], e[1])
	}
	var out strings.Builder
	out.Grow(len(result))
	for _, c := range result {
		if !isInvisiblePaddingChar(c) {
			out.WriteRune(c)
		}
	}
	return out.String()
}

// stripURLs removes http(s) URLs (bare, or wrapped in (), [], <>) and
// tag-like <...> spans; a bare `<`/`>` used as "less than" survives.
func stripURLs(text string) string {
	chars := []rune(text)
	var out strings.Builder
	out.Grow(len(text))
	i := 0
	for i < len(chars) {
		c := chars[i]
		var closing rune
		switch c {
		case '(':
			closing = ')'
		case '[':
			closing = ']'
		case '<':
			closing = '>'
		}
		if closing != 0 && isURLStart(chars, i+1) {
			if off := indexRune(chars[i+1:], closing); off >= 0 {
				i = i + 1 + off + 1
			} else {
				i = len(chars) // unbalanced bracket: drop the rest
			}
			continue
		}
		if c == '<' && isTagStart(chars, i+1) {
			if off := indexRune(chars[i+1:], '>'); off >= 0 {
				i = i + 1 + off + 1
			} else {
				i = len(chars) // unterminated tag: drop the rest
			}
			continue
		}
		if isURLStart(chars, i) {
			j := i
			for j < len(chars) && !unicode.IsSpace(chars[j]) {
				j++
			}
			i = j
			continue
		}
		out.WriteRune(c)
		i++
	}
	return out.String()
}

func indexRune(rs []rune, want rune) int {
	for i, r := range rs {
		if r == want {
			return i
		}
	}
	return -1
}

func isURLStart(chars []rune, idx int) bool {
	for _, scheme := range []string{"http://", "https://"} {
		s := []rune(scheme)
		if idx+len(s) <= len(chars) && string(chars[idx:idx+len(s)]) == scheme {
			return true
		}
	}
	return false
}

func isTagStart(chars []rune, idx int) bool {
	if idx >= len(chars) {
		return false
	}
	c := chars[idx]
	return c == '/' || c == '!' || isASCIILetter(c)
}

// ReplaceASCIICaseInsensitive is an ASCII case-insensitive, literal
// (non-regex) substring replacement.
func ReplaceASCIICaseInsensitive(haystack, needle, replacement string) string {
	if needle == "" {
		return haystack
	}
	hayLower := asciiLower(haystack)
	needleLower := asciiLower(needle)
	var out strings.Builder
	out.Grow(len(haystack))
	i := 0
	for i < len(haystack) {
		if strings.HasPrefix(hayLower[i:], needleLower) {
			out.WriteString(replacement)
			i += len(needleLower)
			continue
		}
		_, size := decodeRuneAt(haystack, i)
		out.WriteString(haystack[i : i+size])
		i += size
	}
	return out.String()
}

func decodeRuneAt(s string, i int) (rune, int) {
	for j, r := range s[i:] {
		if j == 0 {
			size := len(string(r))
			if r == unicode.ReplacementChar && !strings.HasPrefix(s[i:], string(unicode.ReplacementChar)) {
				size = 1
			}
			return r, size
		}
	}
	return 0, 1
}

// asciiLower lowercases ASCII letters only, so byte offsets stay aligned
// with the original string.
func asciiLower(s string) string {
	b := []byte(s)
	for i, c := range b {
		if c >= 'A' && c <= 'Z' {
			b[i] = c + 32
		}
	}
	return string(b)
}
