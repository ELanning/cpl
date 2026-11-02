/* fixture: cboundary */
/* host_oom.c: allocation failure at the C boundary (spec/toolchain.md 12.7, spec/cabi.md 2.4, 4.7): a counting
   allocator whose k-th allocation during three calls fails, swept by oom.rs over every k; each call ends CPL_OK or
   CPL_ERR (its outcome), CPL_FAULT with resource "memory", or CPL_E_NOMEM, never a crash, and every allocation is
   freed by the close. With max_heap_bytes the same paths are a budget, which gives the same outcome on every run.

     host_oom [count]        no failure: the calls' lines, then {"allocations": n, "base_bytes": b}
     host_oom fail <k>       allocation k of the calls (from 0) fails
     host_oom budget <bytes> max_heap_bytes = bytes; prints each call's status */
#include <stdatomic.h>
#include <stdlib.h>
#include "host_common.h"
#include "cboundary.h"

struct counting {
    atomic_long next;       /* the index of the next allocation of the calls */
    atomic_int armed;
    long fail_at;           /* -1: never */
    atomic_long live;       /* allocations not yet freed */
};

static void *count_alloc(void *user, size_t size, size_t align)
{
    struct counting *c = (struct counting *)user;
    if (atomic_load(&c->armed)) {
        long i = atomic_fetch_add(&c->next, 1);
        if (i == c->fail_at)
            return NULL;
    }
    void *p = align <= _Alignof(max_align_t) ? malloc(size == 0 ? 1 : size)
                                             : aligned_alloc(align, (size + align - 1) / align * align);
    if (p != NULL)
        atomic_fetch_add(&c->live, 1);
    return p;
}

static void count_free(void *user, void *ptr, size_t size, size_t align)
{
    (void)size;
    (void)align;
    if (ptr != NULL) {
        atomic_fetch_sub(&((struct counting *)user)->live, 1);
        free(ptr);
    }
}

/* A call's outcome: its expected status, or memory exhaustion in either of its two forms. */
static void judge(const char *what, cpl_status st, cpl_status expected, cpl_error *err, int print_status)
{
    int memory = 0;
    if (st == CPL_FAULT && err != NULL) {
        cpl_strview j = cpl_error_json(err);
        memory = strcmp(cpl_error_code(err), "resource_exhaustion") == 0 && strstr(j.ptr, "\"resource\":\"memory\"");
    }
    int ok = st == expected || memory || st == CPL_E_NOMEM;
    if (!ok)
        printf("%s: unexpected status %d\n", what, (int)st);
    CHECK(ok);
    if (print_status)
        printf("{\"%s\":%d}\n", what, (int)st);
}

int main(int argc, char **argv)
{
    const char *mode = argc > 1 ? argv[1] : "count";
    long arg = argc > 2 ? atol(argv[2]) : -1;
    struct counting c;
    atomic_init(&c.next, 0);
    atomic_init(&c.armed, 0);
    atomic_init(&c.live, 0);
    c.fail_at = strcmp(mode, "fail") == 0 ? arg : -1;
    cpl_allocator alloc = { sizeof(cpl_allocator), 0, &c, count_alloc, count_free };
    cpl_runtime_options o = CPL_RUNTIME_OPTIONS_INIT;
    o.allocator = &alloc;
    if (strcmp(mode, "budget") == 0)
        o.max_heap_bytes = (uint64_t)arg;
    cpl_runtime *rt = NULL;
    CHECK(cboundary_runtime_open(&o, &rt, NULL) == CPL_OK);
    if (rt == NULL)
        return 1;

    /* the inputs, before the sweep */
    static const int64_t xs[20] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20 };
    cpl_value items = { { 0, 0 } }, size = { { 0, 0 } }, t = { { 0, 0 } }, id = { { 0, 0 } }, page = { { 0, 0 } };
    int setup = cpl_list_from_i64(rt, xs, 20, &items, NULL) == CPL_OK && cpl_int_from_i64(rt, 3, &size, NULL) == CPL_OK &&
                cpl_text_new(rt, "  a page  ", 10, &t, NULL) == CPL_OK &&
                cpl_wrapper_new(rt, cboundary_type_page_id(), t, &id, NULL) == CPL_OK;
    cpl_value fields[2] = { id, t };
    setup = setup && cpl_struct_new(rt, cboundary_type_page(), fields, 2, &page, NULL) == CPL_OK;
    cpl_stats before = CPL_STATS_INIT;
    cpl_runtime_stats(rt, &before);
    int print = strcmp(mode, "budget") == 0;
    if (!setup) {
        printf("{\"setup\":\"out of memory\"}\n");      /* a budget below the inputs */
    } else {
        atomic_store(&c.armed, 1);
        cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } }, out = { { 0, 0 } };
        cpl_error *err = NULL;
        line l;
        int lines = strcmp(mode, "count") == 0;
        if (lines) {
            line_call(&l, rt, "reader.pages.paginate_ints");
            line_arg(&l, items);
            line_arg(&l, size);
        }
        cpl_status st = cboundary_paginate_ints(rt, items, size, &ok, &bad, NULL, &err);
        if (lines)
            line_value(&l, "ok", ok);
        judge("paginate_ints", st, CPL_OK, err, print);
        cpl_error_release(err);
        cpl_value_release(rt, ok);
        cpl_value_release(rt, bad);
        ok = bad = (cpl_value){ { 0, 0 } };
        err = NULL;
        st = cboundary_positive_size(rt, "  77 ", 5, &ok, &bad, NULL, &err);
        judge("positive_size", st, CPL_OK, err, print);
        cpl_error_release(err);
        cpl_value_release(rt, ok);
        cpl_value_release(rt, bad);
        err = NULL;
        st = cboundary_clean(rt, page, &out, NULL, &err);
        judge("clean", st, CPL_OK, err, print);
        cpl_error_release(err);
        cpl_value_release(rt, out);
        atomic_store(&c.armed, 0);
        if (strcmp(mode, "count") == 0)
            printf("{\"allocations\":%ld,\"base_bytes\":%llu}\n", atomic_load(&c.next),
                   (unsigned long long)before.live_bytes);
    }
    cpl_value_release(rt, page);
    cpl_value_release(rt, id);
    cpl_value_release(rt, t);
    cpl_value_release(rt, size);
    cpl_value_release(rt, items);
    int status = host_finish(rt);
    CHECK(atomic_load(&c.live) == 0);                        /* every allocation was freed */
    if (atomic_load(&c.live) != 0)
        printf("leaked %ld allocations\n", atomic_load(&c.live));
    return status || atomic_load(&c.live) != 0;
}
