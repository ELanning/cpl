# CPL grammar, edition 0.1

This document is the complete lexical and syntactic grammar for the three
textual forms the CPL toolchain reads:

| Form | File extension | Root production | Section |
| - | - | - | - |
| CPL source | `.cpl` | `source_file` | 3 |
| Work (coordination) declarations | `.work` | `work_file` | 4 |
| Semantic patch forms | `.patch` (D40) | `patch_file` | 4.5 |

It is derived from `design-doc.md`, which remains authoritative for intent.
Where the design is explicit this grammar follows it. Where the design is
silent a rule is chosen and marked with a line beginning `Decision:` that
gives the rule, the rationale, and the rejected alternative. Where the
design is internally inconsistent the chosen reading is marked
`Design conflict:`. Decisions are numbered `D1`, `D2`, ... so that other
specifications can cite them. Every `Decision:` and `Design conflict:` line
also carries a project-wide stable identifier `[D-nnn]`, assigned in
document order and indexed in `spec/decisions.md`; the local `D<n>` numbers
remain the citation form inside the specifications.

Scope boundary: this document defines what parses and what the parser
produces (the abstract syntax). Name resolution, typing, effect checking,
contract semantics, and work-coordinator semantics are specified elsewhere;
where a syntactic form needs a semantic rule to be meaningful, that rule is
mentioned briefly and marked `Semantics:` so the two specifications can be
written in parallel without contradiction.

Diagnostics: the lexical and syntax errors this document names (quoted in
parentheses, for example `semicolons are not part of CPL`) are reported as
the structured failure data the design requires (design lines 227-239).

Decision: [D-001] D59. a lexical or syntax diagnostic belongs to one of two
*classes*, lexical and syntax, and carries a stable `code`: the
`parse`-category code that `spec/semantics.md` 9.4 assigns to its message
(that table is the code catalogue; the two class names are not codes and
are never emitted), a `message` that begins with the text quoted in this
document (D72 fixes the shape of the rest), the byte span and line/column of
the offending token (for a lexical error about part of a token, that part:
the escape sequence of `invalid_escape`, the misplaced separator or the
leading zeros of `invalid_int_literal`, the control character of a text
literal), and, for a message of the generic shape, the
`expected` and `found` fields (D72); where this document names a syntax
error case without quoting a message (a chained comparison, `Variant()`, a
spread that is not first, a trailing comma in a generic list, two
statements on one line, generic arguments after something other than a
name chain), the message is the generic `expected <description>, found
<token>` and the code is `syntax_error`. A file with a lexical or
syntax error yields no syntax tree and exactly one diagnostic, the earliest
error in source order (D70 says how an implementation may resynchronise to
find it), and the remaining files of the project are still parsed
(`spec/semantics.md` S52 then performs no resolution or typing for the
program). Rationale: one diagnostic per file is deterministic across
implementations, and reporting the errors found after a resynchronisation
produces implementation-specific cascades that agents would have to learn
to ignore; one code catalogue, owned by the document that defines every
other diagnostic (semantics S60), is what lets a shared negative corpus
match codes exactly. Rejected alternatives: reporting every error found
after resynchronising; emitting the two class names
`lexical_error`/`syntax_error` as the codes (an earlier revision of this
decision; two implementations would then disagree with the catalogue).

Decision: [D-226] D70. an implementation may *resynchronise* after an error
and keep going: the lexer skips an offending character or closes an
unterminated literal at the line end, and the parser skips to the end of
the line or to the next token that begins a statement, member, match arm,
item, or `.work`/`.patch` item, so that lexing and parsing are total over
any input and never panic on garbage or truncated text. Resynchronisation
is invisible in the result: `ParseResult::value` is `None`
(`spec/toolchain.md` 1.5) and `diagnostics` holds exactly one diagnostic,
the error with the smallest start offset (a lexical and a syntax error at
the same offset are reported as the lexical one, and two lexical errors
that start at one offset as the one the lexer meets first, reading the
token from left to right: `00.5` is `invalid_int_literal` for its leading
zeros, not `float_literal_unsupported`); since a lexical error about part
of a token spans that part (D59), a syntax error at the token's start
precedes it (`1 1__0` is the `syntax_error` at the second literal, and a
text literal with a bad escape where a type is expected is the
`syntax_error` at the literal), carrying its
`spec/semantics.md` 9.4 code; nothing is reported during a speculative
parse (D42), and the syntax tree never contains error-recovery nodes.
`invalid_utf8` is raised when the file's bytes are read, before lexing
(`spec/toolchain.md` 3.1), and the file then counts as failed exactly like
one with a lexical error. Rationale: an earlier wording of D59 forbade
"error recovery or resynchronisation", which described an implementation
technique rather than an observable; resynchronising is what makes the
parser panic-free and lets it report the earliest error rather than the
first one it happened to meet, while reporting a single error keeps D59's
determinism, because two correct implementations resynchronise differently
and would otherwise disagree on every error after the first. Rejected
alternatives: forbidding resynchronisation (unobservable, untestable);
reporting every error found (implementation-specific cascades).

Decision: [D-238] D72. a message this document quotes is the fixed
beginning of the emitted `message`, not necessarily the whole of it: the
message is the quoted text, with a placeholder in angle brackets (`<word>`,
`<path>`, `<token>`) replaced, `<word>` and `<path>` by the actual
spelling and `<token>` by the description of the offending token that
`, found <token>` uses (D36's message begins
`` unknown requirement identifier `compile`; ``), optionally followed
by a parenthesised hint and, when the quoted text begins with `expected`
and names no found token itself, by `, found <token>`, where `<token>`
describes the offending token (`` `{` ``, `` identifier `x` ``, `end of
line`, `end of file`); the message `unexpected character` is followed by
the character, in backticks, or as `U+XXXX` when it does not print. A syntax error this document names
without quoting a message has the generic message `expected <description>,
found <token>`, in which `<description>` names what the grammar admits at
that point and may end in a parenthesised hint (`` expected end of line
after the statement, found `let` ``, `` expected a payload type (a variant
without payload omits the parentheses), found `)` ``). Every message of the
shape `expected <description>, found <token>`, quoted or not, carries the
data fields `expected` (the description, hint included) and `found` (the
token description); every other message carries the fields
`spec/semantics.md` 9.4 lists for its code. A lexical error this document
names without quoting a message has a one-sentence message of the
implementation's choosing. The code is always exactly the 9.4 code, so a
test matches the code exactly and a quoted message as a prefix. Rationale:
the hint is what makes a parse diagnostic actionable (`` expected `->` (the
return type is mandatory; write `-> Unit`) `` names the rule and the fix),
exact equality would either forbid hints or freeze every hint into this
document, and a fixed beginning still lets a negative corpus and an agent
recognise each quoted case, while the `expected` and `found` fields carry
the same facts as data. Rejected alternatives: exact message equality (no
hints); free-form messages with only the code fixed (a quoted case could
not be recognised from its message).

Implementation limits. The grammar bounds neither nesting nor the length of
an expression, but a recursive-descent parser recurses once per nesting
level, and every later walk of a tree (drop, clone, the debug dump,
serialisation, the checker) recurses once per operator of a left-nested
operator chain, along one path of the tree at a time, so the bounds are
part of the contract:

Decision: [D-239] D73. every implementation accepts at least 64 levels of
nesting and at least 1000 operators along a path of nested expressions, and may accept more;
an input beyond its limits is the syntax error `nesting is too deep (more
than 64 levels)` or `an expression has too many operators (more than 1000);
split it into bindings` (code `syntax_error`, with the implementation's own
limit in place of 64 or 1000), reported at the token where the limit is
exceeded: for nesting, the first token after the bracket, `{`, prefix
operator, `<`, or `:` that opens the excess level (the keyword itself for
an `if` or `match`; a `NEWLINE` is a token here, so after a `{` that ends
its line the report is at that line break), and for operators, the
operator that makes the operators on its path, counted so far, exceed the
limit (those of the expressions around it, its own expression's, and the
most of any expression nested in its own expression before it). Whether a `<` after a name chain begins a generic call (D42) is
decided as a parser without a depth limit would decide it, and the limit
then applies to the parse that follows, so a generic call nested past the
limit is the nesting error, never a chain of comparisons. One nesting level is one bracket pair (`( )`, `[ ]`, or the
`{ }` of a struct literal, a set, or a `use` list), one brace body (a block
or any other `lines(X)` body), one `if` or `match` expression (each `if` of
an `else if` chain counts one, cumulatively: a chain of `n` branches nests
`n` levels, so a flat chain of 63 `if`s in a function body exceeds the
minimum although it is written flat), one prefix operator (`not`, unary `-`), one
generic argument list (the `<...>` of a type or of a generic call), or one
`forall` predicate; levels are counted from the top level of the file, so
the braces of a function body are its first level. The operators of an
expression are its binary operators and its postfix operators (`.member`, a
call, `?`, a generic call, a struct literal), counted along each path of
nested expressions in an outermost expression (the expression of a
statement or clause, or a refinement predicate): an expression nested in
another (an operand, an argument, a condition, a statement of a block, an
arm) adds its operators to those of every expression around it, while
expressions side by side (the arms of one `match`, the statements of one
block, the arguments of one call) count separately, and the limit bounds
the most operators on any one path. A `match` of thousands of arms, or a
block of thousands of statements, each with a few operators, is within
it. Rationale: the limits exist so that no
input, however deep or long, can overflow the stack of the parser or of a
recursive consumer of the tree; at 64 levels the reference parser and
formatter stay under 1 MiB of stack in a debug build (a requirement of the
reference toolchain, `spec/toolchain.md` T65), while real programs nest in single
digits and use a handful of operators per expression, and fixing a minimum
makes every program within it portable between implementations. Rejected
alternatives: no limit (a crafted input crashes the toolchain); a limit
chosen by each implementation with no minimum (a program one implementation
accepts could be rejected by another); measuring the depth of the finished
tree (the tree is what cannot be built).

The example projects under `examples/reader/` and `examples/holes/` are the
acceptance corpus for this grammar: every file there must parse (`cpl
check` parses the `work/` files too, for syntax only, `spec/semantics.md`
S53), the standard library's stub files (`std/*.cpl`, `spec/stdlib.md`
Section 11) must parse, including the bodiless and definition-less forms of
3.5 and 3.9 that only they may use, and Section 6 traces every
design-document code block through the productions below.

## 0. Notation

The grammar is written in a compact EBNF:

| Notation | Meaning |
| - | - |
| `name = ...` | definition of nonterminal `name` (snake_case) |
| `"fn"` | a terminal: a keyword or punctuation token with that exact spelling |
| `"base"`, `"body"`, ... (Section 4 only) | a contextual keyword: a `LOWER_IDENT` token whose spelling is exactly that word (Section 1.4); the `.work`/`.patch` parser compares spellings. The quoted facet spelling `"name"` in 4.4 is unrelated to the expression nonterminal `name` of 3.12 |
| `INT`, `TEXT`, `LOWER_IDENT`, `UPPER_IDENT`, `HOLE`, `DOC_COMMENT`, `NEWLINE`, `EOF` | token classes produced by the lexer (Appendix A maps them to `TokenKind` names) |
| `A B` | sequence |
| `A \| B` | alternatives |
| `A?` | zero or one |
| `A*` | zero or more |
| `A+` | one or more |
| `( A )` | grouping |
| `// text` | commentary inside a grammar block |

Three meta-productions are used throughout. `lines(X)` is a possibly empty
newline-separated list, optionally preceded by one `NEWLINE` (the line
terminator the lexer emits when the opening `{` ends its line, Section 1.8
step 2) and optionally followed by one; `commas(X)` is a possibly empty
comma-separated list with an optional trailing comma; `commas1(X)` is the
same with at least one element:

```ebnf
lines(X)   = NEWLINE? ( X ( NEWLINE X )* NEWLINE? )?
commas(X)  = commas1(X)?
commas1(X) = X ( "," X )* ","?
```

`lines(X)` appears only inside braces that open a *significant-newline*
context (Section 2.2); `commas(X)` and `commas1(X)` appear only inside
brackets that open an *insignificant-newline* context, which is why they
never mention `NEWLINE`.

## 1. Lexical structure

### 1.1 Source text

| Rule | Value |
| - | - |
| Encoding | UTF-8. Invalid UTF-8 is the parse diagnostic `invalid_utf8`, raised when the file is read, before lexing (D70); the file then counts as failed like one with a lexical error. A leading byte-order mark is skipped (the formatter removes it). |
| Line terminator | LF, or CRLF (treated as one terminator). A CR not followed by LF is a lexical error. |
| Horizontal whitespace | space (U+0020) and tab (U+0009). The formatter emits four spaces per indentation level; indentation is never significant. |
| Non-ASCII characters | Permitted only inside `TEXT` literals and comments. Anywhere else they are a lexical error. |
| File end | A final line terminator is optional. |

Decision: [D-002] D1. comments may contain non-ASCII characters. Rationale: the
design restricts identifiers to ASCII and makes strings Unicode but says
nothing about comments; prose for human readers should not be constrained.
Rejected alternative: ASCII-only comments.

One lexer serves all three file forms. The parser selects the root
production from the file extension (`.cpl`, `.work`, `.patch`; the third
form is D40); the token stream is identical.

### 1.2 Comments

| Form | Lexing | Meaning |
| - | - | - |
| `//` to end of line | discarded; produces no token | ordinary comment |
| `///` to end of line | one `DOC_COMMENT` token carrying the text after `///` with one leading space removed if present | documentation comment |
| `////` (four or more slashes) | ordinary comment | not documentation (D60) |

A `///` comment must be the first non-whitespace on its line; a `///` after
code on the same line is a lexical error (`doc comment must begin a line`).
A comment of either form runs to the end of its line, which only LF or
CRLF ends (1.1): a lone CR inside a comment does not end it and is still
the lexical error `invalid_line_terminator`, so the rest of the line never
becomes tokens. A comment of either form may not contain a control
character (U+0000-U+001F other than tab and CR, and U+007F: `control
character in comment`) or a bidirectional formatting character
(U+202A-U+202E and U+2066-U+2069: `bidirectional formatting character in
comment`), D75; each is reported at the character, and the comment still
ends at the end of its line.

Decision: [D-343] D75. control characters and the bidirectional
embedding, override, and isolate characters are rejected in comments, and
the bidirectional ones in text literals too, where `\u{202E}` still writes
one. Rationale: each makes the source display differently from what it is:
an override inside a comment or a literal reorders the code after it on
the screen of every reviewer (CVE-2021-42574, "Trojan Source"), and
comments, doc comments above all, are copied into generated Rust, Go,
Java, C, and Python, where a raw control character can end a comment or
corrupt the file. A program never needs either raw, and the escape keeps
every value writable. Rejected alternative: a warning (a review sees the
displayed text, not the warning), and rejecting them only in doc comments
(an ordinary comment reorders the display as well).

Decision: [D-003] D60. a comment beginning with four or more slashes (`////`) is an
ordinary comment, not documentation. Rationale: the design says
documentation comments start with `///` (line 44) and does not address
longer runs; `////`-style banner and separator lines are common, and
treating them as documentation would put them into the `docs` facet and
change its content version. Rust uses the same convention. Rejected
alternative: treating `////...` as a doc comment whose text begins with
`/`.

Doc comments attach to the declaration that follows them and form its
`docs` facet. All `DOC_COMMENT` tokens that attach to one declaration or
member form one docs value: they are concatenated in source order and
joined with `\n`, even when an annotation line separates two groups of
them (the formatter moves every doc comment of a `decl_prefix` before the
annotations, Section 3.3, so canonical text has one contiguous run and the
same docs value). The grammar admits `DOC_COMMENT*` in exactly these
places:

| Position | Attaches to |
| - | - |
| before the `module` header | the module (`spec/semantics.md` 1.2: the docs of several files of one module are concatenated in file-path order) |
| before a top-level declaration (`type`, `struct`, `enum`, `capability`, `fn`, `test`, `property`), before or after its annotations | that declaration |
| before a struct field, an enum variant, or a capability operation | that member |
| before `change` or `task` in a `.work` file, before `patch` in a `.patch` file | that item |

A `DOC_COMMENT` token anywhere else (for example before a statement, a
match arm, a parameter, or a `require` item) is a syntax error (`doc
comment must precede a declaration`, code `doc_comment_position`). So is a
doc comment in one of the positions above that is not followed by the
declaration, member, or item it documents: one before the `}` that closes
a body, one at the end of the file (also after annotations), one before
`use`, one before a `.work` item other than `task` (`base`, `target`,
`workers`, `integrate`), one before the `expect` line of a patch form, and
one between a header and its `{` (`struct S`, then a doc comment, then
`{`). The diagnostic is reported at the doc comment, not at the token that
follows it. The lexer does not emit a
`NEWLINE` token for the line terminator that ends a doc comment, nor for
blank lines directly after it (Section 2.1), so a doc comment and the
declaration it documents are adjacent in the token stream.

### 1.3 Identifiers

```ebnf
identifier   = ident_start ident_char*            // ASCII only
ident_start  = "a".."z" | "A".."Z" | "_"
ident_char   = ident_start | "0".."9"
```

The lexer classifies every identifier that is not a keyword:

| Spelling | Token | Class name in this document |
| - | - | - |
| exactly `_` | `"_"` (TokenKind `Underscore`) | wildcard |
| first character `A`..`Z` | `UPPER_IDENT` | PascalCase class |
| first character `a`..`z` or `_` (with at least one more character) | `LOWER_IDENT` | snake_case class |

Decision: [D-004] D2. the identifier case convention is enforced by the parser, not
by a lint. The lexer splits identifiers into `UPPER_IDENT` and
`LOWER_IDENT`, and every grammar position names the class it accepts (for
example `fn LOWER_IDENT`, `struct UPPER_IDENT`). Rationale: the class is
load-bearing in two places where a lint would be too late: a bare
`UPPER_IDENT` in a pattern is a nullary constructor while a bare
`LOWER_IDENT` is a binding (`Err(Unavailable)` versus `Err(e)`), and
`UPPER_IDENT "{"` in expression position is a struct literal. Making the
class part of the grammar gives one deterministic reading without name
resolution. Rejected alternative: a single `Ident` token plus a naming lint,
which (as in Rust) makes `Err(Unavailable)` silently become a catch-all
binding when the variant is misspelled.

| Grammar position | Required class |
| - | - |
| module path segments, function names, parameters, fields, `let`/`var` bindings, capability parameter names, task and change names, effect references, hole names | `LOWER_IDENT` |
| type names, struct names, enum names, enum variants, capability names, generic parameters | `UPPER_IDENT` |
| `use` list items, member names after `.`, selector segments | either |

Names with internal capitals (`pageId`) or a leading underscore (`_tmp`) are
accepted by the parser; `cpl check` may lint them. `_x` is a `LOWER_IDENT`.

Decision: [D-005] D61. where the design's grammar outline writes `identifier` or
`arguments`, this grammar names the narrower class the design's examples
use: annotation names (`@id`, `@prove`; a keyword is also admitted, D71),
hole names (`?build_pages`, D10),
`change` and `task` names (`pagination_update`, `implementation`), and the
names of an `after` list are `LOWER_IDENT`; annotation arguments are
`literal`s (the design's only argument is the `TEXT` `"pgn_7c2d"`).
Rationale: the outline (design lines 617-619 and 626-629) is abbreviated
and has one `identifier` class throughout, while the examples are
uniformly snake_case in these positions; fixing the class gives the same
deterministic reading D2 gives everywhere else, and an expression argument
to an annotation would need a place in the dependency graph that the
design does not describe. Rejected alternative: accepting either class and
arbitrary expressions in these positions and rejecting them in `cpl check`.

Two identifiers have contextual meaning and are otherwise ordinary
`LOWER_IDENT` tokens: `self` (the underlying value inside a refinement
predicate) and `result` (the returned value inside an `ensures` clause).
See Section 5 (l).

### 1.4 Keywords

Reserved words are never identifiers, in any of the three file forms.

| Reserved keywords (33) |
| - |
| `and` `assert` `capability` `decreases` `effects` `else` `ensures` `enum` `false` `fn` `for` `forall` `if` `in` `invariant` `let` `match` `module` `not` `or` `property` `pub` `requires` `return` `struct` `test` `true` `type` `use` `using` `var` `where` `while` |

Decision: [D-006] D3. `requires`, `ensures`, `effects`, `invariant`, `decreases`,
`using`, `where`, `forall`, and `assert` are reserved rather than
contextual. Rationale: they begin continuation lines (Section 2.4) or
statements, and an agent reading `ensures = 3` should never have to decide
whether `ensures` is a variable. Five of the nine (`requires`, `ensures`,
`effects`, `invariant`, `decreases`) are terminals of the design's grammar
outline (lines 611-624); the other four (`using`, `where`, `forall`,
`assert`) never appear in the outline but begin a parameter, predicate,
quantifier, or statement in the design's examples and are reserved for the
same diagnostic reason. Rejected alternative: contextual recognition only
in header position, which is parseable but makes diagnostics worse.

Decision: [D-007] D4. eight words are reserved for future editions and are a syntax
error in 0.1 (`` `<word>` is reserved for a future edition ``, code
`reserved_word`): `as` `async` `await`
`break` `continue` `ghost` `lemma` `loop`. The lexer emits them as the
single token kind `ReservedWord`. Rationale: only three are grounded in the
design, which names ghost assertions as a later extension of the verified
subset (line 100) and says host-level scheduling "does not add
application-level `async` syntax to the initial CPL language" (line 461),
so `ghost`, `async`, and `await` are spellings a later edition is likely to
claim; `as`, `break`, `continue`, `lemma`, and `loop` are reserved by this
specification on its own account, as the spellings import renaming, loop
control, and named proof lemmas would most plausibly take, not because the
design mentions them (it does not). Reserving the spellings now avoids
breaking renames later. Rejected alternative: reserving nothing (cheaper
now, breaking later).

Contextual keywords exist only in `.work` and `.patch` files. The lexer
emits them as ordinary `LOWER_IDENT` tokens; the `.work` and `.patch`
parsers recognize the spelling in the positions where Section 4 expects
it, and in `.cpl` files they are ordinary identifiers (a function may be
named `add` or `write`).

| Contextual keywords (`.work` / `.patch` only) |
| - |
| `add` `after` `base` `change` `depend` `expect` `git` `goal` `integrate` `no_holes` `no_unresolved_assumptions` `patch` `preserve` `prove` `require` `review` `target` `task` `typecheck` `workers` `write` |

Decision: [D-008] D5. the work-coordination vocabulary is contextual rather than
reserved. Rationale: `add`, `base`, `target`, `write`, and `task` are
common value names in ordinary code, and the `.work` grammar is
position-driven (LL(1)) so no ambiguity arises. Rejected alternative:
reserving them in all forms.

Words that are deliberately *not* keywords: `len`, `is_empty`, `push`,
`take`, `drop`, `drop_last`, `flatten`, `check`, `gen`, `self`, `result`,
`Ok`, `Err`, `Some`, `None`, `Unit`, `List`, `Option`, `Result`, `Int`,
`Bool`, `Text`, `Bytes`, and the facet names `identity`, `name`,
`signature`, `contract`, `body`, `docs`, `verification_policy`,
`definition`. They are ordinary identifiers resolved by the prelude or by
the selector grammar. The facet name `effects` is the keyword token
`KwEffects`; the selector grammar (Section 4.4) accepts that token in facet
position. Likewise an annotation name may be any reserved keyword token
(`@property`, whose name is the keyword `property`): the annotation grammar
(Section 3.3, D71) accepts a keyword after `@`.

### 1.5 Literals

```ebnf
int_literal  = digit ( digit | "_" digit )*        // token INT
digit        = "0".."9"
text_literal = '"' ( text_char | escape )* '"'    // token TEXT
text_char    = any Unicode scalar value except '"', "\\", and the control characters of D62
escape       = "\\" ( "\\" | '"' | "n" | "r" | "t" | "u{" hex_digit+ "}" )   // hex_digit+ is bounded: 1 to 6 digits (table below)
hex_digit    = "0".."9" | "a".."f" | "A".."F"
bool_literal = "true" | "false"
literal      = INT | TEXT | bool_literal
```

| Literal | Rules |
| - | - |
| `INT` | Decimal only. `_` may appear only between two digits (`1_000_000`; not `_1`, `1_`, `1__0`). The literal `0` is the only one that may begin with `0` (`007` is a lexical error: `leading zeros are not permitted`). No sign: `-5` is unary minus applied to `5` (Section 5 (j)). The token carries the digit string; Int is arbitrary precision so there is no range check. |
| `TEXT` | Delimited by `"`. May contain any Unicode scalar value except `"`, `\`, and the control characters listed in D62; a raw line terminator inside a literal is a lexical error (`unterminated text literal`), any other excluded control character is `control character in text literal`, and a raw bidirectional formatting character (U+202A-U+202E, U+2066-U+2069) is `bidirectional formatting character in text literal` (D75). Exactly six escapes: `\\`, `\"`, `\n`, `\r`, `\t`, and `\u{H}` with one to six hex digits denoting a Unicode scalar value (the grammar's `hex_digit+` is bounded to six: a seventh digit is the lexical error `unicode escape has more than six hex digits`; surrogates `D800`-`DFFF` and values above `10FFFF` are lexical errors). Any other character after `\` is a lexical error. The token carries the decoded string. |
| `Bool` | the keywords `true` and `false` |
| Float | None in 0.1. After an `INT` the lexer reports `floating-point literals are not supported in edition 0.1` if a `.` followed by a digit follows, and `a literal must be separated from an identifier` if an identifier character follows (`1e5`, `2x`). |
| Bytes | None in 0.1. |
| Unit | No literal form. |

Decision: [D-009] D6. integer literals are decimal only. Rationale: 0.1 exposes
only arbitrary-precision `Int` at the surface; hexadecimal and binary forms
matter for machine-width types that have no surface syntax yet. Rejected
alternative: `0x`/`0b` prefixes.

Decision: [D-010] D7. a `TEXT` literal is single-line; newlines are written `\n`.
Rationale: the layout algorithm (Section 2) is line-local, and diagnostics
map lines to statements. Rejected alternative: raw or triple-quoted
multi-line strings (deferred, not precluded).

Decision: [D-011] D62. the control characters excluded from a `TEXT` literal are
exactly U+0000-U+001F other than U+0009 (tab), and U+007F (DEL); of these,
U+000A and U+000D are the raw line terminator reported as `unterminated
text literal` and the rest are `control character in text literal`. The C1
range U+0080-U+009F and every other non-ASCII scalar value are permitted
raw. Rationale: the exclusion stays within ASCII, so the lexer needs no
Unicode category table and the rule is the same on every platform;
non-ASCII content is the design's "strings are Unicode" and is not
inspected further. Rejected alternative: excluding Unicode general category
Cc in full.

Decision: [D-012] D8. there is no `Bytes` literal in 0.1; `Bytes` values come from
the prelude and adapters. To keep the design space open, an identifier or
keyword immediately followed by `"` (`b"..."`, `test"x"`) is a lexical
error (`prefixed string literals are reserved`), so a future `b"..."` form
cannot change the meaning of code that lexes today. Rejected alternative:
`b"..."` now.

Decision: [D-013] D9. the single value of type `Unit` is written `Unit`, a prelude
constructor resolved exactly like `None`; no `()` form exists. Rationale:
the design says the prelude supplies `Unit` "and their constructors", and
`()` would introduce tuple-like syntax CPL does not have. Rejected
alternative: `()`.

### 1.6 Operators and punctuation

Longest match applies. The complete list:

| Spelling | TokenKind | Use |
| - | - | - |
| `+` | `Plus` | addition |
| `-` | `Minus` | subtraction, unary minus, `->` component |
| `*` | `Star` | multiplication; namespace wildcard in selectors |
| `/` | `Slash` | division |
| `%` | `Percent` | remainder |
| `++` | `PlusPlus` | list concatenation |
| `==` | `EqEq` | equality |
| `!=` | `BangEq` | inequality |
| `<` | `Lt` | less than; opens generic lists |
| `<=` | `LtEq` | less or equal |
| `>` | `Gt` | greater than; closes generic lists |
| `>=` | `GtEq` | greater or equal |
| `=` | `Eq` | binding, assignment, refinement |
| `->` | `Arrow` | return type |
| `=>` | `FatArrow` | match arm |
| `?` | `Question` | postfix propagation (Section 1.7) |
| `@` | `At` | annotation |
| `.` | `Dot` | member access, paths, selectors |
| `..` | `DotDot` | struct literal spread |
| `,` | `Comma` | separator |
| `:` | `Colon` | type annotation, field initializer, quantifier body |
| `(` `)` | `LParen` `RParen` | grouping, calls, payloads |
| `[` `]` | `LBracket` `RBracket` | list literals |
| `{` `}` | `LBrace` `RBrace` | blocks, bodies, sets, struct literals |

There is no `>>` token: `List<List<T>>` lexes as `List` `<` `List` `<` `T`
`>` `>`, so no token splitting is ever needed. The following characters are
lexical errors with the diagnostics shown: `;` (`semicolons are not part
of CPL`), `!` alone (`use not`), `&` and `|` (`use and / or`), and any of
`# $ ' \` ^ ~ \\` outside a text literal (`unexpected character`).

### 1.7 The `?` rule

Decision: [D-014] D10. a `?` immediately followed (no intervening whitespace) by an
identifier-start character lexes as one `HOLE` token carrying the
identifier; any other `?` is the `Question` token (postfix propagation).
The hole name must be a `LOWER_IDENT` spelling; `?Foo` and `?_` are lexical
errors (`hole names are snake_case`). Rationale: the design calls the hole
"a separate grammatical form"; deciding it in the lexer keeps the
expression grammar free of a prefix/postfix conflict (`x?` versus `?x`).
Rejected alternative: a parser rule on token adjacency, which requires
position bookkeeping the lexer already has.

The characters after `?` are read as an `identifier` (Section 1.3). The
hole is a lexical error when that spelling is `_` or begins with an
upper-case letter (`hole names are snake_case`) and when it is a reserved
keyword or a reserved-future word (`a hole name cannot be a keyword`:
`?if`, `?loop`, `?test`). `?self` and `?result` are legal holes, since
`self` and `result` are ordinary identifiers (D50).

Consequences: `x?` then a newline is propagation; `x? y` is propagation
followed by a syntax error; `x?y` lexes as `x` `HOLE(y)` and is a syntax
error; `?build_pages` is a hole; `?b"x"` is the hole `?b` followed by a
text literal, not the error of D8 (which concerns an identifier or keyword
token directly before `"`, and a hole is neither), so it is a syntax error
wherever no literal may follow the hole (as a statement:
`expected end of line after the statement, found text literal "x"`).

### 1.8 Lexer algorithm

The lexer is a single left-to-right pass with these steps at each position:

1. Skip spaces and tabs.
2. At a line terminator: emit `NEWLINE` unless the previously emitted token
   is `NEWLINE` or `DOC_COMMENT`, or no token has been emitted yet. This
   collapses blank lines, comment-only lines, and leading lines.
3. At `//`: if exactly `///` follows, lex a `DOC_COMMENT` to end of line
   (error if not at line start); otherwise skip to end of line.
4. At an identifier start: read `identifier`. If the spelling is in the
   reserved table, emit the keyword token; if in the reserved-future table,
   emit `ReservedWord`; if `_`, emit `Underscore`; otherwise emit
   `UPPER_IDENT` or `LOWER_IDENT` by first character. Whatever the spelling
   was classified as (keyword, reserved word, `_`, or identifier), if the
   next character is `"`, report the D8 error (`test"x"` is
   `prefixed_literal_reserved` like `b"x"`).
5. At a digit: read `int_literal`, applying the separator and leading-zero
   rules, then the Float and adjacency checks of Section 1.5.
6. At `"`: read `text_literal`, decoding escapes.
7. At `?`: apply Section 1.7.
8. Otherwise match the longest operator or punctuation spelling from
   Section 1.6; unmatched characters are lexical errors.
9. At end of input emit `EOF` (no synthetic `NEWLINE` is added).

Every token records its byte span and line/column for diagnostics; the
formatter additionally needs the original text of comments it preserves.

## 2. Newlines and layout

### 2.1 NEWLINE tokens

The lexer emits at most one `NEWLINE` token per run of line terminators,
comment-only lines, and blank lines (Section 1.8 step 2), none after a
`DOC_COMMENT`, and none at the start of the file. Consequently:

- a blank line or an ordinary comment between two lines never changes the
  parse: a header line, a blank line, a `// note`, and then a continuation
  line are separated by exactly one `NEWLINE` token;
- a doc comment is always immediately followed by the tokens of the
  declaration it documents.

### 2.2 Parser newline modes

The parser keeps a stack of newline modes, initially `[Significant]`.

| Mode | Behaviour of `NEWLINE` |
| - | - |
| `Significant` | a statement, item, clause, or arm terminator; the grammar names it explicitly (`NEWLINE`, `NEWLINE?`, `lines(X)`) |
| `Insignificant` | skipped: the token fetch loop discards `NEWLINE` tokens before returning the next token |

Pushes and pops happen when the parser *consumes* the bracket token, so the
mode is decided by the production being parsed, never by the lexer:

| Consumed token | Mode pushed |
| - | - |
| `(` `[` | `Insignificant` |
| `{` of a struct literal, an `effects` set, a `use` list, or a `.work` selector set (`write`, `add`, `depend`, `preserve`) | `Insignificant` |
| `{` of a block, a `struct`/`enum`/`capability` body, a `match` body, or a `change`/`task`/`integrate` body | `Significant` |
| `)` `]` `}` | pops the matching mode |

Pseudo-code:

```text
fn next_token(self):
    loop:
        t = lexer.next()
        if t.kind == Newline and self.modes.last() == Insignificant: continue
        return t

fn open(self, bracket, mode): expect(bracket); self.modes.push(mode)
fn close(self, bracket):       expect(bracket); self.modes.pop()
```

Because the stack is restored on every closing bracket, a block inside
parentheses (`foo(if c { 1 } else { 2 })`) has significant newlines inside
its braces and insignificant ones outside, and a parenthesized expression
inside a block may span any number of lines. `<` and `>` never push a
mode: where newlines are significant a line break inside a generic list
ends the statement and is therefore a syntax error (`expected
<description>, found end of line`), and inside an insignificant-newline context it is
discarded like any other line break; the formatter never breaks a generic
list (`spec/toolchain.md` 2.4 rule 6).

### 2.3 Statement termination

In `Significant` mode a statement, declaration, member, clause, or match
arm ends at the first `NEWLINE` token that is not inside a nested
insignificant context, or immediately before a `}` that closes the
enclosing body, or at `EOF`. Two statements never share a line: there is no
separator token. An expression therefore ends at the end of its line unless
the line ends inside `(`, `[`, or a struct literal `{`, with one exception:
the headers of `if`, `else`, and `match` may be followed by a `NEWLINE`
before their `{` (`NEWLINE? block` in `if_expr`, `NEWLINE? "{"` in
`match_expr`), decided by the Section 2.4 rule-3 lookahead whether the
`if`/`match` is a statement or an operand (`let label = if page.len > size`
followed by `{` on the next line is legal). Apart from that lookahead, which
only ever consumes a `NEWLINE` directly followed by `{`, the expression
parser never consumes a `NEWLINE`.

Not continuations (each is a syntax error, `expected expression, found end
of line`, or parses as two statements):

```text
let total = a +          // error: the line ends after a binary operator
    b
let text = page          // two statements: `let text = page` and `.text` (error)
    .text
let x =                  // error
    compute()
```

Continuations (inside brackets newlines are insignificant):

```text
let total = (a
    + b)
let pages = paginate(
    items,
    size,
)
let labels = [
    "a",
    "b",
]
let page = Page {
    ..base,
    text: trim(base.text),
}
```

Decision: [D-015] D11. a binary operator at the end of a line does not continue the
expression, and a leading `.` or operator on the next line does not either;
the only continuation mechanisms are enclosure in `(`, `[`, or a struct
literal `{`, and the `{` of an `if`/`else`/`match` header on the line after
the header (Section 2.4). Rationale: the design says long contract
expressions use explicit parentheses, and a single rule ("a line is a
statement unless a bracket is open or the next line is a header's `{`") is
what both agents and the formatter can apply with at most the one token of
lookahead that Section 2.4 rule 3 already needs. Rejected alternative:
operator-at-end-of-line continuation (Python/Go style), which makes `a +`
followed by a blank line ambiguous to a reader.

### 2.4 Header continuation lines

A declaration or loop *header* may continue onto following lines. Each
continuation line begins with one of the keywords in the table, and the
body's opening `{` may itself start a line. Blank lines and ordinary
comments between the header and its continuation lines are permitted
(they collapse into the single `NEWLINE` the grammar names).

| Header | Continuation lines allowed before the body | Grammar |
| - | - | - |
| `fn` signature | `effects`, `requires`, `ensures`, `decreases`, then `{`, or nothing more (the bodiless form, 3.9) | `fn_decl` |
| capability operation | `requires`, `ensures`; never a body (3.8) | `op_decl` |
| `while` condition | `invariant`, `decreases`, then `{` | `while_stmt` |
| `property "name"` | `for`, then `{` | `property_decl` |
| `patch` selector | `expect`, then `{` | `patch_form` |
| `if` condition, `else`, `match` scrutinee, `test "name"`, `struct`/`enum`/`capability` header, `change`/`task`/`integrate` header | `{` only | respective productions |

Rules:

1. Each clause (`effects`, `requires`, `ensures`, `decreases`, `invariant`,
   `for`, `expect`) occupies its own line: it is preceded by a `NEWLINE`
   token and its expression extends to the end of the line. Two clauses on
   one line are a syntax error.
2. When at least one clause is present, the body's `{` must start a new
   line (the clause expression ends at `NEWLINE`). When no clause is
   present the `{` may follow the header on the same line or start the
   next line (`NEWLINE? block`).
3. The parser implements this with one token of lookahead past a
   `NEWLINE`: after parsing the header (or a clause) it peeks past an
   optional `NEWLINE`; if the next token is a continuation keyword for
   this header or `{`, it continues the declaration. Otherwise the
   `NEWLINE` is left in place, and what follows depends on the header. A
   `fn` header, with or without clauses, is then complete without a body:
   this is the bodiless form of 3.9 (the third `fn_tail` alternative, the
   standard library's stub form, `spec/stdlib.md` 10.2 G1). A capability
   operation (`op_decl`) is then complete: an operation never has a body
   (3.8). After every other header the body is mandatory, and its absence
   is a syntax error whose message names what the header still admits
   (D72): `` expected `{` `` after a header of the last row of the table,
   `` expected `{` or a clause `` after a `while` header or one of its
   clauses, `` expected a `for` clause or `{` `` after a `property` header
   or one of its `for` clauses, and `` expected `expect` on its own line ``
   or `` expected `{` on its own line `` after a `patch` selector or its
   `expect` clause.
4. In a significant-newline context `else` must be on the same line as the
   `}` closing the preceding block (`} else {`): the `NEWLINE` after that
   `}` ends the statement, so a line beginning with `else` is an error
   (D12), reported with the code `misplaced_else` and the message `` `else`
   must follow the closing `}` of the preceding block on the same line ``.
   This holds in every significant-newline context of all three file
   forms, wherever a line may begin a statement, a member, a match arm, or
   an item: in blocks (the block of a `.patch` form included), in
   `struct`, `enum`, `capability`, and `match` bodies, in `change`, `task`,
   and `integrate` bodies, and at the top level of `.cpl`, `.work`, and
   `.patch` files. Where the grammar requires one particular token instead
   (the `module` header at the start of a `.cpl` file, the declaration
   after an annotation line), a line beginning with `else` is the generic
   syntax error for that token. Inside an insignificant-newline context
   (`foo(if c { 1 }` with `else { 2 })` on the next line) the `NEWLINE` is
   discarded before the parser sees it and the `else` is accepted; the
   formatter rejoins it to the `}`. The rule follows from the mode stack: implementations never
   compare line numbers.

Decision: [D-016] D12. `else` must follow the closing `}` of the preceding block on
the same line wherever newlines are significant; the restriction is a
consequence of the mode stack (Section 2.2), not a separate check.
Rationale: after `}` a `NEWLINE` ends the `if` statement, and permitting
`else` on the next line would require the parser to reopen a finished
statement; this is Go's rule and it keeps "newline terminates" absolute.
Rejected alternative: Rust's whitespace-insensitive `else`.

Decision: [D-017] D63. after any header the body's `{` may start the next line
(`NEWLINE? block`, rule 2). Rationale: the design's grammar outline writes
`function = ... type, [effects], contract*, block` with its `newline`
symbols only as clause terminators; it writes none between `type` and the
first `ensures` of the `paginate` example either, so it is silent on layout
between a header and what follows, not in conflict with the `paginate_ints`
example (design lines 566-570), which puts `{` alone on the line after the
return type with no clauses. Rule 2 accepts that example. Rejected
alternative: requiring `{` on the header line when there are no clauses.

### 2.5 Trailing commas

A trailing comma is permitted in every comma-separated list that lives in an
insignificant-newline context: parameters, arguments, list literals,
struct-literal entries, `effects` sets, `use` lists, annotation arguments,
and `.work` selector sets (`commas(X)` and `commas1(X)` everywhere). It is
not permitted in generic lists (`<T, U>`) or `after` lists, which are not
bracketed. Enum variant payload lists are parenthesized and accept it. The
rule is purely syntactic; the formatter decides when to emit one.

### 2.6 Summary of the layout algorithm

```text
1. Lexer: emit NEWLINE for each run of line ends (collapsed), none after
   DOC_COMMENT, none before the first token.
2. Parser: skip NEWLINE while the top mode is Insignificant
   (inside ( ), [ ], struct-literal { }, set-like { }).
3. Parser: in Significant mode, NEWLINE terminates the current statement,
   item, clause, or arm; `}` and EOF also terminate. A NEWLINE directly
   after a Significant `{` is the leading `NEWLINE?` of `lines(X)` and
   terminates nothing.
4. Parser: after a header or clause, peek past one NEWLINE for a
   continuation keyword or `{`.
5. Nothing else about whitespace is significant.
```

## 3. Grammar of `.cpl` files

### 3.1 Source file and module header

```ebnf
source_file   = DOC_COMMENT* module_header ( NEWLINE use_decl )* ( NEWLINE declaration )* NEWLINE? EOF
module_header = "module" module_path
module_path   = LOWER_IDENT ( "." LOWER_IDENT )*
```

The `module` header is mandatory and must be the first tokens of the file
after optional module doc comments. It is authoritative for the module path
(the conventional location is `src/a/b/c.cpl` for `module a.b.c`, or
`src/a/b/c/<name>.cpl` for a further file of the same module,
`spec/semantics.md` 1.2; a module may span several files; `cpl check`
warns when a file is not at its conventional path). All `use` declarations precede all other declarations.

Decision: [D-018] D13. `use` declarations must appear between the module header and
the first declaration; a `use` after a declaration is a syntax error.
Rationale: imports are the module's interface dependencies and tooling
reads them without parsing bodies. Rejected alternative: `use` anywhere at
top level.

A `use` after a declaration is reported with the code
`use_after_declaration` and the message `use declarations must precede all
other declarations` (in a `.work` file, a `use` after a `change`: `use
declarations must precede all change declarations`, Section 4.1).

Decision: [D-019] D14. a module has no top-level `let`/`var`/constant declarations
in 0.1; the only declarations are those in `declaration` below. Rationale:
the design excludes hidden mutable globals and never shows a module-level
value. Rejected alternative: top-level `let` constants.

### 3.2 Imports

```ebnf
use_decl = "use" use_path "." "{" commas1(use_item) "}"      // list form:   use std.text.{trim}
         | "use" use_path "." use_item                       // single form: use std.text.trim
use_path = LOWER_IDENT ( "." LOWER_IDENT )*
use_item = LOWER_IDENT | UPPER_IDENT
```

Implementation note: read the dotted identifier sequence greedily; if it is
followed by `.{`, the sequence is `use_path`; otherwise the last segment is
the `use_item` and the preceding segments (at least one) are `use_path`.

Decision: [D-020] D15. the single-item form `use a.b.c` is allowed and imports the
item or submodule `c` of module `a.b` under the name `c`. Rationale: the
design shows only the list form but the single form is the obvious
degenerate case and costs the parser one lookahead. Rejected alternative:
list form only (`use a.b.{c}`), which agents from every other ecosystem
would get wrong.

Decision: [D-021] D16. there is no import renaming (`as`), no glob import (`use a.*`),
and `use` lists must contain at least one item. Rationale: explicit names
keep the semantic index's dependency edges exact; `*` is the `.work`
namespace selector, not an import form. Rejected alternative: Rust-style
`as` and `*`.

### 3.3 Declarations and annotations

```ebnf
declaration = decl_prefix ( type_decl | struct_decl | enum_decl | capability_decl
                          | fn_decl | test_decl | property_decl )
decl_prefix = ( DOC_COMMENT | annotation )*
annotation  = "@" ( LOWER_IDENT | keyword ) ( "(" commas(literal) ")" )? NEWLINE   // keyword: any reserved keyword token of 1.4 (D71): @property
```

Each annotation occupies its own line and precedes `pub`. Annotation
arguments are literals only, never expressions (D61). The canonical order
emitted by the formatter is doc comments first, then annotations in source
order; the parser accepts any interleaving, and all doc comments of the
prefix form one docs value (Section 1.2).

| Annotation | Arguments | Permitted on | Facet |
| - | - | - | - |
| `@id("...")` | one `TEXT` | any top-level declaration | `identity` |
| `@prove` | none | `fn` only | `verification_policy` |
| `@builtin("...")` | one `TEXT` | a bodiless `fn` or a definition-less `type` in the standard library's stub files only (`spec/stdlib.md` 10.2) | `body` |
| `@method` | none | a `@builtin` `fn` with a receiver parameter, standard library only | `signature` |
| `@property` | none | a `@builtin` `fn` with at most a receiver parameter, standard library only | `signature` |

The three standard-library annotations are `unknown_annotation` in a
project file (`spec/semantics.md` 1.8); the grammar admits them everywhere
because the annotation grammar is open (D17).

Decision: [D-022] D17. the annotation grammar is open (any `LOWER_IDENT` or
keyword name with literal arguments) but the vocabulary is closed in 0.1:
`cpl check` rejects an unknown annotation or a known one in the wrong
position. Rationale: the annotation set is tooling vocabulary that will
grow; parsing must not fail on a newer edition's file before the checker
can say which edition is needed. Rejected alternative: hard-coding `id` and
`prove` in the grammar.

Decision: [D-227] D71. the name of an annotation is a `LOWER_IDENT` or any
reserved keyword token of Section 1.4; `@property` is therefore the
annotation named by the keyword `property`, and the AST stores the spelling
(`Annotation::name`, `spec/toolchain.md` 2.2). The reserved-future words of
D4 are not admitted (`@async` is `reserved_word`), and a keyword is still
never a member name (D53). Rationale: `@property` is the spelling the
standard library's normative stub text uses (`spec/stdlib.md` 0.1, 10.2,
Section 11) and the design's outline writes `annotation = "@", identifier`
with no keyword exclusion; after `@` only an annotation name can follow, so
admitting keywords creates no ambiguity, exactly as D53 admits the keyword
`effects` in facet position. Rejected alternatives: renaming the annotation
(`@getter`), which would change the normative stub text; a lexer rule that
classifies `property` as `LOWER_IDENT` after `@` (the token stream would
then depend on the previous token).

### 3.4 Types

```ebnf
type           = type_path generic_args?
type_path      = ( LOWER_IDENT "." )* UPPER_IDENT         // optional module qualification
generic_args   = "<" type ( "," type )* ">"
generic_params = "<" UPPER_IDENT ( "," UPPER_IDENT )* ">"
```

Generic lists have no trailing comma and never change the newline mode
(Section 2.2: a line break inside one is a syntax error where newlines are
significant and is discarded inside brackets; the formatter never emits
one). There are no function types, tuple types, or parenthesized types in
0.1; `Unit` is an ordinary prelude type name.

### 3.5 Type declarations

```ebnf
type_decl       = "pub"? "type" UPPER_IDENT ( wrapper_decl | refinement_decl | opaque_decl )
wrapper_decl    = "(" type ")"                           // pub type PageId(Text)
refinement_decl = "=" type "where" expr                  // pub type Positive = Int where self > 0
opaque_decl     = generic_params?                        // pub type List<T>, pub type Int: a builtin stub (std only)
```

The `where` predicate extends to the end of the line (parenthesize to span
lines) and binds `self` to the underlying value. Semantics: a wrapper is a
nominal type with one payload; a refinement is the underlying type
restricted by the predicate. The third form, a `type` with no definition
(`spec/stdlib.md` 10.2, G2), is the declaration of a builtin type and is
legal only in the standard library's stub files, where it carries
`@builtin("<id>")` (3.3): after `type UPPER_IDENT` the parser reads `<`,
`(`, or `=` if present and otherwise ends the declaration, so the file
parses and `cpl check` reports the check error `missing_body`
(`spec/semantics.md` 1.8) for a definition-less `type` in a project file.
Among `type` declarations generic parameters belong to this third form
only (D19): after `type A<T>` the declaration ends, so `type A<T> = Int
where self > 0` and `type A<T>(T)` are the syntax error `expected end of
line after the declaration` (found `=` and `(` respectively).

Decision: [D-023] D64. a `type ... =` declaration must carry a `where` predicate:
`pub type A = T` without one is a syntax error (`a type declaration is a
wrapper, type A(T), or a refinement, type A = T where ...`). There are no
transparent type aliases in 0.1. Rationale: the design shows exactly two
`type` forms, the nominal wrapper `type PageId(Text)` and the refinement
`type Positive = Int where self > 0` (lines 53-54), and states that an
ordinary `Text` is not a `PageId` (line 75); a transparent alias would add
a notion of type equivalence the design does not have. Rejected
alternative: `type A = T` as a transparent alias (deferred, not precluded).

Decision: [D-024] D18. a nominal wrapper has exactly one payload type. Rationale:
the design's wrapper is a single-field newtype constructed `PageId("...")`;
several fields belong in a `struct`. Rejected alternative: `type Pair(Int,
Int)`.

### 3.6 Structs

```ebnf
struct_decl = "pub"? "struct" UPPER_IDENT generic_params? NEWLINE? "{" lines(field_decl) "}"
field_decl  = DOC_COMMENT* LOWER_IDENT ":" type
```

Fields are one per line, separated by `NEWLINE`, with no commas and no
per-field `pub` (D65). A struct may have zero fields.

Decision: [D-025] D65. fields and variants carry no visibility of their own:
`field_decl` and `variant_decl` admit no `pub`, and every field and variant
of a type is reachable wherever the type is. Rationale: the design's
sentence "Public struct fields and enum variants are accessible through
their exported type in this initial design" (line 71) can be read as "the
fields of a public struct are reachable through it" or as "fields marked
`pub` are"; its examples never write `pub` on a member, so the reading
without per-member visibility is the one its text exercises, and it keeps
the member grammar to one form. Rejected alternative: `"pub"? LOWER_IDENT
":" type` and `"pub"? UPPER_IDENT ...` with per-member visibility
(deferred, not precluded).

Decision: [D-026] D19. `struct` and `enum` declarations accept `generic_params`
(`struct Pair<A, B>`); `type` wrappers, refinements, and capabilities do not
(the standard library's definition-less `opaque_decl` does, for `List<T>`
and `Gen<T>`).
Rationale: the prelude's `List<T>`, `Option<T>`, and `Result<T, E>` are
generic data types, and a user type with the same shape should not need a
different grammar; whether the 0.1 typechecker supports user generic types
is for the semantics specification, and a clear "not supported" there is
better than a parse error here. Rejected alternative: generics on `fn`
only.

### 3.7 Enums

```ebnf
enum_decl    = "pub"? "enum" UPPER_IDENT generic_params? NEWLINE? "{" lines(variant_decl) "}"
variant_decl = DOC_COMMENT* UPPER_IDENT ( "(" commas1(type) ")" )?
```

A variant has zero or more positional payload types; `Variant()` with
empty parentheses is a syntax error (omit the parentheses).

Decision: [D-027] D20. variants carry positional payloads only; there are no
record-style variants (`Missing { id: PageId }`). Rationale: the design's
variants are positional and the pattern grammar (3.13) is then one form.
Rejected alternative: Rust-style struct variants.

### 3.8 Capabilities

```ebnf
capability_decl = "pub"? "capability" UPPER_IDENT NEWLINE? "{" lines(op_decl) "}"
op_decl         = DOC_COMMENT* "fn" LOWER_IDENT "(" commas(param) ")" "->" type ( NEWLINE contract_clause )*
```

Operations are signatures only: no body, no `pub`, no generics, no `using`
parameters, no `effects` clause (the operation is itself the effect). The
parser distinguishes a contract line from the next operation by peeking
past the `NEWLINE` for `requires`/`ensures` (Section 2.4 rule 3).

Decision: [D-028] D21. capability operations may carry `requires`/`ensures` clauses.
Rationale: the design says adapter contracts "are assumptions unless
independently verified", which presupposes a place to write them. Rejected
alternative: signatures without contracts, which would force boundary
predicates into every caller.

### 3.9 Functions

```ebnf
fn_decl          = "pub"? "fn" LOWER_IDENT generic_params? "(" params ")" "->" type fn_tail
fn_tail          = NEWLINE? block                            // no clauses: `{` on the same or next line
                 | ( NEWLINE fn_clause )+ NEWLINE block      // clauses: one per line, then `{` on its own line
                 | ( NEWLINE fn_clause )*                    // bodiless: a builtin stub (std only, spec/stdlib.md 10.2 G1)
fn_clause        = effects_clause | contract_clause | decreases_clause
effects_clause   = "effects" "{" commas(effect_ref) "}"
effect_ref       = LOWER_IDENT "." LOWER_IDENT             // capability parameter . operation
contract_clause  = ( "requires" | "ensures" ) expr
decreases_clause = "decreases" expr
params           = ( param ( "," param )* ( "," using_param )* ","?
                   | using_param ( "," using_param )* ","? )?
param            = LOWER_IDENT ":" type
using_param      = "using" LOWER_IDENT ":" type
```

Rules enforced by the parser:

| Rule | Diagnostic |
| - | - |
| the return type is mandatory (`-> Unit` is written out) | `` expected `->` `` |
| positional parameters precede `using` parameters | `positional parameter after using parameter` |
| at most one `effects` clause and at most one `decreases` clause | `duplicate effects clause` / `duplicate decreases clause` |
| clauses may appear in any order (D66); the formatter emits `effects`, then `requires` clauses in source order, then `ensures` clauses in source order, then `decreases` | none |
| clause expressions are parsed with struct literals disabled (Section 5 (a)) | none |

The relative order of `requires` clauses is preserved because the design
checks preconditions sequentially (`a % b` is safe after `b != 0`).

A `fn` whose header (and clauses, if any) is followed by neither a clause
nor `{` is complete without a body (the third `fn_tail` alternative): the
Section 2.4 rule-3 lookahead leaves the `NEWLINE` in place and the
declaration ends. This is the form of the standard library's `@builtin`
stubs (`spec/stdlib.md` 10.2, G1); in a project file a bodiless `fn` parses
and is the check error `missing_body` (`spec/semantics.md` 1.8), not a
syntax error. The missing-body error of rule 3 therefore never arises
after a `fn` header or a capability operation; it arises only after the
headers whose bodies are mandatory (`while`, `property`, `patch`, and the
`{`-only headers of the Section 2.4 table).

Decision: [D-029] D66. the parser accepts a function's `effects`, `requires`,
`ensures`, and `decreases` clauses, and a loop's `invariant` and
`decreases` clauses (3.11), in any order; the formatter emits the canonical
order (`effects`, `requires` in source order, `ensures` in source order,
`decreases`; `invariant`s in source order, then `decreases`). Rationale:
the design's outline fixes `[effects], contract*` (line 613) and
invariants before `decreases` (lines 623-624), and line 634 says contracts
"are ordered requires clauses followed by ensures clauses in canonical
output", which makes the outline's order a canonical-output rule for the
contract clauses; applying the same parse-any-order, format-canonical
treatment to `effects` and `decreases` gives one rule for all clauses.
Rejected alternative: enforcing the outline's order in the parser, which
turns a formatting matter into a syntax error.

Decision: [D-030] D22. a function header may carry a `decreases` clause. The design's
grammar outline lists only `[effects], contract*`, but its prose says "a
recursive verified function similarly declares a decreasing measure", which
needs a place in the syntax. Rejected alternative: an annotation
`@decreases(...)`, which would make the measure a literal rather than an
expression.

Decision: [D-031] D23. `effects` sets list `parameter.operation` references only;
there is no wildcard (`db.*`) and no capability-type form (`PageRead.get`).
Rationale: the design's bound is per handle and per operation; a wildcard
would widen bounds silently. Rejected alternative: `db.*`.

### 3.10 Tests and properties

```ebnf
test_decl     = "test" TEXT test_using? ( NEWLINE? block | NEWLINE effects_clause NEWLINE block )
property_decl = "property" TEXT test_using? ( NEWLINE for_clause )+ ( NEWLINE effects_clause )? NEWLINE block
test_using    = "(" using_param ( "," using_param )* ","? ")"    // Milestone 3 (D74)
for_clause    = "for" LOWER_IDENT "in" expr                 // expr parsed with struct literals disabled
```

A test or property name is a `TEXT` literal; tests and properties take no
`pub` (D67). Semantics: names must be non-empty and unique within their
module; tests have stable identities like other declarations and are
addressed by selectors through their namespace (D69).

From Milestone 3 a test or property may declare `using` parameters, in
parentheses after its name, and an `effects` clause, on its own line after
the header (a test) or after the `for` clauses (a property), with the body's
`{` then on a line of its own; `spec/semantics.md` S83 binds the parameters
to fixtures. The canonical layout keeps the parenthesized list on the
header line (`test "loads" (using db: PageRead)`) and indents the `effects`
clause by four spaces, as a function's clauses are.

Decision: [D-339] D74. A test or property takes an optional parenthesized
list of `using` parameters after its name and an optional `effects` clause.
Rationale: "service-dependent tests must declare fixtures" (design line
600), and a test can reach a capability only through a handle, which in CPL
comes only from a `using` parameter (semantics S26); the parameter list of
`fn` is reused so that a test's header reads like a function's, and the
explicit `effects` clause keeps every effect bound in a header (semantics
S26). Rejected alternatives: an annotation (`@fixture(db: PageRead)`),
which would put a binding-like construct outside the grammar of
parameters; implicit handles named by the manifest (a test body would use
names declared nowhere in its source).

Decision: [D-032] D67. `test` and `property` declarations take no `pub`.
Rationale: the design says tests "live in namespaces like ordinary
declarations" (line 285) and that declarations are private unless `pub`
(line 71), but a test is never imported or called from source; it is
addressed by tooling through selectors (`test(tests.pagination.*)`), so
visibility would have no meaning for it. Rejected alternative: `"pub"?
"test"`, accepted and ignored.

### 3.11 Blocks and statements

```ebnf
block        = "{" lines(statement) "}"
statement    = let_stmt | var_stmt | assign_stmt | return_stmt | assert_stmt | while_stmt
             | concurrent_stmt | context_stmt | expr_stmt
let_stmt     = "let" ( LOWER_IDENT | "_" ) ( ":" type )? "=" expr
var_stmt     = "var" LOWER_IDENT ( ":" type )? "=" expr
assign_stmt  = LOWER_IDENT "=" expr
return_stmt  = "return" expr?
assert_stmt  = "assert" expr
while_stmt   = "while" expr while_tail                      // condition parsed with struct literals disabled
while_tail   = NEWLINE? block
             | ( NEWLINE while_clause )+ NEWLINE block
while_clause = "invariant" expr | decreases_clause
concurrent_stmt = "concurrent" "{" lines(statement) "}"     // contextual: `concurrent` followed by `{`
context_stmt = "context" LOWER_IDENT "=" expr               // contextual: `context`, a name, then `=`
expr_stmt    = expr
```

`concurrent` is contextual, like `parallel`: it begins a `concurrent_stmt`
only at the start of a statement and only when the next token is `{`, so
`fn concurrent()` and variables of that name keep working. Every line of
its block is a task and the block has no tail expression (the parser keeps
a final expression line as a statement); which statements may be tasks is
the checker's rule (`spec/concurrency.md`), so the grammar accepts any.

`context` is contextual too: it begins a `context_stmt` only at the start
of a statement and only when a `LOWER_IDENT` and `=` follow, so `context =
1` assigns a variable named `context`. The statement starts the call
stack's state machine (`spec/state-machines.md`); the formatter writes it
`context NAME = EXPR`.

Block value: if the last statement of a block is an `expr_stmt`, the parser
stores it as the block's *tail expression* (`Block { statements, tail:
Option<ExprStmt> }`, `spec/toolchain.md` 2.2) and it is the block's value;
otherwise the block's value is
`Unit`. A block may be empty (`{}`), with value `Unit`. A bare block is not
a statement or an expression; blocks occur only as the bodies named in this
grammar.

The parser tells `assign_stmt` from `expr_stmt` by two-token lookahead
(`LOWER_IDENT "="`). `return` followed by `NEWLINE` or `}` has no value.
`while` accepts any number of `invariant` clauses and at most one
`decreases` (duplicate is an error), in any order (D66); the formatter
emits invariants in source order followed by `decreases`.

Semantics notes (owned by the semantics specification, recorded here so the
grammar is not over-read): a `var` may be reassigned only within its
function; an `expr_stmt` that is not the tail expression must have type
`Unit` (discarding a `List` returned by `push` is an error); `return` with no
value is permitted only where the return type is `Unit`; `assert` is an
executable check whose failure is an execution fault.

Decision: [D-033] D24. `let _ = expr` is the only way to discard a non-`Unit`
value. Rationale: because non-tail expression statements must be `Unit`,
an explicit discard form is required, and `_` is already the wildcard.
Rejected alternative: allowing silent discards.

Decision: [D-034] D25. `let`/`var` bind a single name; there is no destructuring
(`let Page { id, text } = p`), no compound assignment (`x += 1`), and no
field assignment (`p.text = t`, impossible since records are immutable
values). Rationale: one binding form keeps the formatter and the hole
diagnostics ("visible bindings") simple; `match` destructures. Rejected
alternative: irrefutable patterns in `let`.

Decision: [D-035] D26. `return` is a statement, not an expression. Inside a match
arm or `if` branch it is written in a block: `Err(e) => { return Err(e) }`.
Rationale: `?` covers the common early-exit, and keeping `return` out of the
expression grammar means every expression has a value. Rejected alternative:
Rust's `return` expression of type never.

Decision: [D-036] D27. `assert` is a statement taking exactly one expression; there
is no message argument. Rationale: the structured diagnostic already carries
the expression text, location, and bindings. Rejected alternative:
`assert expr, "message"`.

Decision: [D-037] D28. a bare block is not an expression or statement. Rationale: it
would collide with the struct-literal rule (Section 5 (a)) after a name on
the previous line and adds scoping with no design motivation. Rejected
alternative: Rust block expressions.

### 3.12 Expressions

```ebnf
expr           = forall_expr | or_expr
forall_expr    = "forall" LOWER_IDENT "in" or_expr ":" expr
or_expr        = and_expr ( "or" and_expr )*
and_expr       = cmp_expr ( "and" cmp_expr )*
cmp_expr       = add_expr ( cmp_op add_expr )?             // comparisons do not chain
cmp_op         = "==" | "!=" | "<" | "<=" | ">" | ">="
add_expr       = mul_expr ( ( "+" | "-" | "++" ) mul_expr )*
mul_expr       = unary_expr ( ( "*" | "/" | "%" ) unary_expr )*
unary_expr     = "not" unary_expr | "-" unary_expr | HOLE | postfix_expr
postfix_expr   = primary postfix*
postfix        = "." member_name                            // page.len, result.drop_last, StoreError.Missing
               | generic_args? "(" args ")"                 // f(x), check<Positive>(n), page.take(2)
               | "?"                                        // propagate
member_name    = LOWER_IDENT | UPPER_IDENT
args           = ( expr ( "," expr )* ( "," using_arg )* ","?
                 | using_arg ( "," using_arg )* ","? )?
using_arg      = "using" expr
primary        = literal | name | struct_literal | list_literal | paren_expr | if_expr | match_expr
name           = LOWER_IDENT | UPPER_IDENT
paren_expr     = "(" expr ")"
list_literal   = "[" commas(expr) "]"
struct_literal = type_path "{" struct_entries "}"
struct_entries = ( ( ".." expr | field_init ) ( "," field_init )* ","? )?
field_init     = LOWER_IDENT ":" expr
if_expr        = "if" expr NEWLINE? block ( "else" ( NEWLINE? block | if_expr ) )?   // condition: struct literals disabled
match_expr     = "match" expr NEWLINE? "{" lines(match_arm) "}"                       // scrutinee: struct literals disabled
match_arm      = pattern "=>" ( block | expr )
```

Precedence, highest first, exactly as the design states it:

| Level | Forms | Associativity |
| - | - | - |
| 1 | postfix: `.member`, call `(...)`, generic call `<T>(...)`, `?` | left (applied in order) |
| 2 | prefix: `not`, unary `-`, hole `?name` | prefix, right-nested (`not not a`, `- -a`) |
| 3 | `*` `/` `%` | left |
| 4 | `+` `-` `++` | left |
| 5 | `==` `!=` `<` `<=` `>` `>=` | none: one comparison per operand chain |
| 6 | `and` | left |
| 7 | `or` | left |
| 8 | `forall x in c: p` | prefix form; `p` extends to the end of the expression |

`if` and `match` are primaries and may appear as operands
(`1 + if c { 1 } else { 2 }`); `forall` may not (Section 5 (f)). Note that
`not` binds tighter than comparison, as the design's precedence list
requires: `not a == b` is `(not a) == b`; write `not (a == b)`. A hole
admits no postfix operator: `?h.len` is a syntax error (parenthesize if
ever needed).

Parser rules for the postfix level:

1. *Name chains.* `a.b.c` is parsed as `Member(Member(Name a, b), c)`
   whatever `a` turns out to be (module, value, type, or enum). Name
   resolution folds a leading chain whose root is a module into a qualified
   reference. This is what makes `page.len` (property), `result.drop_last()`
   (method call), `gen.list(gen.int)` (module function), and
   `StoreError.Missing(id)` (qualified variant) parse the same way.
2. *Struct literal.* When the postfix expression so far is a pure name chain
   (a `name` followed only by `.member_name` postfixes) whose last segment
   is `UPPER_IDENT`, and the next token is `{`, and struct literals are
   enabled in the current context, the chain is reinterpreted as
   `type_path` and a `struct_literal` is parsed (Section 5 (a)). If the
   chain is not of the form `( LOWER_IDENT "." )* UPPER_IDENT` (an
   `UPPER_IDENT` before the last segment, as in `StoreError.Missing { ...
   }`), the `{` is a syntax error (`a struct literal names a type; variants
   have positional payloads`), since D20 has no record-style variants.
3. *Generic call.* `generic_args` may appear only when the postfix
   expression so far is a pure name chain (`check<Positive>(n)`,
   `std.check<Positive>(n)`, `x.convert<Int>()`); `f(x)<T>(y)` is a syntax
   error. On `<` after a name chain the parser speculatively parses
   `generic_args "("`; on failure it rewinds and the `<` is a comparison
   (Section 5 (b)).
4. *Calls.* Any postfix expression may be followed by `(args)`; the
   typechecker rejects non-callable targets (0.1 has no function values, so
   a callee is either a name chain or a postfix expression ending in
   `.member_name` whose member is a method or capability operation:
   `[1, 2, 3].take(2)`, `(-5).abs()`, `parse_size(text)?.is_ok()`,
   `spec/semantics.md` S11 and `spec/stdlib.md` S38). Positional arguments
   precede `using` arguments (`positional argument after using argument`,
   code `positional_after_using`, as for parameters in 3.9).

Decision: [D-038] D29. a struct literal's spread `..base` must be the first entry and
may appear at most once; explicit fields follow and override. Rationale: the
design writes `Page { ..page, text: trim(page.text) }`; fixing the position
makes "base then overrides" readable in one direction. Rejected
alternative: Rust's trailing `..base`.

Decision: [D-039] D30. struct literals have no field punning (`Page { id, text }`)
and take no explicit generic arguments (`Pair<Int, Text> { ... }`); the type
is inferred from the fields and the expected type. Rationale: `name: expr`
is one explicit form, and dropping `<...>` before `{` removes a second
speculative parse. Rejected alternatives: punning; turbofish-style type
arguments.

Decision: [D-040] D31. call-site `using` arguments take an arbitrary expression
(`using db`, `using ctx.reader`). Rationale: capabilities are forwarded
from parameters and fields; a restriction to bare names would be arbitrary.
Rejected alternative: `using LOWER_IDENT` only.

Decision: [D-041] D32. there are no closures, lambdas, or function values in the
0.1 grammar. Rationale: the design never shows one and effect tracking
through the call graph is simplest with named callees. Rejected
alternative: `fn(x) -> x + 1` literals.

### 3.13 Patterns

```ebnf
pattern   = "_"                                              // wildcard
          | "-"? INT                                         // Int literal
          | TEXT                                             // Text literal
          | "true" | "false"                                 // Bool literal
          | LOWER_IDENT                                      // binding
          | ctor_path ( "(" commas1(pattern) ")" )?          // constructor, nested patterns
ctor_path = ( LOWER_IDENT "." )* UPPER_IDENT ( "." UPPER_IDENT )*
```

`ctor_path` names an enum variant (`Missing`, `StoreError.Missing`,
`reader.pages.StoreError.Missing`), a prelude constructor (`Ok`, `Err`,
`Some`, `None`), or a nominal wrapper (`PageId(t)` unwraps the payload). A
bare `UPPER_IDENT` is a nullary constructor (`Unavailable`, `None`); a bare
`LOWER_IDENT` is a binding. Unqualified names resolve as in Section 5 (e).
The parser tells the two `LOWER_IDENT`-initial alternatives apart with the
same two-token lookahead as `assign_stmt` (3.11): a pattern beginning
`LOWER_IDENT "."` is a `ctor_path` (`reader.pages.StoreError.Missing(id)`);
a `LOWER_IDENT` followed by anything else is a binding.

Decision: [D-042] D33. the pattern language of 0.1 has no struct patterns
(`Page { id, .. }`), list patterns (`[]`, `[x, ..rest]`), or-patterns
(`A | B`), range patterns, `name @ pattern` bindings, or `if` guards.
Rationale: the design's patterns are constructors, literals, bindings, and
`_`; exhaustiveness checking is then a product of finite constructor sets,
and the semantic patch grammar does not need to understand richer
patterns. Rejected alternative: Rust's full pattern language (deferred,
not precluded).

## 4. Grammar of `.work` and `.patch` files

Both forms share the `.cpl` lexer and layout rules. A `.work` file contains
no expressions other than `TEXT` and `INT` literals, selectors, and the
`git("ref")` form; a `.patch` form additionally contains exactly one `.cpl`
`block` (Section 4.5), inside which the whole expression grammar of Section
3 applies. Their keywords are the contextual keywords of Section 1.4 except
`test`, which is the reserved token `KwTest` and is accepted by
`requirement` below.

### 4.1 Work file

```ebnf
work_file      = ( use_decl NEWLINE )* change_decl ( NEWLINE change_decl )* NEWLINE? EOF
change_decl    = DOC_COMMENT* "change" LOWER_IDENT NEWLINE? "{" lines(change_item) "}"
change_item    = "base" git_ref
               | "target" git_ref
               | "workers" INT
               | task_decl
               | integrate_decl
git_ref        = "git" "(" TEXT ")"
```

`use_decl` is the `.cpl` production of Section 3.2; it makes the imported
names available to selectors (`paginate.body` for
`reader.pages.paginate.body`). A `.work` file declares at least one change:
an empty file, or one holding only `use` declarations, is the syntax error
`` expected `change` (a .work file declares at least one change) `` (code
`syntax_error`), reported at the end of the file. A `use` after a `change`
is `use_after_declaration` (`use declarations must precede all change
declarations`, D13).

Rules enforced by the parser: `base`, `target`, `workers`, and `integrate`
appear at most once per change (`duplicate base`, ...); task names are
unique within a change; items may appear in any order, and the formatter
emits `base`, `target`, `workers`, tasks in source order, `integrate`.
Semantics (work coordinator): `base` and `target` are required, `workers`
defaults to 1, `after` names must refer to tasks of the same change and the
graph must be acyclic.

Decision: [D-043] D34. a `.work` file holds one or more `change` declarations, each
with a unique name across the project, so `cpl work integrate
pagination_update` resolves by name. The uniqueness is the work
coordinator's to enforce when it resolves a change by name (Milestone 6);
neither the parser nor Milestone 1's `cpl work check` checks it, and `cpl
work check`, which validates the changes of one file, accepts a name used
twice (`spec/toolchain.md` Appendix B has no code for it). Rationale: the CLI addresses changes
by name, not by file. Rejected alternative: exactly one change per file.

Decision: [D-044] D35. the only ref form is `git("<ref>")`; the `TEXT` is passed to
Git unchanged. Semantics (work coordinator): `base` may be any Git ref
(branch, tag, or commit id) and is resolved to a full commit id when the
change opens; `target` must name a branch, because the integrator advances
it and checks its head at integration (design line 351), so a tag or
commit id as `target` is a configuration error. Rationale: the design
resolves `base` to a commit object id when the change opens; other
version-control systems have no design. Rejected alternative: bare `main`
identifiers.

### 4.2 Tasks

```ebnf
task_decl      = DOC_COMMENT* "task" LOWER_IDENT after_clause? NEWLINE? "{" lines(task_item) "}"
after_clause   = "after" LOWER_IDENT ( "," LOWER_IDENT )*       // no trailing comma; same line as task
task_item      = "write" selector_set
               | "add" selector_set
               | "depend" selector_set
               | "preserve" selector_set
               | "goal" TEXT
selector_set   = "{" commas(selector) "}"                       // insignificant newlines; may be empty: write {}
```

Each item kind appears at most once per task (`duplicate write set`); the
formatter emits `write`, `add`, `depend`, `preserve`, `goal`. An empty set
`write {}` is legal and means "no write authority" (the design's `audit`
task). Every selector in an `add` set must be a namespace wildcard
(`add { tests.pagination.* }`): the design defines `add` as permission to
create declarations in a namespace (line 353), so a facet or declaration
selector in an `add` set is a syntax error (`add names a namespace; write
<path>.*`, with the selector's path: `add { tests.pagination }` is `add
names a namespace; write tests.pagination.*`).

Decision: [D-045] D68. in 0.1 the requests a task declaration makes are its `write`
and `add` sets (with `depend` naming assumptions and `preserve` recording
equality requirements); `task_item` has no form for requesting filesystem,
process, or network access, and those grants come solely from project
policy (`cpl.toml`) and the submitter's authority. Rationale: the design
says "Task declarations request capabilities within the submitter's
authorization" (line 361) and that granted permissions are "the
intersection of project policy, the submitter's authority, and task
requests" (line 367), but its `.work` example and its outline
(`task_item*`) show no request item beyond the edit scopes; the edit scopes
are the requests this grammar can express, and a request vocabulary for
host resources belongs with the coordinator's sandbox design, which the
design leaves to enforcement ("Merely parsing a declaration does not
sandbox an agent", line 361). Rejected alternative: a `request {
filesystem, network, ... }` item with a fixed vocabulary, deferred so that
the grammar does not name a security vocabulary the coordinator does not
yet enforce.

### 4.3 Integration

```ebnf
integrate_decl = "integrate" NEWLINE? "{" lines(require_item) "}"
require_item   = "require" requirement
requirement    = "typecheck"
               | "no_holes"
               | "no_unresolved_assumptions"
               | "prove" "(" selector ")"
               | "test" "(" selector ")"                        // "test" is the reserved keyword token
               | "review" "(" LOWER_IDENT ")"                   // a task name of the same change
```

Decision: [D-046] D36. the requirement vocabulary is closed in 0.1 and fixed in the
grammar: `typecheck`, `no_holes`, `no_unresolved_assumptions`,
`prove(selector)`, `test(selector)`, `review(task)`. Any other token
written after `require` is an unknown requirement, the diagnostic
`unknown_requirement` with the field `found` and a message that lists the
six forms (`` unknown requirement <token>; expected typecheck, no_holes,
no_unresolved_assumptions, prove(selector), test(selector), or
review(task) ``), except that a reserved-future word there is
`reserved_word` (D4); `require` followed by the end of the line, `}`, or
the end of the file names no requirement at all and is the generic syntax
error `expected a requirement (typecheck, no_holes,
no_unresolved_assumptions, prove(selector), test(selector), or
review(task))`. `no_unresolved_assumptions` is
the design's strict integration requirement (line 136). Semantics (work
coordinator): it rejects a candidate whose proof evidence depends on an
assumption about a CPL implementation (an unchecked callee or a hole)
that is neither discharged nor explicitly admitted by project policy;
assumptions about external adapters are not rejected by it and must remain
named in the evidence. Rationale: the integration policy is frozen per run
and is the security boundary of a change; a closed grammar means a stale
or malicious `.work` file cannot name a requirement the integrator
silently ignores, and the design's own words place assumption strictness
among the integration requirements rather than in `cpl.toml`, which admits
assumptions but does not reject them. Rejected alternatives: an open
`name(args)` form validated by the coordinator; assumption strictness as
`cpl.toml` policy only.

### 4.4 Selectors

```ebnf
selector       = selector_path "." "*"                          // every declaration in a namespace: tests.pagination.*
               | selector_path "." facet                        // one facet: paginate.body, reader.pages.paginate.contract
               | selector_path                                  // a whole declaration: paginate, Positive
selector_path  = selector_seg ( "." selector_seg )*
selector_seg   = LOWER_IDENT | UPPER_IDENT
facet          = "identity" | "name" | "signature" | "contract" | "effects"
               | "body" | "docs" | "verification_policy" | "definition"
```

Implementation note: read the dotted segments greedily; if the final
segment is `*` the selector is a namespace wildcard; if the final segment is
spelled as a facet name (or is the `KwEffects` token) it is a facet
selector; otherwise the selector denotes a whole declaration. The quoted
words of `facet` are contextual spellings of `LOWER_IDENT` tokens (Section
0), so `"name"` here is the facet spelling, not the expression nonterminal
`name` of 3.12.

Decision: [D-047] D37. a final segment spelled as a facet name is always the facet.
A declaration that happens to be named `body` or `contract` is selected as
`module.body.definition`. Rationale: facet names are the fixed vocabulary of
the semantic index and selectors must parse without name resolution.
Rejected alternative: resolving the ambiguity by lookup.

Decision: [D-048] D38. a bare declaration selector (`paginate`, `Positive`) is
equivalent to `.definition`: in `prove(...)` it names the declaration whose
obligations are proved, and in a `write`/`depend`/`preserve` set it covers
every facet (an `add` set takes namespace wildcards only, Section 4.2). `*`
names all declarations of a namespace, that is of the module so named and
of every module whose path extends it (`tests.*` covers `tests.pagination`
and `tests.pagination.edge`; `spec/toolchain.md` T43), and is never a facet
wildcard (`paginate.*` is an error if `paginate` is a declaration and no
module exists under the path; where one does, the namespace is that
module's, as a chain resolves to the longest module path,
`spec/semantics.md` 2.4, and `spec/toolchain.md` T67 gives the code).
Rationale: the design writes `prove(paginate)` and `Positive.definition`
side by side with the same meaning for the declaration as a whole. Rejected
alternative: requiring an explicit facet everywhere.

Decision: [D-049] D69. a selector reaches a test or property only through its
namespace wildcard (`tests.pagination.*`); `selector_seg` admits no `TEXT`
segment, so a single test cannot be named in a `write`, `preserve`,
`prove`, or `test(...)` position in 0.1. Rationale: the design addresses
tests only as `tests.pagination.*` (lines 330, 342) and requires a test
selector to match at least one test (line 421); a `TEXT` segment would be
the one selector form that is not an identifier path. Rejected
alternative: `selector_seg = ... | TEXT` as a final segment
(`tests.pagination."pagination handles empty input"`), deferred, not
precluded.

Semantics (work coordinator): selectors are resolved by the language
tooling against the `use` imports of the `.work` file and otherwise fully
qualified; they are never filesystem globs; a selector that matches nothing
is a configuration error, except that an `add` namespace may name a
namespace in which no module exists yet, since creating the first module
there is what `add` permits (`spec/toolchain.md` 7.2, `cpl work check`).

### 4.5 Patch file

```ebnf
patch_file     = patch_form ( NEWLINE patch_form )* NEWLINE? EOF
patch_form     = DOC_COMMENT* "patch" selector NEWLINE "expect" "base" "(" selector ")" NEWLINE block
```

`expect` is a clause and `patch selector` a header in the sense of Section
2.4, so `expect` starts its own line and the `{` starts the next one
(rules 1 and 2); a one-line `patch x expect base(x) {` is a syntax error.
The `block` is the `.cpl` production of Section 3.11, parsed in the lexical
and generic scope of the function the selector names (so `T`, `items`, and
`size` resolve inside the `paginate.body` patch). Inside the block,
`patch`, `expect`, and `base` are ordinary identifiers again.

Semantics (work coordinator): the two selectors of a patch form are
resolved against the `use` imports of the `.work` file that declares the
submitting task (the submission artifact carries the task identity, design
line 415), then as fully qualified paths; `paginate.body` in the corpus
patch resolves through `use reader.pages.{paginate, Positive}` of
`pagination.work`. A patch submitted outside any task must use fully
qualified selectors (`reader.pages.paginate.body`).

Decision: [D-050] D39. in 0.1 a patch targets a `body` facet only, its payload is a
`block`, and `expect base(selector)` is mandatory and is the only `expect`
form. The patch selector and the expected selector must denote the same
facet (the coordinator checks this). Rationale: the design's outline gives
the patch exactly one payload production, `block` (lines 630-631), and a
block can replace only a body; its example (389-411) and prose (413)
describe body replacement with an exact facet-version check. Changes to
the other facets a task may be authorized for are not dropped by this
restriction; they take the other submission path: `write` "permits changes
to specified existing facets" (line 353), the `preserve` set names
`signature`, `contract`, `effects`, and `verification_policy` individually
(lines 320-325), renames and moves are authorized structural edits (line
417), and the swarm demonstration requires a deliberate concurrent contract
change (line 653) under a scope such as `write { paginate.contract }`
(line 365, "Specification changes require their own explicit scope", is a
statement about authority, which Section 4.2 expresses). Semantics (work
coordinator): a candidate is the task's isolated worktree plus zero or
more patch forms; a facet change, rename, or move is submitted as an
ordinary `.cpl` edit in the worktree, and the coordinator computes the
facet-level diff of the candidate against the task's baseline and checks
it against the `write`, `add`, and `preserve` sets (line 415); a patch
form is a body-only shortcut that lets a body replacement tolerate
unrelated file movement while refusing a stale base (line 413). Rejected
alternative: a payload grammar per facet (contract, signature, effects,
docs, and a rename form for `name`), which the design's outline does not
have and which would have to be designed together with per-facet version
checks.

Decision: [D-051] D40. patch forms have a file form of their own, extension
`.patch`, selected by the parser like `.cpl` and `.work` (Section 1.1); a
`.patch` file contains only patch forms (no `module`, `use`, or
declarations), and a patch form may not appear inside a `.work` or `.cpl`
file. Rationale: the design names only `.cpl` and `.work` as file forms
(line 7) and describes the patch only as content a candidate "can contain"
(line 387), so where its text lives is undecided; a file of its own keeps
the submission artifact separate from the work declaration, which is frozen
for the run (line 351), and its selectors take their scope from the
submitting task's change (Semantics above), not from imports of its own.
Rejected alternatives: embedding patches in `.work` files; a `.cpl` file
that mixes declarations and patch forms.

## 5. Disambiguation

Each entry fixes one reading. The letters match the questions the grammar
had to answer.

### (a) Struct literal `Type {` versus a block `{`

Decision: [D-052] D41. struct literals are disabled at the top level of a *header
expression* and re-enabled inside any bracket within it. Header expressions
are: the condition of `if` and `while`, the scrutinee of `match`, the range
of a `for` clause, and the expressions of `requires`, `ensures`,
`invariant`, and `decreases` clauses. The parser keeps a boolean
`struct_literals_allowed`, initially true. To parse a header expression it
saves the flag, sets it to false, parses the expression, and restores the
saved value *before* consuming the `NEWLINE` or `{` that ends the header;
the flag is therefore false only while the header expression itself is
being read, and the body, the match arms, any `else` branch, and every
later statement see the restored value (true at statement level). Within a
header expression the flag is set to true, again with save and restore, on
consuming `(`, `[`, a call's argument list, a struct literal's `{`, a
block's `{`, or a `match` body's `{`. In `if page == Page { .. } {` the
`{` after `Page` therefore opens the `if` body and `Page` is a nullary
constructor reference; the parse then fails at `..`, which cannot begin a
statement (`if page == Page {}` instead parses as an `if` with an empty
body and leaves `page == Page` to the typechecker). The intended form is
`if page == (Page { .. }) {`. Both struct literals below are legal:

```text
fn relabel(c: Bool, p: Page) -> Page {
    if c {
        let _ = 1
    }
    Page { ..p, text: "x" }              // flag restored after the `if` header
}

fn pick(r: Result<Page, StoreError>, p: Page) -> Page {
    match r {
        Ok(q) => Page { ..q, text: "x" }   // arms see the restored flag
        Err(_) => p
    }
}
```

Rationale: this is Rust's rule; it needs no lookahead and keeps `{` after a
header unambiguous. Rejected alternative: scanning ahead for a matching `}`
followed by another `{`.

### (b) `ident<Type>(` generic call versus `<` comparison

Decision: [D-053] D42. after a pure name chain the parser, on seeing `<`,
speculatively parses `generic_args` followed by `(`; if that succeeds the
result is a generic call, otherwise it rewinds to the `<` and the postfix
loop ends, leaving `<` to `cmp_expr`. The speculative parse is
authoritative: when `generic_args "("` parses, the generic-call reading is
taken even where a comparison reading also exists. Within one operand chain
the two readings never coexist, because comparisons do not chain (`a < B >
(c)` would be the chained form the grammar rejects); across a comma they
can: `f(a < B, C > (d))` is one argument, the generic call `a<B, C>(d)`,
never the two comparisons `a < B` and `C > (d)`. In an argument list or
list literal, a comparison whose right operand is a type-shaped name
followed by `,` ... `>` `(` must therefore be parenthesized: `f((a < B), C
> (d))`. Speculation is bounded by the type grammar (no operators, no
parentheses) and, where newlines are significant, by the line (a type
cannot contain `NEWLINE`); inside an insignificant-newline context (Section
2.2) `NEWLINE` tokens are discarded before the parser sees them, so there
the speculative parse may span lines: `f(a <` followed on the next line by
`B > (c))` is the generic call `a<B>(c)`, and a comparison reading needs
the parentheses described above. Rejected alternative: a turbofish `::<T>`
or `check[Positive]` spelling, which the design does not use.

### (c) Prefix `?name` versus postfix `?`

D10 (Section 1.7) decides this in the lexer: `?` immediately
followed by an identifier-start character is a `HOLE` token; any other `?`
is `Question`. The parser never sees an ambiguous `?`.

### (d) `len` as a property versus method calls

Decision: [D-054] D43. the grammar has one member-access form, `.member_name`,
optionally followed by a call. `page.len`, `rest.len`, `page.is_empty()`,
`rest.take(size)`, and `result.drop_last()` all parse as
`Member(expr, name)` with or without a following `Call`. Whether a member is
a property (used without parentheses: `len`) or a method (`is_empty()`,
`take(n)`, `drop(n)`, `drop_last()`, `push(value)`) is recorded in the
type's member table and checked by the typechecker: `page.len()` is
rejected with `len is a property; write page.len`, and `page.is_empty`
without a call is rejected with `is_empty is a method; write
page.is_empty()`. Rationale: the design fixes the spelling of each member
and the grammar should not encode a per-name table. Rejected alternative:
a lexer or parser list of property names.

### (e) Enum variant resolution

Decision: [D-055] D44. an unqualified `UPPER_IDENT` constructor in a pattern or an
expression (`Missing(_)`, `Unavailable`, `None`, `PageId(t)`) resolves (1)
against the constructors of the expected type when one is known from
context (the scrutinee's type for a pattern, the parameter or annotation
type for an expression): the variants of an enum, or the single
constructor of a nominal wrapper `type`; then (2) against the constructors
in lexical scope: the variants of every enum and the constructor of every
nominal wrapper declared in the module or imported by `use`, together with
those of the prelude's `Option`, `Result`, and `Unit`, which take part as
if imported. Step (2) must yield exactly one candidate; two candidates (a
variant and a wrapper of the same name, or a module variant named like a
prelude constructor, such as `None`) are an error naming both and
suggesting the qualified form; no declaration shadows another. The
qualified forms `StoreError.Missing(x)`,
`reader.pages.StoreError.Missing(x)`, and `reader.pages.PageId(t)` are
always available, and the prelude constructors `Ok`, `Err`, `Some`, `None`,
and `Unit` are always in scope. A bare `UPPER_IDENT` in expression position
is always a constructor reference, never a type (types occur only in type
positions); the wrapper constructor is spelled with the type's name,
`PageId("page-1")`, exactly as the design constructs it (line 75).
Rationale: the design writes `Err(Missing(_))` unqualified, which needs
expected-type resolution; the lexical fallback keeps `Missing(id)` usable
where no type is expected yet; treating a wrapper as a one-constructor type
gives `PageId(t)` the same rule in expressions and in patterns (3.13).
Rejected alternatives: requiring qualification or `use` of variants;
letting local declarations shadow prelude constructors.

### (f) Extent of the `forall` predicate

Decision: [D-056] D45. in `forall x in c: p`, the collection `c` is an `or_expr`
(it stops at the `:`) and the predicate `p` is a full `expr` that extends as
far as possible, to the end of the line or the enclosing bracket, and may
itself be a `forall`. `forall` is the lowest-precedence form: it may appear
only as a complete expression, inside parentheses, or as the predicate of
another `forall`; `a and forall x in c: p` is a syntax error (write
`a and (forall x in c: p)`). Hence `forall page in result: page.len > 0 and
page.len <= size` quantifies the whole conjunction. Rationale: the design
writes exactly that example and a maximally extending prefix form must not
be a left operand. Rejected alternative: making `forall` a primary with a
parenthesized body.

### (g) `if` as an expression versus a statement

Decision: [D-057] D46. there is one `if_expr` production; `if` is always an
expression. As an expression statement its value is discarded (and must
therefore be `Unit`, which an `if` without `else` always is). Used as a
value, the typechecker requires an `else` branch and compatible branch
types, as the design states. The `else` keyword must follow the closing `}`
on the same line (D12). Rejected alternative: separate statement and
expression productions.

### (h) Empty list `[]`

The grammar accepts `list_literal` with zero elements. Semantics: its
element type comes from the expected type (a `let` annotation, a parameter
type, the return type, or the other operand of `==`), and `let x = []`
without an annotation is a type error (`cannot infer the element type`).

### (i) `return` with or without a value

Decision: [D-058] D47. `return expr?`; the value is optional. Because `NEWLINE` or
`}` terminates the statement there is no ambiguity. `return` without a value
is permitted only in a function returning `Unit`. Rationale: `Unit`
functions (`-> Result<Unit, E>` aside) need an early exit too. Rejected
alternative: `return Unit` spelled out.

### (j) Negative literals and unary minus

Decision: [D-059] D48. the lexer never produces a negative `INT`; in expressions `-5`
is `Neg(Lit 5)` and the compiler may fold it. `-x.len` is `-(x.len)`
(postfix binds tighter), `2 * -3` is `2 * (-3)`, `a -1` is the subtraction
`a - 1`, and `- 5` with a space equals `-5`. Only in pattern position does
the grammar read `"-"? INT` as a single literal pattern. Rejected
alternative: lexing `-5` as one token, which breaks `a-5`.

### (k) `else if` chains

Decision: [D-060] D49. `else` may be followed by another `if_expr`, giving `else if`
chains with a single final `else`. There is no `elif`. Rationale: the
design shows `if` and `else` only; chains are the universal expectation.
Rejected alternative: nested `else { if ... }` only.

### (l) `result` and `self`

Decision: [D-061] D50. `result` and `self` are ordinary `LOWER_IDENT` tokens with
contextual meaning. In an `ensures` clause `result` denotes the returned
value and shadows any other binding of that name; in a `where` predicate
`self` denotes the underlying value. A parameter or local named `result`
in a function that has `ensures` clauses is a check error (the design's
`label(result: Result<Page, StoreError>)` has no `ensures` and is legal);
binding `self` anywhere is a lint warning. Rationale: reserving `result`
would reject a design example; contextual meaning is exactly what the
design describes ("named `result`"). Rejected alternative: making both
reserved keywords.

### (m) Empty blocks

Decision: [D-062] D51. a block may be empty: `{}` or `{` `NEWLINE` `}` has no
statements and value `Unit`. Rationale: `if c {}` and placeholder bodies
should not need a dummy expression. Rejected alternative: requiring at
least one statement.

### Further rulings

Decision: [D-063] D52. `_` alone is the wildcard token (`Underscore`), legal in
patterns and in `let _ =`; `_x` is a `LOWER_IDENT`. Rationale: Rust's
convention, and the only `_` the design uses is the pattern wildcard.
Rejected alternative: treating `_` as an identifier.

Decision: [D-064] D53. a member name after `.` is a `LOWER_IDENT` or `UPPER_IDENT`
and never a keyword: `x.type` is a syntax error (`expected a member name`).
The exceptions are the facet `effects` in selectors (Section 4.4) and
annotation names (D71), neither of which is a member name. Rejected
alternative: keyword escapes such as `r#type`.

Decision: [D-065] D54. `..` appears only as the struct-literal spread; there are no
range expressions or range patterns. Rejected alternative: `a..b` ranges.

Decision: [D-066] D55. match arms are separated by `NEWLINE`, never by commas; a `,`
after an arm body is a syntax error with the diagnostic `match arms are
separated by newlines`. Rationale: the design shows no commas and states
"one statement per line". Rejected alternative: optional commas.

A `,` after the element of any other newline-separated body is reported the
same way, naming the element: `statements are separated by newlines`,
`fields are ...`, `variants ...`, `operations ...`, `change items ...`,
`task items ...`, and `requirements are separated by newlines` (code
`syntax_error`).

Decision: [D-067] D56. `test` is reserved and `tests` is an ordinary identifier, so
the design's module `tests.pagination` is a valid module path while a
module or function named `test` is not. Rejected alternative: making `test`
contextual.

## 6. Examples: every design-document code block

This section takes each code block of `design-doc.md` written in `.cpl`,
`.work`, or patch syntax and lists the productions it exercises, top level
first. Line numbers refer to `design-doc.md`. The corpus column gives the
file in `examples/` that contains the block verbatim. Blocks in other
languages (JSON at 229-239, shell at 251-259, TOML at 544-557 which is
copied verbatim into `examples/reader/cpl.toml`, Python at 578-583,
TypeScript at 587-591) are not CPL syntax and are not traced. Every CPL
block below parses under Sections 1-4 without modification. Two refinements
of the design's grammar *outline* were needed on the way: `NEWLINE? block`
after a header (D63), without which the `paginate_ints` example's `{` on
its own line would not parse, and the `decreases` clause on a function
header (D22), which no design example uses but the design's prose at line
172 requires ("a recursive verified function similarly declares a
decreasing measure").

Layout facts common to all traces: blank lines inside a block or body are a
single `NEWLINE` token; the line terminator after a `{` that ends its line
is the leading `NEWLINE?` of `lines(X)` (Section 0), so every multi-line
body below is `"{" NEWLINE ... "}"`; `{` on its own line after a header is
`NEWLINE? block`; each `ensures`/`invariant`/`for`/`effects` line is
`NEWLINE clause`.

### E1. Core syntax (lines 48-69) - `examples/reader/src/reader/pages.cpl`

```text
source_file
  module_header            "module" module_path(reader.pages)
  NEWLINE use_decl         list form: use_path(std.text) "." "{" use_item(trim) "}"
  NEWLINE declaration      type_decl: "pub" "type" PageId wrapper_decl("(" type(Text) ")")
  NEWLINE declaration      type_decl: "pub" "type" Positive refinement_decl: "=" type(Int) "where" expr
                             expr -> cmp_expr: add_expr(name self) ">" add_expr(INT 0)
  NEWLINE declaration      struct_decl: "pub" "struct" Page "{" lines(field_decl: id ":" type(PageId) ;
                             text ":" type(Text)) "}"
  NEWLINE declaration      enum_decl: "pub" "enum" StoreError "{" lines(variant_decl: Missing "(" type(PageId) ")" ;
                             variant_decl: Unavailable) "}"
  NEWLINE declaration      fn_decl: "fn" clean "(" params(param page ":" Page) ")" "->" type(Page)
                             fn_tail: block (same line)
                             block: lines(expr_stmt) = tail expression
                               struct_literal: type_path(Page) "{" struct_entries(".." expr(name page) ","
                                 field_init(text ":" postfix_expr(name trim, call(args(postfix_expr(name page "." text)))))) "}"
  EOF
```

The struct literal is legal here because an expression statement is not a
header expression (D41). `self` is a `LOWER_IDENT` with contextual meaning
(D50).

### E2. Expressions and errors (lines 79-92) - `pages.cpl`

```text
declaration fn_decl   "fn" label "(" param result ":" type(Result generic_args(Page "," StoreError)) ")" "->" Text block
  block tail expr_stmt -> match_expr: "match" expr(name result) "{" lines(match_arm) "}"
    match_arm   pattern ctor_path(Ok) "(" pattern(LOWER_IDENT page) ")" "=>" expr(postfix: name page "." text)
    match_arm   pattern ctor_path(Err) "(" pattern(ctor_path(Missing) "(" pattern("_") ")") ")" "=>" expr(TEXT)
    match_arm   pattern ctor_path(Err) "(" pattern(ctor_path(Unavailable)) ")" "=>" expr(TEXT)
declaration fn_decl   "fn" positive_size "(" param text ":" Text ")" "->" type(Result<Positive, ConstraintError>) block
  statement let_stmt  "let" n "=" expr(postfix_expr: name parse_size, call(args(name text)), "?")
  statement expr_stmt (tail) postfix_expr: name check, generic_args("<" type(Positive) ">"), call(args(name n))
```

`Missing(_)` and `Unavailable` are unqualified constructors resolved by the
expected type `StoreError` (D44); `Unavailable` is a bare `UPPER_IDENT` and
therefore a constructor, not a binding (D2). `check<Positive>(n)` is the
speculative generic call of D42. The parameter name `result` is legal
because `label` has no `ensures` clause (D50).

### E3. Functions and contracts, hole version (lines 104-114) - `examples/holes/src/holes/pages.cpl`

```text
declaration
  decl_prefix   annotation "@" id "(" commas(literal: TEXT "pgn_7c2d") ")" NEWLINE
                annotation "@" prove NEWLINE
  fn_decl       "pub" "fn" paginate generic_params("<" T ">") "(" params(items ":" List<T> "," size ":" Positive) ")"
                "->" type(List generic_args(List generic_args(T)))
  fn_tail       NEWLINE contract_clause "ensures" expr(cmp_expr: call(flatten, args(name result)) "==" name items)
                NEWLINE contract_clause "ensures" forall_expr "forall" page "in" or_expr(name result) ":"
                          expr(and_expr: cmp_expr(page.len ">" 0) "and" cmp_expr(page.len "<=" size))
                NEWLINE contract_clause "ensures" forall_expr "forall" page "in" or_expr(postfix: result "." drop_last call())
                          ":" expr(cmp_expr(page.len "==" size))
                NEWLINE block: lines(expr_stmt: unary_expr HOLE(build_pages)) = tail expression
```

`List<List<T>>` lexes as five tokens with two separate `>` (Section 1.6).
The `forall` predicate extends over the whole conjunction (D45). `?build_pages`
is one `HOLE` token (D10) and is the block's tail expression.

### E4. divide_exactly (lines 120-128) - `pages.cpl`

```text
fn_decl   "fn" divide_exactly "(" params(a ":" Int "," b ":" Int) ")" "->" Int
fn_tail   NEWLINE contract_clause "requires" cmp_expr(name b "!=" INT 0)
          NEWLINE contract_clause "requires" cmp_expr(mul_expr(a "%" b) "==" INT 0)
          NEWLINE contract_clause "ensures"  cmp_expr(mul_expr(name result "*" b) "==" a)
          NEWLINE block: tail expr_stmt mul_expr(a "/" b)
```

### E5. Completed paginate with loop invariants (lines 142-168) - `pages.cpl`

The header and contract lines are identical to E3. The body:

```text
block "{" lines(statement) "}"
  var_stmt    "var" pages ":" type(List<List<T>>) "=" list_literal("[" commas() "]")      // [] typed by the annotation (h)
  var_stmt    "var" rest "=" expr(name items)
  while_stmt  "while" expr(cmp_expr: postfix(rest "." len) ">" name size)
    while_tail NEWLINE while_clause "invariant" cmp_expr(add_expr(call(flatten, pages) "++" rest) "==" items)
               NEWLINE while_clause "invariant" forall_expr(page "in" pages ":" cmp_expr(page.len "==" size))
               NEWLINE while_clause decreases_clause "decreases" postfix(rest "." len)
               NEWLINE block
                 assign_stmt pages "=" postfix(pages "." push call(args(postfix(rest "." take call(args(size))))))
                 assign_stmt rest  "=" postfix(rest "." drop call(args(size)))
  expr_stmt   if_expr "if" expr(unary_expr "not" postfix(rest "." is_empty call())) block(assign_stmt pages "=" ...)
  expr_stmt   (tail) name pages
```

`++` sits at the additive level, so `flatten(pages) ++ rest == items` is
`(flatten(pages) ++ rest) == items`. `not` applies to the whole postfix
expression `rest.is_empty()` (level 1 binds tighter than level 2). The `if`
without `else` is an expression statement of type `Unit` (D46). The blank
line after `var rest = items` is one `NEWLINE`.

### E6. Capabilities and effects (lines 178-205) - `pages.cpl`

```text
capability_decl "pub" "capability" PageRead "{" lines(op_decl: "fn" get "(" param id ":" PageId ")" "->" Result<Page, StoreError>) "}"
capability_decl "pub" "capability" PageWrite "{" lines(op_decl: "fn" put "(" param page ":" Page ")" "->" Result<Unit, StoreError>) "}"
fn_decl   "pub" "fn" load_page "(" params(param id ":" PageId "," using_param "using" db ":" PageRead ",") ")"
          "->" Result<Page, StoreError>
fn_tail   NEWLINE effects_clause "effects" "{" commas(effect_ref db "." get) "}"
          NEWLINE block: tail postfix(name db "." get call(args(name id)))
fn_decl   "pub" "fn" load_text  ... same header shape ...
fn_tail   NEWLINE effects_clause "effects" "{" effect_ref(db.get) "}"
          NEWLINE block
            let_stmt  "let" page "=" postfix(name load_page call(args(name id "," using_arg "using" expr(name db))) "?")
            expr_stmt (tail) postfix(name Ok call(args(postfix(page "." text))))
```

The parameter list spans lines because `(` opens an insignificant-newline
context, and the trailing comma after `using db: PageRead,` is `commas`'s
optional `,` (Section 2.5). `using db` at the call site is `using_arg`
(D31); it follows the positional argument as required.

### E7. Hole in a body (lines 219-223) - `examples/holes/src/holes/pages.cpl`

```text
fn_decl "fn" summarize "(" param page ":" Page ")" "->" Text block(tail expr_stmt: unary_expr HOLE(summary))
```

### E8. Tests and properties (lines 265-281) - `examples/reader/src/tests/pagination.cpl`

```text
test_decl   "test" TEXT("pagination preserves a short tail") block
  assert_stmt "assert" cmp_expr(call(paginate, args(list_literal(1 "," 2 "," 3) "," INT 2)) "=="
                list_literal(list_literal(1 "," 2) "," list_literal(3)))
test_decl   "test" TEXT("pagination handles empty input") block
  let_stmt    "let" items ":" type(List<Int>) "=" list_literal()
  assert_stmt "assert" cmp_expr(call(paginate, args(items "," 2)) "==" list_literal())
property_decl "property" TEXT("pagination preserves input")
  NEWLINE for_clause "for" items "in" expr(postfix(name gen "." list call(args(postfix(name gen "." int)))))
  NEWLINE for_clause "for" size  "in" expr(postfix(name gen "." positive_int))
  NEWLINE block
  assert_stmt "assert" cmp_expr(call(flatten, args(call(paginate, args(items "," size)))) "==" name items)
```

`gen.list(gen.int)` is a name chain followed by a call (Section 3.12 rule
1); whether `gen` is a module or a value is resolution's business. The
empty literal in `assert paginate(items, 2) == []` takes its type from the
other operand (h). Literal `2` where `Positive` is expected is the design's
compile-time refinement proof (semantics).

### E9. The `.work` declaration (lines 309-347) - `examples/reader/work/pagination.work`

```text
work_file
  use_decl NEWLINE    "use" use_path(reader.pages) "." "{" commas1(use_item paginate "," use_item Positive) "}"
  change_decl         "change" pagination_update "{" lines(change_item) "}"
    change_item   "base"    git_ref("git" "(" TEXT "main" ")")
    change_item   "target"  git_ref("git" "(" TEXT "main" ")")
    change_item   "workers" INT 3
    change_item   task_decl "task" implementation "{" lines(task_item) "}"
      task_item   "write"    selector_set("{" selector(selector_path paginate "." facet body) "}")
      task_item   "depend"   selector_set(selector(paginate "." contract) "," selector(Positive "." definition))
      task_item   "preserve" selector_set( four facet selectors across lines, trailing "," )
      task_item   "goal"     TEXT
    change_item   task_decl "task" examples "{" task_item "add" selector_set(selector(selector_path tests.pagination "." "*")) ; "goal" TEXT "}"
    change_item   task_decl "task" audit after_clause("after" implementation "," examples) "{" "write" selector_set() ; "goal" TEXT "}"
    change_item   integrate_decl "integrate" "{" lines(require_item) "}"
      require_item "require" requirement typecheck
      require_item "require" requirement "prove" "(" selector(paginate) ")"
      require_item "require" requirement "test"  "(" selector(tests.pagination "." "*") ")"
      require_item "require" requirement "review" "(" audit ")"
      require_item "require" requirement no_holes
  EOF
```

The multi-line `preserve { ... }` set is legal because a selector set
opens an insignificant-newline context (Section 2.2), and its trailing
comma is `commas`'s. `write {}` is an empty set (Section 4.2). `test` in
`require test(...)` is the reserved keyword token accepted by
`requirement` (D56). `tests.pagination.*` is a namespace wildcard (D38).

### E10. The patch form (lines 389-411) - `examples/reader/work/pagination.patch`

```text
patch_file
  patch_form "patch" selector(selector_path paginate "." facet body)
             NEWLINE "expect" "base" "(" selector(paginate "." body) ")"
             NEWLINE block   // byte-for-byte the body of E5
  EOF
```

The block is the `.cpl` `block` production parsed in `paginate`'s scope
(D39), so `T`, `items`, and `size` resolve.

### E11. Adapter module (lines 514-538) - `examples/reader/src/reader/images.cpl`

```text
source_file
  module_header   "module" module_path(reader.images)
  NEWLINE use_decl "use" use_path(reader.pages) "." "{" use_item(Positive) "}"
  NEWLINE use_decl "use" use_path(std.foreign) "." "{" use_item(ForeignError) "}"
  NEWLINE struct_decl "pub" "struct" Dimensions "{" lines(field_decl width ":" Positive ; height ":" Positive) "}"
  NEWLINE capability_decl "pub" "capability" ImageCodec "{" lines(op_decl "fn" dimensions "(" param image ":" Bytes ")"
                            "->" Result<Dimensions, ForeignError>) "}"
  NEWLINE fn_decl "pub" "fn" image_width "(" params(param image ":" Bytes "," using_param codec ":" ImageCodec ",") ")"
                  "->" Result<Positive, ForeignError>
    fn_tail NEWLINE effects_clause "effects" "{" effect_ref(codec "." dimensions) "}"
            NEWLINE block
              let_stmt  "let" dimensions "=" postfix(name codec "." dimensions call(args(name image)) "?")
              expr_stmt (tail) postfix(name Ok call(args(postfix(name dimensions "." width))))
  EOF
```

Two `use` declarations from different modules are two `use_decl` items
(D13). The local `dimensions` shadows nothing: `codec.dimensions` is a
member of `codec`, and `dimensions.width` a member of the local.

### E12. Export wrapper (lines 565-574) - `pages.cpl`

```text
fn_decl "pub" "fn" paginate_ints "(" params(param items ":" List<Int> "," param size ":" Int ",") ")"
        "->" type(Result generic_args(List<List<Int>> "," ConstraintError))
fn_tail NEWLINE? block     // `{` alone on the next line, no clauses (D63)
  let_stmt  "let" checked_size "=" postfix(name check generic_args(Positive) call(args(name size)) "?")
  expr_stmt (tail) postfix(name Ok call(args(postfix(name paginate call(args(items "," checked_size))))))
```

### E13. The design's grammar outline (lines 610-632)

The outline is not CPL source; this grammar refines it as follows.

| Outline production | This document | Difference |
| - | - | - |
| `function` | `fn_decl` (3.9) | adds `decreases` (D22); accepts `effects`, contracts, and `decreases` in any order and restores the outline's order on output (D66); makes the newline before `block` explicit (Section 2.4, D63) |
| `effects` | `effects_clause` | adds an optional trailing comma (Section 2.5) |
| `contract` | `contract_clause` | the terminating `newline` is the `NEWLINE` that precedes the next clause or the body |
| `annotation` | `annotation` | name is `LOWER_IDENT` or a keyword (D61, D71); arguments restricted to literals (D61) |
| `hole` | `HOLE` token | `?` and the identifier must be adjacent; the name must be snake_case and not a keyword (D10, D61) |
| `propagate` | `postfix "?"` | identical |
| `while_loop` | `while_stmt` | clauses may appear in any order; the formatter emits invariants then `decreases` (D66) |
| `change`, `task` | `change_decl`, `task_decl` (4.1, 4.2) | item sets spelled out; names and `after` names are `LOWER_IDENT` (D61); `add` takes namespace wildcards only (4.2) |
| `patch` | `patch_form` (4.5) | `facet_ref` is `selector`; `expect` and `{` each start a new line (Section 2.4) |

### Corpus notes

Decision: [D-068] D57. the example corpus treats `flatten`, `check`,
`ConstraintError`, and `gen` as prelude names and adds no `use` lines for
them; the library specification confirms the reading (`spec/stdlib.md` 1.1
and S1: `flatten`, `check`, and `ConstraintError` are `std.prelude`
declarations in scope everywhere, and `gen` is the module `std.gen` bound
in the prelude scope), as does `spec/semantics.md` 1.7, so the glue lines
described below are not needed.
Design conflict: [D-069] design line 71 enumerates the prelude as "Int, Bool, Text,
Bytes, Unit, List, Option, Result, and their constructors", yet the design
uses `check<Positive>` and `ConstraintError` (lines 75, 88-91, 566-573) in
a module whose only import is `std.text.{trim}`, `flatten` (108, 146, 154,
279) in that module, in the holes example, and in the tests, and `gen`
(276-277) in the tests, whose module header and imports the design does
not show at all (line 285 only places them in `tests.pagination`; the
corpus's header and its `use reader.pages.{paginate}` are glue). The corpus
keeps the design's text and resolves the conflict in favour of the
examples; the grammar parses identically under either reading, so the
choice binds only the library specification, which owns the prelude's
final content. Rejected alternative: `// glue:` import lines in the style
of D58 (`use std.list.{flatten}`, `use std.check.{check,
ConstraintError}`, `use std.gen.{gen}`), which would keep line 71's
prelude but invent `std` module paths the design never names; if the
library specification places these names in `std`, the corpus gains those
glue lines and this decision is reversed.

Decision: [D-070] D58. the design says only that `parse_size` "returns `Result<Int,
ConstraintError>`"; the corpus supplies a two-line glue body that delegates
to `std.text.parse_int` (a standard-library decimal parser with that return
type, to be specified by the library specification) and imports it with a
second `use std.text.{...}` line so the design's own `use` line stays
verbatim. Rationale: a body that executes keeps the `reader` project free of
holes, which the `holes` project exists to exercise. Rejected alternative:
`?parse_size` as a hole in `reader`.

All glue in the corpus is marked with a `// glue:` comment on the preceding
line; everything else is byte-for-byte the design's text.

## 7. Appendix A: token kinds

Rust-friendly names for the lexer's `TokenKind` enum. The grammar's token
classes map as shown.

```rust
pub enum TokenKind {
    // structure
    Newline,            // NEWLINE
    Eof,                // EOF
    DocComment,         // DOC_COMMENT  (payload: text)
    // literals and names
    Int,                // INT          (payload: digit string without separators)
    Text,               // TEXT         (payload: decoded string)
    LowerIdent,         // LOWER_IDENT
    UpperIdent,         // UPPER_IDENT
    Underscore,         // "_"
    Hole,               // HOLE         (payload: name)
    ReservedWord,       // as async await break continue ghost lemma loop (D4)
    // reserved keywords (33)
    KwAnd, KwAssert, KwCapability, KwDecreases, KwEffects, KwElse, KwEnsures,
    KwEnum, KwFalse, KwFn, KwFor, KwForall, KwIf, KwIn, KwInvariant, KwLet,
    KwMatch, KwModule, KwNot, KwOr, KwProperty, KwPub, KwRequires, KwReturn,
    KwStruct, KwTest, KwTrue, KwType, KwUse, KwUsing, KwVar, KwWhere, KwWhile,
    // operators and punctuation
    Plus, Minus, Star, Slash, Percent, PlusPlus,
    EqEq, BangEq, Lt, LtEq, Gt, GtEq,
    Eq, Arrow, FatArrow, Question, At, Dot, DotDot, Comma, Colon,
    LParen, RParen, LBracket, RBracket, LBrace, RBrace,
}
```

Contextual keywords (`add after base change depend expect git goal
integrate no_holes no_unresolved_assumptions patch preserve prove require
review target task typecheck workers write`) and facet names other than
`effects` are `LowerIdent` tokens; the `.work`/`.patch` parser compares
spellings.

Payloads: `TokenKind` itself carries none. Every token is a `Token { kind,
span, text }` (`spec/toolchain.md` 1.5) whose `text` holds the payload the
comments above name (the digit string of an `INT` without separators, the
decoded value of a `TEXT`, the name of a `HOLE` without `?`, the spelling
of an identifier, keyword, or reserved word, the text of a `DOC_COMMENT`
after `///` with one leading space removed) and is empty for punctuation,
`NEWLINE`, and `EOF`.

## 8. Appendix B: nonterminals

Parser functions are named `parse_<nonterminal>`. Meta-productions
(`lines`, `commas`, `commas1`) are helper combinators, not AST nodes.

| Group | Nonterminals |
| - | - |
| files | `source_file` `work_file` `patch_file` |
| module and imports | `module_header` `module_path` `use_decl` `use_path` `use_item` |
| declarations | `declaration` `decl_prefix` `annotation` `type_decl` `wrapper_decl` `refinement_decl` `opaque_decl` `struct_decl` `field_decl` `enum_decl` `variant_decl` `capability_decl` `op_decl` `fn_decl` `fn_tail` `fn_clause` `effects_clause` `effect_ref` `contract_clause` `decreases_clause` `params` `param` `using_param` `test_decl` `property_decl` `for_clause` |
| types | `type` `type_path` `generic_args` `generic_params` |
| statements | `block` `statement` `let_stmt` `var_stmt` `assign_stmt` `return_stmt` `assert_stmt` `while_stmt` `while_tail` `while_clause` `concurrent_stmt` `context_stmt` `expr_stmt` |
| expressions | `expr` `forall_expr` `or_expr` `and_expr` `cmp_expr` `cmp_op` `add_expr` `mul_expr` `unary_expr` `postfix_expr` `postfix` `member_name` `args` `using_arg` `primary` `name` `paren_expr` `list_literal` `struct_literal` `struct_entries` `field_init` `if_expr` `match_expr` `match_arm` `literal` |
| patterns | `pattern` `ctor_path` |
| lexical (Section 1, not parser functions) | `identifier` `ident_start` `ident_char` `int_literal` `digit` `text_literal` `text_char` `escape` `hex_digit` `bool_literal` |
| work | `change_decl` `change_item` `git_ref` `task_decl` `after_clause` `task_item` `selector_set` `integrate_decl` `require_item` `requirement` `selector` `selector_path` `selector_seg` `facet` |
| patch | `patch_form` |

AST node names follow the nonterminals; the normative Rust definitions are
`spec/toolchain.md` 2.2 (`Module`, `UseDecl`, `FnDecl`, `Block`,
`ExprKind::{Literal, Name, Member, Call, Propagate, Unary, Binary, Forall,
Hole, List, Struct, Paren, If, Match}`, `MatchArm { pattern, body:
ArmBody::{Expr(Expr), Block(Block)} }` (an arm body is a block or an
expression, 3.12; there is no block expression node), `PatternKind::{Wildcard,
Int, Text, Bool, Binding, Ctor}`, `ChangeDecl`, `TaskDecl`, `Selector`,
`PatchForm`). `Paren` exists because the formatter preserves parentheses
(`spec/toolchain.md` 2.4 rule 8); it is a layout node that name resolution
and typing see through.

## 9. Appendix C: decision index

| Id | Rule |
| - | - |
| D1 | comments may contain non-ASCII |
| D2 | identifier case classes are enforced by the parser via `UPPER_IDENT`/`LOWER_IDENT` |
| D3 | `requires ensures effects invariant decreases using where forall assert` are reserved |
| D4 | `as async await break continue ghost lemma loop` reserved for future editions |
| D5 | work vocabulary is contextual, `.work`/`.patch` only |
| D6 | Int literals are decimal only |
| D7 | Text literals are single-line |
| D8 | no Bytes literal; identifier or keyword directly before `"` is an error |
| D9 | the Unit value is the prelude constructor `Unit` |
| D10 | `?` adjacent to an identifier is a `HOLE` token; the name is snake_case and not a keyword |
| D11 | no operator or leading-dot line continuation; brackets and a header's `{` only |
| D12 | `else` on the same line as the preceding `}` where newlines are significant |
| D13 | `use` declarations precede all other declarations |
| D14 | no module-level values |
| D15 | single-item `use a.b.c` is allowed |
| D16 | no `as`, no glob imports, non-empty use lists |
| D17 | annotation grammar open (`LOWER_IDENT` or keyword name), vocabulary closed at check |
| D18 | wrappers have exactly one payload type |
| D19 | `struct`/`enum` accept generic parameters (and std's definition-less `type`) |
| D20 | positional variant payloads only |
| D21 | capability operations may carry contracts |
| D22 | `fn` headers may carry `decreases` |
| D23 | effect refs are `param.op`, no wildcards |
| D24 | `let _ =` is the discard form |
| D25 | single-name `let`/`var`, no destructuring or compound/field assignment |
| D26 | `return` is a statement |
| D27 | `assert` takes one expression, no message |
| D28 | no bare block expressions |
| D29 | spread is first and unique in struct literals |
| D30 | no field punning, no type args on struct literals |
| D31 | call-site `using` takes an expression |
| D32 | no function values |
| D33 | minimal pattern language |
| D34 | one or more `change` per `.work` file |
| D35 | `git("ref")` is the only ref form; `target` must name a branch |
| D36 | closed requirement set of six forms, including `no_unresolved_assumptions`; any other token after `require` is `unknown_requirement` (a reserved word `reserved_word`), nothing after it the generic syntax error |
| D37 | trailing facet-name segment is always the facet |
| D38 | bare selector = `.definition`; `*` is a namespace wildcard covering submodules (T43); `add` sets take wildcards only |
| D39 | patches target `body`, payload is a block, `expect base(...)` mandatory; other facets change by worktree edits |
| D40 | `.patch` is the patch file form and holds only patch forms |
| D41 | struct literals disabled in header expressions; flag restored before the header's `{` |
| D42 | speculative generic-call parse, authoritative when it succeeds; bounded by the line only where newlines are significant |
| D43 | one member-access form; property/method decided by the type |
| D44 | constructor resolution (variants and wrappers): expected type, then lexical scope incl. prelude, qualified always |
| D45 | `forall` extent and placement |
| D46 | `if` is always an expression |
| D47 | `return` value optional |
| D48 | negative literals via unary minus; folded only in patterns |
| D49 | `else if` chains |
| D50 | `result`/`self` contextual identifiers |
| D51 | empty blocks allowed |
| D52 | `_` wildcard token vs `_x` identifier |
| D53 | member names are never keywords (facet `effects` and annotation names excepted) |
| D54 | `..` only as spread |
| D55 | match arms separated by newlines only |
| D56 | `test` reserved, `tests` ordinary |
| D57 | corpus prelude assumptions (Design conflict with design line 71), confirmed by stdlib 1.1 and semantics 1.7 |
| D58 | `parse_size` glue via `std.text.parse_int` |
| D59 | lexical/syntax diagnostics carry the semantics 9.4 `parse` code, a message beginning with the quoted text (D72), span, and the `expected`/`found` fields of generic messages; a failed file yields exactly one diagnostic, the earliest error |
| D60 | `////` is an ordinary comment |
| D61 | annotation, hole, change, task, and `after` names are `LOWER_IDENT`; annotation arguments are literals |
| D62 | control characters excluded from `TEXT` are ASCII C0 (except tab) and DEL |
| D63 | `{` may start the line after any header (`NEWLINE? block`) |
| D64 | `type A = T` requires `where`; no transparent aliases |
| D65 | no per-field or per-variant `pub` |
| D66 | clauses parse in any order; the formatter restores the outline's order |
| D67 | tests and properties take no `pub` |
| D68 | task requests are the `write`/`add` sets; host-resource grants come from project policy |
| D69 | tests are selected only through namespace wildcards |
| D70 | implementations may resynchronise internally after an error; the result is still one diagnostic (the earliest) and no tree; `invalid_utf8` is raised at file load |
| D71 | an annotation name is a `LOWER_IDENT` or any reserved keyword token (`@property`) |
| D72 | a quoted message is the fixed beginning of the emitted message (placeholders filled, then an optional parenthesised hint and `, found <token>`); unquoted syntax errors are `expected <description>, found <token>`; generic messages carry `expected` and `found` |
| D73 | minimum implementation limits: 64 levels of nesting and 1000 operators along a path of nested expressions, both `syntax_error` beyond the implementation's limits |
| D74 | tests and properties may declare `using` parameters in parentheses after the name and an `effects` clause (Milestone 3) |
| D75 | control and bidirectional formatting characters are rejected in comments; bidirectional ones also raw in text literals |
