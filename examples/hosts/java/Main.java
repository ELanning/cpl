import pricing.cpl.Fault;
import pricing.cpl.Int;
import pricing.cpl.List;
import pricing.shop.pricing.*;

/** A Java program that calls the CPL module shop.pricing as Java code (cpl build --target java). */
public final class Main {
    public static void main(String[] args) {
        Line apples = new Line("FOOD-APPLE", Int.of(125), Int.of(6));
        Line mug = new Line("MUG", Int.of(899), Int.of(2));
        List<Entry> order = List.of(
            new Entry(apples, new Discount.BuyNGetOneFree(Int.of(2))),
            new Entry(mug, new Discount.PercentOff(Int.of(10))));

        System.out.println("subtotal: " + Pricing.subtotal(order));
        // Capabilities are interfaces: a lambda implements one.
        TaxTable rates = sku -> Int.of(sku.startsWith("FOOD-") ? 5 : 20);
        java.util.List<String> log = new java.util.ArrayList<>();
        Int total = Pricing.checkout(order, rates, log::add);
        System.out.println("total: " + total + " (" + Pricing.dollars(total) + ")");
        System.out.println("log: " + String.join("; ", log));
        System.out.println("parse_quantity(\" 12 \"): " + Pricing.parseQuantity(" 12 "));
        System.out.println("parse_quantity(\"0\"): " + Pricing.parseQuantity("0"));
        System.out.println("factorial(25): " + Pricing.factorial(Int.of(25)));

        // An argument outside its type is refused at the boundary; a failed contract is a fault.
        // Both reach the host as a Fault, an unchecked exception.
        Line gift = new Line("GIFT", Int.of(-1), Int.of(1));
        try {
            System.out.println("line_total(unit_price -1): " + Pricing.lineTotal(gift, new Discount.NoDiscount()));
        } catch (Fault f) {
            System.out.println("line_total(unit_price -1): " + f.code);
        }
        try {
            System.out.println("dollars(-5): " + Pricing.dollars(Int.of(-5)));
        } catch (Fault f) {
            System.out.println("dollars(-5): " + f.code + " " + f.clauseKind);
        }
    }
}
