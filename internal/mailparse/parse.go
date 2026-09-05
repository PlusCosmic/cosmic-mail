// Package mailparse turns raw RFC 822 bytes into the cached body parts,
// attachment metadata and preview snippets the store keeps.
//
// It is a port of the parsing half of the Rust `sync/imap.rs` — the part
// classification follows mail-parser's rules (which text parts are the
// bodies, which parts are listed attachments, which are inline images) so
// that messages cached by the Rust build and by this one look the same, and
// so the deterministic part index stored in `attachments.part_index` keeps
// pointing at the same MIME part when an attachment is refetched.
package mailparse

import (
	"bytes"
	"encoding/base64"
	"errors"
	"io"
	"strings"

	"github.com/emersion/go-message"
	_ "github.com/emersion/go-message/charset" // registers the charset table
	gomail "github.com/emersion/go-message/mail"

	"cosmicmail/internal/store"
)

const (
	// InlineImageMaxBytes is the per-part decoded-size cap for inlining a
	// `cid:` image as a `data:` URI.
	InlineImageMaxBytes = 512 * 1024
	// InlineImageTotalBudget is the per-message total budget for inlined
	// `cid:` image payloads.
	InlineImageTotalBudget = 2 * 1024 * 1024
)

// FetchedBody is a fetched, parsed message body.
type FetchedBody struct {
	Text *string
	// HTML body with in-cap inline `cid:` images already rewritten to `data:` URIs.
	HTML    *string
	ToAddrs []string
	CcAddrs []string
	// Attachment metadata (listed attachments and inline `cid:` parts).
	Attachments []store.AttachmentMeta
}

type mimeKind int

const (
	kindOther mimeKind = iota
	kindMultipartMixed
	kindMultipartAlternative
	kindMultipartRelated
	kindMultipartDigest
	kindMultipartOther
	kindTextPlain
	kindTextHTML
	kindTextOther
	kindInline // image, audio, video
	kindMessage
)

func (k mimeKind) isMultipart() bool {
	return k == kindMultipartMixed || k == kindMultipartAlternative || k == kindMultipartRelated || k == kindMultipartDigest || k == kindMultipartOther
}

// part is one MIME part in deterministic (DFS pre-order) parse order.
type part struct {
	kind        mimeKind
	mediaType   string // lowercased type/subtype, "" when absent
	ctParams    map[string]string
	disposition string
	dispParams  map[string]string
	contentID   string
	isHTML      bool
	isText      bool
	contents    []byte // decoded bytes (charset-converted for text)
}

// parsed is the flat view of a message, mirroring mail-parser's Message.
type parsed struct {
	parts       []*part
	textBody    []int
	htmlBody    []int
	attachments []int
	toAddrs     []string
	ccAddrs     []string
}

// containerState mirrors mail-parser's MessageParserState for one
// multipart (or message) container.
type containerState struct {
	kind          mimeKind
	inAlternative bool
	parts         int
	needHTMLBody  bool
	needTextBody  bool
}

func classify(mediaType string, parent mimeKind) (kind mimeKind, isInline, isText bool) {
	if mediaType == "" {
		if parent == kindMultipartDigest {
			return kindMessage, false, false
		}
		return kindTextPlain, true, true
	}
	typ, sub, _ := strings.Cut(mediaType, "/")
	switch typ {
	case "multipart":
		switch sub {
		case "mixed":
			return kindMultipartMixed, false, false
		case "alternative":
			return kindMultipartAlternative, false, false
		case "related":
			return kindMultipartRelated, false, false
		case "digest":
			return kindMultipartDigest, false, false
		}
		return kindMultipartOther, false, false
	case "text":
		switch sub {
		case "plain":
			return kindTextPlain, true, true
		case "html":
			return kindTextHTML, true, true
		}
		return kindTextOther, false, true
	case "image", "audio", "video":
		return kindInline, true, false
	case "message":
		if sub == "rfc822" || sub == "global" {
			return kindMessage, false, false
		}
	}
	return kindOther, false, false
}

// Parse parses raw RFC 822 bytes. It never fails on malformed input: an
// unparseable message becomes a single text/plain body holding the raw
// bytes, the way the Rust build's lenient parser would have read it.
func Parse(raw []byte) *parsed {
	p := &parsed{}
	entity, err := message.Read(bytes.NewReader(raw))
	if entity == nil || (err != nil && !message.IsUnknownCharset(err) && !message.IsUnknownEncoding(err)) {
		text := string(raw)
		p.parts = []*part{{kind: kindTextPlain, mediaType: "text/plain", isText: true, contents: []byte(text)}}
		p.textBody = []int{0}
		p.htmlBody = []int{0}
		return p
	}
	if list, err := (&gomail.Header{Header: entity.Header}).AddressList("To"); err == nil {
		p.toAddrs = formatAddrs(list)
	}
	if list, err := (&gomail.Header{Header: entity.Header}).AddressList("Cc"); err == nil {
		p.ccAddrs = formatAddrs(list)
	}
	root := &containerState{kind: kindMessage, needHTMLBody: true, needTextBody: true}
	p.walk(entity, root)
	return p
}

func formatAddrs(list []*gomail.Address) []string {
	out := make([]string, 0, len(list))
	for _, a := range list {
		if a == nil || a.Address == "" {
			continue
		}
		if a.Name != "" {
			out = append(out, a.Name+" <"+a.Address+">")
		} else {
			out = append(out, a.Address)
		}
	}
	return out
}

// walk visits entity as the next part of the container described by state,
// assigning it the next flat index, then descends into its children.
func (p *parsed) walk(e *message.Entity, state *containerState) {
	state.parts++
	mediaType, ctParams, ctErr := e.Header.ContentType()
	if ctErr != nil {
		mediaType, ctParams = "", nil
	}
	if e.Header.Get("Content-Type") == "" {
		mediaType = ""
	}
	mediaType = strings.ToLower(strings.TrimSpace(mediaType))
	kind, isInline, isText := classify(mediaType, state.kind)
	disposition, dispParams, dispErr := e.Header.ContentDisposition()
	if dispErr != nil {
		disposition, dispParams = "", nil
	}
	disposition = strings.ToLower(strings.TrimSpace(disposition))

	pt := &part{kind: kind, mediaType: mediaType, ctParams: ctParams, disposition: disposition, dispParams: dispParams,
		contentID: strings.TrimSpace(e.Header.Get("Content-ID")), isText: isText}
	index := len(p.parts)
	p.parts = append(p.parts, pt)

	if kind.isMultipart() {
		if mr := e.MultipartReader(); mr != nil {
			child := &containerState{
				kind:          kind,
				inAlternative: state.inAlternative || kind == kindMultipartAlternative,
				needHTMLBody:  state.needHTMLBody,
				needTextBody:  state.needTextBody,
			}
			for {
				sub, err := mr.NextPart()
				if sub == nil || (err != nil && !message.IsUnknownCharset(err) && !message.IsUnknownEncoding(err)) {
					break
				}
				p.walk(sub, child)
			}
			// Body selections made inside an alternative group are the
			// container's business, but the need_* flags propagate back the
			// way mail-parser's state stack pops them.
			state.needHTMLBody, state.needTextBody = child.needHTMLBody, child.needTextBody
			return
		}
		// A multipart with no usable boundary parses as text/other.
		pt.kind, kind, isText, isInline = kindTextOther, kindTextOther, true, false
	}

	body, _ := io.ReadAll(e.Body)
	pt.contents = body

	if kind == kindMessage {
		// Nested message: an attachment part, whose own parts follow it in
		// the flat order.
		p.attachments = append(p.attachments, index)
		if nested, err := message.Read(bytes.NewReader(body)); nested != nil && (err == nil || message.IsUnknownCharset(err) || message.IsUnknownEncoding(err)) {
			inner := &containerState{kind: kindMessage, needHTMLBody: true, needTextBody: true}
			p.walkNestedChildren(nested, inner)
		}
		return
	}

	inline := isInline && disposition != "attachment" &&
		(state.parts == 1 || (state.kind != kindMultipartRelated && (kind == kindInline || ctParams["name"] == "")))

	var addToHTML, addToText bool
	if state.kind == kindMultipartAlternative {
		switch kind {
		case kindTextHTML:
			addToHTML = true
		case kindTextPlain:
			addToText = true
		}
	} else if inline {
		if state.inAlternative && (state.needTextBody || state.needHTMLBody) {
			switch kind {
			case kindTextHTML:
				state.needTextBody = false
			case kindTextPlain:
				state.needHTMLBody = false
			}
		}
		addToHTML, addToText = state.needHTMLBody, state.needTextBody
	}
	if addToHTML {
		p.htmlBody = append(p.htmlBody, index)
	}
	if addToText {
		p.textBody = append(p.textBody, index)
	}
	if isText {
		pt.isHTML = kind == kindTextHTML
		if (!addToHTML && pt.isHTML) || (!addToText && !pt.isHTML) {
			p.attachments = append(p.attachments, index)
		}
		return
	}
	p.attachments = append(p.attachments, index)
}

// walkNestedChildren walks the parts of a nested message/rfc822 entity
// without giving the nested root its own index (the enclosing attachment
// part already has one).
func (p *parsed) walkNestedChildren(e *message.Entity, state *containerState) {
	mediaType, _, err := e.Header.ContentType()
	if err == nil && strings.HasPrefix(strings.ToLower(mediaType), "multipart/") {
		if mr := e.MultipartReader(); mr != nil {
			kind, _, _ := classify(strings.ToLower(mediaType), state.kind)
			child := &containerState{kind: kind, inAlternative: kind == kindMultipartAlternative, needHTMLBody: true, needTextBody: true}
			for {
				sub, err := mr.NextPart()
				if sub == nil || (err != nil && !message.IsUnknownCharset(err) && !message.IsUnknownEncoding(err)) {
					break
				}
				p.walk(sub, child)
			}
		}
		return
	}
	p.walk(e, state)
}

// bodyText mirrors mail-parser's `body_text(0)`: the first text body, or the
// first html body converted to text.
func (p *parsed) bodyText() *string {
	if len(p.textBody) == 0 {
		return nil
	}
	pt := p.parts[p.textBody[0]]
	var s string
	if pt.isHTML {
		s = HTMLToText(string(pt.contents))
	} else {
		s = string(pt.contents)
	}
	return &s
}

// bodyHTML mirrors mail-parser's `body_html(0)`: the first html body, or the
// first text body wrapped as HTML.
func (p *parsed) bodyHTML() *string {
	if len(p.htmlBody) == 0 {
		return nil
	}
	pt := p.parts[p.htmlBody[0]]
	var s string
	if pt.isHTML {
		s = string(pt.contents)
	} else {
		s = TextToHTML(string(pt.contents))
	}
	return &s
}

// ParseBody decodes raw RFC 822 bytes into cached body parts plus
// attachment metadata. Inline `cid:` image references in the HTML body are
// rewritten to `data:` URIs under strict per-part and per-message size caps;
// over-cap parts keep their `cid:` reference (which renders blank under the
// reader CSP). No network I/O.
func ParseBody(raw []byte) FetchedBody {
	p := Parse(raw)
	html := p.bodyHTML()
	if html != nil {
		if images := p.inlineImageDataURIs(); len(images) > 0 {
			rewritten := rewriteCIDReferences(*html, images)
			html = &rewritten
		}
	}
	return FetchedBody{
		Text:        p.bodyText(),
		HTML:        html,
		ToAddrs:     nonNil(p.toAddrs),
		CcAddrs:     nonNil(p.ccAddrs),
		Attachments: p.extractAttachments(),
	}
}

func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}

// AttachmentBytes re-parses raw and returns the decoded contents of the part
// at partIndex (a stable index from a previous ParseBody).
func AttachmentBytes(raw []byte, partIndex uint32) ([]byte, error) {
	p := Parse(raw)
	if int(partIndex) >= len(p.parts) {
		return nil, errors.New("attachment part not found in message")
	}
	return p.parts[partIndex].contents, nil
}

func (pt *part) mimeType() string {
	if pt.mediaType == "" {
		if pt.kind == kindTextPlain {
			return "text/plain"
		}
		return "application/octet-stream"
	}
	return pt.mediaType
}

// NormalizeContentID strips surrounding whitespace and angle brackets.
func NormalizeContentID(cid string) string {
	cid = strings.TrimSpace(cid)
	cid = strings.TrimPrefix(cid, "<")
	cid = strings.TrimSuffix(cid, ">")
	return strings.TrimSpace(cid)
}

// attachmentName mirrors mail-parser's attachment_name: the disposition
// filename, else the content-type name.
func (pt *part) attachmentName() string {
	if n := pt.dispParams["filename"]; n != "" {
		return n
	}
	return pt.ctParams["name"]
}

// extractAttachments lists attachment metadata (listed attachments and
// inline cid: parts) in deterministic parse order.
func (p *parsed) extractAttachments() []store.AttachmentMeta {
	out := []store.AttachmentMeta{}
	for _, idx := range p.attachments {
		pt := p.parts[idx]
		// Nested messages and multipart containers: no downloadable payload.
		if pt.kind == kindMessage || pt.kind.isMultipart() {
			continue
		}
		var contentID *string
		if cid := NormalizeContentID(pt.contentID); cid != "" {
			contentID = &cid
		}
		dispositionAttachment := pt.disposition == "attachment"
		// A part with a Content-ID that is not an explicit attachment is
		// inline (multipart/related embedded images render via cid:).
		isInline := !dispositionAttachment && contentID != nil
		// go-message decodes RFC 2231 parameters; RFC 2047 encoded-words
		// inside a filename value are decoded here.
		filename := DecodeMIMEWords(pt.attachmentName())
		out = append(out, store.AttachmentMeta{
			PartIndex: uint32(idx),
			Filename:  filename,
			MimeType:  pt.mimeType(),
			SizeBytes: uint32(len(pt.contents)),
			IsInline:  isInline,
			ContentID: contentID,
		})
	}
	return out
}

// inlineImageDataURIs builds (lowercased content-id, data: URI) pairs for
// inline images within the per-part and per-message size caps.
func (p *parsed) inlineImageDataURIs() [][2]string {
	total := 0
	var out [][2]string
	for _, idx := range p.attachments {
		pt := p.parts[idx]
		if pt.contentID == "" {
			continue
		}
		mime := pt.mimeType()
		if !strings.HasPrefix(mime, "image/") {
			continue
		}
		n := len(pt.contents)
		if n == 0 || n > InlineImageMaxBytes {
			continue
		}
		if total+n > InlineImageTotalBudget {
			continue // over the per-message budget: leave the cid: reference
		}
		total += n
		cid := NormalizeContentID(pt.contentID)
		if cid == "" {
			continue
		}
		out = append(out, [2]string{strings.ToLower(cid), "data:" + mime + ";base64," + base64.StdEncoding.EncodeToString(pt.contents)})
	}
	return out
}

// rewriteCIDReferences replaces each `cid:<content-id>` reference in html
// with its data: URI, ASCII case-insensitively.
func rewriteCIDReferences(html string, images [][2]string) string {
	result := html
	for _, img := range images {
		result = ReplaceASCIICaseInsensitive(result, "cid:"+img[0], img[1])
	}
	return result
}
