/* test_list.c: List (spec/cabi.md 2.3, 2.6, 2.7): the immortal [], `++`, flatten, and every list builtin of
   cpl_rt_list.c with the results of crates/cpl-eval/src/builtins/prelude.rs: counts compared with the length before
   any conversion (a huge count clamps, a negative one is the precondition fault), get None for a negative or huge
   index, the operand shared when the result equals it, contains by structural equality, and the memory fault of every
   allocating builtin. */
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

/* program_stub.c's declarations */
enum { D_PAGEID = 7, D_PAGE = 8, STUB_TAKE = 12, STUB_DROP = 13, SITE_CALL = 4 };

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

static void restart(void)
{
    end();
    begin();
}

static cplrt_w text(const char *s)
{
    cplrt_w w = 0;
    CHECK(CPLRT(text_make)(&rt, s, strlen(s), &w) == CPLRT_OK);
    return w;
}

static cplrt_w big(const char *decimal)
{
    cplrt_w w = 0;
    CHECK(CPLRT(int_parse)(&rt, decimal, strlen(decimal), &w) == CPLRT_OK);
    return w;
}

static cplrt_w node(uint8_t kind, uint32_t def, const cplrt_w *items, uint32_t n)
{
    cplrt_w w = 0;
    CHECK(CPLRT(node_make)(&rt, kind, def, 0, items, n, &w) == CPLRT_OK);
    return w;
}

static cplrt_w list(const cplrt_w *items, uint32_t n)
{
    return node(CPLRT_LIST, 0, items, n);
}

/* [a, a + 1, ..., a + n - 1] */
static cplrt_w ints(int64_t a, uint32_t n)
{
    cplrt_w items[64];
    for (uint32_t i = 0; i < n; i++)
        items[i] = cplrt_small(a + (int64_t)i);
    return list(items, n);
}

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

static uint32_t rc(cplrt_w w)
{
    return atomic_load(&cplrt_obj_of(w)->u.live.rc);
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

static void test_empty_and_concat(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w e = CPLRT(empty_list)();
    CHECK(cplrt_obj_of(e)->kind == CPLRT_LIST && cplrt_len(e) == 0 && rc(e) == CPLRT_RC_IMMORTAL);
    cplrt_node *fresh = CPLRT(list_alloc)(&rt, 3);
    CHECK(fresh != NULL && fresh->hdr.len == 3 && fresh->item[0] == 0 && fresh->item[1] == 0 && fresh->item[2] == 0);
    /* a partly filled list is released as it is: 0 holds nothing */
    fresh->item[0] = text("held");
    cplrt_release(cplrt_word_of(&fresh->hdr));
    CHECK(atomic_load(&rt.live_objects) == objects);

    cplrt_w t = text("t"), a_items[2] = { cplrt_small(1), t }, a = list(a_items, 2), b = ints(7, 3), r = 0;
    CHECK(CPLRT(list_concat)(&rt, a, b, &r) == CPLRT_OK && renders(r, "[1, \"t\", 7, 8, 9]"));
    CHECK(cplrt_item(r, 1) == t && rc(t) == 3u); /* the elements are shared, each retained */
    CHECK(CPLRT(list_concat)(&rt, a, e, &r) == CPLRT_OK && r == a);
    CHECK(CPLRT(list_concat)(&rt, e, b, &r) == CPLRT_OK && r == b);
    CHECK(CPLRT(list_concat)(&rt, e, e, &r) == CPLRT_OK && r == e);
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    CHECK(CPLRT(list_concat)(&rt, a, b, &r) == CPLRT_NOMEM && r == e);
    rt.max_heap_bytes = 0;
    cplrt_release(a);
    cplrt_release(b);
    cplrt_release(t);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_flatten(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    cplrt_w e = CPLRT(empty_list)(), r = 0;
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && r == e);
    cplrt_w two_empty[2] = { e, e }, ee = list(two_empty, 2);
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, ee, &r) == CPLRT_OK && r == e);
    cplrt_w only = ints(1, 2), one_of[3] = { e, only, e }, lone = list(one_of, 3);
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, lone, &r) == CPLRT_OK && r == only); /* shared */
    cplrt_w x = ints(1, 1), y = ints(2, 2), z = ints(4, 3), parts[5] = { x, e, y, z, e }, many = list(parts, 5);
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, many, &r) == CPLRT_OK && renders(r, "[1, 2, 3, 4, 5, 6]"));
    CHECK(cplrt_len(r) == 6u);
    /* the inner elements are the same words, retained once more */
    cplrt_w t = text("s"), ti[1] = { t }, tl = list(ti, 1), tparts[2] = { tl, tl }, tt = list(tparts, 2);
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, tt, &r) == CPLRT_OK && renders(r, "[\"s\", \"s\"]"));
    CHECK(cplrt_item(r, 0) == t && cplrt_item(r, 1) == t && rc(t) == 4u);
    cplrt_w nested_items[2] = { tt, ee }, nested = list(nested_items, 2);
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, nested, &r) == CPLRT_OK &&
          renders(r, "[[\"s\"], [\"s\"], [], []]")); /* one level only */
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    cplrt_w keep = r;
    CHECK(memory_fault(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, many, &r)) && r == keep);
    restart();
    CHECK(CPLRT(bi_flatten)(&cx, SITE_CALL, CPLRT_NONE, lone, &r) == CPLRT_OK && r == only); /* needs no memory */
    rt.max_heap_bytes = 0;
    end();
    cplrt_w all[] = { r, ee, only, lone, x, y, z, many, t, tl, tt, nested };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

typedef int (*bi1)(cplrt_cx *, uint32_t, uint32_t, cplrt_w, cplrt_w *);
typedef int (*bi2)(cplrt_cx *, uint32_t, uint32_t, cplrt_w, cplrt_w, cplrt_w *);

/* f(xs, n) renders as `want`; `same` when it must be xs itself */
static void count_case(bi2 f, uint32_t stub, cplrt_w xs, cplrt_w n, const char *want, int same)
{
    cplrt_w r = 0;
    CHECK(f(&cx, SITE_CALL, stub, xs, n, &r) == CPLRT_OK && renders(r, want));
    if (same)
        CHECK(r == xs);
    else
        CHECK(r != xs);
    if (strcmp(want, "[]") == 0)
        CHECK(r == CPLRT(empty_list)());
    cplrt_release(r);
}

static void test_counts(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    cplrt_w xs = ints(10, 3), e = CPLRT(empty_list)();
    cplrt_w huge = big("1180591620717411303424"), neg_huge = big("-1180591620717411303424");
    cplrt_w max_small = cplrt_small(CPLRT_SMALL_MAX), u32 = cplrt_small(4294967296);
    bi2 take = CPLRT(bi_list_take), drop = CPLRT(bi_list_drop);
    CHECK(CPLRT(bi_list_len)(&cx, SITE_CALL, CPLRT_NONE, xs, &e) == CPLRT_OK && e == cplrt_small(3));
    e = CPLRT(empty_list)();
    cplrt_w r = 0;
    CHECK(CPLRT(bi_list_len)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && r == cplrt_small(0));
    CHECK(CPLRT(bi_list_is_empty)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && r == CPLRT_TRUE);
    CHECK(CPLRT(bi_list_is_empty)(&cx, SITE_CALL, CPLRT_NONE, xs, &r) == CPLRT_OK && r == CPLRT_FALSE);

    count_case(take, STUB_TAKE, xs, cplrt_small(0), "[]", 0);
    count_case(take, STUB_TAKE, xs, cplrt_small(1), "[10]", 0);
    count_case(take, STUB_TAKE, xs, cplrt_small(2), "[10, 11]", 0);
    count_case(take, STUB_TAKE, xs, cplrt_small(3), "[10, 11, 12]", 1);
    count_case(take, STUB_TAKE, xs, cplrt_small(4), "[10, 11, 12]", 1);
    count_case(take, STUB_TAKE, xs, u32, "[10, 11, 12]", 1);
    count_case(take, STUB_TAKE, xs, max_small, "[10, 11, 12]", 1);
    count_case(take, STUB_TAKE, xs, huge, "[10, 11, 12]", 1); /* clamps before any conversion (stdlib.md 4.1) */
    count_case(take, STUB_TAKE, e, cplrt_small(0), "[]", 1);
    count_case(take, STUB_TAKE, e, huge, "[]", 1);
    count_case(drop, STUB_DROP, xs, cplrt_small(0), "[10, 11, 12]", 1);
    count_case(drop, STUB_DROP, xs, cplrt_small(1), "[11, 12]", 0);
    count_case(drop, STUB_DROP, xs, cplrt_small(2), "[12]", 0);
    count_case(drop, STUB_DROP, xs, cplrt_small(3), "[]", 0);
    count_case(drop, STUB_DROP, xs, cplrt_small(9), "[]", 0);
    count_case(drop, STUB_DROP, xs, u32, "[]", 0);
    count_case(drop, STUB_DROP, xs, huge, "[]", 0);
    count_case(drop, STUB_DROP, e, cplrt_small(0), "[]", 1);
    count_case(drop, STUB_DROP, e, cplrt_small(5), "[]", 1);
    /* the precondition n >= 0, checked first, for small and big counts and an empty list */
    cplrt_w negs[4] = { cplrt_small(-1), cplrt_small(CPLRT_SMALL_MIN), neg_huge, cplrt_small(-1) };
    cplrt_w lists[4] = { xs, xs, xs, CPLRT(empty_list)() };
    for (int i = 0; i < 4; i++) {
        r = cplrt_small(77);
        CHECK(precondition_fault(take(&cx, SITE_CALL, STUB_TAKE, lists[i], negs[i], &r)) && r == cplrt_small(77));
        restart();
        CHECK(precondition_fault(drop(&cx, SITE_CALL, STUB_DROP, lists[i], negs[i], &r)) && r == cplrt_small(77));
        restart();
    }

    r = 0;
    CHECK(CPLRT(bi_list_drop_last)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && r == e);
    CHECK(CPLRT(bi_list_drop_last)(&cx, SITE_CALL, CPLRT_NONE, xs, &r) == CPLRT_OK && renders(r, "[10, 11]"));
    cplrt_w one = ints(5, 1);
    CHECK(CPLRT(bi_list_drop_last)(&cx, SITE_CALL, CPLRT_NONE, one, &r) == CPLRT_OK && r == CPLRT(empty_list)());

    cplrt_w t = text("x");
    CHECK(CPLRT(bi_list_push)(&cx, SITE_CALL, CPLRT_NONE, e, cplrt_small(1), &r) == CPLRT_OK && renders(r, "[1]"));
    CHECK(CPLRT(bi_list_push)(&cx, SITE_CALL, CPLRT_NONE, xs, t, &r) == CPLRT_OK &&
          renders(r, "[10, 11, 12, \"x\"]"));
    CHECK(cplrt_item(r, 3) == t && rc(t) == 2u && renders(xs, "[10, 11, 12]")); /* xs is unchanged */
    CHECK(CPLRT(bi_list_push)(&cx, SITE_CALL, CPLRT_NONE, r, r, &r) == CPLRT_OK &&
          renders(r, "[10, 11, 12, \"x\", [10, 11, 12, \"x\"]]")); /* the operand and the output may be one slot */
    cplrt_release(r);
    cplrt_release(t);
    cplrt_release(one);
    cplrt_release(huge);
    cplrt_release(neg_huge);
    cplrt_release(xs);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_elements(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    cplrt_w xs = ints(10, 3), e = CPLRT(empty_list)(), r = 0;
    cplrt_w huge = big("18446744073709551616"), neg_huge = big("-18446744073709551616");
    struct {
        cplrt_w index;
        const char *want;
    } gets[] = { { cplrt_small(0), "Some(10)" }, { cplrt_small(2), "Some(12)" }, { cplrt_small(3), "None" },
                 { cplrt_small(-1), "None" },    { cplrt_small(-3), "None" },    { cplrt_small(CPLRT_SMALL_MIN), "None" },
                 { cplrt_small(CPLRT_SMALL_MAX), "None" }, { huge, "None" },     { neg_huge, "None" } };
    for (size_t i = 0; i < sizeof gets / sizeof gets[0]; i++) {
        CHECK(CPLRT(bi_list_get)(&cx, SITE_CALL, CPLRT_NONE, xs, gets[i].index, &r) == CPLRT_OK &&
              renders(r, gets[i].want));
        CHECK(cplrt_obj_of(r)->u.live.def == rt.program->std.option);
    }
    CHECK(CPLRT(bi_list_get)(&cx, SITE_CALL, CPLRT_NONE, e, cplrt_small(0), &r) == CPLRT_OK && renders(r, "None"));
    CHECK(r == CPLRT(enum_static)(rt.program, rt.program->std.option, CPL_OPTION_NONE));
    CHECK(CPLRT(bi_list_first)(&cx, SITE_CALL, CPLRT_NONE, xs, &r) == CPLRT_OK && renders(r, "Some(10)"));
    CHECK(CPLRT(bi_list_last)(&cx, SITE_CALL, CPLRT_NONE, xs, &r) == CPLRT_OK && renders(r, "Some(12)"));
    CHECK(CPLRT(bi_list_first)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && renders(r, "None"));
    CHECK(CPLRT(bi_list_last)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && renders(r, "None"));
    cplrt_w t = text("only"), ti[1] = { t }, tl = list(ti, 1);
    CHECK(CPLRT(bi_list_last)(&cx, SITE_CALL, CPLRT_NONE, tl, &r) == CPLRT_OK && cplrt_payload(r, 0) == t);

    CHECK(CPLRT(bi_list_reverse)(&cx, SITE_CALL, CPLRT_NONE, e, &r) == CPLRT_OK && r == e);
    CHECK(CPLRT(bi_list_reverse)(&cx, SITE_CALL, CPLRT_NONE, xs, &r) == CPLRT_OK && renders(r, "[12, 11, 10]"));
    CHECK(CPLRT(bi_list_reverse)(&cx, SITE_CALL, CPLRT_NONE, tl, &r) == CPLRT_OK && renders(r, "[\"only\"]") && r != tl);
    CHECK(cplrt_item(r, 0) == t && rc(t) == 3u);

    /* contains: structural ==, left to right (2.8) */
    cplrt_w id1 = node(CPLRT_WRAPPER, D_PAGEID, &t, 1), t2 = text("only"), id2 = node(CPLRT_WRAPPER, D_PAGEID, &t2, 1);
    cplrt_w f1[2] = { id1, t }, f2[2] = { id2, t2 }, f3[2] = { id2, cplrt_small(1) };
    cplrt_w p1 = node(CPLRT_STRUCT, D_PAGE, f1, 2), p2 = node(CPLRT_STRUCT, D_PAGE, f2, 2);
    cplrt_w p3 = node(CPLRT_STRUCT, D_PAGE, f3, 2);
    cplrt_w pages_items[2] = { p3, p1 }, pages = list(pages_items, 2);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, pages, p2, &r) == CPLRT_OK && r == CPLRT_TRUE);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, pages, id2, &r) == CPLRT_OK && r == CPLRT_FALSE);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, e, cplrt_small(1), &r) == CPLRT_OK && r == CPLRT_FALSE);
    cplrt_w b1 = big("-18446744073709551617"), b2 = big("-18446744073709551617");
    cplrt_w mixed_items[3] = { cplrt_small(1), CPLRT_TRUE, b1 }, mixed = list(mixed_items, 3);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, mixed, b2, &r) == CPLRT_OK && r == CPLRT_TRUE);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, mixed, CPLRT_TRUE, &r) == CPLRT_OK && r == CPLRT_TRUE);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, mixed, CPLRT_FALSE, &r) == CPLRT_OK && r == CPLRT_FALSE);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, mixed, cplrt_small(2), &r) == CPLRT_OK &&
          r == CPLRT_FALSE);
    cplrt_w nested_items[2] = { e, xs }, nested = list(nested_items, 2), xs2 = ints(10, 3);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, nested, xs2, &r) == CPLRT_OK && r == CPLRT_TRUE);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, nested, e, &r) == CPLRT_OK && r == CPLRT_TRUE);
    end();
    cplrt_w all[] = { xs, huge, neg_huge, t, tl, id1, t2, id2, p1, p2, p3, pages, b1, b2, mixed, nested, xs2 };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_memory(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w xs = ints(1, 4), deep1 = CPLRT(empty_list)(), deep2 = CPLRT(empty_list)();
    for (int i = 0; i < 100; i++) { /* deep enough that equality needs more than its local work stack */
        cplrt_w n1 = list(&deep1, 1), n2 = list(&deep2, 1);
        cplrt_release(deep1);
        cplrt_release(deep2);
        deep1 = n1;
        deep2 = n2;
    }
    cplrt_w deeps = list(&deep1, 1);
    begin();
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    cplrt_w keep = cplrt_small(3), r = keep;
    CHECK(memory_fault(CPLRT(bi_list_take)(&cx, SITE_CALL, STUB_TAKE, xs, cplrt_small(2), &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_drop)(&cx, SITE_CALL, STUB_DROP, xs, cplrt_small(2), &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_drop_last)(&cx, SITE_CALL, CPLRT_NONE, xs, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_push)(&cx, SITE_CALL, CPLRT_NONE, xs, cplrt_small(0), &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_get)(&cx, SITE_CALL, CPLRT_NONE, xs, cplrt_small(0), &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_first)(&cx, SITE_CALL, CPLRT_NONE, xs, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_last)(&cx, SITE_CALL, CPLRT_NONE, xs, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_reverse)(&cx, SITE_CALL, CPLRT_NONE, xs, &r)) && r == keep);
    restart();
    CHECK(memory_fault(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, deeps, deep2, &r)) && r == keep);
    restart();
    /* what needs no memory still works */
    CHECK(CPLRT(bi_list_take)(&cx, SITE_CALL, STUB_TAKE, xs, cplrt_small(9), &r) == CPLRT_OK && r == xs);
    /* (rendering needs memory: test the variant) */
    CHECK(CPLRT(bi_list_get)(&cx, SITE_CALL, CPLRT_NONE, xs, cplrt_small(9), &r) == CPLRT_OK &&
          cplrt_variant(r) == CPL_OPTION_NONE && cplrt_payload_len(r) == 0u);
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, xs, cplrt_small(4), &r) == CPLRT_OK && r == CPLRT_TRUE);
    rt.max_heap_bytes = 0;
    CHECK(CPLRT(bi_list_contains)(&cx, SITE_CALL, CPLRT_NONE, deeps, deep2, &r) == CPLRT_OK && r == CPLRT_TRUE);
    end();
    cplrt_release(xs);
    cplrt_release(deep1);
    cplrt_release(deep2);
    cplrt_release(deeps);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_owned_growth(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    cplrt_w xs = CPLRT(empty_list)();
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &xs, cplrt_small(1)) == CPLRT_OK);
    cplrt_w buffer = xs;
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    /* Unique spare capacity works even when no further heap allocation is permitted. */
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &xs, cplrt_small(2)) == CPLRT_OK && xs == buffer);
    rt.max_heap_bytes = 0;
    cplrt_w snapshot = xs;
    cplrt_retain(snapshot);
    cplrt_w tail = ints(3, 2);
    CHECK(cplrt_list_concat_owned(&cx, &xs, tail) == CPLRT_OK);
    CHECK(renders(snapshot, "[1, 2]") && renders(xs, "[1, 2, 3, 4]"));
    CHECK(cplrt_list_concat_owned(&cx, &xs, xs) == CPLRT_OK);
    CHECK(renders(xs, "[1, 2, 3, 4, 1, 2, 3, 4]"));
    CHECK(cplrt_list_concat_owned(&cx, &xs, CPLRT(empty_list)()) == CPLRT_OK);
    buffer = xs;
    uint32_t refs = rc(tail);
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    CHECK(memory_fault(cplrt_list_concat_owned(&cx, &xs, tail)) && xs == buffer && rc(tail) == refs);
    restart();
    CHECK(memory_fault(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &xs, tail)) && xs == buffer && rc(tail) == refs);
    rt.max_heap_bytes = 0;
    restart();
    CHECK(renders(xs, "[1, 2, 3, 4, 1, 2, 3, 4]"));
    cplrt_release(xs);
    cplrt_release(snapshot);
    cplrt_release(tail);

    /* Borrowed heap children and the old list itself survive a buffer replacement. */
    cplrt_w label = text("kept");
    xs = list(&label, 1);
    cplrt_release(label);
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &xs, cplrt_item(xs, 0)) == CPLRT_OK);
    CHECK(renders(xs, "[\"kept\", \"kept\"]"));
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &xs, xs) == CPLRT_OK);
    CHECK(renders(xs, "[\"kept\", \"kept\", [\"kept\", \"kept\"]]"));
    cplrt_release(xs);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static int is_view(cplrt_w w)
{
    return cplrt_is_obj(w) && (cplrt_obj_of(w)->flags & CPLRT_F_VIEW) != 0;
}

static cplrt_w view_base(cplrt_w w)
{
    return ((const cplrt_listview *)cplrt_obj_of(w))->base;
}

static cplrt_w drop_n(cplrt_w xs, int64_t n)
{
    cplrt_w r = 0;
    CHECK(CPLRT(bi_list_drop)(&cx, SITE_CALL, STUB_DROP, xs, cplrt_small(n), &r) == CPLRT_OK);
    return r;
}

static cplrt_w take_n(cplrt_w xs, int64_t n)
{
    cplrt_w r = 0;
    CHECK(CPLRT(bi_list_take)(&cx, SITE_CALL, STUB_TAKE, xs, cplrt_small(n), &r) == CPLRT_OK);
    return r;
}

/* take, drop, and drop_last view the operand's buffer (2.3): a view of a view views the base, a short result is a copy,
   a view reads, compares, and releases as the list it views, and an owned append grows a view in place only when it
   alone holds the base and ends where the base ends. */
static void test_views(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    begin();
    cplrt_w xs = ints(100, 20);
    cplrt_w v = drop_n(xs, 5);
    CHECK(is_view(v) && view_base(v) == xs && cplrt_len(v) == 15 && rc(xs) == 2u);
    CHECK(cplrt_item(v, 0) == cplrt_small(105) && cplrt_item(v, 14) == cplrt_small(119));
    cplrt_w w = drop_n(v, 3), u = take_n(w, 9);
    CHECK(is_view(w) && view_base(w) == xs && cplrt_item(w, 0) == cplrt_small(108) && cplrt_len(w) == 12);
    CHECK(is_view(u) && view_base(u) == xs && cplrt_len(u) == 9 && cplrt_item(u, 8) == cplrt_small(116));
    CHECK(renders(u, "[108, 109, 110, 111, 112, 113, 114, 115, 116]"));
    cplrt_w short_ = take_n(w, 3), last = 0;
    CHECK(!is_view(short_) && renders(short_, "[108, 109, 110]"));
    CHECK(CPLRT(bi_list_drop_last)(&cx, SITE_CALL, CPLRT_NONE, w, &last) == CPLRT_OK && is_view(last) &&
          cplrt_len(last) == 11 && cplrt_item(last, 10) == cplrt_small(118));
    /* equal to a list of the same items, and to itself through another view */
    cplrt_w same = ints(108, 9), first = 0;
    int eq = 0;
    CHECK(CPLRT(value_equal)(&rt, u, same, &eq) == CPLRT_OK && eq == 1);
    cplrt_w from8 = drop_n(xs, 8), again = take_n(from8, 9);
    cplrt_release(from8);
    CHECK(CPLRT(value_equal)(&rt, u, again, &eq) == CPLRT_OK && eq == 1);
    CHECK(CPLRT(value_equal)(&rt, u, w, &eq) == CPLRT_OK && eq == 0);
    CHECK(CPLRT(bi_list_first)(&cx, SITE_CALL, CPLRT_NONE, w, &first) == CPLRT_OK &&
          cplrt_payload(first, 0) == cplrt_small(108));
    /* a list holding views, released */
    cplrt_w held[2] = { v, w };
    cplrt_w outer = list(held, 2);
    cplrt_w rest = drop_n(outer, 1);
    CHECK(renders(rest, "[[108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119]]"));
    cplrt_release(rest);
    cplrt_release(outer);

    /* in place: a unique view of a base it alone holds, ending at the base's end, with spare capacity */
    cplrt_w grown = CPLRT(empty_list)();
    for (int i = 0; i < 20; i++)
        CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &grown, cplrt_small(i)) == CPLRT_OK);
    cplrt_w tailv = drop_n(grown, 2);
    cplrt_release(grown);
    cplrt_w base = view_base(tailv);
    CHECK(is_view(tailv) && rc(base) == 1u);
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &tailv, cplrt_small(99)) == CPLRT_OK);
    CHECK(is_view(tailv) && view_base(tailv) == base && cplrt_len(tailv) == 19 &&
          cplrt_item(tailv, 18) == cplrt_small(99));
    cplrt_w more = ints(7, 2);
    CHECK(cplrt_list_concat_owned(&cx, &tailv, more) == CPLRT_OK && is_view(tailv) && cplrt_len(tailv) == 21 &&
          cplrt_item(tailv, 20) == cplrt_small(8));
    /* a shared view copies; the snapshot is unchanged */
    cplrt_w snap = tailv;
    cplrt_retain(snap);
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &tailv, cplrt_small(5)) == CPLRT_OK && !is_view(tailv));
    CHECK(cplrt_len(snap) == 21 && cplrt_len(tailv) == 22 && cplrt_item(tailv, 0) == cplrt_small(2));
    cplrt_release(snap);
    cplrt_release(tailv);
    /* a view that does not end at its base's end is rebuilt, its items retained */
    cplrt_w front = take_n(xs, 12);
    CHECK(is_view(front));
    CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &front, cplrt_small(-1)) == CPLRT_OK && !is_view(front));
    CHECK(cplrt_len(front) == 13 && cplrt_item(front, 11) == cplrt_small(111) && renders(xs, "[100, 101, 102, 103, "
          "104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119]"));
    cplrt_release(front);

    /* a queue: drop the front, push twice at the back; the base's dead prefix never outgrows the queue */
    cplrt_w q = ints(0, 1);
    uint64_t bytes = 0;
    for (int64_t i = 1; i < 5000; i++) {
        cplrt_w next = drop_n(q, 1);
        cplrt_release(q);
        q = next;
        CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &q, cplrt_small(i)) == CPLRT_OK);
        CHECK(cplrt_list_push_owned(&cx, SITE_CALL, CPLRT_NONE, &q, cplrt_small(-i)) == CPLRT_OK);
        if (i == 2500)
            bytes = atomic_load(&rt.live_bytes);
    }
    CHECK(cplrt_len(q) == 5000 && cplrt_item(q, 0) == cplrt_small(2500) && cplrt_item(q, 4999) == cplrt_small(-4999));
    CHECK(atomic_load(&rt.live_bytes) < bytes + 5000u * 8u * 8u); /* linear in the queue, not in the pushes */
    cplrt_release(q);

    cplrt_release(v);
    cplrt_release(w);
    cplrt_release(u);
    cplrt_release(short_);
    cplrt_release(last);
    cplrt_release(same);
    cplrt_release(again);
    cplrt_release(first);
    cplrt_release(more);
    cplrt_release(xs);
    end();
    CHECK(atomic_load(&rt.live_objects) == objects);
}

int main(void)
{
    rttest_runtime_init(&rt, 0);
    test_empty_and_concat();
    test_flatten();
    test_counts();
    test_elements();
    test_memory();
    test_owned_growth();
    test_views();
    uint64_t left = rttest_runtime_fini(&rt);
    CHECK(left == 0);
    printf("test_list: %d checks, %d failed\n", checks, failures);
    return failures == 0 ? 0 : 1;
}
