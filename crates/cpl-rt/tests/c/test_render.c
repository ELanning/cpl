/* test_render.c: the text buffer, quote_text, the {name} renderer, and value text (spec/cabi.md 2.9, 3.3.2; semantics
   S40): every value text below is byte for byte what `cpl run` of the Milestone 1 evaluator prints as `result_text`
   for the same value (the table was generated from it with --arg; crates/cpl-eval/src/json.rs value_text_untyped),
   the fallbacks of a value that does not match its declaration are those of field_name, variant_name, and bare_name,
   and deep and wide values render on the explicit work stack, which reports an allocation failure. */
#define _GNU_SOURCE 1
#define CPLRT_NS rttest
#define CPLRT(name) cplrt_rttest__##name
#define CPLPKG(name) cplpkg_rttest__##name
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void rttest_runtime_init(cplrt_runtime *rt, uint64_t max_heap_bytes);
uint64_t rttest_runtime_fini(cplrt_runtime *rt);

static int failures, checks;
#define CHECK(c)                                                                                                       \
    do {                                                                                                               \
        checks++;                                                                                                      \
        if (!(c)) {                                                                                                    \
            failures++;                                                                                                \
            printf("%s:%d: CHECK(%s) failed\n", __FILE__, __LINE__, #c);                                               \
        }                                                                                                              \
    } while (0)

enum { D_RESULT = 2, D_PAGEID = 7, D_PAGE = 8, D_PAGE_ERROR = 9, D_PAIR = 10, D_EMPTY = 11, D_ESC = 18, T_NEST = 29 };

static cplrt_runtime rt;

/* { type (a CPL_X of program_stub.c), value JSON given to `cpl run --arg`, the compact value JSON of its `result`, its
   `result_text` }: generated from the Milestone 1 evaluator */
typedef struct render_case {
    uint32_t type;
    const char *json, *value_json, *value_text;
} render_case;
static const render_case render_cases[] = {
    { 3, "\"\\u0000\\u0001\\u0002\\u0003\\u0004\\u0005\\u0006\\u0007\\u0008\\u0009\\u000a\\u000b\\u000c\\u000d\\u000e\\u000f\\u0010\\u0011\\u0012\\u0013\\u0014\\u0015\\u0016\\u0017\\u0018\\u0019\\u001a\\u001b\\u001c\\u001d\\u001e\\u001f\\u007f \\u0080\\u00e9\\u2028\\ud83d\\ude00\\\\\\\"/~\"", "\"\\u0000\\u0001\\u0002\\u0003\\u0004\\u0005\\u0006\\u0007\\b\\t\\n\\u000b\\f\\r\\u000e\\u000f\\u0010\\u0011\\u0012\\u0013\\u0014\\u0015\\u0016\\u0017\\u0018\\u0019\\u001a\\u001b\\u001c\\u001d\\u001e\\u001f\177 \302\200\303\251\342\200\250\360\237\230\200\\\\\\\"/~\"", "\"\\u{0}\\u{1}\\u{2}\\u{3}\\u{4}\\u{5}\\u{6}\\u{7}\\u{8}\\t\\n\\u{b}\\u{c}\\r\\u{e}\\u{f}\\u{10}\\u{11}\\u{12}\\u{13}\\u{14}\\u{15}\\u{16}\\u{17}\\u{18}\\u{19}\\u{1a}\\u{1b}\\u{1c}\\u{1d}\\u{1e}\\u{1f}\\u{7f} \302\200\303\251\342\200\250\360\237\230\200\\\\\\\"/~\"" },
    { 3, "\"\"", "\"\"", "\"\"" },
    { 3, "\"plain\"", "\"plain\"", "\"plain\"" },
    { 1, "0", "0", "0" },
    { 1, "-1", "-1", "-1" },
    { 1, "4611686018427387903", "4611686018427387903", "4611686018427387903" },
    { 1, "4611686018427387904", "4611686018427387904", "4611686018427387904" },
    { 1, "-4611686018427387904", "-4611686018427387904", "-4611686018427387904" },
    { 1, "-4611686018427387905", "-4611686018427387905", "-4611686018427387905" },
    { 1, "18446744073709551616", "18446744073709551616", "18446744073709551616" },
    { 1, "-340282366920938463463374607431768211456", "-340282366920938463463374607431768211456", "-340282366920938463463374607431768211456" },
    { 2, "true", "true", "true" },
    { 2, "false", "false", "false" },
    { 4, "{\"$type\":\"Bytes\",\"hex\":\"\"}", "{\"$type\":\"Bytes\",\"hex\":\"\"}", "Bytes(\"\")" },
    { 4, "{\"$type\":\"Bytes\",\"hex\":\"00017f80ff\"}", "{\"$type\":\"Bytes\",\"hex\":\"00017f80ff\"}", "Bytes(\"00017f80ff\")" },
    { 5, "{\"$type\":\"Unit\"}", "{\"$type\":\"Unit\"}", "Unit" },
    { 15, "[]", "[]", "[]" },
    { 15, "[1,-2,3]", "[1,-2,3]", "[1, -2, 3]" },
    { 23, "[[],[],[1]]", "[[],[],[1]]", "[[], [], [1]]" },
    { 23, "[[1,2],[],[3]]", "[[1,2],[],[3]]", "[[1, 2], [], [3]]" },
    { 13, "{\"$type\":\"reader.pages.Page\",\"id\":{\"$type\":\"reader.pages.PageId\",\"$payload\":[\"a\\\"b\"]},\"text\":\"x\\ny\"}", "{\"$type\":\"reader.pages.Page\",\"id\":{\"$payload\":[\"a\\\"b\"],\"$type\":\"reader.pages.PageId\"},\"text\":\"x\\ny\"}", "Page { id: PageId(\"a\\\"b\"), text: \"x\\ny\" }" },
    { 18, "[]", "[]", "[]" },
    { 14, "{\"$type\":\"reader.pages.PageError\",\"$variant\":\"Unavailable\"}", "{\"$type\":\"reader.pages.PageError\",\"$variant\":\"Unavailable\"}", "Unavailable" },
    { 14, "{\"$type\":\"reader.pages.PageError\",\"$variant\":\"Missing\",\"$payload\":[{\"$type\":\"reader.pages.PageId\",\"$payload\":[\"\"]}]}", "{\"$payload\":[{\"$payload\":[\"\"],\"$type\":\"reader.pages.PageId\"}],\"$type\":\"reader.pages.PageError\",\"$variant\":\"Missing\"}", "Missing(PageId(\"\"))" },
    { 19, "{\"$type\":\"rttest.shapes.Pair\",\"zeta\":7,\"alpha\":\"\\t\",\"mid\":false}", "{\"$type\":\"rttest.shapes.Pair\",\"alpha\":\"\\t\",\"mid\":false,\"zeta\":7}", "Pair { zeta: 7, alpha: \"\\t\", mid: false }" },
    { 20, "{\"$type\":\"rttest.shapes.Empty\"}", "{\"$type\":\"rttest.shapes.Empty\"}", "Empty {}" },
    { 17, "{\"$type\":\"Option\",\"$variant\":\"None\"}", "{\"$type\":\"Option\",\"$variant\":\"None\"}", "None" },
    { 17, "{\"$type\":\"Option\",\"$variant\":\"Some\",\"$payload\":[-9]}", "{\"$payload\":[-9],\"$type\":\"Option\",\"$variant\":\"Some\"}", "Some(-9)" },
    { 25, "{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":[12345678901234567890123]}", "{\"$payload\":[12345678901234567890123],\"$type\":\"Result\",\"$variant\":\"Ok\"}", "Ok(12345678901234567890123)" },
    { 25, "{\"$type\":\"Result\",\"$variant\":\"Err\",\"$payload\":[\"\\u0001\"]}", "{\"$payload\":[\"\\u0001\"],\"$type\":\"Result\",\"$variant\":\"Err\"}", "Err(\"\\u{1}\")" },
};

static cplrt_w text(const char *s)
{
    cplrt_w w = 0;
    CHECK(CPLRT(text_make)(&rt, s, strlen(s), &w) == CPLRT_OK);
    return w;
}

static cplrt_w node(uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items, uint32_t n)
{
    cplrt_w w = 0;
    CHECK(CPLRT(node_make)(&rt, kind, def, variant, items, n, &w) == CPLRT_OK);
    return w;
}

static cplrt_w decode(uint32_t type, const char *json)
{
    cplrt_sbuf pointer, detail;
    cplrt_sbuf_init(&pointer, &rt);
    cplrt_sbuf_init(&detail, &rt);
    cplrt_w w = 0;
    int st = CPLRT(read_value_json)(&rt, type, json, strlen(json), &w, &pointer, &detail);
    CHECK(st == CPLRT_OK);
    if (st != CPLRT_OK)
        printf("  decoding %s: %s at %s\n", json, detail.ptr ? detail.ptr : "", pointer.ptr ? pointer.ptr : "");
    CPLRT(sbuf_free)(&pointer);
    CPLRT(sbuf_free)(&detail);
    return w;
}

/* The value text of w is exactly `expected`; the buffer is freed. */
static int renders(cplrt_w w, const char *expected)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_value_text)(&b, rt.program, w);
    int ok = b.status == CPLRT_OK && b.ptr != NULL && b.len == strlen(expected) && strcmp(b.ptr, expected) == 0;
    if (!ok)
        printf("  value text: %s\n  expected:   %s\n", b.ptr != NULL ? b.ptr : "(null)", expected);
    CPLRT(sbuf_free)(&b);
    return ok;
}

static int quotes(const char *s, size_t n, const char *expected)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_quote_text)(&b, s, n);
    int ok = b.status == CPLRT_OK && b.len == strlen(expected) && strcmp(b.ptr, expected) == 0;
    if (!ok)
        printf("  quote_text: %s\n  expected:   %s\n", b.ptr != NULL ? b.ptr : "(null)", expected);
    CPLRT(sbuf_free)(&b);
    return ok;
}

/* ---- the text buffer ------------------------------------------------------------------------------------------- */

static void test_sbuf(void)
{
    uint64_t live = atomic_load(&rt.live_bytes);
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CHECK(b.ptr == NULL && b.len == 0 && b.cap == 0 && b.status == CPLRT_OK && b.rt == &rt);
    CPLRT(sbuf_put)(&b, "", 0);
    CHECK(b.ptr != NULL && b.len == 0 && b.ptr[0] == '\0'); /* NUL-terminated once anything was written */
    CPLRT(sbuf_puts)(&b, "ab");
    CPLRT(sbuf_putc)(&b, 'c');
    CPLRT(sbuf_put)(&b, "d\000e", 3);
    CHECK(b.len == 6 && memcmp(b.ptr, "abcd\000e", 7) == 0);
    CPLRT(sbuf_free)(&b);
    CHECK(b.ptr == NULL && b.len == 0 && b.cap == 0 && atomic_load(&rt.live_bytes) == live);
    CPLRT(sbuf_u64)(&b, 0);
    CPLRT(sbuf_putc)(&b, ' ');
    CPLRT(sbuf_u64)(&b, 7);
    CPLRT(sbuf_putc)(&b, ' ');
    CPLRT(sbuf_u64)(&b, 1234567890);
    CPLRT(sbuf_putc)(&b, ' ');
    CPLRT(sbuf_u64)(&b, UINT64_MAX);
    CHECK(strcmp(b.ptr, "0 7 1234567890 18446744073709551615") == 0);
    CPLRT(sbuf_free)(&b);
    CPLRT(sbuf_hex)(&b, (const uint8_t *)"\000\001\017\020\177\200\253\377", 8);
    CHECK(strcmp(b.ptr, "00010f107f80abff") == 0);
    CPLRT(sbuf_hex)(&b, NULL, 0);
    CHECK(b.len == 16);
    CPLRT(sbuf_free)(&b);
    /* RFC 6901 segments: "~" is "~0", "/" is "~1" (cpl_eval::json::escape_segment) */
    CPLRT(sbuf_pointer_segment)(&b, "a~b/c", 5);
    CPLRT(sbuf_pointer_segment)(&b, "", 0);
    CPLRT(sbuf_pointer_segment)(&b, "~1/~0", 5);
    CPLRT(sbuf_pointer_segment)(&b, "$payload", 8);
    CHECK(strcmp(b.ptr, "/a~0b~1c//~01~1~00/$payload") == 0);
    CPLRT(sbuf_free)(&b);
    /* growth across many doublings */
    for (int i = 0; i < 10000; i++)
        CPLRT(sbuf_puts)(&b, "0123456789");
    CHECK(b.status == CPLRT_OK && b.len == 100000 && b.cap > b.len && b.ptr[b.len] == '\0');
    CHECK(memcmp(b.ptr + 99990, "0123456789", 10) == 0);
    CPLRT(sbuf_free)(&b);
    /* a failed growth is sticky: nothing is written after it, and the status survives sbuf_free */
    CPLRT(sbuf_puts)(&b, "kept");
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    for (int i = 0; i < 100; i++)
        CPLRT(sbuf_puts)(&b, "0123456789");
    CHECK(b.status == CPLRT_NOMEM && memcmp(b.ptr, "kept", 4) == 0 && b.len < 1000);
    size_t len = b.len;
    rt.max_heap_bytes = 0;
    CPLRT(sbuf_puts)(&b, "more");
    CPLRT(sbuf_hex)(&b, (const uint8_t *)"\001", 1);
    CPLRT(sbuf_pointer_segment)(&b, "x", 1);
    CHECK(b.len == len && b.status == CPLRT_NOMEM);
    CPLRT(sbuf_free)(&b);
    CHECK(b.status == CPLRT_NOMEM && b.ptr == NULL);
    CHECK(atomic_load(&rt.live_bytes) == live);
    /* a buffer without a runtime uses the C library's allocator (R3's error objects before a runtime exists) */
    cplrt_sbuf_init(&b, NULL);
    CPLRT(sbuf_puts)(&b, "no runtime");
    CPLRT(write_quote_text)(&b, "\n", 1);
    CHECK(b.status == CPLRT_OK && strcmp(b.ptr, "no runtime\"\\n\"") == 0 && atomic_load(&rt.live_bytes) == live);
    CPLRT(sbuf_free)(&b);
}

/* ---- quote_text (cpl_diag::id::quote_text) ------------------------------------------------------------------------ */

static void test_quote_text(void)
{
    CHECK(quotes("", 0, "\"\""));
    CHECK(quotes("plain text", 10, "\"plain text\""));
    CHECK(quotes("a\\b\"c", 5, "\"a\\\\b\\\"c\""));
    CHECK(quotes("\n\r\t", 3, "\"\\n\\r\\t\""));
    CHECK(quotes("\000", 1, "\"\\u{0}\""));
    CHECK(quotes("\001\010\013\014\016\020\037", 7, "\"\\u{1}\\u{8}\\u{b}\\u{c}\\u{e}\\u{10}\\u{1f}\""));
    CHECK(quotes("\177", 1, "\"\\u{7f}\""));
    CHECK(quotes(" ~/{}'", 6, "\" ~/{}'\""));
    CHECK(quotes("\302\200\303\251\342\200\250\360\237\230\200", 11, "\"\302\200\303\251\342\200\250\360\237\230\200\""));
    /* every control and U+007F, as the evaluator writes the first table entry */
    cplrt_w all = decode(render_cases[0].type, render_cases[0].json);
    const cplrt_text *t = cplrt_text_of(all);
    CHECK(quotes(t->utf8, t->bytes, render_cases[0].value_text));
    cplrt_release(all);
}

/* ---- the {name} renderer (cpl_rtspec::messages::render) ------------------------------------------------------------ */

static int renders_template(const char *tmpl, const char *const *names, const char *const *values, uint32_t n,
                            const char *expected)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_template)(&b, tmpl, names, values, n);
    int ok = b.status == CPLRT_OK && b.ptr != NULL && strcmp(b.ptr, expected) == 0;
    if (!ok)
        printf("  template %s: %s (status %d)\n  expected: %s\n", tmpl, b.ptr ? b.ptr : "(null)", b.status, expected);
    CPLRT(sbuf_free)(&b);
    return ok;
}

static void test_template(void)
{
    const char *names[4] = { "a", "b_2", "a", "clause" };
    const char *values[4] = { "1", "{a}", "ignored", "n >= 0" };
    CHECK(renders_template("", names, values, 4, ""));
    CHECK(renders_template("no placeholder", names, values, 0, "no placeholder"));
    CHECK(renders_template("{a}", names, values, 4, "1"));
    CHECK(renders_template("x{a}y{b_2}z", names, values, 4, "x1y{a}z")); /* a value is never rescanned */
    CHECK(renders_template("{a}{a}", names, values, 4, "11"));          /* the first argument of a name wins */
    CHECK(renders_template("{{a}}", names, values, 4, "{1}"));
    CHECK(renders_template("{} {A} {1a} {a-b} {a b} {", names, values, 0, "{} {A} {1a} {a-b} {a b} {"));
    CHECK(renders_template("{a", names, values, 4, "{a"));
    CHECK(renders_template("{abc", names, values, 0, "{abc"));
    CHECK(renders_template("requires clause failed: {clause}", names, values, 4, "requires clause failed: n >= 0"));
    CHECK(renders_template(CPLRT_MSG_DIVISION_BY_ZERO, names, values, 0, "division by zero"));
    const char *cn[2] = { "clause_kind", "clause" }, *cv[2] = { "ensures", "result > 0" };
    CHECK(renders_template(CPLRT_MSG_CONTRACT_VIOLATION, cn, cv, 2, "ensures clause failed: result > 0"));
    /* a placeholder without an argument: CPLRT_INVALID, sticky */
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_template)(&b, "x {missing} y", names, values, 4);
    CHECK(b.status == CPLRT_INVALID);
    CPLRT(sbuf_puts)(&b, "more");
    CHECK(b.status == CPLRT_INVALID && (b.ptr == NULL || b.len == 0));
    CPLRT(sbuf_free)(&b);
}

/* ---- value text ------------------------------------------------------------------------------------------------------ */

static void test_value_text_cases(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    for (size_t i = 0; i < sizeof render_cases / sizeof render_cases[0]; i++) {
        cplrt_w v = decode(render_cases[i].type, render_cases[i].json);
        if (!renders(v, render_cases[i].value_text))
            printf("  render case %zu\n", i);
        cplrt_release(v);
    }
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_value_text_direct(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w a = text("a"), one = cplrt_small(1), items[4] = { one, cplrt_small(2), cplrt_small(3), cplrt_small(4) };
    CHECK(renders(CPLRT_TRUE, "true") && renders(CPLRT_FALSE, "false") && renders(cplrt_small(-7), "-7"));
    CHECK(renders(CPLRT(empty_text)(), "\"\"") && renders(CPLRT(empty_list)(), "[]"));
    CHECK(renders(CPLRT(empty_bytes)(), "Bytes(\"\")"));
    CHECK(renders(CPLRT(enum_static)(rt.program, 0, 0), "Unit"));
    cplrt_w id = node(CPLRT_WRAPPER, D_PAGEID, 0, &a, 1), fields[2] = { id, a };
    cplrt_w page = node(CPLRT_STRUCT, D_PAGE, 0, fields, 2), empty = node(CPLRT_STRUCT, D_EMPTY, 0, NULL, 0);
    CHECK(renders(page, "Page { id: PageId(\"a\"), text: \"a\" }") && renders(empty, "Empty {}"));
    cplrt_w ok = node(CPLRT_ENUM, D_RESULT, 0, &page, 1), err = node(CPLRT_ENUM, D_RESULT, 1, &empty, 1);
    cplrt_w both[2] = { ok, err }, pair_list = node(CPLRT_LIST, 0, 0, both, 2);
    CHECK(renders(pair_list, "[Ok(Page { id: PageId(\"a\"), text: \"a\" }), Err(Empty {})]"));
    /* declaration names are written as the tables hold them, unescaped, as bare_name and field_name are */
    cplrt_w esc = node(CPLRT_STRUCT, D_ESC, 0, items, 2);
    CHECK(renders(esc, "E\"sc { a\"b: 1, c/d~e: 2 }"));
    /* a payload of two values, and the wide form of a variant index (2.3) renders by the index */
    cplrt_w two = node(CPLRT_ENUM, D_PAGE_ERROR, 0, items, 2);
    CHECK(renders(two, "Missing(1, 2)"));
    cplrt_w wide = node(CPLRT_ENUM, D_PAGE_ERROR, 70000u, items, 2), wide_empty = node(CPLRT_ENUM, D_PAGE_ERROR, 65536u,
                                                                                     NULL, 0);
    CHECK(renders(wide, "<variant 70000>(1, 2)") && renders(wide_empty, "<variant 65536>"));
    /* values that do not match their declaration: field_name, variant_name, bare_name */
    cplrt_w long_pair = node(CPLRT_STRUCT, D_PAIR, 0, items, 4);
    CHECK(renders(long_pair, "Pair { zeta: 1, alpha: 2, mid: 3, <field 3>: 4 }"));
    cplrt_w nodef = node(CPLRT_STRUCT, 999, 0, items, 1), nodef_enum = node(CPLRT_ENUM, 999, 3, NULL, 0);
    cplrt_w nodef_wrap = node(CPLRT_WRAPPER, 999, 0, items, 1), nodef_empty = node(CPLRT_STRUCT, 999, 0, NULL, 0);
    CHECK(renders(nodef, "{def 999} { <field 0>: 1 }") && renders(nodef_enum, "<variant 3>"));
    CHECK(renders(nodef_wrap, "{def 999}(1)") && renders(nodef_empty, "{def 999} {}"));
    cplrt_w struct_as_enum = node(CPLRT_ENUM, D_PAGE, 0, items, 1), enum_as_struct = node(CPLRT_STRUCT, D_PAGE_ERROR, 0,
                                                                                          items, 1);
    CHECK(renders(struct_as_enum, "<variant 0>(1)") && renders(enum_as_struct, "PageError { <field 0>: 1 }"));
    cplrt_w all[] = { a, id, page, empty, ok, err, pair_list, esc, two, wide, wide_empty, long_pair, nodef, nodef_enum,
                      nodef_wrap, nodef_empty, struct_as_enum, enum_as_struct };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- deep and wide values on the explicit work stack (2.4) ------------------------------------------------------------ */

static void test_deep_and_wide(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    enum { DEPTH = 200000 };
    /* [[[...[]...]]], DEPTH + 1 brackets each way */
    cplrt_w deep = CPLRT(empty_list)();
    for (int i = 0; i < DEPTH; i++) {
        cplrt_w n = node(CPLRT_LIST, 0, 0, &deep, 1);
        cplrt_release(deep);
        deep = n;
    }
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_value_text)(&b, rt.program, deep);
    int shape = b.status == CPLRT_OK && b.len == 2u * (DEPTH + 1u);
    for (size_t i = 0; shape && i < b.len; i++)
        shape = b.ptr[i] == (i <= DEPTH ? '[' : ']');
    CHECK(shape);
    CPLRT(sbuf_free)(&b);
    /* the same depth through every composite: Some(PageId(Page { id: ..., text: "t" })) ... */
    cplrt_w t = text("t"), mixed = cplrt_small(0);
    for (int i = 0; i < 3000; i++) {
        cplrt_w f[2] = { mixed, t };
        cplrt_w s = node(CPLRT_STRUCT, D_PAGE, 0, f, 2);
        cplrt_w w = node(CPLRT_WRAPPER, D_PAGEID, 0, &s, 1);
        cplrt_w e = node(CPLRT_ENUM, 1, CPL_OPTION_SOME, &w, 1);
        cplrt_release(mixed);
        cplrt_release(s);
        cplrt_release(w);
        mixed = e;
    }
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_value_text)(&b, rt.program, mixed);
    const char *open = "Some(PageId(Page { id: ", *close = ", text: \"t\" }))";
    size_t ol = strlen(open), cl = strlen(close);
    shape = b.status == CPLRT_OK && b.len == 3000u * (ol + cl) + 1u;
    for (size_t i = 0; shape && i < 3000u; i++)
        shape = memcmp(b.ptr + i * ol, open, ol) == 0 && memcmp(b.ptr + 3000u * ol + 1u + i * cl, close, cl) == 0;
    CHECK(shape && b.ptr[3000u * ol] == '0');
    CPLRT(sbuf_free)(&b);
    /* a wide list */
    cplrt_node *wide = CPLRT(list_alloc)(&rt, 10000);
    CHECK(wide != NULL);
    for (uint32_t i = 0; i < 10000; i++)
        wide->item[i] = cplrt_small((int64_t)i - 5000);
    cplrt_w ww = cplrt_word_of(&wide->hdr);
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_value_text)(&b, rt.program, ww);
    CHECK(b.status == CPLRT_OK && strncmp(b.ptr, "[-5000, -4999, ", 15) == 0 && b.ptr[b.len - 1] == ']');
    CHECK(b.len >= 11 && strcmp(b.ptr + b.len - 11, "4998, 4999]") == 0);
    CPLRT(sbuf_free)(&b);
    /* the work stack's growth can fail: CPLRT_NOMEM in the buffer, nothing leaked */
    uint64_t live = atomic_load(&rt.live_bytes);
    cplrt_sbuf_init(&b, &rt);
    CPLRT(sbuf_put)(&b, "", 0);
    rt.max_heap_bytes = atomic_load(&rt.live_bytes) + 4096u;
    CPLRT(write_value_text)(&b, rt.program, deep);
    CHECK(b.status == CPLRT_NOMEM);
    rt.max_heap_bytes = 0;
    CPLRT(sbuf_free)(&b);
    CHECK(atomic_load(&rt.live_bytes) == live);
    cplrt_release(deep);
    cplrt_release(mixed);
    cplrt_release(t);
    cplrt_release(ww);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

int main(void)
{
    rttest_runtime_init(&rt, 0);
    test_sbuf();
    test_quote_text();
    test_template();
    test_value_text_cases();
    test_value_text_direct();
    test_deep_and_wide();
    uint64_t left = rttest_runtime_fini(&rt);
    CHECK(left == 0);
    printf("test_render: %d checks, %d failed\n", checks, failures);
    return failures == 0 ? 0 : 1;
}
