/* test_text.c: Text and Bytes (spec/cabi.md 2.3, 2.6, 2.7): strict UTF-8 validation with Utf8Error::valid_up_to
   offsets, scalar counts, the immortal "" and empty Bytes, `++`, and every Text, Bytes, std.text, and std.bytes builtin
   of cpl_rt_text.c, with the exact results of crates/cpl-eval/src/builtins/{prelude,text,bytes}.rs (the messages and
   value texts of parse_int and to_text as `cpl run` of the Milestone 1 evaluator prints them), the precondition faults,
   and the memory fault of every allocating builtin. */
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

/* program_stub.c's declarations: the stubs with preconditions and a call site */
enum { STUB_SPLIT = 14, STUB_FROM_LIST = 15, SITE_CALL = 4 };

static cplrt_runtime rt;
static cplrt_cx cx;
static cplrt_memory_record memory;

static void begin(void)
{
    CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, UINT32_MAX, 1, 0, 0, NULL) == CPLRT_OK);
}

static void end(void)
{
    CPLRT(cx_end)(&cx);
}

/* A fault ended the builtin: the evaluation is over; a new one begins. */
static void restart(void)
{
    end();
    begin();
}

static cplrt_w textn(const char *s, size_t n)
{
    cplrt_w w = 0;
    CHECK(CPLRT(utf8_valid)(s, n, NULL, NULL));
    CHECK(CPLRT(text_make)(&rt, s, n, &w) == CPLRT_OK);
    return w;
}

static cplrt_w text(const char *s)
{
    return textn(s, strlen(s));
}

static cplrt_w bytes(const char *data, size_t n)
{
    cplrt_w w = 0;
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)data, n, &w) == CPLRT_OK);
    return w;
}

static cplrt_w list(const cplrt_w *items, uint32_t n)
{
    cplrt_w w = 0;
    CHECK(CPLRT(node_make)(&rt, CPLRT_LIST, 0, 0, items, n, &w) == CPLRT_OK);
    return w;
}

static cplrt_w big(const char *decimal)
{
    cplrt_w w = 0;
    CHECK(CPLRT(int_parse)(&rt, decimal, strlen(decimal), &w) == CPLRT_OK);
    return w;
}

/* The Text w holds exactly the n bytes s, with the right scalar count and the NUL after them. */
static int is_textn(cplrt_w w, const char *s, size_t n)
{
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_TEXT)
        return 0;
    const cplrt_text *t = cplrt_text_of(w);
    size_t scalars = 0;
    if (!CPLRT(utf8_valid)(s, n, NULL, &scalars))
        return 0;
    return t->bytes == n && memcmp(t->utf8, s, n) == 0 && t->utf8[n] == '\0' && t->scalars == scalars;
}

static int is_text(cplrt_w w, const char *s)
{
    return is_textn(w, s, strlen(s));
}

static int is_bytes(cplrt_w w, const char *data, size_t n)
{
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_BYTES)
        return 0;
    const cplrt_bytes *b = cplrt_bytes_of(w);
    return b->len == n && (n == 0 || memcmp(b->data, data, n) == 0);
}

/* The value text of w is exactly `expected` (cpl_eval::json::value_text_untyped). */
static int renders(cplrt_w w, const char *expected)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, &rt);
    CPLRT(write_value_text)(&b, rt.program, w);
    int ok = b.status == CPLRT_OK && b.ptr != NULL && strcmp(b.ptr, expected) == 0;
    if (!ok)
        printf("  value text: %s\n  expected:   %s\n", b.ptr != NULL ? b.ptr : "(null)", expected);
    CPLRT(sbuf_free)(&b);
    return ok;
}

static int precondition_fault(int st)
{
    return st == CPLRT_FAULT && cx.fault != NULL && cx.fault->kind == CPLRT_FK_CONTRACT_VIOLATION &&
           cx.fault->clause_kind == CPLRT_CK_REQUIRES;
}

static int memory_fault(int st)
{
    return st == CPLRT_FAULT && cx.fault != NULL && cx.fault->kind == CPLRT_FK_RESOURCE_EXHAUSTION &&
           cx.fault->resource == CPLRT_RES_MEMORY;
}

/* UTF-8 of the scalar c into out; its length. */
static size_t enc(uint32_t c, char *out)
{
    if (c < 0x80u) {
        out[0] = (char)c;
        return 1;
    }
    if (c < 0x800u) {
        out[0] = (char)(0xC0u | (c >> 6));
        out[1] = (char)(0x80u | (c & 0x3Fu));
        return 2;
    }
    if (c < 0x10000u) {
        out[0] = (char)(0xE0u | (c >> 12));
        out[1] = (char)(0x80u | ((c >> 6) & 0x3Fu));
        out[2] = (char)(0x80u | (c & 0x3Fu));
        return 3;
    }
    out[0] = (char)(0xF0u | (c >> 18));
    out[1] = (char)(0x80u | ((c >> 12) & 0x3Fu));
    out[2] = (char)(0x80u | ((c >> 6) & 0x3Fu));
    out[3] = (char)(0x80u | (c & 0x3Fu));
    return 4;
}

/* ---- UTF-8 validation: core::str::from_utf8 and Utf8Error::valid_up_to ----------------------------------------- */

typedef struct utf8_case {
    const char *s;
    size_t len;
    int valid;
    size_t n; /* the scalar count when valid, else valid_up_to */
} utf8_case;

#define U8(lit, valid, n) { lit, sizeof(lit) - 1u, valid, n }
static const utf8_case utf8_cases[] = {
    U8("", 1, 0),
    U8("abc", 1, 3),
    U8("a\000b", 1, 3),
    U8("\177", 1, 1),
    U8("\302\200", 1, 1),             /* U+0080, the first two-byte scalar */
    U8("\337\277", 1, 1),             /* U+07FF */
    U8("\340\240\200", 1, 1),         /* U+0800 */
    U8("\355\237\277", 1, 1),         /* U+D7FF, below the surrogates */
    U8("\356\200\200", 1, 1),         /* U+E000, above them */
    U8("\357\277\277", 1, 1),         /* U+FFFF */
    U8("\360\220\200\200", 1, 1),     /* U+10000 */
    U8("\364\217\277\277", 1, 1),     /* U+10FFFF */
    U8("a\303\251\342\202\254\360\237\230\200z", 1, 5),
    U8("\200", 0, 0),                 /* a lone continuation byte */
    U8("\277", 0, 0),
    U8("a\303", 0, 1),                /* truncated at the end */
    U8("a\303(", 0, 1),               /* a non-continuation second byte */
    U8("\300\200", 0, 0),             /* overlong two-byte forms */
    U8("\301\277", 0, 0),
    U8("\340\200\200", 0, 0),         /* overlong three-byte forms */
    U8("\340\237\277", 0, 0),
    U8("\355\240\200", 0, 0),         /* U+D800, a surrogate */
    U8("\355\277\277", 0, 0),         /* U+DFFF */
    U8("\360\200\200\200", 0, 0),     /* overlong four-byte forms */
    U8("\360\217\277\277", 0, 0),
    U8("\364\220\200\200", 0, 0),     /* above U+10FFFF */
    U8("\365\200\200\200", 0, 0),
    U8("\370\210\200\200\200", 0, 0), /* five-byte forms do not exist */
    U8("\376", 0, 0),
    U8("\377", 0, 0),
    U8("ab\342\202", 0, 2),           /* a truncated three-byte sequence */
    U8("\342\202A", 0, 0),
    U8("\342A\202", 0, 0),
    U8("x\360\237\230", 0, 1),        /* a truncated four-byte sequence */
    U8("x\360\237\230A", 0, 1),
    U8("\303\251\360\237\230\200\376", 0, 6),
    U8("ok\303\251\355\240\200", 0, 4),
};

static void test_utf8(void)
{
    for (size_t i = 0; i < sizeof utf8_cases / sizeof utf8_cases[0]; i++) {
        const utf8_case *c = &utf8_cases[i];
        size_t up_to = 12345, scalars = 12345;
        int valid = CPLRT(utf8_valid)(c->s, c->len, &up_to, &scalars);
        CHECK(valid == c->valid);
        if (c->valid) {
            CHECK(scalars == c->n && up_to == 12345);
        } else {
            CHECK(up_to == c->n && scalars == 12345);
        }
        if (valid != c->valid || (c->valid ? scalars : up_to) != c->n)
            printf("  utf8 case %zu: valid %d, scalars %zu, valid_up_to %zu\n", i, valid, scalars, up_to);
        CHECK(CPLRT(utf8_valid)(c->s, c->len, NULL, NULL) == c->valid); /* either output may be NULL */
    }
    /* every scalar of the BMP edges and the planes round-trips, every surrogate fails */
    char buf[4];
    for (uint32_t c = 0; c <= 0x10FFFFu; c += (c < 0x3000u ? 1u : 61u)) {
        size_t n = enc(c, buf), scalars = 0, up_to = 99;
        int surrogate = c >= 0xD800u && c <= 0xDFFFu;
        int valid = CPLRT(utf8_valid)(buf, n, &up_to, &scalars);
        CHECK(surrogate ? (!valid && up_to == 0) : (valid && scalars == 1));
    }
}

/* ---- construction and `++` ---------------------------------------------------------------------------------------- */

static void test_make_and_concat(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w e = 0, y = 0;
    CHECK(CPLRT(text_make)(&rt, "ignored", 0, &e) == CPLRT_OK && e == CPLRT(empty_text)());
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"x", 0, &y) == CPLRT_OK && y == CPLRT(empty_bytes)());
    CHECK(cplrt_text_of(e)->bytes == 0 && cplrt_text_of(e)->scalars == 0 && cplrt_text_of(e)->utf8[0] == '\0');
    CHECK(cplrt_bytes_of(y)->len == 0);
    CHECK(atomic_load(&rt.live_objects) == objects); /* immortals are not counted */
    cplrt_w t = text("a\303\251\360\237\230\200");
    CHECK(is_text(t, "a\303\251\360\237\230\200") && cplrt_text_of(t)->scalars == 3);
    cplrt_w nul = textn("a\000b", 3);
    CHECK(cplrt_text_of(nul)->bytes == 3 && cplrt_text_of(nul)->scalars == 3 && cplrt_text_of(nul)->utf8[3] == '\0');
    cplrt_text *fresh = CPLRT(text_alloc)(&rt, 2, 1);
    CHECK(fresh != NULL && fresh->bytes == 2 && fresh->scalars == 1 && fresh->utf8[2] == '\0');
    CHECK(fresh != NULL && fresh->hdr.kind == CPLRT_TEXT && atomic_load(&fresh->hdr.u.live.rc) == 1u);
    memcpy(fresh->utf8, "\303\251", 2);
    cplrt_w fw = cplrt_word_of(&fresh->hdr);
    cplrt_bytes *fb = CPLRT(bytes_alloc)(&rt, 3);
    CHECK(fb != NULL && fb->len == 3 && fb->hdr.kind == CPLRT_BYTES);
    memcpy(fb->data, "\001\002\003", 3);
    cplrt_w bw = cplrt_word_of(&fb->hdr);
    CHECK(CPLRT(text_alloc)(&rt, SIZE_MAX - 8u, 0) == NULL && CPLRT(bytes_alloc)(&rt, SIZE_MAX - 8u) == NULL);

    cplrt_w r = 0;
    CHECK(CPLRT(text_concat)(&rt, t, fw, &r) == CPLRT_OK && is_text(r, "a\303\251\360\237\230\200\303\251"));
    CHECK(cplrt_text_of(r)->scalars == 4);
    CHECK(CPLRT(text_concat)(&rt, t, e, &r) == CPLRT_OK && r == t); /* an operand when the other is empty */
    CHECK(CPLRT(text_concat)(&rt, e, fw, &r) == CPLRT_OK && r == fw);
    CHECK(CPLRT(text_concat)(&rt, e, e, &r) == CPLRT_OK && r == e);
    CHECK(CPLRT(bytes_concat)(&rt, bw, bw, &r) == CPLRT_OK && is_bytes(r, "\001\002\003\001\002\003", 6));
    CHECK(CPLRT(bytes_concat)(&rt, bw, y, &r) == CPLRT_OK && r == bw);
    CHECK(CPLRT(bytes_concat)(&rt, y, bw, &r) == CPLRT_OK && r == bw);
    /* out of memory: CPLRT_NOMEM and the slot unchanged */
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    cplrt_w keep = r;
    CHECK(CPLRT(text_concat)(&rt, t, t, &r) == CPLRT_NOMEM && r == keep);
    CHECK(CPLRT(bytes_concat)(&rt, bw, bw, &r) == CPLRT_NOMEM && r == keep);
    CHECK(CPLRT(text_make)(&rt, "x", 1, &r) == CPLRT_NOMEM && r == keep);
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"x", 1, &r) == CPLRT_NOMEM && r == keep);
    CHECK(CPLRT(text_make)(&rt, "", 0, &r) == CPLRT_OK && r == e); /* the immortal needs no memory */
    rt.max_heap_bytes = 0;
    cplrt_w all[] = { t, nul, fw, bw };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- len and is_empty ------------------------------------------------------------------------------------------- */

static void test_lengths(void)
{
    begin();
    cplrt_w t = text("a\303\251\360\237\230\200"), b = bytes("\000\001\002\003", 4), r = 0;
    CHECK(CPLRT(bi_text_len)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && r == cplrt_small(3));
    CHECK(CPLRT(bi_text_len)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_text)(), &r) == CPLRT_OK && r == cplrt_small(0));
    CHECK(CPLRT(bi_text_is_empty)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && r == CPLRT_FALSE);
    CHECK(CPLRT(bi_text_is_empty)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_text)(), &r) == CPLRT_OK && r == CPLRT_TRUE);
    CHECK(CPLRT(bi_bytes_len)(&cx, SITE_CALL, CPLRT_NONE, b, &r) == CPLRT_OK && r == cplrt_small(4));
    CHECK(CPLRT(bi_bytes_len)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_bytes)(), &r) == CPLRT_OK && r == cplrt_small(0));
    CHECK(CPLRT(bi_bytes_is_empty)(&cx, SITE_CALL, CPLRT_NONE, b, &r) == CPLRT_OK && r == CPLRT_FALSE);
    CHECK(CPLRT(bi_bytes_is_empty)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_bytes)(), &r) == CPLRT_OK &&
          r == CPLRT_TRUE);
    end();
    cplrt_release(t);
    cplrt_release(b);
}

/* ---- trim: the 25 White_Space scalars of stdlib.md S22, from CPLRT_WHITE_SPACE ------------------------------------ */

static const uint32_t white_space[25] = { 0x9,    0xA,    0xB,    0xC,    0xD,    0x20,   0x85,   0xA0,   0x1680,
                                          0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008,
                                          0x2009, 0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 };
/* neighbours of the ranges and the scalars a Unicode library might strip: not White_Space (U+180E stopped being one
   in Unicode 6.3; U+200B and U+FEFF never were) */
static const uint32_t not_white_space[24] = { 0x0,    0x8,    0xE,    0x1F,   0x21,   0x84,   0x86,   0x9F,
                                              0xA1,   0x167F, 0x1681, 0x180E, 0x1FFF, 0x200B, 0x2027, 0x202A,
                                              0x202E, 0x2030, 0x205E, 0x2060, 0x2FFF, 0x3001, 0xFEFF, 0x1F600 };

typedef int (*bi1)(cplrt_cx *, uint32_t, uint32_t, cplrt_w, cplrt_w *);

static void trim_case(bi1 f, const char *in, size_t n, const char *want, size_t wn)
{
    cplrt_w t = textn(in, n), r = 0;
    CHECK(f(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && is_textn(r, want, wn));
    if (n == wn)
        CHECK(r == t); /* nothing stripped: the operand itself (same_or_new) */
    if (wn == 0)
        CHECK(r == CPLRT(empty_text)());
    cplrt_release(r);
    cplrt_release(t);
}

static void test_trim(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    char ws[128], in[512], tmp[8];
    size_t wsn = 0;
    CHECK(CPLRT_WHITE_SPACE_N == 10u);
    for (int i = 0; i < 25; i++) {
        size_t k = enc(white_space[i], tmp);
        memcpy(ws + wsn, tmp, k);
        wsn += k;
        /* each alone is stripped by all three */
        trim_case(CPLRT(bi_text_trim), tmp, k, "", 0);
        trim_case(CPLRT(bi_text_trim_start), tmp, k, "", 0);
        trim_case(CPLRT(bi_text_trim_end), tmp, k, "", 0);
    }
    for (int i = 0; i < 24; i++) {
        size_t k = enc(not_white_space[i], tmp);
        trim_case(CPLRT(bi_text_trim), tmp, k, tmp, k);
        /* stripping stops at it from either side */
        size_t n = 0;
        memcpy(in + n, " \t", 2);
        n += 2;
        memcpy(in + n, tmp, k);
        n += k;
        memcpy(in + n, "\342\200\250\n", 4);
        n += 4;
        trim_case(CPLRT(bi_text_trim), in, n, tmp, k);
        trim_case(CPLRT(bi_text_trim_start), in, n, in + 2, n - 2);
        trim_case(CPLRT(bi_text_trim_end), in, n, in, n - 4);
    }
    /* all 25 on both sides of "x y" */
    size_t n = 0;
    memcpy(in, ws, wsn);
    n += wsn;
    memcpy(in + n, "x y", 3);
    n += 3;
    memcpy(in + n, ws, wsn);
    n += wsn;
    trim_case(CPLRT(bi_text_trim), in, n, "x y", 3);
    trim_case(CPLRT(bi_text_trim_start), in, n, in + wsn, n - wsn);
    trim_case(CPLRT(bi_text_trim_end), in, n, in, n - wsn);
    trim_case(CPLRT(bi_text_trim), ws, wsn, "", 0);
    trim_case(CPLRT(bi_text_trim), "", 0, "", 0);
    trim_case(CPLRT(bi_text_trim), "\303\251", 2, "\303\251", 2);
    /* the result's scalar count: 25 + 3 + 25 scalars, 3 kept */
    cplrt_w t = textn(in, n), r = 0;
    CHECK(cplrt_text_of(t)->scalars == 53u);
    CHECK(CPLRT(bi_text_trim_start)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && cplrt_text_of(r)->scalars == 28u);
    cplrt_release(r);
    cplrt_release(t);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- starts_with, ends_with, contains ---------------------------------------------------------------------------- */

typedef int (*bi2)(cplrt_cx *, uint32_t, uint32_t, cplrt_w, cplrt_w, cplrt_w *);

static int pred(bi2 f, const char *a, const char *b)
{
    cplrt_w x = text(a), y = text(b), r = 0;
    CHECK(f(&cx, SITE_CALL, CPLRT_NONE, x, y, &r) == CPLRT_OK && (r == CPLRT_TRUE || r == CPLRT_FALSE));
    cplrt_release(x);
    cplrt_release(y);
    return r == CPLRT_TRUE;
}

static void test_search(void)
{
    begin();
    bi2 sw = CPLRT(bi_text_starts_with), ew = CPLRT(bi_text_ends_with), ct = CPLRT(bi_text_contains);
    CHECK(pred(sw, "abc", "") && pred(sw, "", "") && !pred(sw, "", "a") && pred(sw, "abc", "ab"));
    CHECK(pred(sw, "abc", "abc") && !pred(sw, "abc", "abcd") && !pred(sw, "abc", "b"));
    CHECK(pred(sw, "\303\251t\303\251", "\303\251") && !pred(sw, "e\314\201", "\303\251"));
    CHECK(pred(ew, "abc", "") && pred(ew, "", "") && !pred(ew, "", "c") && pred(ew, "abc", "bc"));
    CHECK(pred(ew, "abc", "abc") && !pred(ew, "abc", "zabc") && !pred(ew, "abc", "b"));
    CHECK(pred(ew, "t\303\251", "\303\251") && !pred(ew, "t\303\251", "e"));
    CHECK(pred(ct, "abc", "") && pred(ct, "", "") && !pred(ct, "", "a") && pred(ct, "abc", "b"));
    CHECK(pred(ct, "abc", "abc") && !pred(ct, "abc", "abd") && !pred(ct, "ab", "abc"));
    CHECK(pred(ct, "aaab", "aab") && pred(ct, "abababc", "ababc") && !pred(ct, "ababab", "abc"));
    CHECK(pred(ct, "h\303\251llo", "\303\251l") && pred(ct, "x\360\237\230\200y", "\360\237\230\200"));
    CHECK(!pred(ct, "h\303\251llo", "el") && pred(ct, "a\000b", "") && pred(ct, "ab", "b"));
    end();
}

/* ---- split and join ------------------------------------------------------------------------------------------------ */

static void split_case(const char *t, const char *sep, const char *want)
{
    cplrt_w x = text(t), y = text(sep), r = 0;
    CHECK(CPLRT(bi_text_split)(&cx, SITE_CALL, STUB_SPLIT, x, y, &r) == CPLRT_OK && renders(r, want));
    for (uint32_t i = 0; cplrt_is_obj(r) && i < cplrt_len(r); i++) {
        cplrt_w piece = cplrt_item(r, i);
        CHECK(is_textn(piece, cplrt_text_of(piece)->utf8, cplrt_text_of(piece)->bytes));
    }
    cplrt_release(r);
    cplrt_release(x);
    cplrt_release(y);
}

static void join_case(const char *const *parts, uint32_t n, const char *sep, const char *want)
{
    cplrt_w items[8];
    for (uint32_t i = 0; i < n; i++)
        items[i] = text(parts[i]);
    cplrt_w xs = list(items, n), y = text(sep), r = 0;
    CHECK(CPLRT(bi_text_join)(&cx, SITE_CALL, CPLRT_NONE, xs, y, &r) == CPLRT_OK && is_text(r, want));
    if (want[0] == '\0')
        CHECK(r == CPLRT(empty_text)());
    cplrt_release(r);
    cplrt_release(xs);
    cplrt_release(y);
    for (uint32_t i = 0; i < n; i++)
        cplrt_release(items[i]);
}

static void test_split_join(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    /* stdlib.md S24 */
    split_case("", ",", "[\"\"]");
    split_case("a,", ",", "[\"a\", \"\"]");
    split_case("aaa", "aa", "[\"\", \"a\"]");
    split_case("a,b,,c", ",", "[\"a\", \"b\", \"\", \"c\"]");
    split_case(",", ",", "[\"\", \"\"]");
    split_case("abc", "abc", "[\"\", \"\"]");
    split_case("ab", "abc", "[\"ab\"]");
    split_case("a--b---c", "--", "[\"a\", \"b\", \"-c\"]");
    split_case("\303\251x\303\251y\303\251", "\303\251", "[\"\", \"x\", \"y\", \"\"]");
    split_case("one two", " ", "[\"one\", \"two\"]");
    split_case("a\nb", "\n", "[\"a\", \"b\"]");
    /* the precondition first: not sep.is_empty() */
    cplrt_w x = text("abc"), r = cplrt_small(7);
    int st = CPLRT(bi_text_split)(&cx, SITE_CALL, STUB_SPLIT, x, CPLRT(empty_text)(), &r);
    CHECK(precondition_fault(st) && r == cplrt_small(7));
    restart();
    st = CPLRT(bi_text_split)(&cx, SITE_CALL, STUB_SPLIT, CPLRT(empty_text)(), CPLRT(empty_text)(), &r);
    CHECK(precondition_fault(st) && r == cplrt_small(7));
    restart();
    cplrt_release(x);

    const char *none[1] = { "" };
    const char *one[1] = { "a" };
    const char *three[3] = { "a", "b", "c" };
    const char *empties[2] = { "", "" };
    const char *uni[2] = { "\303\251", "\303\274" };
    join_case(none, 0, ",", "");
    join_case(one, 1, ",", "a");
    join_case(three, 3, ", ", "a, b, c");
    join_case(three, 3, "", "abc");
    join_case(empties, 2, "", "");
    join_case(empties, 2, "-", "-");
    join_case(uni, 2, "\342\200\224", "\303\251\342\200\224\303\274");
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- to_upper, to_lower: ASCII only (stdlib.md S23) --------------------------------------------------------------- */

static void case_case(bi1 f, const char *in, const char *want)
{
    cplrt_w t = text(in), r = 0;
    CHECK(f(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && is_text(r, want));
    if (strcmp(in, want) == 0)
        CHECK(r == t); /* nothing changed: the operand */
    cplrt_release(r);
    cplrt_release(t);
}

static void test_case(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    bi1 up = CPLRT(bi_text_to_upper), low = CPLRT(bi_text_to_lower);
    case_case(up, "abcXYZ \303\251 \303\237 \304\261", "ABCXYZ \303\251 \303\237 \304\261");
    case_case(up, "az", "AZ");
    case_case(up, "ABC", "ABC");
    case_case(up, "@[`{", "@[`{");
    case_case(up, "", "");
    case_case(up, "\303\251\303\240", "\303\251\303\240");
    case_case(low, "ABCxyz \303\200 \303\211", "abcxyz \303\200 \303\211");
    case_case(low, "AZ", "az");
    case_case(low, "abc", "abc");
    case_case(low, "@[`{", "@[`{");
    case_case(low, "\303\200BC", "\303\200bc");
    /* every ASCII byte */
    char all[129], up_want[129], low_want[129];
    for (int i = 0; i < 128; i++) {
        all[i] = (char)(i == 0 ? 1 : i);
        up_want[i] = (char)(all[i] >= 'a' && all[i] <= 'z' ? all[i] - 32 : all[i]);
        low_want[i] = (char)(all[i] >= 'A' && all[i] <= 'Z' ? all[i] + 32 : all[i]);
    }
    all[128] = up_want[128] = low_want[128] = '\0';
    case_case(up, all, up_want);
    case_case(low, all, low_want);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- parse_int, from_int, chars ---------------------------------------------------------------------------------- */

static void parse_case(const char *in, size_t n, const char *want_text)
{
    cplrt_w t = textn(in, n), r = 0;
    CHECK(CPLRT(bi_text_parse_int)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && renders(r, want_text));
    if (cplrt_is_obj(r)) {
        CHECK(cplrt_obj_of(r)->kind == CPLRT_ENUM && cplrt_obj_of(r)->u.live.def == rt.program->std.result);
        if (cplrt_variant(r) == CPL_RESULT_ERR) {
            cplrt_w ce = cplrt_payload(r, 0);
            CHECK(cplrt_obj_of(ce)->u.live.def == rt.program->std.constraint_error);
            CHECK(is_text(cplrt_field(ce, CPL_CONSTRAINT_ERROR_F_TYPE_NAME), "Int"));
        }
    }
    cplrt_release(r);
    cplrt_release(t);
}

static void test_parse_from_chars(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    /* stdlib.md S25: ("+" | "-")? [0-9]+, leading zeros accepted, -0 is 0 */
    parse_case("42", 2, "Ok(42)");
    parse_case("-0", 2, "Ok(0)");
    parse_case("+17", 3, "Ok(17)");
    parse_case("007", 3, "Ok(7)");
    parse_case("-4611686018427387904", 20, "Ok(-4611686018427387904)");
    parse_case("123456789012345678901234567890", 30, "Ok(123456789012345678901234567890)");
    parse_case("-000123456789012345678901234567890", 34, "Ok(-123456789012345678901234567890)");
    /* the message of the evaluator: `cpl run` of std.text.parse_int("1\t2\u{0}") prints exactly this value text */
    parse_case("1\t2\000", 4,
               "Err(ConstraintError { type_name: \"Int\", message: \"\\\"1\\\\t2\\\\u{0}\\\" is not a decimal integer\" })");
    parse_case("", 0, "Err(ConstraintError { type_name: \"Int\", message: \"\\\"\\\" is not a decimal integer\" })");
    parse_case(" 1", 2, "Err(ConstraintError { type_name: \"Int\", message: \"\\\" 1\\\" is not a decimal integer\" })");
    parse_case("1 ", 2, "Err(ConstraintError { type_name: \"Int\", message: \"\\\"1 \\\" is not a decimal integer\" })");
    parse_case("+", 1, "Err(ConstraintError { type_name: \"Int\", message: \"\\\"+\\\" is not a decimal integer\" })");
    parse_case("-", 1, "Err(ConstraintError { type_name: \"Int\", message: \"\\\"-\\\" is not a decimal integer\" })");
    parse_case("+-1", 3, "Err(ConstraintError { type_name: \"Int\", message: \"\\\"+-1\\\" is not a decimal integer\" })");
    parse_case("1_000", 5,
               "Err(ConstraintError { type_name: \"Int\", message: \"\\\"1_000\\\" is not a decimal integer\" })");
    parse_case("0x10", 4, "Err(ConstraintError { type_name: \"Int\", message: \"\\\"0x10\\\" is not a decimal integer\" })");
    parse_case("\331\241", 2, /* ARABIC-INDIC DIGIT ONE */
               "Err(ConstraintError { type_name: \"Int\", message: \"\\\"\331\241\\\" is not a decimal integer\" })");
    parse_case("\"\\\177", 3,
               "Err(ConstraintError { type_name: \"Int\", message: \"\\\"\\\\\\\"\\\\\\\\\\\\u{7f}\\\" is not a decimal "
               "integer\" })");

    cplrt_w r = 0;
    CHECK(CPLRT(bi_text_from_int)(&cx, SITE_CALL, CPLRT_NONE, cplrt_small(0), &r) == CPLRT_OK && is_text(r, "0"));
    CHECK(CPLRT(bi_text_from_int)(&cx, SITE_CALL, CPLRT_NONE, cplrt_small(-120), &r) == CPLRT_OK && is_text(r, "-120"));
    cplrt_w b = big("-98765432109876543210");
    CHECK(CPLRT(bi_text_from_int)(&cx, SITE_CALL, CPLRT_NONE, b, &r) == CPLRT_OK && is_text(r, "-98765432109876543210"));
    cplrt_release(b);

    cplrt_w t = text("a\303\251\360\237\230\200\000");
    cplrt_release(t);
    t = textn("a\303\251\360\237\230\200\000", 8);
    CHECK(CPLRT(bi_text_chars)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && cplrt_len(r) == 4);
    CHECK(is_text(cplrt_item(r, 0), "a") && is_text(cplrt_item(r, 1), "\303\251"));
    CHECK(is_text(cplrt_item(r, 2), "\360\237\230\200") && is_textn(cplrt_item(r, 3), "\000", 1));
    CHECK(renders(r, "[\"a\", \"\303\251\", \"\360\237\230\200\", \"\\u{0}\"]"));
    CHECK(CPLRT(bi_text_chars)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_text)(), &r) == CPLRT_OK &&
          r == CPLRT(empty_list)());
    cplrt_release(t);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- std.bytes ------------------------------------------------------------------------------------------------------- */

static void test_bytes(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    cplrt_w vals[4] = { cplrt_small(0), cplrt_small(1), cplrt_small(171), cplrt_small(255) };
    cplrt_w xs = list(vals, 4), r = 0;
    CHECK(CPLRT(bi_bytes_from_list)(&cx, SITE_CALL, STUB_FROM_LIST, xs, &r) == CPLRT_OK &&
          is_bytes(r, "\000\001\253\377", 4));
    CHECK(renders(r, "Bytes(\"0001abff\")"));
    cplrt_w y = r;
    r = 0;
    CHECK(CPLRT(bi_bytes_from_list)(&cx, SITE_CALL, STUB_FROM_LIST, CPLRT(empty_list)(), &r) == CPLRT_OK &&
          r == CPLRT(empty_bytes)());
    /* the precondition: forall v in values: v >= 0 and v <= 255, left to right */
    cplrt_w bad[4][3] = { { cplrt_small(1), cplrt_small(256), cplrt_small(-1) },
                          { cplrt_small(-1), cplrt_small(0), cplrt_small(0) },
                          { cplrt_small(0), cplrt_small(0), 0 },
                          { cplrt_small(CPLRT_SMALL_MIN), cplrt_small(0), cplrt_small(0) } };
    bad[2][2] = big("100000000000000000000");
    for (int i = 0; i < 4; i++) {
        cplrt_w l = list(bad[i], 3);
        r = cplrt_small(9);
        CHECK(precondition_fault(CPLRT(bi_bytes_from_list)(&cx, SITE_CALL, STUB_FROM_LIST, l, &r)) &&
              r == cplrt_small(9));
        restart();
        cplrt_release(l);
    }
    cplrt_release(bad[2][2]);

    r = 0;
    CHECK(CPLRT(bi_bytes_to_list)(&cx, SITE_CALL, CPLRT_NONE, y, &r) == CPLRT_OK && renders(r, "[0, 1, 171, 255]"));
    CHECK(CPLRT(bi_bytes_to_list)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_bytes)(), &r) == CPLRT_OK &&
          r == CPLRT(empty_list)());
    cplrt_w t = text("h\303\251");
    CHECK(CPLRT(bi_bytes_from_text)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && is_bytes(r, "h\303\251", 3));
    CHECK(CPLRT(bi_bytes_from_text)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_text)(), &r) == CPLRT_OK &&
          r == CPLRT(empty_bytes)());

    /* to_text: strict, the offset of Utf8Error::valid_up_to; `cpl run` prints this value text for
       std.bytes.to_text(std.bytes.from_list([104, 105, 226, 130])) */
    cplrt_w hb = bytes("hi\342\202", 4);
    CHECK(CPLRT(bi_bytes_to_text)(&cx, SITE_CALL, CPLRT_NONE, hb, &r) == CPLRT_OK &&
          renders(r, "Err(ConstraintError { type_name: \"Text\", message: \"invalid UTF-8 at byte 2\" })"));
    CHECK(is_text(cplrt_field(cplrt_payload(r, 0), CPL_CONSTRAINT_ERROR_F_TYPE_NAME), "Text"));
    for (size_t i = 0; i < sizeof utf8_cases / sizeof utf8_cases[0]; i++) {
        const utf8_case *c = &utf8_cases[i];
        cplrt_w in = bytes(c->s, c->len);
        CHECK(CPLRT(bi_bytes_to_text)(&cx, SITE_CALL, CPLRT_NONE, in, &r) == CPLRT_OK);
        CHECK(cplrt_variant(r) == (c->valid ? CPL_RESULT_OK : CPL_RESULT_ERR));
        if (c->valid) {
            CHECK(is_textn(cplrt_payload(r, 0), c->s, c->len));
        } else {
            char want[128];
            snprintf(want, sizeof want, "Err(ConstraintError { type_name: \"Text\", message: \"invalid UTF-8 at byte %zu\" })",
                     c->n);
            CHECK(renders(r, want));
        }
        cplrt_release(in);
    }
    CHECK(CPLRT(bi_bytes_to_text)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_bytes)(), &r) == CPLRT_OK &&
          renders(r, "Ok(\"\")") && cplrt_payload(r, 0) == CPLRT(empty_text)());
    cplrt_release(r);
    cplrt_release(hb);
    cplrt_release(t);
    cplrt_release(y);
    cplrt_release(xs);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- the memory fault of every allocating builtin (semantics S76) ------------------------------------------------- */

static void test_memory(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w t = text(" aB "), sep = text(","), csv = text("a,b"), nums = text("12"), b = bytes("ab", 2);
    cplrt_w parts[2] = { text("x"), text("y") };
    cplrt_w xs = list(parts, 2);
    cplrt_w vals[2] = { cplrt_small(1), cplrt_small(2) };
    cplrt_w ints = list(vals, 2);
    cplrt_w bad = bytes("\377", 1), huge = big("-123456789012345678901234567890");
    begin();
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    cplrt_w keep = cplrt_small(5), r = keep;
    CHECK(memory_fault(CPLRT(bi_text_trim)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_trim_start)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_trim_end)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_split)(&cx, SITE_CALL, STUB_SPLIT, csv, sep, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_join)(&cx, SITE_CALL, CPLRT_NONE, xs, sep, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_to_upper)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_to_lower)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_parse_int)(&cx, SITE_CALL, CPLRT_NONE, nums, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_parse_int)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_from_int)(&cx, SITE_CALL, CPLRT_NONE, huge, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_text_chars)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_bytes_from_list)(&cx, SITE_CALL, STUB_FROM_LIST, ints, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_bytes_to_list)(&cx, SITE_CALL, CPLRT_NONE, b, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_bytes_from_text)(&cx, SITE_CALL, CPLRT_NONE, t, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_bytes_to_text)(&cx, SITE_CALL, CPLRT_NONE, b, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_bytes_to_text)(&cx, SITE_CALL, CPLRT_NONE, bad, &r)) && r == keep);
    restart();
    /* what needs no memory still works: immediates, the operand, the immortals */
    CHECK(CPLRT(bi_text_len)(&cx, SITE_CALL, CPLRT_NONE, t, &r) == CPLRT_OK && r == cplrt_small(4));
    CHECK(CPLRT(bi_text_contains)(&cx, SITE_CALL, CPLRT_NONE, t, sep, &r) == CPLRT_OK && r == CPLRT_FALSE);
    CHECK(CPLRT(bi_text_trim)(&cx, SITE_CALL, CPLRT_NONE, csv, &r) == CPLRT_OK && r == csv);
    CHECK(CPLRT(bi_text_to_upper)(&cx, SITE_CALL, CPLRT_NONE, sep, &r) == CPLRT_OK && r == sep);
    CHECK(CPLRT(bi_text_join)(&cx, SITE_CALL, CPLRT_NONE, CPLRT(empty_list)(), sep, &r) == CPLRT_OK &&
          r == CPLRT(empty_text)());
    rt.max_heap_bytes = 0;
    end();
    cplrt_w all[] = { t, sep, csv, nums, b, parts[0], parts[1], xs, ints, bad, huge };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_owned_append(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    const char chunk[] = { 'a', 0, (char)0xce, (char)0xbb };
    char expected[128];
    for (size_t i = 0; i < sizeof expected; i++)
        expected[i] = chunk[i % sizeof chunk];
    for (int is_text = 0; is_text <= 1; is_text++) {
        begin();
        cplrt_w value = is_text ? textn(chunk, sizeof chunk) : bytes(chunk, sizeof chunk);
        cplrt_w empty = is_text ? CPLRT(empty_text)() : CPLRT(empty_bytes)();
        int (*append)(cplrt_cx *, cplrt_w *, cplrt_w) =
            is_text ? cplrt_text_concat_owned : cplrt_bytes_concat_owned;
        CHECK(append(&cx, &value, value) == CPLRT_OK);
        cplrt_w saved = 0;
        CPLRT_COPY(saved, value);
        CHECK(append(&cx, &value, value) == CPLRT_OK && value != saved);
        CHECK(is_text ? is_textn(saved, expected, 8) : is_bytes(saved, expected, 8));
        CHECK(is_text ? is_textn(value, expected, 16) : is_bytes(value, expected, 16));
        cplrt_w same = value;
        rt.max_heap_bytes = atomic_load(&rt.live_bytes);
        CHECK(append(&cx, &value, value) == CPLRT_OK && value == same);
        CHECK(append(&cx, &value, value) == CPLRT_OK && value == same);
        CHECK(is_text ? is_textn(value, expected, 64) : is_bytes(value, expected, 64));
        CHECK(append(&cx, &value, empty) == CPLRT_OK && value == same);
        CHECK(memory_fault(append(&cx, &value, value)) && value == same);
        CHECK(is_text ? is_textn(value, expected, 64) : is_bytes(value, expected, 64));
        CHECK(atomic_load(&cplrt_obj_of(value)->u.live.rc) == 1u);
        rt.max_heap_bytes = 0;
        end();
        cplrt_release(value);
        cplrt_release(saved);
        CHECK(atomic_load(&rt.live_objects) == objects);
    }
}

int main(void)
{
    rttest_runtime_init(&rt, 0);
    test_utf8();
    test_make_and_concat();
    test_lengths();
    test_trim();
    test_search();
    test_split_join();
    test_case();
    test_parse_from_chars();
    test_bytes();
    test_memory();
    test_owned_append();
    uint64_t left = rttest_runtime_fini(&rt);
    CHECK(left == 0);
    printf("test_text: %d checks, %d failed\n", checks, failures);
    return failures == 0 ? 0 : 1;
}
