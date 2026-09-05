package mailparse

import (
	"strings"
	"testing"
)

func s(v string) *string { return &v }

func TestUnquoteDisplayName(t *testing.T) {
	cases := map[string]string{
		`\"Amazon.co.uk\"`:                      "Amazon.co.uk",
		`say \"hi\" now`:                        `say "hi" now`,
		`AC\DC`:                                 `AC\DC`,
		`"Plain Name"`:                          "Plain Name",
		`"He said \"hi\""`:                      `He said "hi"`,
		"Jane Doe":                              "Jane Doe",
		`"`:                                     `"`,
		"":                                      "",
		`""`:                                    "",
		DecodeMIMEWords("=?UTF-8?B?Sm9zw6k=?="): "José",
	}
	for in, want := range cases {
		if got := UnquoteDisplayName(in); got != want {
			t.Errorf("UnquoteDisplayName(%q) = %q, want %q", in, got, want)
		}
	}
	if got := DecodeMIMEWords("=?UTF-8?B?Sm9zw6k=?="); got != "José" {
		t.Fatalf("decode: %q", got)
	}
}

func TestCIDRewriteIsCaseInsensitiveAndLiteral(t *testing.T) {
	if got := ReplaceASCIICaseInsensitive("<img src=CID:Foo@X>", "cid:foo@x", "data:x"); got != "<img src=data:x>" {
		t.Fatal(got)
	}
	if got := ReplaceASCIICaseInsensitive("cid:a.b+c", "cid:a.b+c", "D"); got != "D" {
		t.Fatal(got)
	}
	if got := ReplaceASCIICaseInsensitive("nothing here", "cid:x", "D"); got != "nothing here" {
		t.Fatal(got)
	}
	if got := ReplaceASCIICaseInsensitive("héllo CID:x wörld", "cid:x", "Y"); got != "héllo Y wörld" {
		t.Fatal(got)
	}
}

func TestSnippetFromText(t *testing.T) {
	cases := []struct{ in, want string }{
		{"tick tock backblaze logo (https://link.example.com/xyz) Big Sale! [https://track.example.com/abc?x=1] View in browser", "tick tock backblaze logo Big Sale! View in browser"},
		{"Check this out: https://example.com/path?q=1 today", "Check this out: today"},
		{"See <https://example.com/x> for details", "See for details"},
		{"Order #1234 (thanks!) has shipped", "Order #1234 (thanks!) has shipped"},
		{"Hi team, quick update on the roadmap for next week.", "Hi team, quick update on the roadmap for next week."},
		{"  -- View in browser (https://example.com/a) --  ", "View in browser"},
		{"\n\n\n   \n\n <link rel=stylesheet href=https://marber-cdn.economist.com/foundations/latest/css/font-face.css>\nThe Economist July 16th 2026 For subscribers", "The Economist July 16th 2026 For subscribers"},
		{"Before <div class=\"foo\" id='bar'> After", "Before After"},
		{"Hello </p> world </div>", "Hello world"},
		{"Real content <div class=\"unterminated", "Real content"},
		{"1 < 2 and 5 > 4", "1 < 2 and 5 > 4"},
		{"aw <3 that's sweet", "aw <3 that's sweet"},
		{"See <https://example.com/x> for details <br> ok", "See for details ok"},
		{"A Message From Our Team &zwnj; &zwnj; &zwnj; &zwnj; Here is the real update for this week.", "A Message From Our Team Here is the real update for this week."},
		{"A Message From Our Team &zwnj;͏ &zwnj;͏ &zwnj;͏ Real content here", "A Message From Our Team Real content here"},
		{"Hello&nbsp;World", "Hello World"},
		{"Before &zwj; &shy; After", "Before After"},
		{"Hi &ZWNJ; there", "Hi there"},
		{"Hi&NBSP;there", "Hi there"},
	}
	for _, c := range cases {
		if got := SnippetFromText(c.in); got != c.want {
			t.Errorf("SnippetFromText(%q) = %q, want %q", c.in, got, c.want)
		}
	}
	long := strings.Repeat("word ", 100)
	if got := SnippetFromText(long); len([]rune(got)) != SnippetMaxChars {
		t.Fatalf("cap: %d", len([]rune(got)))
	}
}

func TestSnippetForBody(t *testing.T) {
	html := "<html><head><style type=\"text/css\">\nbody { font-family: Arial; color: #333; }\n.row > .cell { display:flex; }\n</style><script>var x = 1 < 2;</script></head>\n<body><p>Hello team,</p><p>Here&#39;s the update &amp; summary.</p>\n<p>&nbsp;</p></body></html>"
	if got := SnippetForBody(nil, s(html)); got == nil || *got != "Hello team, Here's the update & summary." {
		t.Fatalf("html fallback: %v", deref(got))
	}
	if got := SnippetForBody(s("Plain text wins"), s("<p>HTML loses</p>")); got == nil || *got != "Plain text wins" {
		t.Fatalf("prefers text: %v", deref(got))
	}
	if got := SnippetForBody(s("   "), s("<p>Fallback content</p>")); got == nil || *got != "Fallback content" {
		t.Fatalf("blank text: %v", deref(got))
	}
	if got := SnippetForBody(nil, nil); got != nil {
		t.Fatal("nil/nil")
	}
	if got := SnippetForBody(s(""), s("")); got != nil {
		t.Fatal("empty/empty")
	}
	if got := SnippetForBody(s("   "), s("<style>a{}</style>")); got != nil {
		t.Fatalf("style only: %v", *got)
	}
}

const gluedUberStyleText = "TripreceiptTotal$24.5015Jul202620:44Refunded15Jul2026,20:44PaymentVisa1234SubtotalCityToCityFare$20.00BookingFee$2.50Tolls$2.00ThanksforridingwithUber"

const partiallyGluedUberReproText = "15 Jul 202620:44Refunded15 Jul 2026 , 20:44Just a quick update, Harry\nWe adjusted the total for your trip to Hoe Street.Total£19.54 Your refund has been applied.To view your updated receipt, open the app."

func TestSnippetForBodyGluedText(t *testing.T) {
	html := "<html><body><p>Trip receipt total is $24.50, refunded on 15 Jul 2026.</p></body></html>"
	if got := SnippetForBody(s(gluedUberStyleText), s(html)); got == nil || *got != "Trip receipt total is $24.50, refunded on 15 Jul 2026." {
		t.Fatalf("fallback: %v", deref(got))
	}

	same := "<html><body><table><tr><td>" + partiallyGluedUberReproText + "</td></tr></table></body></html>"
	got := SnippetForBody(s(partiallyGluedUberReproText), s(same))
	if got == nil {
		t.Fatal("nil")
	}
	for _, want := range []string{"44 Refunded", "Refunded 15", "44 Just"} {
		if !strings.Contains(*got, want) {
			t.Errorf("missing %q in %q", want, *got)
		}
	}
	for _, bad := range []string{"44Refunded", "44Just"} {
		if strings.Contains(*got, bad) {
			t.Errorf("glued seam left %q in %q", bad, *got)
		}
	}

	prose := "Hi team, just a quick update on the roadmap for next week. We shipped the new snippet heuristic and it looks solid."
	if got := SnippetForBody(s(prose), s("<html><body><p>HTML alt version, should not be used</p></body></html>")); got == nil || *got != prose {
		t.Fatalf("prose: %v", deref(got))
	}
	paypal := "Your PayPal payment went through at 3pm as expected, nothing else for you to do."
	if got := SnippetForBody(s(paypal), nil); got == nil || *got != paypal {
		t.Fatalf("paypal: %v", deref(got))
	}
	textOnly := SnippetForBody(s(gluedUberStyleText), nil)
	if textOnly == nil || *textOnly != SnippetFromText(deglue(gluedUberStyleText)) || !strings.Contains(*textOnly, "44 Refunded") || strings.Contains(*textOnly, "44Refunded") {
		t.Fatalf("text-only deglue: %v", deref(textOnly))
	}
}

func TestDeglue(t *testing.T) {
	cases := map[string]string{
		"44Refunded":                  "44 Refunded",
		"20:44Just":                   "20:44 Just",
		"appliedPrevious":             "applied Previous",
		"Refunded15":                  "Refunded 15",
		"total19":                     "total 19",
		"Street.Total":                "Street. Total",
		"applied.To":                  "applied. To",
		"one,Two!Three?Four;Five:Six": "one, Two! Three? Four; Five: Six",
		"at 3pm today":                "at 3pm today",
		"15 Jul 202620:44":            "15 Jul 202620:44",
		"visit example.com now":       "visit example.com now",
		"HTML and PDF":                "HTML and PDF",
		"Refunded 15 Jul":             "Refunded 15 Jul",
	}
	for in, want := range cases {
		if got := deglue(in); got != want {
			t.Errorf("deglue(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestHTMLToTextKeepsBareLessThanSigns(t *testing.T) {
	if got := HTMLToText("<p>1 < 2 and <3 sweet</p>"); got != "1 < 2 and <3 sweet\n" {
		t.Fatalf("%q", got)
	}
	if got := HTMLToText("x <br/>y</p><!-- c --> z"); got != "x \ny\n z" && got != "x\ny\n z" {
		t.Fatalf("%q", got)
	}
	if got := HTMLToText("trailing <"); got != "trailing <" {
		t.Fatalf("%q", got)
	}
}

func TestHTMLToTextBasics(t *testing.T) {
	if got := HTMLToText("<p>Hello&nbsp;<b>world</b>&#x21;</p><!-- c --><br>next"); got != "Hello world!\n\nnext" {
		t.Fatalf("%q", got)
	}
	if got := TextToHTML("a<b\r\nc"); got != "<html><body>a&lt;b<br/>c</body></html>" {
		t.Fatalf("%q", got)
	}
}

func deref(p *string) string {
	if p == nil {
		return "<nil>"
	}
	return *p
}
