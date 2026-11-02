/* cpl_rt_exe.c: the native executable's main (spec/cabi.md 8): `bin/<prefix> <function> [--arg JSON]...
   [--max-call-depth N]` runs one exported function with the semantics of `cpl run`: the refusals, the binding of the
   using parameters from the package's grants, the refined positions of the arguments, the call on a thread with 1 GiB
   of stack, and one JSON line on stdout. File-scope names begin with exe_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct exe_state {
    cplrt_runtime *rt;
    const cplrt_export *x;
    cplrt_w *args;
    const cplrt_capref **refs;
    uint32_t max_call_depth;
    int exit_code;
} exe_state;

static void exe_puts(cplrt_sbuf *b, const char *s)
{
    CPLRT(sbuf_puts)(b, s);
}

/* {"error": <S49>} for a refusal or usage error; exit 2 (1 for a failed grant). */
static int exe_error(cplrt_runtime *rt, const char *code, const char *category, const char *message,
                     const char *key, const char *value, int exit_code)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    exe_puts(&b, "{\"error\":{\"code\":");
    CPLRT(write_json_string)(&b, code, strlen(code));
    exe_puts(&b, ",\"category\":");
    CPLRT(write_json_string)(&b, category, strlen(category));
    exe_puts(&b, ",\"severity\":\"error\",\"message\":");
    CPLRT(write_json_string)(&b, message, strlen(message));
    if (key != NULL) {
        CPLRT(sbuf_putc)(&b, ',');
        CPLRT(write_json_string)(&b, key, strlen(key));
        CPLRT(sbuf_putc)(&b, ':');
        CPLRT(write_json_string)(&b, value, strlen(value));
    }
    exe_puts(&b, "}}");
    if (b.status == CPLRT_OK && b.ptr != NULL)
        puts(b.ptr);
    CPLRT(sbuf_free)(&b);
    return exit_code;
}

static void exe_call(void *arg)
{
    exe_state *st = (exe_state *)arg;
    cplrt_runtime *rt = st->rt;
    const cplrt_program *p = rt->program;
    const cplrt_export *x = st->x;
    const cplrt_def *fd = &p->defs[x->function];
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    exe_puts(&b, "{\"function\":");
    CPLRT(write_json_string)(&b, fd->qualified_name, strlen(fd->qualified_name));
    exe_puts(&b, ",\"arguments\":[");
    for (uint32_t i = 0; i < x->nparams; i++) {
        if (i > 0)
            CPLRT(sbuf_putc)(&b, ',');
        CPLRT(write_value_json)(&b, p, st->args[i]);
    }
    CPLRT(sbuf_putc)(&b, ']');
    cplrt_cx cx;
    cplrt_memory_record mem;
    int ok = CPLRT_OK;
    for (uint32_t i = 0; i < x->nparams && ok == CPLRT_OK; i++) {
        if (CPLRT(cx_begin)(&cx, rt, &mem, st->max_call_depth, 1, 0, 0, NULL) != CPLRT_OK) {
            st->exit_code = exe_error(rt, "interop_out_of_memory", "interop", "memory was exhausted", NULL, NULL, 2);
            CPLRT(sbuf_free)(&b);
            return;
        }
        ok = CPLRT(argument_refined)(&cx, x->function, i, x->types[i], st->args[i]);
        if (ok != CPLRT_OK)
            break;
        CPLRT(cx_end)(&cx);
    }
    cplrt_w out = 0;
    if (ok == CPLRT_OK) {
        if (CPLRT(cx_begin)(&cx, rt, &mem, st->max_call_depth, 1, 0, 0, NULL) != CPLRT_OK) {
            st->exit_code = exe_error(rt, "interop_out_of_memory", "interop", "memory was exhausted", NULL, NULL, 2);
            CPLRT(sbuf_free)(&b);
            return;
        }
        ok = p->code->fns[x->function](&cx, st->args, st->refs, &out);
    }
    if (ok == CPLRT_OK) {
        exe_puts(&b, ",\"result\":");
        CPLRT(write_value_json)(&b, p, out);
        exe_puts(&b, ",\"result_text\":");
        cplrt_sbuf t;
        cplrt_sbuf_init(&t, rt);
        CPLRT(sbuf_put)(&t, "", 0);
        CPLRT(write_value_text)(&t, p, out);
        CPLRT(write_json_string)(&b, t.ptr != NULL ? t.ptr : "", t.len);
        CPLRT(sbuf_free)(&t);
        exe_puts(&b, ",\"fault\":null}");
        st->exit_code = 0;
    } else {
        exe_puts(&b, ",\"result\":null,\"result_text\":null,\"fault\":");
        if (cx.fault != NULL)
            CPLRT(write_s49)(&b, p, cx.fault);
        else
            exe_puts(&b, "null");
        CPLRT(sbuf_putc)(&b, '}');
        st->exit_code = 1;
    }
    cplrt_release(out);
    CPLRT(cx_end)(&cx);
    if (b.status == CPLRT_OK && b.ptr != NULL)
        puts(b.ptr);
    else
        st->exit_code = 2;
    CPLRT(sbuf_free)(&b);
}

int main(int argc, char **argv)
{
    /* spec/python.md 6.6: re-executed as the C adapter host of a worker binding */
    const char *mode = getenv("CPL_WORKER_MODE");
    if (mode != NULL && strcmp(mode, "1") == 0)
        return CPLRT(worker_main)();
    CPLRT(worker_self) = 1;
    const cplrt_program *p = &CPLPKG(program);
    cpl_runtime *prt = NULL;
    if (cplrt_runtime_open(p, NULL, &prt, NULL) != CPL_OK) {
        fputs("cannot open a runtime\n", stderr);
        return 2;
    }
    cplrt_runtime *rt = (cplrt_runtime *)(void *)prt;
    int code = 2;
    if (argc < 2)
        return exe_error(rt, "usage_bad_argument", "usage", "usage: <function> [--arg JSON]... [--max-call-depth N]",
                         NULL, NULL, 2);
    const char *function = argv[1];
    const char **jsons = (const char **)calloc((size_t)argc, sizeof *jsons);
    uint32_t njson = 0;
    uint32_t max_call_depth = p->max_call_depth;
    for (int i = 2; i < argc; i++) {
        if (strcmp(argv[i], "--arg") == 0 && i + 1 < argc) {
            jsons[njson++] = argv[++i];
        } else if (strcmp(argv[i], "--max-call-depth") == 0 && i + 1 < argc) {
            char *end = NULL;
            unsigned long v = strtoul(argv[++i], &end, 10);
            if (end == NULL || *end != '\0' || v > UINT32_MAX) {
                free(jsons);
                return exe_error(rt, "usage_bad_argument", "usage", "--max-call-depth takes a u32", NULL, NULL, 2);
            }
            max_call_depth = (uint32_t)v;
        } else {
            free(jsons);
            return exe_error(rt, "usage_bad_argument", "usage", "unexpected argument", "argument", argv[i], 2);
        }
    }
    const cplrt_export *x = NULL;
    for (uint32_t i = 0; i < p->nexports && x == NULL; i++)
        if (strcmp(p->defs[p->exports[i].function].qualified_name, function) == 0)
            x = &p->exports[i];
    if (x == NULL) {
        free(jsons);
        return exe_error(rt, "index_unknown_symbol", "index", "the package exports no such function", "symbol",
                         function, 2);
    }
    /* a using parameter without a grant of its capability (semantics S78) */
    const cplrt_grant **grants = (const cplrt_grant **)calloc(x->nusing + 1u, sizeof *grants);
    for (uint32_t k = 0; k < x->nusing; k++) {
        for (uint32_t g = 0; g < p->ngrants && grants[k] == NULL; g++)
            if (p->grants[g].capability == x->caps[k])
                grants[k] = &p->grants[g];
        if (grants[k] == NULL) {
            free(jsons);
            free(grants);
            return exe_error(rt, "cannot_run_effectful", "resolve",
                             "the function has a using parameter whose capability the package grants no binding for",
                             "function", function, 2);
        }
    }
    if (njson != x->nparams) {
        free(jsons);
        free(grants);
        return exe_error(rt, "usage_bad_argument", "usage", "the number of arguments does not match the parameters",
                         NULL, NULL, 2);
    }
    cplrt_w *args = (cplrt_w *)calloc(x->nparams + 1u, sizeof *args);
    for (uint32_t i = 0; i < x->nparams; i++) {
        cplrt_sbuf pointer, detail;
        cplrt_sbuf_init(&pointer, rt);
        cplrt_sbuf_init(&detail, rt);
        int r = CPLRT(read_value_json)(rt, x->types[i], jsons[i], strlen(jsons[i]), &args[i], &pointer, &detail);
        if (r != CPLRT_OK) {
            char msg[512];
            snprintf(msg, sizeof msg, "argument %u: %s", (unsigned)i, detail.ptr != NULL ? detail.ptr : "invalid");
            CPLRT(sbuf_free)(&pointer);
            CPLRT(sbuf_free)(&detail);
            for (uint32_t k = 0; k < i; k++)
                cplrt_release(args[k]);
            free(args);
            free(jsons);
            free(grants);
            return exe_error(rt, "usage_bad_argument", "usage", msg, NULL, NULL, 2);
        }
        CPLRT(sbuf_free)(&pointer);
        CPLRT(sbuf_free)(&detail);
    }
    /* bind the using parameters from the grants, in parameter order */
    cplrt_capobj **caps = (cplrt_capobj **)calloc(x->nusing + 1u, sizeof *caps);
    const cplrt_capref **refs = (const cplrt_capref **)calloc(x->nusing + 1u, sizeof *refs);
    int bound = 1;
    for (uint32_t k = 0; k < x->nusing && bound; k++) {
        void *state = NULL;
        cpl_error *e = NULL;
        cpl_status s = grants[k]->init != NULL ? grants[k]->init(prt, &state, &e) : CPL_OK;
        CPLRT(api_table).cpl_error_release(e);
        if (s != CPL_OK) {
            bound = 0;
            code = exe_error(rt, "interop_bind_failed", "interop", "the binding could not be granted", "binding",
                             grants[k]->binding, 1);
            break;
        }
        caps[k] = CPLRT(cap_new_obj)(rt, grants[k]->capability, grants[k]->impl, state, grants[k]->binding, NULL);
        if (caps[k] == NULL) {
            bound = 0;
            code = exe_error(rt, "interop_out_of_memory", "interop", "memory was exhausted", NULL, NULL, 2);
            break;
        }
        refs[k] = CPLRT(cap_ref)(caps[k]);
    }
    if (bound) {
        exe_state st = { rt, x, args, refs, max_call_depth, 2 };
        if (CPLRT(run_on_thread)((size_t)CPLRT_LIMIT_EVAL_STACK_SIZE, exe_call, &st) != CPLRT_OK)
            exe_call(&st);
        code = st.exit_code;
    }
    for (uint32_t k = 0; k < x->nusing; k++)
        if (caps[k] != NULL)
            CPLRT(cap_unref)(caps[k]);
    for (uint32_t i = 0; i < x->nparams; i++)
        cplrt_release(args[i]);
    free(caps);
    free(refs);
    free(args);
    free(jsons);
    free(grants);
    fflush(stdout);
    (void)CPLRT(api_table).cpl_runtime_close(prt, CPL_CLOSE_FORCE, NULL);
    return code;
}
