/* fixture: cboundary */
/* host_threads.c: one runtime shared by threads (spec/toolchain.md 12.7, spec/cabi.md 4.4): eight threads call
   exports concurrently on shared input handles, through one shared granted capability and one shared host
   capability; their results are released on other threads; cpl_cancel_request comes from another thread while calls
   that carry the token run. threads.rs runs it under ThreadSanitizer (setarch -R), c_hosts.rs in the other
   configurations. */
#include <pthread.h>
#include <stdatomic.h>
#include "host_common.h"
#include "cboundary.h"

#define THREADS 8
#define ROUNDS 50

static cpl_runtime *rt;
static cpl_value items, size, page_id;     /* shared inputs, borrowed by every call */
static cpl_cap codec, db;                  /* a shared grant and a shared host capability */
static cpl_cancel token;
static cpl_value results[THREADS][ROUNDS * 3];
static atomic_int calls, cancelled;

static cpl_status get(void *user, cpl_call *call, cpl_value id, cpl_value *result)
{
    (void)user;
    atomic_fetch_add(&calls, 1);
    cpl_runtime *r = cpl_call_runtime(call);
    cpl_value text = { { 0, 0 } }, page = { { 0, 0 } };
    cpl_status st = cpl_text_new(r, "shared", 6, &text, NULL);
    cpl_value fields[2] = { id, text };
    if (st == CPL_OK)
        st = cpl_struct_new(r, cboundary_type_page(), fields, 2, &page, NULL);
    if (st == CPL_OK)
        st = cpl_result_ok(r, page, result, NULL);
    cpl_value_release(r, page);
    cpl_value_release(r, text);
    return st;
}

static void *worker(void *p)
{
    int me = (int)(intptr_t)p;
    static const uint8_t image[12] = { 'I', 'M', 'G', 'T', 0x80, 0x02, 0, 0, 0xe0, 0x01, 0, 0 };
    cpl_call_options co = CPL_CALL_OPTIONS_INIT;
    co.cancel = token;
    for (int r = 0; r < ROUNDS; r++) {
        cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } };
        cpl_status st = cboundary_paginate_ints(rt, items, size, &ok, &bad, me % 2 ? &co : NULL, NULL);
        if (st == CPL_E_CANCELLED)
            atomic_fetch_add(&cancelled, 1);
        else
            CHECK(st == CPL_OK);
        results[me][3 * r] = ok;
        ok = (cpl_value){ { 0, 0 } };
        CHECK(cboundary_image_width(rt, image, sizeof image, codec, &ok, &bad, NULL, NULL) == CPL_OK);
        results[me][3 * r + 1] = ok;
        ok = (cpl_value){ { 0, 0 } };
        CHECK(cboundary_load_text(rt, page_id, db, &ok, &bad, NULL, NULL) == CPL_OK);
        results[me][3 * r + 2] = ok;
    }
    return NULL;
}

/* Releases the results another thread produced. */
static void *releaser(void *p)
{
    int of = ((int)(intptr_t)p + 1) % THREADS;
    for (int i = 0; i < ROUNDS * 3; i++)
        CHECK(cpl_value_release(rt, results[of][i]) == CPL_OK);
    return NULL;
}

static void *canceller(void *p)
{
    (void)p;
    CHECK(cpl_cancel_request(rt, token) == CPL_OK);
    return NULL;
}

int main(void)
{
    CHECK(cboundary_runtime_open(NULL, &rt, NULL) == CPL_OK);
    static const int64_t xs[10] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 };
    CHECK(cpl_list_from_i64(rt, xs, 10, &items, NULL) == CPL_OK);
    CHECK(cpl_int_from_i64(rt, 3, &size, NULL) == CPL_OK);
    cpl_value t = { { 0, 0 } };
    CHECK(cpl_text_new(rt, "p", 1, &t, NULL) == CPL_OK);
    CHECK(cpl_wrapper_new(rt, cboundary_type_page_id(), t, &page_id, NULL) == CPL_OK);
    cpl_value_release(rt, t);
    CHECK(cboundary_bind_image_codec(rt, &codec, NULL) == CPL_OK);
    cboundary_page_read_ops ops = { sizeof(cboundary_page_read_ops), 0, get, NULL };
    CHECK(cboundary_page_read_new(rt, &ops, NULL, &db, NULL) == CPL_OK);
    CHECK(cpl_cancel_new(rt, &token, NULL) == CPL_OK);

    pthread_t th[THREADS], cancel_thread;
    for (int i = 0; i < THREADS; i++)
        CHECK(pthread_create(&th[i], NULL, worker, (void *)(intptr_t)i) == 0);
    CHECK(pthread_create(&cancel_thread, NULL, canceller, NULL) == 0);
    for (int i = 0; i < THREADS; i++)
        CHECK(pthread_join(th[i], NULL) == 0);
    CHECK(pthread_join(cancel_thread, NULL) == 0);
    CHECK(atomic_load(&calls) == THREADS * ROUNDS);
    for (int i = 0; i < THREADS; i++)
        CHECK(pthread_create(&th[i], NULL, releaser, (void *)(intptr_t)i) == 0);
    for (int i = 0; i < THREADS; i++)
        CHECK(pthread_join(th[i], NULL) == 0);
    printf("{\"threads\":%d,\"rounds\":%d,\"cancelled\":%d}\n", THREADS, ROUNDS, atomic_load(&cancelled));

    /* after the request, a call carrying the token observes it */
    cpl_call_options co = CPL_CALL_OPTIONS_INIT;
    co.cancel = token;
    cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, "reader.pages.paginate_ints");
    line_arg(&l, items);
    line_arg(&l, size);
    cpl_status st = cboundary_paginate_ints(rt, items, size, &ok, &bad, &co, &err);
    CHECK(st == CPL_E_CANCELLED);
    line_error(&l, st, err, "interop_cancelled");

    CHECK(cpl_cancel_release(rt, token) == CPL_OK);
    CHECK(cpl_cap_release(rt, codec) == CPL_OK);
    CHECK(cpl_cap_release(rt, db) == CPL_OK);
    cpl_value_release(rt, items);
    cpl_value_release(rt, size);
    cpl_value_release(rt, page_id);
    return host_finish(rt);
}
