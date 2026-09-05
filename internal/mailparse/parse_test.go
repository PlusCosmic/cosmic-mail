package mailparse

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"strings"
	"testing"
)

func b64(b []byte) string { return base64.StdEncoding.EncodeToString(b) }

func TestExtractsAttachmentMetadataAndMarksNonInline(t *testing.T) {
	raw := "From: a@b.com\r\nTo: c@d.com\r\nSubject: Test\r\nMIME-Version: 1.0\r\n" +
		"Content-Type: multipart/mixed; boundary=\"BOUND\"\r\n\r\n" +
		"--BOUND\r\nContent-Type: text/plain\r\n\r\nHello body\r\n" +
		"--BOUND\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\n" +
		"Content-Disposition: attachment; filename=\"report.pdf\"\r\n" +
		"Content-Transfer-Encoding: base64\r\n\r\n" + b64([]byte("PDF-CONTENT")) + "\r\n--BOUND--\r\n"
	body := ParseBody([]byte(raw))
	if len(body.Attachments) != 1 {
		t.Fatalf("attachments: %+v", body.Attachments)
	}
	att := body.Attachments[0]
	if att.Filename != "report.pdf" || att.MimeType != "application/pdf" || att.SizeBytes != uint32(len("PDF-CONTENT")) || att.IsInline || att.ContentID != nil {
		t.Fatalf("%+v", att)
	}
	// Deterministic part index: root multipart = 0, text = 1, pdf = 2.
	if att.PartIndex != 2 {
		t.Fatalf("part index %d", att.PartIndex)
	}
	if body.Text == nil || strings.TrimSpace(*body.Text) != "Hello body" {
		t.Fatalf("text: %v", deref(body.Text))
	}
	if len(body.ToAddrs) != 1 || body.ToAddrs[0] != "c@d.com" {
		t.Fatalf("to: %v", body.ToAddrs)
	}
	got, err := AttachmentBytes([]byte(raw), 2)
	if err != nil || string(got) != "PDF-CONTENT" {
		t.Fatalf("AttachmentBytes: %q %v", got, err)
	}
}

func TestDecodesRFC2047EncodedFilenames(t *testing.T) {
	encoded := fmt.Sprintf("=?utf-8?B?%s?=", b64([]byte("föö.pdf")))
	raw := "Subject: t\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n" +
		"--B\r\nContent-Type: text/plain\r\n\r\nbody\r\n" +
		"--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"" + encoded + "\"\r\n" +
		"Content-Transfer-Encoding: base64\r\n\r\n" + b64([]byte("data")) + "\r\n--B--\r\n"
	body := ParseBody([]byte(raw))
	if len(body.Attachments) != 1 || body.Attachments[0].Filename != "föö.pdf" {
		t.Fatalf("%+v", body.Attachments)
	}
}

func relatedMessage(html string, imageParts string) string {
	return "Subject: t\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n" +
		"--R\r\nContent-Type: text/html\r\n\r\n" + html + "\r\n" + imageParts + "--R--\r\n"
}

func TestInlinesCIDImageWithinCaps(t *testing.T) {
	png := bytes.Repeat([]byte{0x89}, 1024)
	raw := relatedMessage("<html><body><img src=\"cid:img1@example.com\"></body></html>",
		"--R\r\nContent-Type: image/png\r\nContent-ID: <img1@example.com>\r\nContent-Transfer-Encoding: base64\r\n\r\n"+b64(png)+"\r\n")
	body := ParseBody([]byte(raw))
	if body.HTML == nil || !strings.Contains(*body.HTML, "data:image/png;base64,") || strings.Contains(*body.HTML, "cid:img1") {
		t.Fatalf("html: %v", deref(body.HTML))
	}
	var inline []int
	for i, a := range body.Attachments {
		if a.IsInline {
			inline = append(inline, i)
		}
	}
	if len(inline) != 1 {
		t.Fatalf("inline: %+v", body.Attachments)
	}
	a := body.Attachments[inline[0]]
	if a.ContentID == nil || *a.ContentID != "img1@example.com" || a.MimeType != "image/png" {
		t.Fatalf("%+v", a)
	}
	// The text view of an HTML-only message is the converted HTML.
	if body.Text == nil {
		t.Fatal("expected converted text")
	}
}

func TestLeavesOverPerPartCapImageAsCID(t *testing.T) {
	png := bytes.Repeat([]byte{0x89}, InlineImageMaxBytes+1)
	raw := relatedMessage("<img src=\"cid:big@x\">",
		"--R\r\nContent-Type: image/png\r\nContent-ID: <big@x>\r\nContent-Transfer-Encoding: base64\r\n\r\n"+b64(png)+"\r\n")
	body := ParseBody([]byte(raw))
	if body.HTML == nil || !strings.Contains(*body.HTML, "cid:big@x") || strings.Contains(*body.HTML, "data:image/png") {
		t.Fatalf("html: %v", deref(body.HTML))
	}
}

func TestEnforcesTotalInlineBudget(t *testing.T) {
	png := bytes.Repeat([]byte{0x89}, 500*1024)
	var parts, html strings.Builder
	html.WriteString("<html><body>")
	for i := 0; i < 5; i++ {
		fmt.Fprintf(&html, "<img src=\"cid:i%d@x\">", i)
		fmt.Fprintf(&parts, "--R\r\nContent-Type: image/png\r\nContent-ID: <i%d@x>\r\nContent-Transfer-Encoding: base64\r\n\r\n%s\r\n", i, b64(png))
	}
	html.WriteString("</body></html>")
	body := ParseBody([]byte(relatedMessage(html.String(), parts.String())))
	if body.HTML == nil {
		t.Fatal("no html")
	}
	if n := strings.Count(*body.HTML, "data:image/png;base64,"); n != 4 {
		t.Fatalf("inlined %d, want 4", n)
	}
	if !strings.Contains(*body.HTML, "cid:i4@x") {
		t.Fatal("fifth image should stay a cid reference")
	}
}

func TestTextOnlyMessageGetsSynthesisedHTMLAndNoAttachments(t *testing.T) {
	raw := "From: Shipping <ship@example.com>\r\nTo: test@localhost\r\nSubject: Your order has shipped\r\n" +
		"Content-Type: text/plain; charset=utf-8\r\n\r\nGood news — line one\r\nline two\r\n"
	body := ParseBody([]byte(raw))
	if body.Text == nil || !strings.HasPrefix(*body.Text, "Good news — line one") {
		t.Fatalf("text: %v", deref(body.Text))
	}
	if body.HTML == nil || !strings.HasPrefix(*body.HTML, "<html><body>Good news — line one<br/>") {
		t.Fatalf("html: %v", deref(body.HTML))
	}
	if len(body.Attachments) != 0 {
		t.Fatalf("attachments: %+v", body.Attachments)
	}
}

func TestAlternativePicksBothBodiesAndListsOnlyRealAttachments(t *testing.T) {
	raw := "Subject: t\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"M\"\r\n\r\n" +
		"--M\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n" +
		"--A\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nCaf=E9 plain\r\n" +
		"--A\r\nContent-Type: text/html\r\n\r\n<p>Caf&eacute; html</p>\r\n" +
		"--A--\r\n" +
		"--M\r\nContent-Type: application/zip; name=\"archive.zip\"\r\nContent-Transfer-Encoding: base64\r\n\r\n" + b64([]byte("zipzip")) + "\r\n" +
		"--M--\r\n"
	body := ParseBody([]byte(raw))
	if body.Text == nil || strings.TrimSpace(*body.Text) != "Café plain" {
		t.Fatalf("text: %v", deref(body.Text))
	}
	if body.HTML == nil || !strings.Contains(*body.HTML, "Caf&eacute; html") {
		t.Fatalf("html: %v", deref(body.HTML))
	}
	if len(body.Attachments) != 1 || body.Attachments[0].Filename != "archive.zip" || body.Attachments[0].PartIndex != 4 || body.Attachments[0].IsInline {
		t.Fatalf("%+v", body.Attachments)
	}
}

func TestMalformedInputNeverPanics(t *testing.T) {
	for _, raw := range []string{"", "garbage", "Content-Type: multipart/mixed; boundary=x\r\n\r\nnot really", "Subject: only\r\n"} {
		body := ParseBody([]byte(raw))
		if body.Attachments == nil || body.ToAddrs == nil {
			t.Fatalf("nil lists for %q", raw)
		}
	}
}
