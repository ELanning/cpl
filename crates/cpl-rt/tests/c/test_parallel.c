/* Structured scheduling, shared captures, nested joins, ownership reuse, cancellation, and memory budgets. */
#define _GNU_SOURCE 1
#define CPLRT_NS rttest
#define CPLRT(name) cplrt_rttest__##name
#define CPLPKG(name) cplpkg_rttest__##name
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdio.h>
#define CHECK(c) do { if (!(c)) { fprintf(stderr, "check failed at line %d: %s\n", __LINE__, #c); abort(); } } while (0)
#include <stdlib.h>
#include <time.h>
void rttest_runtime_init(cplrt_runtime *, uint64_t);
uint64_t rttest_runtime_fini(cplrt_runtime *);
static _Atomic int active, peak;

static int task(cplrt_cx *cx, cplrt_w item, const cplrt_w *captures, cplrt_w *out)
{
    int now = atomic_fetch_add(&active, 1) + 1;
    int before = atomic_load(&peak);
    while (now > before && !atomic_compare_exchange_weak(&peak, &before, now)) { }
    struct timespec pause = { 0, 1000000 };
    nanosleep(&pause, NULL);
    cplrt_w xs = 0;
    CPLRT_COPY(xs, captures[0]);
    int status = CPLRT_OK;
    for (int i = 0; i < 20; i++) {
        if (cplrt_list_push_owned(cx, CPLRT_NO_SITE, CPLRT_NONE, &xs, item) != CPLRT_OK) {
            status = CPLRT_FAULT;
            break;
        }
    }
    if (status == CPLRT_OK) CPLRT_MOVE(*out, xs);
    cplrt_release(xs);
    atomic_fetch_sub(&active, 1);
    return status;
}

static int nested(cplrt_cx *cx, cplrt_w item, const cplrt_w *captures, cplrt_w *out)
{
    (void)item;
    return cplrt_parallel_map(cx, CPLRT_NO_SITE, captures[1], task, captures, out);
}

static void checkpoints(cplrt_runtime *rt)
{
    cplrt_cx cx;
    cplrt_memory_record memory;
    _Atomic uint32_t cancel = 0, failure = UINT32_MAX;
    CHECK(CPLRT(cx_begin)(&cx, rt, &memory, 100, 1, 0, 0, &cancel) == CPLRT_OK);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_OK);
    cx.parallel_failure = &failure;
    cx.parallel_index = 5;
    atomic_store(&failure, 6);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_OK);
    atomic_store(&failure, 5);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_OK);
    atomic_store(&failure, 4);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_FAULT);
    CHECK(cx.cancelled && cx.fault == NULL);
    CPLRT(cx_end)(&cx);

    CHECK(CPLRT(cx_begin)(&cx, rt, &memory, 100, 1, 0, 0, &cancel) == CPLRT_OK);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_OK);
    atomic_store(&cancel, 1);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_FAULT);
    CHECK(cx.cancelled && cx.fault == NULL);
    CPLRT(cx_end)(&cx);

    CHECK(CPLRT(cx_begin)(&cx, rt, &memory, 100, 1, 0, 0, NULL) == CPLRT_OK);
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_OK);
    cx.deadline_ns = 1; /* expired, without relying on a wall-clock sleep */
    CHECK(cplrt_fast_tick(&cx, CPLRT_NO_SITE) == CPLRT_FAULT);
    CHECK(!cx.cancelled && cx.fault != NULL && cx.fault->resource == CPLRT_RES_TIME);
    CPLRT(cx_end)(&cx);
}

int main(void)
{
    CHECK(setenv("CPL_PARALLELISM", "4", 1) == 0);
    cplrt_runtime rt;
    rttest_runtime_init(&rt, 0);
    checkpoints(&rt);
    cplrt_w items[16], input = 0, original = 0;
    for (int i = 0; i < 16; i++) items[i] = cplrt_small(i);
    CHECK(CPLRT(node_make)(&rt, CPLRT_LIST, 0, 0, items, 16, &input) == CPLRT_OK);
    CHECK(CPLRT(node_make)(&rt, CPLRT_LIST, 0, 0, items, 2, &original) == CPLRT_OK);
    cplrt_w captures[2] = { original, original };
    uint64_t bytes = atomic_load(&rt.live_bytes), objects = atomic_load(&rt.live_objects);
    for (unsigned pass = 0; pass < 20; pass++) {
        cplrt_cx cx;
        cplrt_memory_record memory;
        cplrt_w result = 0;
        rt.max_heap_bytes = pass == 0 ? 0 : bytes + (uint64_t)(pass - 1) * 1024u;
        CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, 100, 1, 1000, 0, NULL) == CPLRT_OK);
        int st = cplrt_parallel_map(&cx, CPLRT_NO_SITE, input, pass == 0 ? nested : task, captures, &result);
        if (st == CPLRT_OK) {
            CHECK(cplrt_len(result) == 16);
        } else {
            CHECK(cx.fault != NULL);
            CHECK(result == 0);
        }
        CHECK(cplrt_len(original) == 2);
        CHECK(cplrt_item(original, 0) == cplrt_small(0));
        CPLRT(cx_end)(&cx);
        cplrt_release(result);
        CHECK(atomic_load(&rt.live_objects) == objects);
        CHECK(atomic_load(&rt.live_bytes) == bytes);
        CHECK(atomic_load(&active) == 0);
    }
    CHECK(atomic_load(&peak) >= 2 && atomic_load(&peak) <= 4);
    rt.max_heap_bytes = 0;
    cplrt_cx cx;
    cplrt_memory_record memory;
    _Atomic uint32_t cancel = 1;
    cplrt_w result = 0;
    CHECK(CPLRT(cx_begin)(&cx, &rt, &memory, 100, 1, 1000, 0, &cancel) == CPLRT_OK);
    CHECK(cplrt_parallel_map(&cx, CPLRT_NO_SITE, input, task, captures, &result) == CPLRT_FAULT);
    CHECK(cx.cancelled && result == 0);
    CPLRT(cx_end)(&cx);
    cplrt_release(input);
    cplrt_release(original);
    CHECK(rttest_runtime_fini(&rt) == 0);
    return 0;
}
