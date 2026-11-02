/* fixture: cboundary */
/* host_ownership.c: ownership at the C boundary (spec/toolchain.md 12.7, spec/cabi.md 4.3, 4.5): dup and release
   pairs; a double release, a use after release, a handle of a second runtime of the same package while that runtime
   is open and after it closed, a null output (each its documented interop_* code, nothing freed or leaked); views
   valid until release; a release from another thread; cpl_runtime_close refusing live handles
   (interop_runtime_in_use) and then CPL_CLOSE_FORCE; a nested Option 10^5 deep built and released on a 128 KiB thread
   (the release is iterative); error objects released after the close. */
#include <pthread.h>
#include "host_common.h"
#include "cboundary.h"

static cpl_status divide(cpl_runtime *rt, cpl_value a, cpl_value b, cpl_value *out, cpl_error **err)
{
    return cboundary_divide_exactly(rt, a, b, out, NULL, err);
}

/* An export call's refusal: its status and code, its parameter, and nothing written to the output. */
static void refused(cpl_status st, cpl_error *err, cpl_status want_st, const char *code, const char *parameter,
                    cpl_value out)
{
    CHECK(st == want_st);
    CHECK(err != NULL);
    if (err != NULL) {
        CHECK(strcmp(cpl_error_code(err), code) == 0);
        cpl_strview j = cpl_error_json(err);
        if (parameter != NULL) {
            char want[64];
            snprintf(want, sizeof want, "\"parameter\":\"%s\"", parameter);
            CHECK(strstr(j.ptr, want) != NULL);
        }
        printf("{\"refused\":%.*s}\n", (int)j.len, j.ptr);
        cpl_error_release(err);
    }
    CHECK(out.bits[0] == 0 && out.bits[1] == 0);
}

struct release_job {
    cpl_runtime *rt;
    cpl_value v;
    cpl_status st;
};

static void *release_on_thread(void *p)
{
    struct release_job *j = (struct release_job *)p;
    j->st = cpl_value_release(j->rt, j->v);
    return NULL;
}

/* A nested Option 10^5 deep, built and released on a thread with a 128 KiB stack. */
struct deep_job {
    cpl_runtime *rt;
    int ok;
};

static void *deep(void *p)
{
    struct deep_job *j = (struct deep_job *)p;
    cpl_value v = { { 0, 0 } };
    j->ok = cpl_int_from_i64(j->rt, 1, &v, NULL) == CPL_OK;
    for (int i = 0; i < 100000 && j->ok; i++) {
        cpl_value outer = { { 0, 0 } };
        j->ok = cpl_option_some(j->rt, v, &outer, NULL) == CPL_OK;
        j->ok = j->ok && cpl_value_release(j->rt, v) == CPL_OK;
        v = outer;
    }
    j->ok = j->ok && cpl_value_release(j->rt, v) == CPL_OK;
    return NULL;
}

int main(void)
{
    cpl_runtime *rt = NULL, *other = NULL;
    cpl_error *err = NULL;
    CHECK(cboundary_runtime_open(NULL, &rt, &err) == CPL_OK);
    CHECK(cboundary_runtime_open(NULL, &other, &err) == CPL_OK);
    cpl_value a = { { 0, 0 } }, b = { { 0, 0 } }, out = { { 0, 0 } };
    CHECK(cpl_int_from_i64(rt, 6, &a, NULL) == CPL_OK && cpl_int_from_i64(rt, 3, &b, NULL) == CPL_OK);

    /* dup and release pairs: a dup is its own handle */
    cpl_value d = { { 0, 0 } };
    CHECK(cpl_value_dup(rt, a, &d, NULL) == CPL_OK);
    CHECK(d.bits[0] != a.bits[0]);
    CHECK(cpl_value_release(rt, d) == CPL_OK);
    int64_t n = 0;
    CHECK(cpl_int_to_i64(rt, a, &n, NULL) == CPL_OK && n == 6);   /* the original lives on */

    /* a double release, and a use after release */
    CHECK(cpl_value_dup(rt, a, &d, NULL) == CPL_OK);
    CHECK(cpl_value_release(rt, d) == CPL_OK);
    CHECK(cpl_value_release(rt, d) == CPL_E_ARGUMENT);
    CHECK(cpl_value_release(rt, (cpl_value){ { 0, 0 } }) == CPL_OK);   /* "no value": a no-op */
    cpl_status st = divide(rt, d, b, &out, &err);
    refused(st, err, CPL_E_ARGUMENT, "interop_handle_released", "a", out);
    err = NULL;

    /* a handle of a second runtime of the same package, while it is open */
    cpl_value foreign = { { 0, 0 } };
    CHECK(cpl_int_from_i64(other, 6, &foreign, NULL) == CPL_OK);
    st = divide(rt, foreign, b, &out, &err);
    refused(st, err, CPL_E_ARGUMENT, "interop_handle_foreign", "a", out);
    err = NULL;
    CHECK(cpl_value_release(rt, foreign) == CPL_E_ARGUMENT);        /* refused, and not freed */
    CHECK(cpl_value_release(other, foreign) == CPL_OK);
    /* and after it was closed */
    cpl_value late = { { 0, 0 } };
    CHECK(cpl_int_from_i64(other, 9, &late, NULL) == CPL_OK);
    CHECK(cpl_runtime_close(other, CPL_CLOSE_FORCE, NULL) == CPL_OK);
    st = divide(rt, a, late, &out, &err);
    refused(st, err, CPL_E_ARGUMENT, "interop_handle_foreign", "b", out);
    err = NULL;

    /* a null output */
    st = cboundary_divide_exactly(rt, a, b, NULL, NULL, &err);
    refused(st, err, CPL_E_ARGUMENT, "interop_null_pointer", "out", out);
    err = NULL;

    /* the call still works, and its line is the evaluator's */
    line l;
    line_call(&l, rt, "reader.pages.divide_exactly");
    line_arg(&l, a);
    line_arg(&l, b);
    st = divide(rt, a, b, &out, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK)
        line_value(&l, "value", out);
    else
        line_error(&l, st, err, NULL);
    err = NULL;

    /* views are valid until their handle is released */
    cpl_value t = { { 0, 0 } };
    CHECK(cpl_text_new(rt, "h\0llo", 5, &t, NULL) == CPL_OK);
    cpl_strview view = { NULL, 0 };
    CHECK(cpl_text_view(rt, t, &view, NULL) == CPL_OK && view.len == 5 && view.ptr[5] == '\0');
    size_t scalars = 0;
    CHECK(cpl_text_scalars(rt, t, &scalars, NULL) == CPL_OK && scalars == 5);
    cpl_value t2 = { { 0, 0 } };
    CHECK(cpl_value_dup(rt, t, &t2, NULL) == CPL_OK);
    CHECK(cpl_value_release(rt, t) == CPL_OK);
    CHECK(memcmp(view.ptr, "h\0llo", 5) == 0);                       /* still valid: t2 holds the object */
    static const uint8_t raw[4] = { 0, 1, 254, 255 };
    cpl_value bytes = { { 0, 0 } };
    cpl_bytesview bv = { NULL, 0 };
    CHECK(cpl_bytes_new(rt, raw, sizeof raw, &bytes, NULL) == CPL_OK);
    CHECK(cpl_bytes_view(rt, bytes, &bv, NULL) == CPL_OK && bv.len == 4 && memcmp(bv.ptr, raw, 4) == 0);
    CHECK(cpl_value_release(rt, bytes) == CPL_OK);

    /* a release from another thread */
    struct release_job job = { rt, t2, CPL_E_INTERNAL };
    pthread_t th;
    CHECK(pthread_create(&th, NULL, release_on_thread, &job) == 0);
    CHECK(pthread_join(th, NULL) == 0);
    CHECK(job.st == CPL_OK);

    /* a nested Option 10^5 deep on a 128 KiB thread: building and releasing it needs no recursion */
    struct deep_job dj = { rt, 0 };
    pthread_attr_t attr;
    CHECK(pthread_attr_init(&attr) == 0);
    CHECK(pthread_attr_setstacksize(&attr, 128u * 1024u) == 0);
    CHECK(pthread_create(&th, &attr, deep, &dj) == 0);
    CHECK(pthread_join(th, NULL) == 0);
    CHECK(pthread_attr_destroy(&attr) == 0);
    CHECK(dj.ok);

    /* close refuses live handles, then CPL_CLOSE_FORCE; an error object outlives its runtime */
    cpl_error *in_use = NULL;
    CHECK(cpl_runtime_close(rt, 0, &in_use) == CPL_E_STATE);
    CHECK(in_use != NULL && strcmp(cpl_error_code(in_use), "interop_runtime_in_use") == 0);
    cpl_stats stats = CPL_STATS_INIT;
    cpl_runtime_stats(rt, &stats);
    CHECK(stats.live_handles == 3);                                  /* a, b, out */
    CHECK(cpl_value_release(rt, out) == CPL_OK);
    CHECK(cpl_runtime_close(rt, CPL_CLOSE_FORCE, NULL) == CPL_OK);   /* a and b are freed by the forced close */
    cpl_strview j = cpl_error_json(in_use);
    CHECK(strstr(j.ptr, "\"live_handles\":3") != NULL);
    printf("{\"close\":%.*s}\n", (int)j.len, j.ptr);
    cpl_error_release(in_use);
    printf("%s: %d failures\n", failures ? "FAILED" : "ok", failures);
    return failures != 0;
}
