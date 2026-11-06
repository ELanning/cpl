/* cpl_rt_driver.c: the driver's main and cpl-driver protocol 1 (spec/cabi.md 3.8): one compact JSON request per line on
   stdin, one response line on stdout, evaluations on a thread with 1 GiB of stack. A driver is the program's emitted C
   compiled with -DCPL_WITH_TESTS=1, the runtime, every backend-c adapter of the project, and this unit. The protocol's
   own JSON is parsed and rendered in a runtime of its own without a heap budget, so that a configured budget bounds the
   program's values only. File-scope names begin with driver_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct driver_detail {
    char *key, *value;
    size_t key_len, value_len;
} driver_detail;

/* One response of a scripted host. */
typedef struct driver_response {
    char *op;
    size_t op_len;                 /* the decoded name may hold U+0000: compared with its length */
    uint32_t nargs;
    cplrt_w *args;                 /* owned */
    int is_failure;
    cplrt_w result;                /* owned */
    uint32_t kind;
    char *message;
    size_t message_len;
    uint32_t ndetails;
    driver_detail *details;
    int used;
} driver_response;

typedef struct driver_script {
    uint32_t n;
    driver_response *responses;
} driver_script;

typedef struct driver_state {
    cplrt_runtime *proto;          /* parses requests and renders responses; no heap budget */
    cplrt_runtime *rt;             /* the configured runtime the program runs in */
    uint32_t max_call_depth;
    uint64_t time_budget_ms;
    int check_contracts;
    uint64_t max_heap_bytes;
    cplrt_capobj **handles;        /* the bound capabilities, numbered from 0 in bind order */
    uint32_t nhandles, handles_cap;
    int poisoned;
    int done;
    int exit_code;
    char *worker_python;           /* the `workers` request (spec/python.md 6.5), applied to every runtime opened */
    char **worker_env;
    uint32_t nworker_env;
} driver_state;

/* ---- output ------------------------------------------------------------------------------------------------------- */

static void driver_write_line(const char *s, size_t n)
{
    fwrite(s, 1, n, stdout);
    fputc('\n', stdout);
    fflush(stdout);
}

static void driver_send(cplrt_sbuf *b)
{
    if (b->status != CPLRT_OK || b->ptr == NULL) {
        static const char oom[] = "{\"error\":{\"code\":\"interop_out_of_memory\",\"category\":\"interop\","
                                  "\"severity\":\"error\",\"message\":\"memory was exhausted outside an evaluation\"}}";
        driver_write_line(oom, sizeof oom - 1u);
    } else {
        driver_write_line(b->ptr, b->len);
    }
    CPLRT(sbuf_free)(b);
}

static void driver_start(driver_state *st, cplrt_sbuf *b)
{
    cplrt_sbuf_init(b, st->proto);
    CPLRT(sbuf_put)(b, "", 0);
}

/* {"error": E} with an error object the runtime made (released here). */
static void driver_send_error_object(driver_state *st, cpl_error *e)
{
    cplrt_sbuf b;
    driver_start(st, &b);
    CPLRT(sbuf_puts)(&b, "{\"error\":");
    if (e != NULL) {
        cpl_strview j = CPLRT(api_table).cpl_error_json(e);
        CPLRT(sbuf_put)(&b, j.ptr, j.len);
        CPLRT(api_table).cpl_error_release(e);
    } else {
        CPLRT(sbuf_puts)(&b, "null");
    }
    CPLRT(sbuf_putc)(&b, '}');
    driver_send(&b);
}

/* {"error": E} for an interop code with its payload. */
static void driver_error(driver_state *st, uint32_t code, const cplrt_datum *data, uint32_t n)
{
    cpl_error *e = NULL;
    (void)CPLRT(error_interop)(NULL, &e, code, data, n);
    driver_send_error_object(st, e);
}

static void driver_invalid(driver_state *st, const char *pointer, const char *detail)
{
    cplrt_datum d[2];
    memset(d, 0, sizeof d);
    d[0].key = "pointer";
    d[0].form = CPLRT_DATUM_STRING;
    d[0].str = pointer;
    d[1].key = "detail";
    d[1].form = CPLRT_DATUM_STRING;
    d[1].str = detail;
    driver_error(st, CPLRT_INTEROP_JSON_INVALID, d, 2);
}

/* ---- values as node lists (3.8): cpl_rt_wire.c --------------------------------------------------------- */

/* ---- fault records (3.8: R) ------------------------------------------------------------------------------------ */

static const char *driver_code(uint32_t kind)
{
    static const char *const codes[8] = { "internal_error", "contract_violation", "assertion_failed",
                                          "unresolved_hole", "division_by_zero", "refinement_violation",
                                          "resource_exhaustion", "internal_error" };
    return kind < 8u ? codes[kind] : "internal_error";
}

static void driver_write_span(cplrt_sbuf *b, const cplrt_program *p, uint32_t site)
{
    const cplrt_site *s = &p->sites[site];
    CPLRT(sbuf_putc)(b, '[');
    CPLRT(sbuf_u64)(b, s->file);
    CPLRT(sbuf_putc)(b, ',');
    CPLRT(sbuf_u64)(b, s->start);
    CPLRT(sbuf_putc)(b, ',');
    CPLRT(sbuf_u64)(b, s->end);
    CPLRT(sbuf_putc)(b, ']');
}

static void driver_write_fault(driver_state *st, cplrt_sbuf *b, const cplrt_fault *f)
{
    const cplrt_program *p = st->rt->program;
    static const char *const cks[5] = { NULL, "requires", "ensures", "invariant", "decreases" };
    static const char *const res[5] = { NULL, "stack", "memory", "time", "steps" };
    CPLRT(sbuf_puts)(b, "{\"kind\":\"");
    CPLRT(sbuf_puts)(b, driver_code(f->kind));
    CPLRT(sbuf_putc)(b, '"');
    if (f->kind == CPLRT_FK_CONTRACT_VIOLATION && f->clause_kind >= 1u && f->clause_kind <= 4u) {
        CPLRT(sbuf_puts)(b, ",\"clause_kind\":\"");
        CPLRT(sbuf_puts)(b, cks[f->clause_kind]);
        CPLRT(sbuf_putc)(b, '"');
    }
    if (f->kind == CPLRT_FK_RESOURCE_EXHAUSTION && f->resource >= 1u && f->resource <= 4u) {
        CPLRT(sbuf_puts)(b, ",\"resource\":\"");
        CPLRT(sbuf_puts)(b, res[f->resource]);
        CPLRT(sbuf_putc)(b, '"');
    }
    CPLRT(sbuf_puts)(b, ",\"message\":");
    CPLRT(write_json_string)(b, f->message != NULL ? f->message : "", f->message != NULL ? f->message_len : 0u);
    if (f->symbol != NULL) {
        CPLRT(sbuf_puts)(b, ",\"symbol\":");
        CPLRT(write_json_string)(b, f->symbol, strlen(f->symbol));
    }
    if (f->facet == CPLRT_FACET_CONTRACT)
        CPLRT(sbuf_puts)(b, ",\"facet\":\"contract\"");
    if (f->span != CPLRT_NONE && f->span < p->nsites) {
        CPLRT(sbuf_puts)(b, ",\"span\":");
        driver_write_span(b, p, f->span);
    }
    CPLRT(sbuf_puts)(b, ",\"backtrace\":[");
    for (uint32_t i = 0; i < f->nframes; i++) {
        if (i > 0)
            CPLRT(sbuf_putc)(b, ',');
        const cplrt_btframe *fr = &f->frames[i];
        if (fr->site != CPLRT_NONE && fr->site < p->nsites) {
            const cplrt_site *s = &p->sites[fr->site];
            CPLRT(sbuf_putc)(b, '[');
            CPLRT(sbuf_u64)(b, fr->def);
            CPLRT(sbuf_putc)(b, ',');
            CPLRT(sbuf_u64)(b, s->file);
            CPLRT(sbuf_putc)(b, ',');
            CPLRT(sbuf_u64)(b, s->start);
            CPLRT(sbuf_putc)(b, ',');
            CPLRT(sbuf_u64)(b, s->end);
            CPLRT(sbuf_putc)(b, ']');
        } else {
            CPLRT(sbuf_putc)(b, '[');
            CPLRT(sbuf_u64)(b, fr->def);
            CPLRT(sbuf_putc)(b, ']');
        }
    }
    CPLRT(sbuf_puts)(b, "],\"data\":");
    CPLRT(write_payload)(b, f);
    CPLRT(sbuf_puts)(b, ",\"diagnostic\":");
    cplrt_sbuf d;
    cplrt_sbuf_init(&d, st->proto);
    CPLRT(sbuf_put)(&d, "", 0);
    CPLRT(write_s49)(&d, p, f);
    if (d.status == CPLRT_OK)
        CPLRT(write_json_string)(b, d.ptr, d.len);
    else
        b->status = CPLRT_NOMEM;
    CPLRT(sbuf_free)(&d);
    CPLRT(sbuf_putc)(b, '}');
}

/* {"fault": R} or, with `with_violation`, {"fault": R, "violation": {"def": n, "fault": R} | null}. */
static void driver_send_fault(driver_state *st, cplrt_cx *cx, int with_violation)
{
    cplrt_sbuf b;
    driver_start(st, &b);
    if (cx->fault == NULL) {
        CPLRT(sbuf_free)(&b);
        cplrt_datum d;
        memset(&d, 0, sizeof d);
        d.key = "detail";
        d.form = CPLRT_DATUM_STRING;
        d.str = "an evaluation failed without a fault";
        cpl_error *e = NULL;
        (void)CPLRT(error_internal)(NULL, &e, "an evaluation failed without a fault");
        driver_start(st, &b);
        CPLRT(sbuf_puts)(&b, "{\"error\":");
        if (e != NULL) {
            cpl_strview j = CPLRT(api_table).cpl_error_json(e);
            CPLRT(sbuf_put)(&b, j.ptr, j.len);
            CPLRT(api_table).cpl_error_release(e);
        }
        CPLRT(sbuf_putc)(&b, '}');
        driver_send(&b);
        return;
    }
    CPLRT(sbuf_puts)(&b, "{\"fault\":");
    driver_write_fault(st, &b, cx->fault);
    if (with_violation) {
        CPLRT(sbuf_puts)(&b, ",\"violation\":");
        if (cx->violation != NULL) {
            CPLRT(sbuf_puts)(&b, "{\"def\":");
            CPLRT(sbuf_u64)(&b, cx->violation->owner);
            CPLRT(sbuf_puts)(&b, ",\"fault\":");
            driver_write_fault(st, &b, cx->violation);
            CPLRT(sbuf_putc)(&b, '}');
        } else {
            CPLRT(sbuf_puts)(&b, "null");
        }
    }
    CPLRT(sbuf_putc)(&b, '}');
    driver_send(&b);
}

static void driver_send_value(driver_state *st, cplrt_w w)
{
    cplrt_sbuf b;
    driver_start(st, &b);
    CPLRT(sbuf_puts)(&b, "{\"ok\":");
    CPLRT(nodes_write)(&b, st->proto, st->rt->program, w, 1);
    CPLRT(sbuf_putc)(&b, '}');
    driver_send(&b);
}

static void driver_send_text(driver_state *st, const char *s)
{
    driver_write_line(s, strlen(s));
}

/* ---- evaluations --------------------------------------------------------------------------------------------- */

static int driver_begin(driver_state *st, cplrt_cx *cx, cplrt_memory_record *mem, int fresh_budget)
{
    return CPLRT(cx_begin)(cx, st->rt, mem, st->max_call_depth, st->check_contracts,
                           fresh_budget ? st->time_budget_ms : 0u, 0u, NULL);
}

/* ---- the scripted host (3.8: bind_scripted) ------------------------------------------------------------------ */

static void driver_script_free(driver_script *s)
{
    if (s == NULL)
        return;
    for (uint32_t i = 0; i < s->n; i++) {
        driver_response *r = &s->responses[i];
        free(r->op);
        for (uint32_t k = 0; k < r->nargs; k++)
            cplrt_release(r->args[k]);
        free(r->args);
        cplrt_release(r->result);
        free(r->message);
        for (uint32_t k = 0; k < r->ndetails; k++) {
            free(r->details[k].key);
            free(r->details[k].value);
        }
        free(r->details);
    }
    free(s->responses);
    free(s);
}

static cpl_status driver_script_invoke(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args,
                                       cplrt_w *result)
{
    driver_script *s = (driver_script *)cap->user;
    cplrt_cx *cx = CPLRT(call_cx)(call);
    const cplrt_program *p = cx->program;
    const cplrt_op *opd = &p->defs[cap->capability].ops[op];
    for (uint32_t i = 0; i < s->n; i++) {
        driver_response *r = &s->responses[i];
        if (r->used || r->op_len != strlen(opd->name) || memcmp(r->op, opd->name, r->op_len) != 0 ||
            r->nargs != opd->nparams)
            continue;
        int all = 1;
        for (uint32_t k = 0; k < r->nargs && all; k++) {
            int eq = 0;
            if (CPLRT(value_equal)(cx->rt, r->args[k], args[k], &eq) != CPLRT_OK)
                return CPL_E_NOMEM;
            all = eq;
        }
        if (!all)
            continue;
        r->used = 1;
        if (!r->is_failure) {
            cplrt_retain(r->result);
            CPLRT_SET(*result, r->result);
            return CPL_OK;
        }
        for (uint32_t k = 0; k < r->ndetails; k++) {
            cpl_status ds = CPLRT(call_detail)(call, r->details[k].key, r->details[k].key_len, r->details[k].value,
                                               r->details[k].value_len);
            if (ds != CPL_OK)
                return ds;
        }
        return CPLRT(call_fail)(call, (int32_t)r->kind, r->message, r->message_len);
    }
    /* the arguments as value text, joined by ", " */
    cplrt_sbuf a;
    cplrt_sbuf_init(&a, cx->rt);
    CPLRT(sbuf_put)(&a, "", 0);
    for (uint32_t k = 0; k < opd->nparams; k++) {
        if (k > 0)
            CPLRT(sbuf_puts)(&a, ", ");
        CPLRT(write_value_text)(&a, p, args[k]);
    }
    if (a.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&a);
        return CPL_E_NOMEM;
    }
    cpl_status st = CPLRT(call_no_response)(call, a.ptr);
    CPLRT(sbuf_free)(&a);
    return st;
}

static void driver_script_drop(const cplrt_capref *cap)
{
    driver_script_free((driver_script *)cap->user);
}

static const cplrt_capimpl driver_script_impl = { 0u, driver_script_invoke, driver_script_drop };

static char *driver_strdup(const char *s, size_t n)
{
    char *d = (char *)malloc(n + 1u);
    if (d != NULL) {
        memcpy(d, s, n);
        d[n] = '\0';
    }
    return d;
}

static int driver_add_handle(driver_state *st, cplrt_capobj *c)
{
    if (st->nhandles == st->handles_cap) {
        uint32_t cap = st->handles_cap == 0 ? 8u : st->handles_cap * 2u;
        cplrt_capobj **g = (cplrt_capobj **)realloc(st->handles, cap * sizeof *g);
        if (g == NULL)
            return 0;
        st->handles = g;
        st->handles_cap = cap;
    }
    st->handles[st->nhandles++] = c;
    return 1;
}

static void driver_send_handle(driver_state *st, uint32_t n)
{
    char line[48];
    snprintf(line, sizeof line, "{\"handle\":%u}", (unsigned)n);
    driver_send_text(st, line);
}

static uint32_t driver_kind_index(const cplrt_jv *v)
{
    if (v == NULL || v->type != CPLRT_JSON_STRING)
        return CPLRT_NONE;
    for (uint32_t i = 0; i < CPLRT_FOREIGN_KINDS_N; i++)
        if (strlen(CPLRT_FOREIGN_KINDS[i]) == v->len && memcmp(CPLRT_FOREIGN_KINDS[i], v->str, v->len) == 0)
            return i;
    return CPLRT_NONE;
}

/* A foreign failure {"kind", "message", "details": [[k, v], ...]} into the response; 0 when malformed. */
static int driver_failure(const cplrt_jv *f, driver_response *r)
{
    const cplrt_jv *kind = CPLRT(json_member)(f, "kind"), *msg = CPLRT(json_member)(f, "message");
    const cplrt_jv *details = CPLRT(json_member)(f, "details");
    uint32_t k = driver_kind_index(kind);
    if (k == CPLRT_NONE || msg == NULL || msg->type != CPLRT_JSON_STRING || details == NULL ||
        details->type != CPLRT_JSON_ARRAY)
        return 0;
    r->is_failure = 1;
    r->kind = k;
    r->message = driver_strdup(msg->str, msg->len);
    r->message_len = msg->len;
    r->ndetails = details->n;
    r->details = details->n > 0 ? (driver_detail *)calloc(details->n, sizeof *r->details) : NULL;
    if (r->message == NULL || (details->n > 0 && r->details == NULL)) {
        r->ndetails = 0; /* out of memory: refused like a malformed response, and nothing half-built is read */
        return 0;
    }
    for (uint32_t i = 0; i < details->n; i++) {
        const cplrt_jv *pair = &details->items[i];
        if (pair->type != CPLRT_JSON_ARRAY || pair->n != 2u || pair->items[0].type != CPLRT_JSON_STRING ||
            pair->items[1].type != CPLRT_JSON_STRING)
            return 0;
        r->details[i].key = driver_strdup(pair->items[0].str, pair->items[0].len);
        r->details[i].key_len = pair->items[0].len;
        r->details[i].value = driver_strdup(pair->items[1].str, pair->items[1].len);
        r->details[i].value_len = pair->items[1].len;
        if (r->details[i].key == NULL || r->details[i].value == NULL)
            return 0;
    }
    return 1;
}

static void driver_bind_scripted(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    uint32_t capability = 0;
    const cplrt_jv *label = CPLRT(json_member)(req, "label");
    const cplrt_jv *responses = CPLRT(json_member)(req, "responses");
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "capability"), &capability) || capability >= p->ndefs ||
        p->defs[capability].kind != CPLRT_DEF_CAPABILITY) {
        driver_invalid(st, "/capability", "the capability is not a capability declaration of the program");
        return;
    }
    if (label == NULL || label->type != CPLRT_JSON_STRING) {
        driver_invalid(st, "/label", "the label is a string");
        return;
    }
    if (responses == NULL || responses->type != CPLRT_JSON_ARRAY) {
        driver_invalid(st, "/responses", "the responses are an array");
        return;
    }
    driver_script *s = (driver_script *)calloc(1, sizeof *s);
    if (s == NULL) {
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    s->n = responses->n;
    s->responses = responses->n > 0 ? (driver_response *)calloc(responses->n, sizeof *s->responses) : NULL;
    if (responses->n > 0 && s->responses == NULL) {
        s->n = 0;
        driver_script_free(s);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    for (uint32_t i = 0; i < responses->n; i++) {
        const cplrt_jv *r = &responses->items[i];
        driver_response *dr = &s->responses[i];
        const cplrt_jv *op = CPLRT(json_member)(r, "op");
        const char *detail = "a response is {\"op\", \"args\", \"result\" | \"foreign_error\"}";
        if (op == NULL || op->type != CPLRT_JSON_STRING) {
            driver_script_free(s);
            driver_invalid(st, "/responses", detail);
            return;
        }
        dr->op = driver_strdup(op->str, op->len);
        dr->op_len = op->len;
        if (dr->op == NULL) {
            driver_script_free(s);
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
            return;
        }
        if (CPLRT(nodes_read_all)(st->rt, CPLRT(json_member)(r, "args"), &dr->args, &dr->nargs, &detail) != CPLRT_OK) {
            driver_script_free(s);
            driver_invalid(st, "/responses", detail);
            return;
        }
        const cplrt_jv *result = CPLRT(json_member)(r, "result");
        const cplrt_jv *failure = CPLRT(json_member)(r, "foreign_error");
        if (result != NULL) {
            if (CPLRT(nodes_read)(st->rt, result, &dr->result, &detail) != CPLRT_OK) {
                driver_script_free(s);
                driver_invalid(st, "/responses", detail);
                return;
            }
        } else if (failure == NULL || !driver_failure(failure, dr)) {
            driver_script_free(s);
            driver_invalid(st, "/responses", "a response has a result or a foreign_error");
            return;
        }
    }
    char *lab = driver_strdup(label->str, label->len);
    cplrt_capobj *c = lab != NULL ? CPLRT(cap_new_obj)(st->rt, capability, &driver_script_impl, s, lab, NULL) : NULL;
    free(lab);
    if (c == NULL) {
        driver_script_free(s);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    if (!driver_add_handle(st, c)) {
        CPLRT(cap_unref)(c);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    driver_send_handle(st, st->nhandles - 1u);
}

static void driver_bind(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    const cplrt_jv *b = CPLRT(json_member)(req, "binding");
    if (b == NULL || b->type != CPLRT_JSON_STRING) {
        driver_invalid(st, "/binding", "the binding is a string");
        return;
    }
    for (uint32_t i = 0; i < p->ngrants; i++) {
        const cplrt_grant *g = &p->grants[i];
        if (strlen(g->binding) != b->len || memcmp(g->binding, b->str, b->len) != 0)
            continue;
        cplrt_capobj *c = CPLRT(cap_new_obj)(st->rt, g->capability, CPLRT(grant_impl)(g), NULL, g->binding, NULL);
        if (c == NULL || !driver_add_handle(st, c)) {
            if (c != NULL) {
                CPLRT(cap_run_drop_disarm)(c);          /* no init ran */
                CPLRT(cap_unref)(c);
            }
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
            return;
        }
        cpl_error *e = NULL;
        if (CPLRT(grant_init)(st->rt, g, c, &e) != CPL_OK) {
            st->nhandles--;                             /* numbers no handle (3.8) */
            CPLRT(cap_unref)(c);
            driver_send_error_object(st, e);
            return;
        }
        driver_send_handle(st, st->nhandles - 1u);
        return;
    }
    driver_invalid(st, "/binding", "no backend-c binding of the program has that name");
}

/* ---- the evaluation requests ------------------------------------------------------------------------------------ */

/* Every refined position of each argument, each in an evaluation of its own with a fresh deadline (run_function's
   check_arguments): CPLRT_OK when all hold; otherwise the response was sent. */
/* A request's values against the declared types: as many as there are parameters, each of its parameter's type
   (cabi.md 3.8: a malformed request answers interop_json_invalid); a type id of 0 (a type parameter) admits any value.
   1 when they match; else 0, the answer sent. */
static int driver_args_typed(driver_state *st, const char *where, const uint32_t *types, uint32_t expected,
                             const cplrt_w *args, uint32_t n)
{
    int r = CPLRT(values_typed)(st->rt, types, expected, args, n);
    if (r == CPLRT_NOMEM)
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    else if (r != CPLRT_OK)
        driver_invalid(st, where, n != expected ? "the values do not match the parameters in number"
                                                : "a value is not of its parameter's type");
    return r == CPLRT_OK;
}

static int driver_check_arguments(driver_state *st, uint32_t f, const cplrt_w *args, uint32_t n)
{
    const cplrt_program *p = st->rt->program;
    const cplrt_def *d = &p->defs[f];
    if (d->ptypes == NULL)
        return CPLRT_OK;
    for (uint32_t i = 0; i < n && i < d->nmembers; i++) {
        cplrt_cx cx;
        cplrt_memory_record mem;
        if (driver_begin(st, &cx, &mem, 1) != CPLRT_OK) {
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
            return CPLRT_FAULT;
        }
        if (CPLRT(argument_refined)(&cx, f, i, d->ptypes[i], args[i]) != CPLRT_OK) {
            driver_send_fault(st, &cx, 0);
            CPLRT(cx_end)(&cx);
            return CPLRT_FAULT;
        }
        CPLRT(cx_end)(&cx);
    }
    return CPLRT_OK;
}

static void driver_call(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    uint32_t f = 0;
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "function"), &f) || f >= p->ndefs || p->code->fns[f] == NULL) {
        driver_invalid(st, "/function", "the function has no entry in the driver");
        return;
    }
    const cplrt_jv *using = CPLRT(json_member)(req, "using");
    const cplrt_jv *check = CPLRT(json_member)(req, "check_arguments");
    if (using == NULL || using->type != CPLRT_JSON_ARRAY) {
        driver_invalid(st, "/using", "using is an array of handles");
        return;
    }
    const cplrt_def *d = &p->defs[f];
    const cplrt_capref *refs[16] = { 0 };
    if (using->n > 16u || using->n != d->nusing) {
        driver_invalid(st, "/using", "the handles do not match the using parameters in number");
        return;
    }
    for (uint32_t i = 0; i < using->n; i++) {
        uint32_t h = 0;
        if (!CPLRT(jv_u32)(&using->items[i], &h) || h >= st->nhandles) {
            driver_invalid(st, "/using", "a handle the driver did not bind");
            return;
        }
        refs[i] = CPLRT(cap_ref)(st->handles[h]);
        if (d->ucaps == NULL || refs[i]->capability != d->ucaps[i]) {
            driver_invalid(st, "/using", "a handle of another capability than its using parameter's");
            return;
        }
    }
    cplrt_w *args = NULL;
    uint32_t n = 0;
    const char *detail = "";
    int r = CPLRT(nodes_read_all)(st->rt, CPLRT(json_member)(req, "args"), &args, &n, &detail);
    if (r == CPLRT_NOMEM) {
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    if (r != CPLRT_OK) {
        driver_invalid(st, "/args", detail);
        return;
    }
    if (!driver_args_typed(st, "/args", d->ptypes, d->nmembers, args, n)) {
        CPLRT(words_release)(args, n);
        return;
    }
    if (check != NULL && check->type == CPLRT_JSON_TRUE && driver_check_arguments(st, f, args, n) != CPLRT_OK) {
        CPLRT(words_release)(args, n);
        return;
    }
    cplrt_cx cx;
    cplrt_memory_record mem;
    if (driver_begin(st, &cx, &mem, 1) != CPLRT_OK) {
        CPLRT(words_release)(args, n);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    cplrt_w out = 0;
    if (p->code->fns[f](&cx, args, refs, &out) == CPLRT_OK)
        driver_send_value(st, out);
    else
        driver_send_fault(st, &cx, 0);
    cplrt_release(out);
    CPLRT(cx_end)(&cx);
    CPLRT(words_release)(args, n);
}

static void driver_run_body(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    uint32_t t = 0;
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "test"), &t) || t >= p->ndefs || p->code->tests == NULL ||
        p->code->tests[t] == NULL) {
        driver_invalid(st, "/test", "the test has no body in the driver");
        return;
    }
    cplrt_w *binders = NULL;
    uint32_t n = 0;
    const char *detail = "";
    int r = CPLRT(nodes_read_all)(st->rt, CPLRT(json_member)(req, "binders"), &binders, &n, &detail);
    if (r != CPLRT_OK) {
        if (r == CPLRT_NOMEM)
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        else
            driver_invalid(st, "/binders", detail);
        return;
    }
    /* a test has no binders; a property's are its members, of the types in ptypes */
    if (!driver_args_typed(st, "/binders", p->defs[t].ptypes, p->defs[t].nmembers, binders, n)) {
        CPLRT(words_release)(binders, n);
        return;
    }
    cplrt_cx cx;
    cplrt_memory_record mem;
    if (driver_begin(st, &cx, &mem, 1) != CPLRT_OK) {
        CPLRT(words_release)(binders, n);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    if (p->code->tests[t](&cx, binders) == CPLRT_OK)
        driver_send_text(st, "{\"ok\":null}");
    else
        driver_send_fault(st, &cx, 1);
    CPLRT(cx_end)(&cx);
    CPLRT(words_release)(binders, n);
}

static void driver_failing_refinement(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    uint32_t refined = 0;
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "refined"), &refined) || refined >= p->ndefs) {
        driver_invalid(st, "/refined", "the declaration is not in the program");
        return;
    }
    const cplrt_jv *budget = CPLRT(json_member)(req, "budget");
    int fresh = budget != NULL && budget->type == CPLRT_JSON_STRING && budget->len == 5u &&
                memcmp(budget->str, "fresh", 5) == 0;
    uint32_t ri = CPLRT_NONE;
    for (uint32_t i = 0; i < p->nrefs; i++)
        if (p->refinements[i].def == refined)
            ri = i;
    cplrt_w v = 0;
    const char *detail = "";
    const cplrt_jv *value = CPLRT(json_member)(req, "value");
    int r = value != NULL ? CPLRT(nodes_read)(st->rt, value, &v, &detail) : CPLRT_INVALID;
    if (r != CPLRT_OK) {
        if (r == CPLRT_NOMEM)
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        else
            driver_invalid(st, "/value", detail);
        return;
    }
    if (ri == CPLRT_NONE) {
        cplrt_release(v);
        driver_send_text(st, "{\"ok\":null}");
        return;
    }
    if (!driver_args_typed(st, "/value", &p->refinements[ri].base, 1, &v, 1)) {
        cplrt_release(v);
        return;
    }
    cplrt_cx cx;
    cplrt_memory_record mem;
    if (driver_begin(st, &cx, &mem, fresh) != CPLRT_OK) {
        cplrt_release(v);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    const cplrt_refinement *ref = &p->refinements[ri];
    uint32_t failed = CPLRT_NONE;
    int fault = 0;
    for (uint32_t k = 0; k < ref->nchain && failed == CPLRT_NONE; k++) {
        const cplrt_refinement *c = &p->refinements[ref->chain[k]];
        int holds = 0;
        cplrt_pred_entry pred = p->code->preds[c->def];
        if (pred == NULL || pred(&cx, CPLRT_NO_SITE, v, &holds) != CPLRT_OK) {
            fault = 1;
            break;
        }
        if (!holds)
            failed = c->def;
    }
    if (fault) {
        driver_send_fault(st, &cx, 0);
    } else if (failed == CPLRT_NONE) {
        driver_send_text(st, "{\"ok\":null}");
    } else {
        char line[48];
        snprintf(line, sizeof line, "{\"ok\":%u}", (unsigned)failed);
        driver_send_text(st, line);
    }
    CPLRT(cx_end)(&cx);
    cplrt_release(v);
}

static void driver_range_bound(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    uint32_t property = 0, binder = 0;
    const cplrt_jv *side = CPLRT(json_member)(req, "side");
    const cplrt_jv *clause = CPLRT(json_member)(req, "clause");
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "property"), &property) ||
        !CPLRT(jv_u32)(CPLRT(json_member)(req, "binder"), &binder) || side == NULL || side->type != CPLRT_JSON_STRING ||
        clause == NULL || clause->type != CPLRT_JSON_STRING) {
        driver_invalid(st, "", "range_bound takes property, binder, side, and clause");
        return;
    }
    uint32_t hi = side->len == 2u && memcmp(side->str, "hi", 2) == 0;
    cplrt_bound_entry entry = NULL;
    for (uint32_t i = 0; i < p->code->nbounds; i++) {
        const cplrt_bound *b = &p->code->bounds[i];
        if (b->property == property && b->binder == binder && b->hi == hi)
            entry = b->entry;
    }
    if (entry == NULL) {
        driver_invalid(st, "/binder", "the property has no range bound for that binder");
        return;
    }
    cplrt_cx cx;
    cplrt_memory_record mem;
    if (driver_begin(st, &cx, &mem, 0) != CPLRT_OK) {
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    char *text = driver_strdup(clause->str, clause->len);
    cx.range_text = text;
    cplrt_w out = 0;
    if (entry(&cx, &out) == CPLRT_OK)
        driver_send_value(st, out);
    else
        driver_send_fault(st, &cx, 1);
    cplrt_release(out);
    CPLRT(cx_end)(&cx);
    free(text);
}

static void driver_cap_call(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    uint32_t h = 0, op = 0;
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "handle"), &h) || h >= st->nhandles) {
        driver_invalid(st, "/handle", "a handle the driver did not bind");
        return;
    }
    const cplrt_capref *ref = CPLRT(cap_ref)(st->handles[h]);
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "operation"), &op) || op >= p->defs[ref->capability].nmembers) {
        driver_invalid(st, "/operation", "the capability has no such operation");
        return;
    }
    cplrt_w *args = NULL;
    uint32_t n = 0;
    const char *detail = "";
    int r = CPLRT(nodes_read_all)(st->rt, CPLRT(json_member)(req, "args"), &args, &n, &detail);
    if (r != CPLRT_OK) {
        if (r == CPLRT_NOMEM)
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        else
            driver_invalid(st, "/args", detail);
        return;
    }
    const cplrt_op *opd = &p->defs[ref->capability].ops[op];
    if (!driver_args_typed(st, "/args", opd->ptypes, opd->nparams, args, n)) {
        CPLRT(words_release)(args, n);
        return;
    }
    cplrt_cx cx;
    cplrt_memory_record mem;
    if (driver_begin(st, &cx, &mem, 1) != CPLRT_OK) {
        CPLRT(words_release)(args, n);
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        return;
    }
    cplrt_w out = 0;
    cplrt_sbuf foreign;
    cplrt_sbuf_init(&foreign, st->proto);
    CPLRT(sbuf_put)(&foreign, "", 0);
    r = CPLRT(cap_call_raw)(&cx, CPLRT_NO_SITE, ref, op, args, n, &out, &foreign);
    if (r == CPLRT_OK) {
        driver_send_value(st, out);
    } else if (r == CPLRT_INVALID) {
        cplrt_sbuf b;
        driver_start(st, &b);
        CPLRT(sbuf_puts)(&b, "{\"foreign_error\":");
        CPLRT(sbuf_put)(&b, foreign.ptr, foreign.len);
        CPLRT(sbuf_putc)(&b, '}');
        driver_send(&b);
    } else {
        driver_send_fault(st, &cx, 0);
    }
    CPLRT(sbuf_free)(&foreign);
    cplrt_release(out);
    CPLRT(cx_end)(&cx);
    CPLRT(words_release)(args, n);
}

static void driver_from_json(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    const cplrt_jv *type = CPLRT(json_member)(req, "type"), *json = CPLRT(json_member)(req, "json");
    if (type == NULL || type->type != CPLRT_JSON_STRING || json == NULL || json->type != CPLRT_JSON_STRING) {
        driver_invalid(st, "", "from_json takes type and json strings");
        return;
    }
    uint32_t id = 0;
    for (uint32_t i = 1; i < p->ntypes && id == 0; i++)
        if (strlen(p->types[i].text) == type->len && memcmp(p->types[i].text, type->str, type->len) == 0)
            id = i;
    if (id == 0) {
        char *name = driver_strdup(type->str, type->len);
        cplrt_datum d;
        memset(&d, 0, sizeof d);
        d.key = "name";
        d.form = CPLRT_DATUM_STRING;
        d.str = name != NULL ? name : "";
        driver_error(st, CPLRT_INTEROP_TYPE_UNKNOWN, &d, 1);
        free(name);
        return;
    }
    cplrt_sbuf pointer, detail;
    cplrt_sbuf_init(&pointer, st->proto);
    cplrt_sbuf_init(&detail, st->proto);
    cplrt_w w = 0;
    int r = CPLRT(read_value_json)(st->rt, id, json->str, json->len, &w, &pointer, &detail);
    if (r == CPLRT_OK) {
        driver_send_value(st, w);
        cplrt_release(w);
    } else if (r == CPLRT_NOMEM) {
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    } else {
        cplrt_datum d[2];
        memset(d, 0, sizeof d);
        d[0].key = "pointer";
        d[0].form = CPLRT_DATUM_STRING;
        d[0].str = pointer.ptr != NULL ? pointer.ptr : "";
        d[1].key = "detail";
        d[1].form = CPLRT_DATUM_STRING;
        d[1].str = detail.ptr != NULL ? detail.ptr : "";
        driver_error(st, CPLRT_INTEROP_JSON_INVALID, d, 2);
    }
    CPLRT(sbuf_free)(&pointer);
    CPLRT(sbuf_free)(&detail);
}

static void driver_render(driver_state *st, const cplrt_jv *req)
{
    cplrt_w v = 0;
    const char *detail = "";
    const cplrt_jv *value = CPLRT(json_member)(req, "value");
    int r = value != NULL ? CPLRT(nodes_read)(st->rt, value, &v, &detail) : CPLRT_INVALID;
    if (r != CPLRT_OK) {
        if (r == CPLRT_NOMEM)
            driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        else
            driver_invalid(st, "/value", detail);
        return;
    }
    driver_send_value(st, v);
    cplrt_release(v);
}

static void driver_stats(driver_state *st)
{
    cpl_stats s = CPL_STATS_INIT;
    CPLRT(api_table).cpl_runtime_stats(&st->rt->pub, &s);
    char line[512];
    snprintf(line, sizeof line,
             "{\"ok\":{\"live_handles\":%llu,\"live_caps\":%llu,\"live_objects\":%llu,\"live_bytes\":%llu,"
             "\"peak_bytes\":%llu,\"active_calls\":%llu,\"live_buffers\":%llu}}",
             (unsigned long long)s.live_handles, (unsigned long long)s.live_caps, (unsigned long long)s.live_objects,
             (unsigned long long)s.live_bytes, (unsigned long long)s.peak_bytes, (unsigned long long)s.active_calls,
             (unsigned long long)s.live_buffers);
    driver_send_text(st, line);
}

/* ---- configure and shutdown --------------------------------------------------------------------------------------- */

/* {"ok": {"nodes": {...}, "builtins": {...}, "claims": {...}}} (3.8): the counters of a coverage driver, the nodes
   summed by HIR variant; a driver built without coverage refuses with interop_option_unsupported */
static void driver_coverage(driver_state *st)
{
#if defined(CPLRT_COVERAGE) && CPLRT_COVERAGE
    cplrt_sbuf b;
    driver_start(st, &b);
    CPLRT(sbuf_puts)(&b, "{\"ok\":{\"nodes\":{");
    /* summed by kind: the kinds are few, so a quadratic pass over the counters is enough */
    uint32_t n = CPLPKG(ncov);
    int first = 1;
    for (uint32_t k = 0; k < n; k++) {
        const char *kind = CPLPKG(cov_kinds)[k];
        int seen = 0;
        for (uint32_t j = 0; j < k && !seen; j++)
            seen = strcmp(CPLPKG(cov_kinds)[j], kind) == 0;
        if (seen)
            continue;
        uint64_t sum = 0;
        for (uint32_t j = k; j < n; j++)
            if (strcmp(CPLPKG(cov_kinds)[j], kind) == 0)
                sum += atomic_load_explicit(&CPLPKG(cov)[j], memory_order_relaxed);
        if (!first)
            CPLRT(sbuf_putc)(&b, ',');
        first = 0;
        CPLRT(write_json_string)(&b, kind, strlen(kind));
        CPLRT(sbuf_putc)(&b, ':');
        CPLRT(sbuf_u64)(&b, sum);
    }
    CPLRT(sbuf_puts)(&b, "},\"builtins\":{");
    uint32_t nb = CPLRT(cov_nbi)();
    for (uint32_t i = 0; i < nb; i++) {
        uint64_t count = 0;
        const char *id = CPLRT(cov_bi_at)(i, &count);
        if (i > 0)
            CPLRT(sbuf_putc)(&b, ',');
        CPLRT(write_json_string)(&b, id, strlen(id));
        CPLRT(sbuf_putc)(&b, ':');
        CPLRT(sbuf_u64)(&b, count);
    }
    CPLRT(sbuf_puts)(&b, "},\"claims\":{");
    for (uint32_t i = 0; i < CPLRT_COV_CLAIMS; i++) {
        if (i > 0)
            CPLRT(sbuf_putc)(&b, ',');
        const char *name = CPLRT(cov_claim_names)[i];
        CPLRT(write_json_string)(&b, name, strlen(name));
        CPLRT(sbuf_putc)(&b, ':');
        CPLRT(sbuf_u64)(&b, CPLRT(cov_claims_at)(i));
    }
    CPLRT(sbuf_puts)(&b, "}}}");
    driver_send(&b);
#else
    cplrt_datum d;
    memset(&d, 0, sizeof d);
    d.key = "option";
    d.form = CPLRT_DATUM_STRING;
    d.str = "coverage";
    driver_error(st, CPLRT_INTEROP_OPTION_UNSUPPORTED, &d, 1);
#endif
}

static void driver_release_handles(driver_state *st)
{
    for (uint32_t i = st->nhandles; i > 0; i--)
        CPLRT(cap_unref)(st->handles[i - 1u]);
    st->nhandles = 0;
}

/* Closes the configured runtime: 0 when something of it is still live (a leak), with nothing freed. */
static int driver_close(driver_state *st)
{
    driver_release_handles(st);
    if (atomic_load(&st->rt->live_objects) != 0u)
        return 0;
    cpl_error *e = NULL;
    cpl_status s = CPLRT(api_table).cpl_runtime_close(&st->rt->pub, 0u, &e);
    CPLRT(api_table).cpl_error_release(e);
    if (s == CPL_OK)
        st->rt = NULL; /* closed: a failed reopen must not leave it to be used or closed again */
    return s == CPL_OK;
}

static int driver_open(driver_state *st)
{
    cpl_runtime_options o = CPL_RUNTIME_OPTIONS_INIT;
    o.max_call_depth = st->max_call_depth;
    o.max_heap_bytes = st->max_heap_bytes;
    cpl_runtime *rt = NULL;
    if (cplrt_runtime_open(&CPLPKG(program), &o, &rt, NULL) != CPL_OK)
        return 0;
    st->rt = (cplrt_runtime *)(void *)rt;
    if (st->worker_python != NULL || st->nworker_env > 0)
        (void)CPLRT(workers_configure)(st->rt, st->worker_python, (const char *const *)st->worker_env,
                                       st->nworker_env);
    return 1;
}

/* {"op": "workers", "python": "<path>", "env": {"KEY": "value", ...}} (spec/cabi.md 3.8; spec/python.md 6.5): the
   interpreter and the environment additions of later Python workers, kept across configure. */
static void driver_workers_free(char *python, char **entries, uint32_t n)
{
    free(python);
    if (entries != NULL)
        for (uint32_t i = 0; i < n; i++)
            free(entries[i]);
    free(entries);
}

static void driver_workers(driver_state *st, const cplrt_jv *req)
{
    const cplrt_jv *py = CPLRT(json_member)(req, "python");
    const cplrt_jv *env = CPLRT(json_member)(req, "env");
    if (py == NULL || py->type != CPLRT_JSON_STRING || py->len == 0 || memchr(py->str, '\0', py->len) != NULL ||
        (env != NULL && env->type != CPLRT_JSON_OBJECT)) {
        driver_invalid(st, "", "workers takes python (a path) and env (an object of strings)");
        return;
    }
    uint32_t n = env != NULL ? env->n : 0u;
    char *python = driver_strdup(py->str, py->len);
    char **entries = n > 0 ? (char **)calloc(n, sizeof *entries) : NULL;
    if (python == NULL || (n > 0 && entries == NULL))
        goto nomem;
    for (uint32_t i = 0; i < n; i++) {
        const cplrt_jv *k = &env->items[2u * i], *v = &env->items[2u * i + 1u];
        /* a NUL would end the C string early: "PATH\u0000x" must not set PATH */
        if (v->type != CPLRT_JSON_STRING || memchr(k->str, '=', k->len) != NULL || k->len == 0 ||
            memchr(k->str, '\0', k->len) != NULL || memchr(v->str, '\0', v->len) != NULL) {
            driver_workers_free(python, entries, n);
            driver_invalid(st, "/env", "an environment entry is a name without '=' or NUL and a string without NUL");
            return;
        }
        entries[i] = (char *)malloc(k->len + v->len + 2u);
        if (entries[i] == NULL)
            goto nomem;
        memcpy(entries[i], k->str, k->len);
        entries[i][k->len] = '=';
        memcpy(entries[i] + k->len + 1u, v->str, v->len);
        entries[i][k->len + 1u + v->len] = '\0';
    }
    if (CPLRT(workers_configure)(st->rt, python, (const char *const *)entries, n) != CPLRT_OK)
        goto nomem;
    /* Commit only after both copies were allocated: a failed update keeps the previous launch configuration. */
    driver_workers_free(st->worker_python, st->worker_env, st->nworker_env);
    st->worker_python = python;
    st->worker_env = entries;
    st->nworker_env = n;
    driver_send_text(st, "{\"ok\":null}");
    return;
nomem:
    driver_workers_free(python, entries, n);
    driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
}

static void driver_configure(driver_state *st, const cplrt_jv *req)
{
    uint32_t depth = 0;
    uint64_t heap = 0, budget = 0;
    const cplrt_jv *tb = CPLRT(json_member)(req, "time_budget_ms");
    const cplrt_jv *cc = CPLRT(json_member)(req, "check_contracts");
    if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "max_call_depth"), &depth) ||
        !CPLRT(jv_u64)(CPLRT(json_member)(req, "max_heap_bytes"), &heap) || tb == NULL ||
        (tb->type != CPLRT_JSON_NULL && !CPLRT(jv_u64)(tb, &budget)) || cc == NULL ||
        (cc->type != CPLRT_JSON_TRUE && cc->type != CPLRT_JSON_FALSE)) {
        driver_invalid(st, "", "configure takes max_call_depth, time_budget_ms, check_contracts, max_heap_bytes");
        return;
    }
    uint64_t handles = st->rt->live_handles, caps = st->rt->live_caps, objects = 0;
    if (!driver_close(st)) {
        objects = atomic_load(&st->rt->live_objects);
        cplrt_datum d[3];
        memset(d, 0, sizeof d);
        d[0].key = "live_handles";
        d[0].form = CPLRT_DATUM_NUMBER;
        d[0].num = handles + objects;
        d[1].key = "live_caps";
        d[1].form = CPLRT_DATUM_NUMBER;
        d[1].num = caps;
        d[2].key = "live_buffers";
        d[2].form = CPLRT_DATUM_NUMBER;
        d[2].num = st->rt->live_buffers;
        driver_error(st, CPLRT_INTEROP_RUNTIME_IN_USE, d, 3);
        return;
    }
    st->max_call_depth = depth;
    st->max_heap_bytes = heap;
    st->time_budget_ms = tb->type == CPLRT_JSON_NULL ? 0u : budget;
    st->check_contracts = cc->type == CPLRT_JSON_TRUE;
    st->poisoned = 0;
    if (!driver_open(st)) {
        driver_error(st, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        st->done = 1;
        st->exit_code = 2;
        return;
    }
    driver_send_text(st, "{\"ok\":null}");
}

/* ---- tables (3.8) ------------------------------------------------------------------------------------------------- */

static void driver_tables(driver_state *st, const cplrt_jv *req)
{
    const cplrt_program *p = st->rt->program;
    const cplrt_jv *table = CPLRT(json_member)(req, "table");
    const cplrt_jv *index = CPLRT(json_member)(req, "index");
    if (table == NULL || table->type != CPLRT_JSON_STRING) {
        driver_invalid(st, "/table", "the table is a string");
        return;
    }
    static const char *const names[10] = { "defs", "files", "sites", "clauses", "bsites",
                                           "holes", "refinements", "types", "exports", "grants" };
    uint32_t lens[10] = { p->ndefs, p->nfiles, p->nsites, p->nclauses, p->nbsites, p->nholes, p->nrefs, p->ntypes,
                          p->nexports, p->ngrants };
    int t = -1;
    for (int i = 0; i < 10; i++)
        if (strlen(names[i]) == table->len && memcmp(names[i], table->str, table->len) == 0)
            t = i;
    if (t < 0) {
        driver_invalid(st, "/table", "no such table");
        return;
    }
    if (index == NULL) {
        char line[48];
        snprintf(line, sizeof line, "{\"ok\":%u}", (unsigned)lens[t]);
        driver_send_text(st, line);
        return;
    }
    uint32_t i = 0;
    if (!CPLRT(jv_u32)(index, &i) || i >= lens[t]) {
        driver_invalid(st, "/index", "the index is out of range");
        return;
    }
    cplrt_sbuf b;
    driver_start(st, &b);
    CPLRT(sbuf_puts)(&b, "{\"ok\":");
#define DRV_STR(k, v)                                                                                                  \
    do {                                                                                                               \
        CPLRT(sbuf_puts)(&b, "\"" k "\":");                                                                            \
        if ((v) != NULL)                                                                                               \
            CPLRT(write_json_string)(&b, (v), strlen(v));                                                              \
        else                                                                                                           \
            CPLRT(sbuf_puts)(&b, "null");                                                                              \
    } while (0)
#define DRV_NUM(k, v)                                                                                                  \
    do {                                                                                                               \
        CPLRT(sbuf_puts)(&b, "\"" k "\":");                                                                            \
        if ((v) == CPLRT_NONE)                                                                                         \
            CPLRT(sbuf_puts)(&b, "null");                                                                              \
        else                                                                                                           \
            CPLRT(sbuf_u64)(&b, (v));                                                                                  \
    } while (0)
    switch (t) {
    case 0: {
        const cplrt_def *d = &p->defs[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("stable_id", d->stable_id);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("qualified_name", d->qualified_name);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("type_name", d->type_name);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("bare_name", d->bare_name);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("kind", d->kind);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("nmembers", d->nmembers);
        CPLRT(sbuf_puts)(&b, ",\"members\":[");
        for (uint32_t k = 0; k < d->nmembers; k++) {
            if (k > 0)
                CPLRT(sbuf_putc)(&b, ',');
            CPLRT(write_json_string)(&b, d->members[k], strlen(d->members[k]));
        }
        CPLRT(sbuf_puts)(&b, "],\"requires\":[");
        for (uint32_t k = 0; k < d->nrequires; k++) {
            if (k > 0)
                CPLRT(sbuf_putc)(&b, ',');
            CPLRT(sbuf_u64)(&b, d->requires[k]);
        }
        CPLRT(sbuf_puts)(&b, "]}");
        break;
    }
    case 1: CPLRT(write_json_string)(&b, p->files[i], strlen(p->files[i])); break;
    case 2: {
        const cplrt_site *s = &p->sites[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_NUM("file", s->file);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("start", s->start);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("end", s->end);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("line", s->line);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("col", s->col);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("end_line", s->end_line);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("end_col", s->end_col);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    case 3: {
        const cplrt_clause *c = &p->clauses[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("text", c->text);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("kind", c->kind);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("site", c->site);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("owner", c->owner);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("op", c->op);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("owner_id", c->owner_id);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("owner_qualified", c->owner_qualified);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("bsite", c->bsite);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    case 7: {
        const cplrt_type *ty = &p->types[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("text", ty->text);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("display", ty->display);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("form", ty->form);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("def", ty->def);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("elem", ty->elem);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("nmembers", ty->nmembers);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    default: {
        CPLRT(sbuf_putc)(&b, '{');
        DRV_NUM("index", i);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    case 4: {
        const cplrt_bsite *a = &p->bsites[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("text", a->text);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("site", a->site);
        CPLRT(sbuf_puts)(&b, ",\"bindings\":[");
        for (uint32_t k = 0; k < a->n; k++) {
            const cplrt_binding *x = &a->bindings[k];
            if (k > 0)
                CPLRT(sbuf_putc)(&b, ',');
            CPLRT(sbuf_putc)(&b, '{');
            DRV_STR("name", x->name);
            CPLRT(sbuf_putc)(&b, ',');
            DRV_STR("type", x->type);
            CPLRT(sbuf_putc)(&b, ',');
            DRV_STR("kind", x->kind);
            CPLRT(sbuf_putc)(&b, ',');
            DRV_NUM("is_mutable", x->is_mutable);
            CPLRT(sbuf_putc)(&b, ',');
            DRV_NUM("capability", x->capability);
            CPLRT(sbuf_putc)(&b, '}');
        }
        CPLRT(sbuf_puts)(&b, "],\"object\":[");
        for (uint32_t k = 0; k < a->nobject; k++) {
            if (k > 0)
                CPLRT(sbuf_putc)(&b, ',');
            CPLRT(sbuf_u64)(&b, a->object[k]);
        }
        CPLRT(sbuf_puts)(&b, "]}");
        break;
    }
    case 5: {
        /* cplrt_hole is also the macro of the hole helper (cpl_rt.h): the row is named through the array */
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("name", p->holes[i].name);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("expected_type", p->holes[i].expected_type);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("site", p->holes[i].site);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("bsite", p->holes[i].bsite);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    case 6: {
        const cplrt_refinement *r = &p->refinements[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_NUM("def", r->def);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("type_name", r->type_name);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("declared_name", r->declared_name);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("predicate", r->predicate);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("clause", r->clause);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("base", r->base);
        CPLRT(sbuf_puts)(&b, ",\"chain\":[");
        for (uint32_t k = 0; k < r->nchain; k++) {
            if (k > 0)
                CPLRT(sbuf_putc)(&b, ',');
            CPLRT(sbuf_u64)(&b, r->chain[k]);
        }
        CPLRT(sbuf_puts)(&b, "]}");
        break;
    }
    case 8: {
        const cplrt_export *x = &p->exports[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("symbol", x->symbol);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("function", x->function);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("nparams", x->nparams);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("nusing", x->nusing);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("split", x->split);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("ok_form", x->ok_form);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("err_form", x->err_form);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    case 9: {
        const cplrt_grant *g = &p->grants[i];
        CPLRT(sbuf_putc)(&b, '{');
        DRV_STR("binding", g->binding);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("capability", g->capability);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_STR("label", g->label);
        CPLRT(sbuf_putc)(&b, ',');
        DRV_NUM("declarative", g->declarative);
        CPLRT(sbuf_putc)(&b, '}');
        break;
    }
    }
#undef DRV_STR
#undef DRV_NUM
    CPLRT(sbuf_putc)(&b, '}');
    driver_send(&b);
}

/* ---- the loop ---------------------------------------------------------------------------------------------------- */

static void driver_request(driver_state *st, const char *line, size_t len)
{
    cplrt_jv *req = NULL;
    size_t at = 0;
    int r = CPLRT(json_parse)(st->proto, line, len, &req, &at);
    if (r != CPLRT_OK) {
        driver_invalid(st, "", r == CPLRT_NOMEM ? "the request could not be read" : "the request is not JSON");
        return;
    }
    const cplrt_jv *op = CPLRT(json_member)(req, "op");
    if (op == NULL || op->type != CPLRT_JSON_STRING) {
        CPLRT(json_free)(req);
        driver_invalid(st, "/op", "a request has an op");
        return;
    }
#define DRV_IS(name) (op->len == sizeof(name) - 1u && memcmp(op->str, name, op->len) == 0)
    if (DRV_IS("shutdown")) {
        /* the counters after the driver dropped every handle it numbered: what is still live is a leak (3.8) */
        driver_release_handles(st);
        driver_stats(st);
        cpl_error *e = NULL;
        (void)CPLRT(api_table).cpl_runtime_close(&st->rt->pub, CPL_CLOSE_FORCE, &e);
        CPLRT(api_table).cpl_error_release(e);
        st->rt = NULL;
        st->done = 1;
    } else if (DRV_IS("configure")) {
        driver_configure(st, req);
    } else if (st->poisoned || atomic_load(&st->rt->poisoned)) {
        driver_error(st, CPLRT_INTEROP_RUNTIME_POISONED, NULL, 0);
    } else if (DRV_IS("workers")) {
        driver_workers(st, req);
    } else if (DRV_IS("bind")) {
        driver_bind(st, req);
    } else if (DRV_IS("bind_scripted")) {
        driver_bind_scripted(st, req);
    } else if (DRV_IS("call")) {
        driver_call(st, req);
    } else if (DRV_IS("run_body")) {
        driver_run_body(st, req);
    } else if (DRV_IS("failing_refinement")) {
        driver_failing_refinement(st, req);
    } else if (DRV_IS("range_bound")) {
        driver_range_bound(st, req);
    } else if (DRV_IS("cap_call")) {
        driver_cap_call(st, req);
    } else if (DRV_IS("tables")) {
        driver_tables(st, req);
    } else if (DRV_IS("coverage")) {
        driver_coverage(st);
    } else if (DRV_IS("from_json")) {
        driver_from_json(st, req);
    } else if (DRV_IS("render")) {
        driver_render(st, req);
    } else if (DRV_IS("stats")) {
        driver_stats(st);
    } else {
        driver_invalid(st, "/op", "unknown op");
    }
#undef DRV_IS
    CPLRT(json_free)(req);
}

static void driver_loop(void *arg)
{
    driver_state *st = (driver_state *)arg;
    char *line = NULL;
    size_t cap = 0;
    while (!st->done) {
        ssize_t n = getline(&line, &cap, stdin);
        if (n < 0)
            break;
        while (n > 0 && (line[n - 1] == '\n' || line[n - 1] == '\r'))
            n--;
        if (n == 0)
            continue;
        driver_request(st, line, (size_t)n);
    }
    free(line);
}

int main(void)
{
    /* spec/python.md 6.6: re-executed as the C adapter host of a worker binding */
    const char *mode = getenv("CPL_WORKER_MODE");
    if (mode != NULL && strcmp(mode, "1") == 0)
        return CPLRT(worker_main)();
    CPLRT(worker_self) = 1;
    driver_state st;
    memset(&st, 0, sizeof st);
    st.max_call_depth = CPLRT_LIMIT_DEFAULT_MAX_CALL_DEPTH;
    st.check_contracts = 1;
    cpl_runtime *proto = NULL;
    if (cplrt_runtime_open(&CPLPKG(program), NULL, &proto, NULL) != CPL_OK || !driver_open(&st)) {
        fputs("cpl driver: cannot open a runtime\n", stderr);
        return 2;
    }
    st.proto = (cplrt_runtime *)(void *)proto;
    const cpl_library_info *info = &CPLPKG(program).info;
    char banner[256];
    snprintf(banner, sizeof banner, "{\"protocol\":%u,\"runtime\":\"%s\",\"api\":\"%u.%u\",\"program\":\"%s\"}",
             (unsigned)CPLRT_LIMIT_DRIVER_PROTOCOL, info->runtime_version, (unsigned)CPLRT_LIMIT_API_MAJOR,
             (unsigned)CPLRT_LIMIT_API_MINOR, info->program_digest);
    driver_write_line(banner, strlen(banner));
    if (CPLRT(run_on_thread)((size_t)CPLRT_LIMIT_EVAL_STACK_SIZE, driver_loop, &st) != CPLRT_OK)
        driver_loop(&st);
    if (st.rt != NULL) {
        driver_release_handles(&st);
        cpl_error *e = NULL;
        (void)CPLRT(api_table).cpl_runtime_close(&st.rt->pub, CPL_CLOSE_FORCE, &e);
        CPLRT(api_table).cpl_error_release(e);
    }
    free(st.handles);
    driver_workers_free(st.worker_python, st.worker_env, st.nworker_env);
    (void)CPLRT(api_table).cpl_runtime_close(proto, CPL_CLOSE_FORCE, NULL);
    return st.exit_code;
}
