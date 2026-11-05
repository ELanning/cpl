// A Go program that calls the CPL module shop.pricing as Go code (`cpl build --target go`).
package main

import (
	"fmt"
	"strings"

	"pricing"
	"pricing/cpl"
)

// rates is the host's tax table: 5% on food, 20% on everything else.
type rates struct{}

func (rates) RateFor(sku string) cpl.Int {
	if strings.HasPrefix(sku, "FOOD-") {
		return cpl.I(5)
	}
	return cpl.I(20)
}

// auditLog keeps what it is told.
type auditLog struct{ lines []string }

func (l *auditLog) Record(message string) { l.lines = append(l.lines, message) }

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func main() {
	apples := pricing.Line{Sku: "FOOD-APPLE", UnitPrice: cpl.I(125), Quantity: cpl.I(6)}
	mug := pricing.Line{Sku: "MUG", UnitPrice: cpl.I(899), Quantity: cpl.I(2)}
	order := cpl.ListOf(
		pricing.Entry{Line: apples, Discount: pricing.Discount_BuyNGetOneFree{F0: cpl.I(2)}},
		pricing.Entry{Line: mug, Discount: pricing.Discount_PercentOff{F0: cpl.I(10)}},
	)

	fmt.Println("subtotal:", must(pricing.Subtotal(order)))
	log := &auditLog{}
	total := must(pricing.Checkout(order, rates{}, log))
	fmt.Printf("total: %v (%v)\n", total, must(pricing.Dollars(total)))
	fmt.Println("log:", strings.Join(log.lines, "; "))
	fmt.Println("parse_quantity(\" 12 \"):", cpl.ValueText(must(pricing.ParseQuantity(" 12 "))))
	fmt.Println("parse_quantity(\"0\"):", cpl.ValueText(must(pricing.ParseQuantity("0"))))
	fmt.Println("factorial(25):", must(pricing.Factorial(cpl.I(25))))

	// An argument outside its type is refused at the boundary; a failed contract is a fault.
	// Both reach the host as an error.
	gift := pricing.Line{Sku: "GIFT", UnitPrice: cpl.I(-1), Quantity: cpl.I(1)}
	if _, err := pricing.LineTotal(gift, pricing.Discount_NoDiscount{}); err != nil {
		fmt.Println("line_total(unit_price -1):", err.(*cpl.Fault).Code)
	}
	if _, err := pricing.Dollars(cpl.I(-5)); err != nil {
		f := err.(*cpl.Fault)
		fmt.Println("dollars(-5):", f.Code, f.ClauseKind)
	}
}
