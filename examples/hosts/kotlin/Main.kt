// A Kotlin program that calls the CPL module shop.pricing through Kotlin's Java interop
// (cpl build --target java): the generated Java is to Kotlin what any Java library is.

import pricing.cpl.Fault
import pricing.cpl.Int as CplInt
import pricing.cpl.List as CplList
import pricing.shop.pricing.Discount
import pricing.shop.pricing.Entry
import pricing.shop.pricing.Line
import pricing.shop.pricing.Pricing

fun main() {
    val apples = Line("FOOD-APPLE", CplInt.of(125), CplInt.of(6))
    val mug = Line("MUG", CplInt.of(899), CplInt.of(2))
    val order = CplList.of(
        Entry(apples, Discount.BuyNGetOneFree(CplInt.of(2))),
        Entry(mug, Discount.PercentOff(CplInt.of(10))),
    )

    println("subtotal: ${Pricing.subtotal(order)}")
    val log = mutableListOf<String>()
    // Capabilities are Java interfaces: Kotlin lambdas implement them.
    val total = Pricing.checkout(order, { sku -> CplInt.of(if (sku.startsWith("FOOD-")) 5L else 20L) }, { log.add(it) })
    println("total: $total (${Pricing.dollars(total)})")
    println("log: ${log.joinToString("; ")}")
    println("parse_quantity(\" 12 \"): ${Pricing.parseQuantity(" 12 ")}")
    println("parse_quantity(\"0\"): ${Pricing.parseQuantity("0")}")
    println("factorial(25): ${Pricing.factorial(CplInt.of(25))}")

    // An argument outside its type is refused at the boundary; a failed contract is a fault.
    // Both reach the host as a Fault, an unchecked exception.
    try {
        println("line_total(unit_price -1): ${Pricing.lineTotal(Line("GIFT", CplInt.of(-1), CplInt.of(1)), Discount.NoDiscount())}")
    } catch (f: Fault) {
        println("line_total(unit_price -1): ${f.code}")
    }
    try {
        println("dollars(-5): ${Pricing.dollars(CplInt.of(-5))}")
    } catch (f: Fault) {
        println("dollars(-5): ${f.code} ${f.clauseKind}")
    }
    // What a capability returns is checked against its type too.
    try {
        println("a rate of 150: ${Pricing.totalWithTax(order) { CplInt.of(150) }}")
    } catch (f: Fault) {
        println("a rate of 150: ${f.code} ${f.symbol}")
    }

    // Kotlin's operators work on CPL's Int, and its when sees CPL enums as sealed.
    val doubled = total * CplInt.of(2) + CplInt.of(1)
    println("kotlin: total * 2 + 1 = $doubled, over 50 dollars: ${doubled > CplInt.of(5000)}")
    for (entry in order) {
        val what = when (val d = entry.discount) {
            is Discount.NoDiscount -> "no discount"
            is Discount.PercentOff -> "${d.f0}% off"
            is Discount.BuyNGetOneFree -> "buy ${d.f0} get one free"
        }
        println("kotlin: ${entry.line.sku}: $what")
    }
}
