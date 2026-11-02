/* host_common.h: what the host programs of the C host suites share (spec/toolchain.md 12.7): CHECK, and one JSON line
   per checked call, which c_hosts.rs compares with the evaluator's outcome of the same call:

     {"call": "<qualified name>", "args": [<value JSON>...], "using": [<capability>...], <outcome>}

   ("package": "<prefix>" first when the call is of another fixture than the first one the host names), 
   with a capability {"grant": "<binding>"} (the package's backend-c binding, a CAdapter on the evaluator's side) or
   {"label": "<label>", "answers": [{"op": "<name>", "args": [<value JSON>...], "result": <value JSON>} or
   {..., "foreign_error": {"kind", "message", "details": [[k, v]...]}}...]} (the host's callbacks with fixed answers,
   mirrored by a ScriptHost; a result of another type than the operation's names it with "type"), and an
   outcome "value": <value JSON> (a whole result), "ok" or "err": <value JSON> (the payload of a split Result),
   "fault": <the S49 diagnostic of a CPL_FAULT>, or "error": <an error object's JSON> with "expect": "<its code>" (a
   CPL_E_* status, checked field by field), or "protocol": <the S49 diagnostic of a C callback's binding_protocol
   fault, checked for the fields of semantics S80>. Lines of other shapes are not compared. */
#ifndef HOST_COMMON_H
#define HOST_COMMON_H 1
#include <stdio.h>
#include <string.h>
#include "cpl.h"

static int failures;
#define CHECK(c)                                                                                                       \
    do {                                                                                                               \
        if (!(c)) {                                                                                                    \
            failures++;                                                                                                \
            printf("%s:%d: CHECK(%s) failed\n", __FILE__, __LINE__, #c);                                               \
            fflush(stdout);                                                                                            \
        }                                                                                                              \
    } while (0)

/* Zero live handles, capabilities, and objects, then a clean close: 1 when both hold. */
static inline int host_close(cpl_runtime *rt)
{
    cpl_stats stats = CPL_STATS_INIT;
    cpl_runtime_stats(rt, &stats);
    int clean = stats.live_handles == 0 && stats.live_caps == 0 && stats.live_objects == 0;
    if (!clean)
        printf("live: %llu handles, %llu caps, %llu objects\n", (unsigned long long)stats.live_handles,
               (unsigned long long)stats.live_caps, (unsigned long long)stats.live_objects);
    return clean && cpl_runtime_close(rt, 0, NULL) == CPL_OK;
}

/* The end of main (spec/toolchain.md 12.7): host_close, then the summary line and the exit status. */
static inline int host_finish(cpl_runtime *rt)
{
    CHECK(host_close(rt));
    printf("%s: %d failures\n", failures ? "FAILED" : "ok", failures);
    return failures != 0;
}

/* A value's JSON, or null when it cannot be rendered. */
static inline void put_json(cpl_runtime *rt, cpl_value v)
{
    cpl_buf b = { NULL, 0 };
    if (cpl_value_to_json(rt, v, &b, NULL) == CPL_OK) {
        fwrite(b.ptr, 1, b.len, stdout);
        cpl_buf_release(rt, &b);
    } else {
        fputs("null", stdout);
    }
}

typedef struct line {
    cpl_runtime *rt;
    int nargs;
    int closed;
} line;

/* {"call":"<function>","args":[ */
static inline void line_call(line *l, cpl_runtime *rt, const char *function)
{
    l->rt = rt;
    l->nargs = 0;
    l->closed = 0;
    printf("{\"call\":\"%s\",\"args\":[", function);
}

/* {"package":"<prefix>","call":"<function>","args":[  (a call of another package than the host's first fixture) */
static inline void line_call_in(line *l, cpl_runtime *rt, const char *package, const char *function)
{
    l->rt = rt;
    l->nargs = 0;
    l->closed = 0;
    printf("{\"package\":\"%s\",\"call\":\"%s\",\"args\":[", package, function);
}

/* One argument as its value JSON. */
static inline void line_arg(line *l, cpl_value v)
{
    if (l->nargs++ > 0)
        putchar(',');
    put_json(l->rt, v);
}

/* One argument given as value JSON text. */
static inline void line_arg_json(line *l, const char *json)
{
    if (l->nargs++ > 0)
        putchar(',');
    fputs(json, stdout);
}

/* One Int argument. */
static inline void line_arg_i64(line *l, int64_t n)
{
    if (l->nargs++ > 0)
        putchar(',');
    printf("%lld", (long long)n);
}

static inline void line_close_args(line *l)
{
    if (!l->closed) {
        putchar(']');
        l->closed = 1;
    }
}

/* The configuration the call ran under, a JSON object's members ("max_call_depth": n, "time_budget_ms": n), which
   the evaluator's side of the comparison uses (call at most once, after the arguments and before the outcome). */
static inline void line_config(line *l, const char *members)
{
    line_close_args(l);
    printf(",\"config\":{%s}", members);
}

/* The using capabilities, a JSON array's elements (call at most once, after the arguments). */
static inline void line_using(line *l, const char *capabilities)
{
    line_close_args(l);
    printf(",\"using\":[%s]", capabilities);
}


/* ,"<key>":<value JSON>}  (key "value", "ok", or "err") */
static inline void line_value(line *l, const char *key, cpl_value v)
{
    line_close_args(l);
    printf(",\"%s\":", key);
    put_json(l->rt, v);
    puts("}");
    fflush(stdout);
}

/* ,"<key>":<value JSON text>}  (a Unit payload: {"$type":"Unit"}) */
static inline void line_value_json(line *l, const char *key, const char *json)
{
    line_close_args(l);
    printf(",\"%s\":%s}\n", key, json);
    fflush(stdout);
}

/* The outcome of a call that returned st with the error object e (released here): a fault, or an error object
   whose code must be `expect`. */
static inline void line_error(line *l, cpl_status st, cpl_error *e, const char *expect)
{
    line_close_args(l);
    cpl_strview j = { "null", 4 };
    if (e != NULL)
        j = cpl_error_json(e);
    if (st == CPL_FAULT)
        printf(",\"fault\":%.*s}\n", (int)j.len, j.ptr);
    else
        printf(",\"status\":%d,\"error\":%.*s,\"expect\":\"%s\"}\n", (int)st, (int)j.len, j.ptr,
               expect != NULL ? expect : "");
    fflush(stdout);
    cpl_error_release(e);
}

/* ,"protocol":<the fault's JSON>}  (a C callback's binding_protocol, released here) */
static inline void line_protocol(line *l, cpl_error *e)
{
    line_close_args(l);
    cpl_strview j = { "null", 4 };
    if (e != NULL)
        j = cpl_error_json(e);
    printf(",\"protocol\":%.*s}\n", (int)j.len, j.ptr);
    fflush(stdout);
    cpl_error_release(e);
}

#endif
