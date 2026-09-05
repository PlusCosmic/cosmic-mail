// Package xmldom is a tiny read-only element tree over encoding/xml.
//
// Copied from RimForge. Thunderbird's config-v1.1.xml is simple (elements,
// attributes and text, no namespaces); lookups are case-insensitive and
// malformed documents are errors, matching the strict roxmltree parser the
// Rust backend used, so callers treat them as "no config" rather than guess.
package xmldom

import (
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"strings"
)

// Node is one element: its name, its child elements, and its text and
// children interleaved in document order (so Text reads naturally).
type Node struct {
	Name     string
	Attrs    map[string]string
	Children []*Node
	parts    []part
}

// part is either a run of character data or a child element.
type part struct {
	text  string
	child *Node
}

// Parse returns the root element of doc. A UTF-8 BOM and leading whitespace
// are tolerated; any declared encoding is ignored (the bytes are assumed to
// already be UTF-8, lossily decoded by the caller).
func Parse(doc string) (*Node, error) {
	doc = strings.TrimLeft(strings.TrimPrefix(doc, "\uFEFF"), " \t\r\n")
	d := xml.NewDecoder(strings.NewReader(doc))
	d.CharsetReader = func(_ string, input io.Reader) (io.Reader, error) { return input, nil }

	var root *Node
	var stack []*Node
	for {
		tok, err := d.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, fmt.Errorf("xml parse error: %w", err)
		}
		switch t := tok.(type) {
		case xml.StartElement:
			n := &Node{Name: t.Name.Local}
			for _, a := range t.Attr {
				if n.Attrs == nil {
					n.Attrs = map[string]string{}
				}
				n.Attrs[a.Name.Local] = a.Value
			}
			if len(stack) == 0 {
				if root != nil {
					return nil, errors.New("xml parse error: multiple root elements")
				}
				root = n
			} else {
				parent := stack[len(stack)-1]
				parent.Children = append(parent.Children, n)
				parent.parts = append(parent.parts, part{child: n})
			}
			stack = append(stack, n)
		case xml.EndElement:
			if len(stack) == 0 {
				return nil, errors.New("xml parse error: unexpected end element")
			}
			stack = stack[:len(stack)-1]
		case xml.CharData:
			if len(stack) > 0 {
				cur := stack[len(stack)-1]
				cur.parts = append(cur.parts, part{text: string(t)})
			}
		}
	}
	if root == nil {
		return nil, errors.New("xml parse error: no root element")
	}
	if len(stack) != 0 {
		return nil, errors.New("xml parse error: unexpected EOF")
	}
	return root, nil
}

// Is reports whether the node's name equals name, ignoring case.
func (n *Node) Is(name string) bool { return strings.EqualFold(n.Name, name) }

// Child returns the first child element called name (case-insensitive).
func (n *Node) Child(name string) *Node {
	for _, c := range n.Children {
		if c.Is(name) {
			return c
		}
	}
	return nil
}

// ChildrenNamed returns every child element called name (case-insensitive).
func (n *Node) ChildrenNamed(name string) []*Node {
	var out []*Node
	for _, c := range n.Children {
		if c.Is(name) {
			out = append(out, c)
		}
	}
	return out
}

// Text returns all descendant text concatenated and trimmed.
func (n *Node) Text() string {
	var b strings.Builder
	n.collectText(&b)
	return strings.TrimSpace(b.String())
}

func (n *Node) collectText(b *strings.Builder) {
	for _, p := range n.parts {
		if p.child != nil {
			p.child.collectText(b)
		} else {
			b.WriteString(p.text)
		}
	}
}

// Attr returns the value of the named attribute (case-insensitive), or "".
func (n *Node) Attr(name string) string {
	for k, v := range n.Attrs {
		if strings.EqualFold(k, name) {
			return v
		}
	}
	return ""
}

// Descendants returns every descendant element called name, in document
// order.
func (n *Node) Descendants(name string) []*Node {
	var out []*Node
	var visit func(*Node)
	visit = func(x *Node) {
		for _, c := range x.Children {
			if c.Is(name) {
				out = append(out, c)
			}
			visit(c)
		}
	}
	visit(n)
	return out
}
