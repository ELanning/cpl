// Strings that are not Unicode (an unpaired surrogate) reach CPL in every way a Java program can
// pass one: none gets in.
import texts.cpl.Fault;
import texts.cpl.ForeignError;
import texts.cpl.ForeignErrorKind;
import texts.cpl.Int;
import texts.cpl.List;
import texts.cpl.Result;
import texts.texts.Named;
import texts.texts.Shape;
import texts.texts.Source;
import texts.texts.Texts;

public class Main {
    static final String BAD = "\uD800";

    /** Answers with an invalid string, as a value or inside an error. */
    record BadSource(boolean inErr) implements Source {
        public String read(Int key) {
            return BAD;
        }

        public Result<String, ForeignError> fetch(Int key) {
            if (inErr) {
                return Result.err(new ForeignError(ForeignErrorKind.Unavailable, BAD, List.empty()));
            }
            return Result.ok(BAD);
        }
    }

    interface Call {
        Object run();
    }

    static void show(String what, Call c) {
        try {
            System.out.println(what + ": " + c.run());
        } catch (Fault f) {
            System.out.println(what + ": " + f.code + " " + f.symbol);
        }
    }

    public static void main(String[] args) {
        show("length", () -> Texts.length("héllo"));
        show("length bad", () -> Texts.length(BAD));
        show("total bad", () -> Texts.total(List.of("ok", BAD)));
        show("named bad", () -> Texts.named(new Named(BAD, Int.of(1))));
        show("shape bad", () -> Texts.shape(new Shape.Label(BAD)));
        show("shape empty", () -> Texts.shape(new Shape.Empty()));
        show("echo bad label", () -> Texts.echo(Int.of(7), BAD));
        show("read bad", () -> Texts.read(Int.of(1), new BadSource(false)));
        show("fetch bad", () -> Texts.fetch(Int.of(1), new BadSource(false)));
        show("fetch bad error", () -> Texts.fetch(Int.of(1), new BadSource(true)));
    }
}
