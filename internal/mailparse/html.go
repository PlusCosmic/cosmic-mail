package mailparse

import (
	"strconv"
	"strings"
	"unicode/utf8"
)

// addHTMLToken appends one text token, decoding it when it is an HTML
// entity (`&amp;`, `&#39;`, `&#x27;`).
func addHTMLToken(out *strings.Builder, token []byte, addSpace bool) {
	if addSpace {
		out.WriteByte(' ')
	}
	if len(token) >= 2 && token[0] == '&' && token[len(token)-1] == ';' {
		entity := token[1 : len(token)-1]
		var code rune
		found := false
		if len(entity) > 0 && entity[0] == '#' {
			digits := entity[1:]
			radix := 10
			if len(digits) > 0 && (digits[0] == 'x' || digits[0] == 'X') {
				digits = digits[1:]
				radix = 16
			}
			if n, err := strconv.ParseUint(string(digits), radix, 32); err == nil {
				code, found = rune(n), true
			}
		} else if r, ok := htmlEntities[string(entity)]; ok {
			code, found = r, true
		}
		if found {
			if !utf8.ValidRune(code) {
				code = utf8.RuneError
			}
			out.WriteRune(code)
			return
		}
	}
	out.Write(token)
}

// HTMLToText is a port of mail-parser's `html_to_text`: a linear tag
// stripper that drops <head>/<style>/<script>/<template> content and
// comments, turns <br> and </p> into newlines, decodes entities, and
// collapses runs of whitespace to single spaces. The snippet fallback and
// the plain-text view of HTML-only messages both go through it, so its
// output must match what the Rust build produced.
func HTMLToText(input string) string {
	var result strings.Builder
	result.Grow(len(input))
	in := []byte(input)

	inTag, inHead, inStyle, inScript, inTemplate, inComment := false, false, false, false, false, false
	isTokenStart, isAfterSpace, isTagClose, isNewLine := true, false, false, true
	tokenStart, tokenEnd := 0, 0
	tagTokenPos, commentPos := 0, 0

	for pos := 0; pos < len(in); pos++ {
		ch := in[pos]
		if inComment {
			switch {
			case ch == '-':
				commentPos++
			case ch == '>' && commentPos == 2:
				commentPos = 0
				inComment = false
				inTag = false
				isTokenStart = true
			default:
				commentPos = 0
			}
			continue
		}
		skipTokenTracking := false
		switch {
		case ch == '<':
			if !inTag && !inHead && !inStyle && !inScript && !inTemplate && !isTokenStart {
				addHTMLToken(&result, in[tokenStart:tokenEnd+1], isAfterSpace)
				isAfterSpace = false
			}
			tagTokenPos = 0
			inTag = true
			isTokenStart = true
			isTagClose = false
			continue
		case ch == '>' && inTag:
			if tagTokenPos == 1 && tokenStart <= tokenEnd && tokenEnd < len(in) {
				tag := in[tokenStart : tokenEnd+1]
				switch {
				case strings.EqualFold(string(tag), "br") || (strings.EqualFold(string(tag), "p") && isTagClose):
					result.WriteByte('\n')
					isAfterSpace = false
					isNewLine = true
				case strings.EqualFold(string(tag), "head"):
					inHead = !isTagClose
				case strings.EqualFold(string(tag), "style"):
					inStyle = !isTagClose
				case strings.EqualFold(string(tag), "script"):
					inScript = !isTagClose
				case strings.EqualFold(string(tag), "template"):
					inTemplate = !isTagClose
				}
			}
			inTag = false
			isTokenStart = true
			continue
		case ch == '/' && inTag:
			if tagTokenPos == 0 {
				isTagClose = true
			}
			continue
		case ch == '!' && inTag && tagTokenPos == 0:
			if pos+3 <= len(in) && string(in[pos+1:pos+3]) == "--" {
				inComment = true
				continue
			}
		case ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n':
			if !inTag && !inHead && !inStyle && !inScript && !inTemplate {
				if !isTokenStart {
					addHTMLToken(&result, in[tokenStart:tokenEnd+1], isAfterSpace && !isNewLine)
					isNewLine = false
				}
				isAfterSpace = true
			}
			isTokenStart = true
			continue
		case ch == '&' && !inTag && !isTokenStart && !inHead:
			if inStyle || inScript || inTemplate {
				continue
			}
			addHTMLToken(&result, in[tokenStart:tokenEnd+1], isAfterSpace && !isNewLine)
			isNewLine = false
			isTokenStart = true
			isAfterSpace = false
		case ch == ';' && !inTag && !isTokenStart && !inHead:
			if inStyle || inScript || inTemplate {
				continue
			}
			addHTMLToken(&result, in[tokenStart:pos+1], isAfterSpace && !isNewLine)
			isTokenStart = true
			isAfterSpace = false
			isNewLine = false
			skipTokenTracking = true
		}
		if skipTokenTracking {
			continue
		}
		if isTokenStart {
			tokenStart = pos
			isTokenStart = false
			if inTag {
				tagTokenPos++
			}
		}
		tokenEnd = pos
	}

	if !inTag && !isTokenStart && !inHead && !inStyle && !inScript && !inTemplate {
		addHTMLToken(&result, in[tokenStart:tokenEnd+1], isAfterSpace && !isNewLine)
	}
	return result.String()
}

// TextToHTML is a port of mail-parser's `text_to_html`, which the Rust
// build used (via `body_html`) to synthesise an HTML view of a text-only
// message: newlines become <br/>, `<` is escaped, CRs are dropped.
func TextToHTML(input string) string {
	var b strings.Builder
	b.Grow(len(input) + 26)
	b.WriteString("<html><body>")
	for i := 0; i < len(input); i++ {
		switch input[i] {
		case '\n':
			b.WriteString("<br/>")
		case '<':
			b.WriteString("&lt;")
		case '\r':
		default:
			b.WriteByte(input[i])
		}
	}
	b.WriteString("</body></html>")
	return b.String()
}
