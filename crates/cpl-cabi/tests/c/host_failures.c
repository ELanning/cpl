/* fixture: cfaults */
/* host_failures.c: every fault of semantics 6.7 that a checked program can raise, through cfaults_* (spec/toolchain.md
   12.7), each compared with the evaluator: divide_exactly(7, 0) and (7, 2) (no caller), an ensures, an invariant, a
   loop and a header decreases, division_by_zero, unresolved_hole with bindings, assertion_failed in a called body
   claimed by a clause, the refinement_violation of a Positive parameter (with parameter and pointer),
   resource_exhaustion at max_call_depth 0, 3, and 10,000 (on a 256 MiB thread), the stack guard on a 256 KiB thread
   (depth present), a time budget on a loop that never ends, cancellation from another thread (CPL_E_CANCELLED), an
   operation's requires and ensures; and the boundary refusals interop_argument_type_mismatch (with a pointer),
   interop_utf8_invalid (with offset), interop_bool_out_of_range, interop_capability_mismatch, and CPL_E_RANGE from an
   _i64 export. */
#define _POSIX_C_SOURCE 200809L                       /* nanosleep under -std=c11: a host's own choice */
#include <pthread.h>
#include <time.h>
#include "host_common.h"
#include "cfaults.h"

typedef cpl_status (*fn1)(cpl_runtime *, cpl_value, cpl_value *, const cpl_call_options *, cpl_error **);
typedef cpl_status (*fn2)(cpl_runtime *, cpl_value, cpl_value, cpl_value *, const cpl_call_options *, cpl_error **);

static cpl_value int_of(cpl_runtime *rt, int64_t n)
{
    cpl_value v = { { 0, 0 } };
    CHECK(cpl_int_from_i64(rt, n, &v, NULL) == CPL_OK);
    return v;
}

/* The outcome of a non-split export into its line; the status. */
static cpl_status finish(line *l, cpl_runtime *rt, cpl_status st, cpl_value out, cpl_error *err, const char *expect)
{
    if (st == CPL_OK)
        line_value(l, "value", out);
    else
        line_error(l, st, err, expect);
    cpl_value_release(rt, out);
    return st;
}

static cpl_status call1(cpl_runtime *rt, const char *name, fn1 f, int64_t a, const char *config,
                        const cpl_call_options *options)
{
    cpl_value va = int_of(rt, a), out = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, name);
    line_arg(&l, va);
    if (config != NULL)
        line_config(&l, config);
    cpl_status st = f(rt, va, &out, options, &err);
    st = finish(&l, rt, st, out, err, NULL);
    cpl_value_release(rt, va);
    return st;
}

static cpl_status call2(cpl_runtime *rt, const char *name, fn2 f, int64_t a, int64_t b)
{
    cpl_value va = int_of(rt, a), vb = int_of(rt, b), out = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, name);
    line_arg(&l, va);
    line_arg(&l, vb);
    cpl_status st = f(rt, va, vb, &out, NULL, &err);
    st = finish(&l, rt, st, out, err, NULL);
    cpl_value_release(rt, va);
    cpl_value_release(rt, vb);
    return st;
}

/* depth(n) on a runtime with max_call_depth `depth`, on a thread of `stack` bytes. */
struct depth_job {
    uint32_t max_depth;
    int64_t n;
    size_t stack;
    cpl_status st;
};

static void *depth_thread(void *p)
{
    struct depth_job *j = (struct depth_job *)p;
    cpl_runtime_options o = CPL_RUNTIME_OPTIONS_INIT;
    o.max_call_depth = j->max_depth;
    cpl_runtime *rt = NULL;
    CHECK(cfaults_runtime_open(&o, &rt, NULL) == CPL_OK);
    char config[64];
    snprintf(config, sizeof config, "\"max_call_depth\":%u", (unsigned)j->max_depth);
    j->st = call1(rt, "cfaults.faults.depth", cfaults_depth, j->n, config, NULL);
    CHECK(host_close(rt));
    return NULL;
}

static cpl_status depth_on_thread(uint32_t max_depth, int64_t n, size_t stack)
{
    struct depth_job j = { max_depth, n, stack, CPL_E_INTERNAL };
    pthread_attr_t attr;
    pthread_t th;
    CHECK(pthread_attr_init(&attr) == 0);
    CHECK(pthread_attr_setstacksize(&attr, stack) == 0);
    CHECK(pthread_create(&th, &attr, depth_thread, &j) == 0);
    CHECK(pthread_join(th, NULL) == 0);
    CHECK(pthread_attr_destroy(&attr) == 0);
    return j.st;
}

/* cancellation: a thread that requests the token after a pause */
struct cancel_job {
    cpl_runtime *rt;
    cpl_cancel token;
};

static void *canceller(void *p)
{
    struct cancel_job *j = (struct cancel_job *)p;
    struct timespec ts = { 0, 50 * 1000 * 1000 };
    nanosleep(&ts, NULL);
    CHECK(cpl_cancel_request(j->rt, j->token) == CPL_OK);
    return NULL;
}

/* Counter.next answering `answer` */
static cpl_status next(void *user, cpl_call *call, cpl_value limit, cpl_value *result)
{
    (void)limit;
    return cpl_int_from_i64(cpl_call_runtime(call), *(int64_t *)user, result, NULL);
}

static cpl_status ping(void *user, cpl_call *call, cpl_value *result)
{
    (void)user;
    return cpl_int_from_i64(cpl_call_runtime(call), 1, result, NULL);
}

static void draw(cpl_runtime *rt, int64_t limit, int64_t answer, const char *answers)
{
    cfaults_counter_ops ops = { sizeof(cfaults_counter_ops), 0, next, NULL };
    cpl_cap c = { { 0, 0 } };
    CHECK(cfaults_counter_new(rt, &ops, &answer, &c, NULL) == CPL_OK);
    cpl_value vl = int_of(rt, limit), out = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, "cfaults.faults.draw");
    line_arg(&l, vl);
    line_using(&l, answers);
    cpl_status st = cfaults_draw(rt, vl, c, &out, NULL, &err);
    finish(&l, rt, st, out, err, NULL);
    cpl_value_release(rt, vl);
    CHECK(cpl_cap_release(rt, c) == CPL_OK);
}

int main(void)
{
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    CHECK(cfaults_runtime_open(NULL, &rt, &err) == CPL_OK);

    /* contract faults */
    CHECK(call2(rt, "cfaults.faults.divide_exactly", cfaults_divide_exactly, 7, 0) == CPL_FAULT);
    CHECK(call2(rt, "cfaults.faults.divide_exactly", cfaults_divide_exactly, 7, 2) == CPL_FAULT);
    CHECK(call2(rt, "cfaults.faults.divide_exactly", cfaults_divide_exactly, 8, 2) == CPL_OK);
    CHECK(call1(rt, "cfaults.faults.half", cfaults_half, 5, NULL, NULL) == CPL_FAULT);
    CHECK(call1(rt, "cfaults.faults.bad_loop", cfaults_bad_loop, 3, NULL, NULL) == CPL_FAULT);
    CHECK(call1(rt, "cfaults.faults.stuck", cfaults_stuck, 2, NULL, NULL) == CPL_FAULT);
    CHECK(call1(rt, "cfaults.faults.bad_rec", cfaults_bad_rec, 1, NULL, NULL) == CPL_FAULT);
    /* runtime faults */
    CHECK(call2(rt, "cfaults.faults.ratio", cfaults_ratio, 1, 0) == CPL_FAULT);
    CHECK(call1(rt, "cfaults.faults.guarded", cfaults_guarded, 0, NULL, NULL) == CPL_FAULT);
    CHECK(call1(rt, "cfaults.faults.twice", cfaults_twice, 0, NULL, NULL) == CPL_FAULT);
    CHECK(call1(rt, "cfaults.faults.twice", cfaults_twice, 4, NULL, NULL) == CPL_OK);

    /* unresolved_hole with bindings */
    cpl_value t = { { 0, 0 } }, id = { { 0, 0 } }, page = { { 0, 0 } }, out = { { 0, 0 } };
    CHECK(cpl_text_new(rt, "p", 1, &t, NULL) == CPL_OK);
    CHECK(cpl_wrapper_new(rt, cfaults_type_page_id(), t, &id, NULL) == CPL_OK);
    cpl_value fields[2] = { id, t };
    CHECK(cpl_struct_new(rt, cfaults_type_page(), fields, 2, &page, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, "cfaults.faults.summarize");
    line_arg(&l, page);
    cpl_status st = cfaults_summarize(rt, page, &out, NULL, &err);
    CHECK(st == CPL_FAULT);
    finish(&l, rt, st, out, err, NULL);
    err = NULL;
    cpl_value_release(rt, page);
    cpl_value_release(rt, id);
    cpl_value_release(rt, t);

    /* resource_exhaustion at max_call_depth 0, 3, and 10,000 (the last on a 256 MiB thread) */
    CHECK(depth_on_thread(0, 1, 1u << 20) == CPL_FAULT);
    CHECK(depth_on_thread(3, 5, 1u << 20) == CPL_FAULT);
    CHECK(depth_on_thread(3, 2, 1u << 20) == CPL_OK);
    CHECK(depth_on_thread(10000, 20000, 256u << 20) == CPL_FAULT);
    /* the stack guard on a 256 KiB thread: no depth limit, so the guard stops the recursion (depth present) */
    CHECK(depth_on_thread(4000000000u, 100000000, 256u << 10) == CPL_FAULT);

    /* a time budget on a loop that never ends */
    cpl_call_options co = CPL_CALL_OPTIONS_INIT;
    co.time_budget_ms = 100;
    CHECK(call1(rt, "cfaults.faults.spin", cfaults_spin, 0, "\"time_budget_ms\":100", &co) == CPL_FAULT);
    /* cancellation from another thread */
    struct cancel_job cj = { rt, { { 0, 0 } } };
    CHECK(cpl_cancel_new(rt, &cj.token, NULL) == CPL_OK);
    co = (cpl_call_options)CPL_CALL_OPTIONS_INIT;
    co.cancel = cj.token;
    pthread_t th;
    CHECK(pthread_create(&th, NULL, canceller, &cj) == 0);
    cpl_value zero = int_of(rt, 0);
    line_call(&l, rt, "cfaults.faults.spin");
    line_arg(&l, zero);
    st = cfaults_spin(rt, zero, &out, &co, &err);
    CHECK(pthread_join(th, NULL) == 0);
    CHECK(st == CPL_E_CANCELLED);
    finish(&l, rt, st, out, err, "interop_cancelled");
    err = NULL;
    cpl_value_release(rt, zero);
    CHECK(cpl_cancel_release(rt, cj.token) == CPL_OK);

    /* an operation's requires (the caller) and ensures (the binding) */
    draw(rt, 0, 0,
         "{\"label\":\"host\",\"answers\":[]}");
    draw(rt, 5, 7,
         "{\"label\":\"host\",\"answers\":[{\"op\":\"next\",\"args\":[5],\"result\":7}]}");
    draw(rt, 5, 3,
         "{\"label\":\"host\",\"answers\":[{\"op\":\"next\",\"args\":[5],\"result\":3}]}");

    /* boundary refusals */
    cpl_value one = int_of(rt, 1), x = { { 0, 0 } }, items = { { 0, 0 } };
    CHECK(cpl_text_new(rt, "x", 1, &x, NULL) == CPL_OK);
    cpl_value elems[2] = { one, x };
    CHECK(cpl_list_new(rt, elems, 2, &items, NULL) == CPL_OK);   /* elements are not checked against each other */
    line_call(&l, rt, "cfaults.faults.total");
    line_arg_json(&l, "null");
    st = cfaults_total(rt, items, &out, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT && err != NULL && strstr(cpl_error_json(err).ptr, "\"pointer\":\"/1\"") != NULL);
    finish(&l, rt, st, out, err, "interop_argument_type_mismatch");
    err = NULL;
    cpl_value_release(rt, items);
    cpl_value_release(rt, x);
    cpl_value_release(rt, one);

    line_call(&l, rt, "cfaults.faults.choose");
    line_arg_json(&l, "null");
    st = cfaults_choose(rt, 1, "a\xc3", 2, &out, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT && err != NULL && strstr(cpl_error_json(err).ptr, "\"offset\":1") != NULL);
    finish(&l, rt, st, out, err, "interop_utf8_invalid");
    err = NULL;
    line_call(&l, rt, "cfaults.faults.choose");
    line_arg_json(&l, "null");
    st = cfaults_choose(rt, 2, "a", 1, &out, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT);
    finish(&l, rt, st, out, err, "interop_bool_out_of_range");
    err = NULL;

    cfaults_other_ops oops = { sizeof(cfaults_other_ops), 0, ping, NULL };
    cpl_cap other = { { 0, 0 } };
    CHECK(cfaults_other_new(rt, &oops, NULL, &other, NULL) == CPL_OK);
    cpl_value five = int_of(rt, 5);
    line_call(&l, rt, "cfaults.faults.draw");
    line_arg(&l, five);
    st = cfaults_draw(rt, five, other, &out, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT);
    finish(&l, rt, st, out, err, "interop_capability_mismatch");
    err = NULL;
    cpl_value_release(rt, five);
    CHECK(cpl_cap_release(rt, other) == CPL_OK);

    /* CPL_E_RANGE from an _i64 export: 3000000^3 does not fit int64_t */
    int64_t cube = 0;
    line_call(&l, rt, "cfaults.faults.cube");
    line_arg_i64(&l, 3000000);
    st = cfaults_cube_i64(rt, 3000000, &cube, NULL, &err);
    CHECK(st == CPL_E_RANGE);
    line_error(&l, st, err, "interop_int_out_of_range");
    err = NULL;
    CHECK(cfaults_cube_i64(rt, -3, &cube, NULL, NULL) == CPL_OK && cube == -27);
    return host_finish(rt);
}
