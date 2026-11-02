/* test_value.c: heap objects, counts, saturation, immortals, the allocator and the heap budget, the value builders and
   their evaluation forms, `++`, structural equality, and the handle table (spec/cabi.md 2.3, 2.4, 2.8, 4.5). */
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

enum { D_UNIT = 0, D_OPTION = 1, D_RESULT = 2, D_CERR = 3, D_PAGEID = 7, D_PAGE = 8, D_PAGE_ERROR = 9, D_PAIR = 10 };

static cplrt_runtime rt;

static cplrt_w text(const char *s)
{
    cplrt_w w = 0;
    CHECK(CPLRT(text_make)(&rt, s, strlen(s), &w) == CPLRT_OK);
    return w;
}

static cplrt_w integer(int64_t v)
{
    cplrt_w w = 0;
    CHECK(CPLRT(int_from_i64)(&rt, v, &w) == CPLRT_OK);
    return w;
}

static cplrt_w big(const char *decimal)
{
    cplrt_w w = 0;
    CHECK(CPLRT(int_parse)(&rt, decimal, strlen(decimal), &w) == CPLRT_OK);
    return w;
}

static cplrt_w node(uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items, uint32_t n)
{
    cplrt_w w = 0;
    CHECK(CPLRT(node_make)(&rt, kind, def, variant, items, n, &w) == CPLRT_OK);
    return w;
}

static int equal(cplrt_w a, cplrt_w b)
{
    int eq = -1;
    CHECK(CPLRT(value_equal)(&rt, a, b, &eq) == CPLRT_OK);
    return eq;
}

static uint32_t rc(cplrt_w w)
{
    return atomic_load(&cplrt_obj_of(w)->u.live.rc);
}

static void test_words(void)
{
    CHECK(cplrt_is_small(cplrt_small(0)) && cplrt_small_value(cplrt_small(0)) == 0);
    CHECK(cplrt_small_value(cplrt_small(CPLRT_SMALL_MIN)) == CPLRT_SMALL_MIN);
    CHECK(cplrt_small_value(cplrt_small(CPLRT_SMALL_MAX)) == CPLRT_SMALL_MAX);
    CHECK(cplrt_small_value(cplrt_small(-1)) == -1);
    CHECK(!cplrt_is_obj(CPLRT_TRUE) && !cplrt_is_obj(CPLRT_FALSE) && !cplrt_is_obj(CPLRT_EMPTY));
    CHECK(cplrt_truth(cplrt_bool(1)) && !cplrt_truth(cplrt_bool(0)));
    CHECK(strcmp(CPLRT(kind_name)(cplrt_small(3)), "Int") == 0);
    CHECK(strcmp(CPLRT(kind_name)(CPLRT_TRUE), "Bool") == 0);
    cplrt_w t = text("x"), l = node(CPLRT_LIST, 0, 0, &t, 1), s = node(CPLRT_STRUCT, D_PAGE, 0, NULL, 0);
    cplrt_w e = node(CPLRT_ENUM, D_RESULT, 0, &t, 1), w = node(CPLRT_WRAPPER, D_PAGEID, 0, &t, 1);
    cplrt_w b = big("-4611686018427387905"), y = 0;
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"ab", 2, &y) == CPLRT_OK);
    CHECK(strcmp(CPLRT(kind_name)(b), "Int") == 0 && strcmp(CPLRT(kind_name)(t), "Text") == 0);
    CHECK(strcmp(CPLRT(kind_name)(y), "Bytes") == 0 && strcmp(CPLRT(kind_name)(l), "List") == 0);
    CHECK(strcmp(CPLRT(kind_name)(s), "struct") == 0 && strcmp(CPLRT(kind_name)(e), "enum") == 0);
    CHECK(strcmp(CPLRT(kind_name)(w), "wrapper") == 0);
    cplrt_w all[8] = { t, l, s, e, w, b, y, 0 };
    for (int i = 0; i < 7; i++)
        cplrt_release(all[i]);
}

static void test_counts_and_immortals(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w t = text("abc");
    CHECK(atomic_load(&rt.live_objects) == objects + 1u);
    CHECK(rc(t) == 1u);
    cplrt_retain(t);
    CHECK(rc(t) == 2u);
    cplrt_release(t);
    CHECK(rc(t) == 1u);
    /* saturation (2.4): an increment that reaches 2^31 stores the immortal count, which nothing changes again */
    atomic_store(&cplrt_obj_of(t)->u.live.rc, CPLRT_RC_SATURATED - 1u);
    cplrt_retain(t);
    CHECK(rc(t) == CPLRT_RC_IMMORTAL);
    cplrt_release(t);
    cplrt_retain(t);
    CHECK(rc(t) == CPLRT_RC_IMMORTAL);
    /* make it mortal again so that the test frees it */
    atomic_store(&cplrt_obj_of(t)->u.live.rc, 1u);
    cplrt_release(t);
    CHECK(atomic_load(&rt.live_objects) == objects);
    /* the runtime's immortals: "", [], and the empty Bytes */
    cplrt_w e = 0, l = 0, y = 0;
    CHECK(CPLRT(text_make)(&rt, "", 0, &e) == CPLRT_OK && e == CPLRT(empty_text)());
    CHECK(CPLRT(node_make)(&rt, CPLRT_LIST, 0, 0, NULL, 0, &l) == CPLRT_OK && l == CPLRT(empty_list)());
    CHECK(CPLRT(bytes_make)(&rt, NULL, 0, &y) == CPLRT_OK && y == CPLRT(empty_bytes)());
    CHECK(rc(e) == CPLRT_RC_IMMORTAL && rc(l) == CPLRT_RC_IMMORTAL && rc(y) == CPLRT_RC_IMMORTAL);
    cplrt_retain(e);
    cplrt_release(e);
    cplrt_release(e);
    CHECK(rc(e) == CPLRT_RC_IMMORTAL);
    CHECK(cplrt_text_of(e)->bytes == 0 && cplrt_text_of(e)->utf8[0] == '\0' && cplrt_len(l) == 0);
    CHECK(atomic_load(&rt.live_objects) == objects);
    /* payload-less variants are the program's statics */
    cplrt_w unit = 0, none = 0, unavailable = 0;
    CHECK(CPLRT(node_make)(&rt, CPLRT_ENUM, D_UNIT, 0, NULL, 0, &unit) == CPLRT_OK);
    CHECK(unit == CPLRT(enum_static)(rt.program, D_UNIT, 0) && unit != 0);
    CHECK(CPLRT(node_make)(&rt, CPLRT_ENUM, D_OPTION, CPL_OPTION_NONE, NULL, 0, &none) == CPLRT_OK);
    CHECK(none == CPLRT(enum_static)(rt.program, D_OPTION, CPL_OPTION_NONE));
    CHECK(cplrt_variant(none) == CPL_OPTION_NONE && cplrt_obj_of(none)->u.live.def == D_OPTION);
    CHECK(CPLRT(node_make)(&rt, CPLRT_ENUM, D_PAGE_ERROR, 1, NULL, 0, &unavailable) == CPLRT_OK);
    CHECK(rc(unavailable) == CPLRT_RC_IMMORTAL);
    CHECK(CPLRT(enum_static)(rt.program, D_OPTION, CPL_OPTION_SOME) == 0);
    CHECK(CPLRT(enum_static)(rt.program, 9999, 0) == 0);
    CHECK(CPLRT(enum_static)(rt.program, D_UNIT, 1) == 0);
    /* a payload-less variant without a static is allocated */
    cplrt_w ok_empty = 0;
    CHECK(CPLRT(node_make)(&rt, CPLRT_ENUM, D_RESULT, 0, NULL, 0, &ok_empty) == CPLRT_OK);
    CHECK(rc(ok_empty) == 1u && cplrt_len(ok_empty) == 0);
    cplrt_release(ok_empty);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_builders(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w a = text("a"), b = text("b");
    cplrt_w items[2] = { a, b };
    cplrt_w l = node(CPLRT_LIST, 0, 0, items, 2);
    CHECK(cplrt_len(l) == 2 && cplrt_item(l, 0) == a && cplrt_item(l, 1) == b);
    CHECK(rc(a) == 2u && rc(b) == 2u); /* the builder retained the borrowed words */
    cplrt_w page_fields[2] = { 0, b };
    cplrt_w id = node(CPLRT_WRAPPER, D_PAGEID, 0, &a, 1);
    page_fields[0] = id;
    cplrt_w page = node(CPLRT_STRUCT, D_PAGE, 0, page_fields, 2);
    CHECK(cplrt_obj_of(page)->u.live.def == D_PAGE && cplrt_field(page, 0) == id && cplrt_field(page, 1) == b);
    CHECK(cplrt_inner(id) == a && cplrt_obj_of(id)->kind == CPLRT_WRAPPER);
    cplrt_w missing = node(CPLRT_ENUM, D_PAGE_ERROR, 0, &id, 1);
    CHECK(cplrt_variant(missing) == 0 && cplrt_payload_len(missing) == 1 && cplrt_payload(missing, 0) == id);
    /* the wide form of a variant index above 65535 (2.3) */
    cplrt_w wide = node(CPLRT_ENUM, D_PAGE_ERROR, 70000u, items, 2);
    CHECK((cplrt_obj_of(wide)->flags & CPLRT_F_WIDE) != 0 && cplrt_len(wide) == 3);
    CHECK(cplrt_variant(wide) == 70000u && cplrt_payload_len(wide) == 2);
    CHECK(cplrt_payload(wide, 0) == a && cplrt_payload(wide, 1) == b);
    cplrt_w narrow = node(CPLRT_ENUM, D_PAGE_ERROR, 65535u, items, 2);
    CHECK((cplrt_obj_of(narrow)->flags & CPLRT_F_WIDE) == 0 && cplrt_variant(narrow) == 65535u);
    cplrt_w wide_empty = node(CPLRT_ENUM, D_PAGE_ERROR, 65536u, NULL, 0);
    CHECK(cplrt_variant(wide_empty) == 65536u && cplrt_payload_len(wide_empty) == 0);
    /* the evaluation forms */
    cplrt_cx cx;
    cplrt_memory_record memory;
    CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, UINT32_MAX, 1, 0, 0, NULL) == CPLRT_OK);
    cplrt_w r = 0;
    CHECK(cplrt_list_new(&cx, items, 2, &r) == CPLRT_OK && cplrt_len(r) == 2);
    CHECK(cplrt_struct_new(&cx, D_PAGE, page_fields, 2, &r) == CPLRT_OK && cplrt_obj_of(r)->kind == CPLRT_STRUCT);
    CHECK(cplrt_enum_new(&cx, D_PAGE_ERROR, 1, NULL, 0, &r) == CPLRT_OK && r == CPLRT(enum_static)(rt.program, 9, 1));
    CHECK(cplrt_wrap(&cx, D_PAGEID, a, &r) == CPLRT_OK && cplrt_inner(r) == a);
    CHECK(cplrt_result_ok(&cx, a, &r) == CPLRT_OK && cplrt_is_ok(r) && cplrt_payload(r, 0) == a);
    CHECK(cplrt_obj_of(r)->u.live.def == D_RESULT);
    CHECK(cplrt_result_err(&cx, b, &r) == CPLRT_OK && !cplrt_is_ok(r) && cplrt_variant(r) == CPL_RESULT_ERR);
    CHECK(cplrt_option_some(&cx, b, &r) == CPLRT_OK && cplrt_variant(r) == CPL_OPTION_SOME);
    CHECK(cplrt_obj_of(r)->u.live.def == D_OPTION && cplrt_payload(r, 0) == b);
    cplrt_release(r);
    /* the ConstraintError value (builtins::constraint_error) */
    cplrt_w err = 0;
    CHECK(CPLRT(constraint_err)(&rt, "Int", 3, "bad", 3, &err) == CPLRT_OK);
    CHECK(cplrt_variant(err) == CPL_RESULT_ERR && cplrt_obj_of(err)->u.live.def == D_RESULT);
    cplrt_w ce = cplrt_payload(err, 0);
    CHECK(cplrt_obj_of(ce)->u.live.def == D_CERR && cplrt_len(ce) == 2);
    CHECK(strcmp(cplrt_text_of(cplrt_field(ce, CPL_CONSTRAINT_ERROR_F_TYPE_NAME))->utf8, "Int") == 0);
    CHECK(strcmp(cplrt_text_of(cplrt_field(ce, CPL_CONSTRAINT_ERROR_F_MESSAGE))->utf8, "bad") == 0);
    cplrt_release(err);
    CPLRT(cx_end)(&cx);
    cplrt_w all[] = { l, id, page, missing, wide, narrow, wide_empty, a, b };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_concat(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_cx cx;
    cplrt_memory_record memory;
    CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, UINT32_MAX, 1, 0, 0, NULL) == CPLRT_OK);
    cplrt_w ab = text("ab"), cd = text("c\303\251"), empty = CPLRT(empty_text)(), r = 0;
    CHECK(cplrt_concat(&cx, ab, cd, &r) == CPLRT_OK);
    CHECK(strcmp(cplrt_text_of(r)->utf8, "abc\303\251") == 0 && cplrt_text_of(r)->bytes == 5);
    CHECK(cplrt_text_of(r)->scalars == 4);
    CHECK(cplrt_concat(&cx, ab, empty, &r) == CPLRT_OK && r == ab); /* an operand when the other is empty */
    CHECK(cplrt_concat(&cx, empty, cd, &r) == CPLRT_OK && r == cd);
    cplrt_w x = 0, y = 0;
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"\001\002", 2, &x) == CPLRT_OK);
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"\377", 1, &y) == CPLRT_OK);
    CHECK(cplrt_concat(&cx, x, y, &r) == CPLRT_OK && cplrt_bytes_of(r)->len == 3);
    CHECK(memcmp(cplrt_bytes_of(r)->data, "\001\002\377", 3) == 0);
    CHECK(cplrt_concat(&cx, CPLRT(empty_bytes)(), y, &r) == CPLRT_OK && r == y);
    cplrt_w one[1] = { cplrt_small(1) }, two[2] = { cplrt_small(2), ab };
    cplrt_w l1 = node(CPLRT_LIST, 0, 0, one, 1), l2 = node(CPLRT_LIST, 0, 0, two, 2);
    CHECK(cplrt_concat(&cx, l1, l2, &r) == CPLRT_OK && cplrt_len(r) == 3);
    CHECK(cplrt_item(r, 0) == cplrt_small(1) && cplrt_item(r, 1) == cplrt_small(2) && cplrt_item(r, 2) == ab);
    CHECK(cplrt_concat(&cx, l1, CPLRT(empty_list)(), &r) == CPLRT_OK && r == l1);
    CHECK(cplrt_concat(&cx, CPLRT(empty_list)(), l2, &r) == CPLRT_OK && r == l2);
    /* operands of different forms are a type confusion, an internal error */
    CHECK(cplrt_concat(&cx, ab, l1, &r) == CPLRT_FAULT && r == l2);
    CHECK(cx.fault != NULL && cx.fault->kind == CPLRT_FK_INTERNAL_ERROR);
    CPLRT(cx_end)(&cx);
    cplrt_w all[] = { r, ab, cd, x, y, l1, l2 };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

static void test_equality(void)
{
    uint64_t objects = atomic_load(&rt.live_objects);
    cplrt_w a = text("same"), a2 = text("same"), c = text("other");
    CHECK(equal(a, a) && equal(a, a2) && !equal(a, c));
    CHECK(equal(cplrt_small(5), cplrt_small(5)) && !equal(cplrt_small(5), cplrt_small(6)));
    CHECK(!equal(CPLRT_TRUE, CPLRT_FALSE) && equal(CPLRT_TRUE, CPLRT_TRUE));
    cplrt_w b1 = big("-4611686018427387905"), b2 = big("-4611686018427387905"), b3 = big("4611686018427387905");
    CHECK(equal(b1, b2) && !equal(b1, b3) && !equal(b1, cplrt_small(5)));
    cplrt_w x = 0, y = 0, z = 0;
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"\000\001", 2, &x) == CPLRT_OK);
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"\000\001", 2, &y) == CPLRT_OK);
    CHECK(CPLRT(bytes_make)(&rt, (const uint8_t *)"\000\002", 2, &z) == CPLRT_OK);
    CHECK(equal(x, y) && !equal(x, z) && !equal(x, a));
    cplrt_w i1[3] = { a, b1, x }, i2[3] = { a2, b2, y }, i3[3] = { a2, b2, z };
    cplrt_w l1 = node(CPLRT_LIST, 0, 0, i1, 3), l2 = node(CPLRT_LIST, 0, 0, i2, 3), l3 = node(CPLRT_LIST, 0, 0, i3, 3);
    cplrt_w l4 = node(CPLRT_LIST, 0, 0, i1, 2);
    CHECK(equal(l1, l2) && !equal(l1, l3) && !equal(l1, l4));
    /* the same children under another declaration, kind, or variant are unequal */
    cplrt_w s1 = node(CPLRT_STRUCT, D_PAGE, 0, i1, 2), s2 = node(CPLRT_STRUCT, D_PAIR, 0, i1, 2);
    cplrt_w s3 = node(CPLRT_STRUCT, D_PAGE, 0, i2, 2);
    CHECK(equal(s1, s3) && !equal(s1, s2) && !equal(s1, l4));
    cplrt_w e1 = node(CPLRT_ENUM, D_RESULT, 0, &a, 1), e2 = node(CPLRT_ENUM, D_RESULT, 1, &a, 1);
    cplrt_w e3 = node(CPLRT_ENUM, D_RESULT, 0, &a2, 1), e4 = node(CPLRT_ENUM, D_OPTION, 0, &a, 1);
    CHECK(equal(e1, e3) && !equal(e1, e2) && !equal(e1, e4));
    cplrt_w w1 = node(CPLRT_WRAPPER, D_PAGEID, 0, &a, 1), w2 = node(CPLRT_WRAPPER, D_PAGEID, 0, &a2, 1);
    cplrt_w w3 = node(CPLRT_WRAPPER, D_PAGEID, 0, &c, 1);
    CHECK(equal(w1, w2) && !equal(w1, w3) && !equal(w1, a));
    /* deep and wide values compare without recursion; the first difference ends it */
    cplrt_w deep1 = CPLRT(enum_static)(rt.program, D_OPTION, CPL_OPTION_NONE), deep2 = deep1;
    cplrt_retain(deep1);
    cplrt_retain(deep2);
    for (int i = 0; i < 200000; i++) {
        cplrt_w n1 = node(CPLRT_ENUM, D_OPTION, CPL_OPTION_SOME, &deep1, 1);
        cplrt_w n2 = node(CPLRT_ENUM, D_OPTION, CPL_OPTION_SOME, &deep2, 1);
        cplrt_release(deep1);
        cplrt_release(deep2);
        deep1 = n1;
        deep2 = n2;
    }
    CHECK(equal(deep1, deep2));
    cplrt_w d3 = node(CPLRT_ENUM, D_OPTION, CPL_OPTION_SOME, &deep1, 1);
    cplrt_w d4 = node(CPLRT_ENUM, D_OPTION, CPL_OPTION_SOME, &deep2, 1);
    CHECK(equal(d3, d4) && !equal(d3, deep2));
    /* the work stack's allocation can fail: a status, never a crash */
    rt.max_heap_bytes = atomic_load(&rt.live_bytes);
    int eq = -1;
    CHECK(CPLRT(value_equal)(&rt, deep1, deep2, &eq) == CPLRT_NOMEM && eq == -1);
    cplrt_cx cx;
    cplrt_memory_record memory;
    CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, UINT32_MAX, 1, 0, 0, NULL) == CPLRT_OK);
    CHECK(cplrt_equal(&cx, deep1, deep2, &eq) == CPLRT_FAULT);
    CHECK(cx.fault != NULL && cx.fault->kind == CPLRT_FK_RESOURCE_EXHAUSTION && cx.fault->resource == CPLRT_RES_MEMORY);
    CPLRT(cx_end)(&cx);
    rt.max_heap_bytes = 0;
    CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, UINT32_MAX, 1, 0, 0, NULL) == CPLRT_OK);
    CHECK(cplrt_equal(&cx, l1, l2, &eq) == CPLRT_OK && eq == 1);
    CPLRT(cx_end)(&cx);
    cplrt_w all[] = { a, a2, c, b1, b2, b3, x, y, z, l1, l2, l3, l4, s1, s2, s3, e1, e2, e3, e4, w1, w2, w3, deep1,
                      deep2, d3, d4 };
    for (size_t i = 0; i < sizeof all / sizeof all[0]; i++)
        cplrt_release(all[i]);
    CHECK(atomic_load(&rt.live_objects) == objects);
}

/* ---- the allocator ---------------------------------------------------------------------------------------------- */

static size_t counted_allocs, counted_frees, counted_bytes;
static void *counted_alloc(void *user, size_t size, size_t align)
{
    CHECK(user == &counted_allocs && align == _Alignof(max_align_t));
    counted_allocs++;
    counted_bytes += size;
    return malloc(size);
}
static void counted_free(void *user, void *p, size_t size, size_t align)
{
    CHECK(user == &counted_allocs && align == _Alignof(max_align_t));
    counted_frees++;
    counted_bytes -= size;
    free(p);
}

static void test_allocator(void)
{
    cplrt_runtime own;
    rttest_runtime_init(&own, 0);
    own.allocator.struct_size = sizeof own.allocator;
    own.allocator.user = &counted_allocs;
    own.allocator.alloc = counted_alloc;
    own.allocator.free = counted_free;
    void *p = CPLRT(alloc)(&own, 100);
    CHECK(p != NULL && ((uintptr_t)p % _Alignof(max_align_t)) == 0);
    CHECK(counted_allocs == 1 && atomic_load(&own.live_bytes) == counted_bytes && counted_bytes > 100);
    memset(p, 0xAB, 100);
    cplrt_w t = 0;
    CHECK(CPLRT(text_make)(&own, "hello", 5, &t) == CPLRT_OK && counted_allocs == 2);
    CHECK(atomic_load(&own.live_objects) == 1u);
    uint64_t peak = atomic_load(&own.peak_bytes);
    CHECK(peak == atomic_load(&own.live_bytes));
    CPLRT(free)(p);
    cplrt_release(t);
    CHECK(counted_frees == 2 && counted_bytes == 0 && atomic_load(&own.live_bytes) == 0);
    CHECK(atomic_load(&own.peak_bytes) == peak && atomic_load(&own.live_objects) == 0u);
    CPLRT(free)(NULL);
    /* the heap budget (2.4): an allocation that would exceed it fails and charges nothing */
    own.max_heap_bytes = 256;
    void *q = CPLRT(alloc)(&own, 200);
    CHECK(q != NULL);
    uint64_t live = atomic_load(&own.live_bytes);
    CHECK(CPLRT(alloc)(&own, 200) == NULL && atomic_load(&own.live_bytes) == live);
    CHECK(CPLRT(alloc)(&own, SIZE_MAX - 8u) == NULL);
    CPLRT(free)(q);
    CHECK(atomic_load(&own.live_bytes) == 0);
    /* every builder reports the failure and changes nothing */
    own.max_heap_bytes = 1;
    cplrt_w keep = cplrt_small(7);
    CHECK(CPLRT(text_make)(&own, "x", 1, &keep) == CPLRT_NOMEM && keep == cplrt_small(7));
    CHECK(CPLRT(node_make)(&own, CPLRT_LIST, 0, 0, &keep, 1, &keep) == CPLRT_NOMEM && keep == cplrt_small(7));
    CHECK(CPLRT(bytes_make)(&own, (const uint8_t *)"x", 1, &keep) == CPLRT_NOMEM);
    CHECK(CPLRT(int_from_i64)(&own, INT64_MIN, &keep) == CPLRT_NOMEM && keep == cplrt_small(7));
    cplrt_cx cx;
    cplrt_memory_record memory;
    CHECK(CPLRT(cx_begin)(&cx, &own, &memory, UINT32_MAX, 1, 0, 0, NULL) == CPLRT_OK);
    CHECK(cplrt_list_new(&cx, &keep, 1, &keep) == CPLRT_FAULT && keep == cplrt_small(7));
    CHECK(cx.fault != NULL && cx.fault->kind == CPLRT_FK_RESOURCE_EXHAUSTION && cx.fault->resource == CPLRT_RES_MEMORY);
    CPLRT(cx_end)(&cx);
    CHECK(rttest_runtime_fini(&own) == 0);
}

/* ---- the handle table (4.5) ------------------------------------------------------------------------------------- */

static void test_handles(void)
{
    cplrt_runtime own;
    rttest_runtime_init(&own, 0);
    cplrt_w t = 0;
    CHECK(CPLRT(text_make)(&own, "held", 4, &t) == CPLRT_OK);
    cplrt_slot entry = { CPLRT_SLOT_VALUE, 0, { .value = t } };
    uint64_t h1[2], h2[2];
    CHECK(CPLRT(handle_put)(&own, &entry, h1) == CPLRT_OK); /* the reference passes to the table */
    CHECK(h1[1] == (uint64_t)(uintptr_t)own.identity && own.live_handles == 1);
    cplrt_slot got;
    CHECK(CPLRT(handle_get)(&own, h1, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_OK && got.u.value == t);
    CHECK(rc(t) == 2u);
    CPLRT(slot_unref)(&got);
    CHECK(rc(t) == 1u);
    /* the check order of 4.5: null, identity, slot, generation, then the kind */
    const uint64_t null[2] = { 0, 0 }, foreign[2] = { h1[0], h1[1] + 16u }, nosuch[2] = { 99u, h1[1] };
    CHECK(CPLRT(handle_get)(&own, null, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_NULL);
    CHECK(CPLRT(handle_get)(&own, foreign, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_FOREIGN);
    CHECK(CPLRT(handle_get)(&own, nosuch, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_INVALID);
    CHECK(CPLRT(handle_get)(&own, h1, CPLRT_SLOT_CAP, &got) == CPLRT_HANDLE_INVALID);
    CHECK(CPLRT(handle_release)(&own, h1, CPLRT_SLOT_CANCEL) == CPLRT_HANDLE_INVALID);
    CHECK(CPLRT(handle_release)(&own, null, CPLRT_SLOT_VALUE) == CPLRT_HANDLE_NULL);
    cplrt_retain(t);
    CHECK(CPLRT(handle_release)(&own, h1, CPLRT_SLOT_VALUE) == CPLRT_HANDLE_OK && rc(t) == 1u);
    CHECK(own.live_handles == 0);
    /* a second release and any use after release are "released", for the life of the runtime */
    CHECK(CPLRT(handle_release)(&own, h1, CPLRT_SLOT_VALUE) == CPLRT_HANDLE_RELEASED);
    CHECK(CPLRT(handle_get)(&own, h1, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_RELEASED);
    /* free slots are reused first in, first out, with an advanced generation */
    CHECK(CPLRT(handle_put)(&own, &entry, h2) == CPLRT_OK);
    cplrt_retain(t);
    CHECK((uint32_t)h2[0] == (uint32_t)h1[0] && (h2[0] >> 32) == (h1[0] >> 32) + 1u);
    CHECK(CPLRT(handle_get)(&own, h1, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_RELEASED);
    uint64_t more[40][2];
    for (int i = 0; i < 40; i++) {
        cplrt_retain(t);
        CHECK(CPLRT(handle_put)(&own, &entry, more[i]) == CPLRT_OK);
    }
    CHECK(own.live_handles == 41 && own.nslots == 41 && own.slots_cap >= 41);
    CHECK(CPLRT(handle_release)(&own, more[3], CPLRT_SLOT_VALUE) == CPLRT_HANDLE_OK);
    CHECK(CPLRT(handle_release)(&own, more[7], CPLRT_SLOT_VALUE) == CPLRT_HANDLE_OK);
    uint64_t r1[2], r2[2];
    cplrt_retain(t);
    CHECK(CPLRT(handle_put)(&own, &entry, r1) == CPLRT_OK && (uint32_t)r1[0] == (uint32_t)more[3][0]);
    cplrt_retain(t);
    CHECK(CPLRT(handle_put)(&own, &entry, r2) == CPLRT_OK && (uint32_t)r2[0] == (uint32_t)more[7][0]);
    /* a slot whose generation would wrap is retired */
    uint32_t slot = (uint32_t)r1[0];
    own.slots[slot].generation = UINT32_MAX;
    uint64_t last[2] = { ((uint64_t)UINT32_MAX << 32) | slot, r1[1] };
    CHECK(CPLRT(handle_release)(&own, last, CPLRT_SLOT_VALUE) == CPLRT_HANDLE_OK);
    CHECK(own.slots[slot].kind == CPLRT_SLOT_RETIRED);
    CHECK(CPLRT(handle_get)(&own, last, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_RELEASED);
    uint64_t r3[2];
    cplrt_retain(t);
    CHECK(CPLRT(handle_put)(&own, &entry, r3) == CPLRT_OK && (uint32_t)r3[0] != slot);
    /* borrowed slots of a callback's arguments (5.7) */
    uint32_t reserved[2];
    uint32_t before = own.nslots;
    CHECK(CPLRT(handle_reserve)(&own, 2, reserved) == CPLRT_OK && own.live_handles == 41);
    cpl_value bh = CPLRT(handle_borrow)(&own, reserved[0], t);
    CHECK(bh.bits[1] == (uint64_t)(uintptr_t)own.identity && (uint32_t)bh.bits[0] == reserved[0]);
    CHECK(CPLRT(handle_get)(&own, bh.bits, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_OK && got.u.value == t);
    CPLRT(slot_unref)(&got);
    CHECK(CPLRT(handle_release)(&own, bh.bits, CPLRT_SLOT_VALUE) == CPLRT_HANDLE_INVALID);
    uint64_t unborrowed[2] = { ((uint64_t)own.slots[reserved[1]].generation << 32) | reserved[1], bh.bits[1] };
    CHECK(CPLRT(handle_get)(&own, unborrowed, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_INVALID);
    CPLRT(handle_unreserve)(&own, reserved, 2);
    CHECK(CPLRT(handle_get)(&own, bh.bits, CPLRT_SLOT_VALUE, &got) == CPLRT_HANDLE_RELEASED);
    CHECK(own.nslots >= before);
    /* a table that cannot grow: CPLRT_NOMEM, and the reference stays the caller's */
    own.max_heap_bytes = atomic_load(&own.live_bytes);
    uint64_t full[2];
    int st = CPLRT_OK;
    uint32_t k = 0;
    for (; k < 1000 && st == CPLRT_OK; k++) {
        cplrt_retain(t);
        st = CPLRT(handle_put)(&own, &entry, full);
        if (st != CPLRT_OK)
            cplrt_release(t);
    }
    CHECK(st == CPLRT_NOMEM);
    uint32_t want[3];
    CHECK(CPLRT(handle_reserve)(&own, 3, want) == CPLRT_NOMEM);
    own.max_heap_bytes = 0;
    cplrt_release(t);
    CHECK(rttest_runtime_fini(&own) == 0);
}

int main(void)
{
    rttest_runtime_init(&rt, 0);
    test_words();
    test_counts_and_immortals();
    test_builders();
    test_concat();
    test_equality();
    test_allocator();
    test_handles();
    uint64_t left = rttest_runtime_fini(&rt);
    CHECK(left == 0);
    printf("test_value: %d checks, %d failed\n", checks, failures);
    return failures == 0 ? 0 : 1;
}
