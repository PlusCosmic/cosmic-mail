package xmldom

import "testing"

func TestParsesAndMatchesCaseInsensitively(t *testing.T) {
	root, err := Parse("\uFEFF<?xml version=\"1.0\"?>\n<Root><Item>a</Item><ITEM> b </ITEM><other/></Root>")
	if err != nil {
		t.Fatal(err)
	}
	if !root.Is("root") {
		t.Fatalf("root is %q", root.Name)
	}
	if got := len(root.ChildrenNamed("item")); got != 2 {
		t.Fatalf("expected 2 items, got %d", got)
	}
	if got := root.Child("ITEM").Text(); got != "a" {
		t.Fatalf("got %q", got)
	}
	if root.Child("missing") != nil {
		t.Fatal("missing child should be nil")
	}
}

func TestTextConcatenatesDescendants(t *testing.T) {
	root, err := Parse("<a> x <b>y</b> z </a>")
	if err != nil {
		t.Fatal(err)
	}
	if got := root.Text(); got != "x y z" {
		t.Fatalf("got %q", got)
	}
}

func TestMalformedDocumentsAreErrors(t *testing.T) {
	for _, doc := range []string{"", "<a>", "<a><b></a>", "<a></a><b></b>", "not xml"} {
		if _, err := Parse(doc); err == nil {
			t.Errorf("%q should not parse", doc)
		}
	}
}

func TestDottedTagNamesParse(t *testing.T) {
	root, err := Parse("<x><v1.6><li>a</li></v1.6></x>")
	if err != nil {
		t.Fatal(err)
	}
	if root.Children[0].Name != "v1.6" {
		t.Fatalf("got %q", root.Children[0].Name)
	}
}

func TestAttributesAndDescendants(t *testing.T) {
	root, err := Parse(`<clientConfig><emailProvider id="x"><incomingServer type="imap"><hostname>h</hostname></incomingServer><outgoingServer TYPE="smtp"/></emailProvider></clientConfig>`)
	if err != nil {
		t.Fatal(err)
	}
	in := root.Descendants("incomingserver")
	if len(in) != 1 || in[0].Attr("type") != "imap" || in[0].Child("hostname").Text() != "h" {
		t.Fatalf("%+v", in)
	}
	if out := root.Descendants("outgoingServer"); len(out) != 1 || out[0].Attr("type") != "smtp" {
		t.Fatal("outgoing")
	}
	if root.Attr("missing") != "" {
		t.Fatal("missing attr")
	}
}
