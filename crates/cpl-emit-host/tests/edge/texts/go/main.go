// Strings that are not Unicode (invalid UTF-8) reach CPL in every way a Go program can pass one:
// none gets in.
package main

import (
	"fmt"

	"texts"
	"texts/cpl"
)

const bad = "\xff"

// source answers with an invalid string, as a value or inside an error.
type source struct{ inErr bool }

func (source) Read(key cpl.Int) string { return bad }

func (s source) Fetch(key cpl.Int) cpl.Result[string, cpl.ForeignError] {
	if s.inErr {
		return cpl.Err[string](cpl.ForeignError{Kind: cpl.ForeignErrorKind_Unavailable, Message: bad})
	}
	return cpl.Ok[string, cpl.ForeignError](bad)
}

func show[T any](what string, v T, err error) {
	if err != nil {
		f := err.(*cpl.Fault)
		fmt.Println(what+":", f.Code, f.Symbol)
		return
	}
	fmt.Println(what+":", v)
}

func main() {
	v, err := texts.Length("héllo")
	show("length", v, err)
	v, err = texts.Length(bad)
	show("length bad", v, err)
	v, err = texts.Total(cpl.ListLit("ok", bad))
	show("total bad", v, err)
	v, err = texts.NamedFn(texts.Named{Name: bad, N: cpl.I(1)})
	show("named bad", v, err)
	v, err = texts.ShapeFn(texts.Shape_Label{F0: bad})
	show("shape bad", v, err)
	v, err = texts.ShapeFn(texts.Shape_Empty{})
	show("shape empty", v, err)
	v, err = texts.Echo(cpl.I(7), bad)
	show("echo bad label", v, err)
	v, err = texts.Read(cpl.I(1), source{})
	show("read bad", v, err)
	t, err := texts.Fetch(cpl.I(1), source{})
	show("fetch bad", t, err)
	t, err = texts.Fetch(cpl.I(1), source{inErr: true})
	show("fetch bad error", t, err)
}
