/* cpl_rt_fault.c: fault records, the S34/S70/S71 claims, backtraces, and the violation record (spec/cabi.md 3.3.1),
   and every raising helper of cpl_rt.h that emitted code calls (1.4). Each helper fills a record with exactly the fields
   cpl_eval::Fault holds at the point where the evaluator fills them (crates/cpl-eval/src/fault.rs,
   crates/cpl-eval/src/interp.rs, crates/cpl-eval/src/interp/eval.rs, crates/cpl-eval/src/builtins/mod.rs); the
   writers of cpl_rt_json.c render it (3.3.2). Every allocation failure ends in CPLRT(oom), the preallocated memory
   record of semantics S76. File-scope names begin with fault_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <string.h>

/* ---- small helpers ------------------------------------------------------------------------------------------------ */

static const cplrt_def *fault_def(const cplrt_program *p, uint32_t def)
{
    return (p != NULL && def < p->ndefs) ? &p->defs[def] : NULL;
}

/* The innermost active declaration (Interpreter::current_def), NULL at the top level. */
static const cplrt_def *fault_current(const cplrt_cx *cx)
{
    return cx->top != NULL ? fault_def(cx->program, cx->top->def) : NULL;
}

static const cplrt_clause *fault_clause(const cplrt_cx *cx, uint32_t clause)
{
    return clause < cx->program->nclauses ? &cx->program->clauses[clause] : NULL;
}

static const char *fault_clause_kind_name(uint32_t ck)
{
    switch (ck) {
    case CPLRT_CK_REQUIRES: return "requires";
    case CPLRT_CK_ENSURES: return "ensures";
    case CPLRT_CK_INVARIANT: return "invariant";
    default: return "decreases";
    }
}

/* Ends a failed write: the sbuf is freed and the memory record becomes the fault. */
static int fault_sbuf_oom(cplrt_cx *cx, cplrt_sbuf *b)
{
    CPLRT(sbuf_free)(b);
    return CPLRT(oom)(cx);
}

static void fault_sbuf_start(cplrt_cx *cx, cplrt_sbuf *b)
{
    cplrt_sbuf_init(b, cx->rt);
    CPLRT(sbuf_put)(b, "", 0);
}

/* Renders tmpl into b (initialized here); CPLRT_OK, or the memory fault (b freed). */
static int fault_render(cplrt_cx *cx, cplrt_sbuf *b, const char *tmpl, const char *const *names,
                        const char *const *values, uint32_t n)
{
    fault_sbuf_start(cx, b);
    CPLRT(write_template)(b, tmpl, names, values, n);
    if (b->status != CPLRT_OK || b->ptr == NULL)
        return fault_sbuf_oom(cx, b);
    return CPLRT_OK;
}

/* ---- records ------------------------------------------------------------------------------------------------------- */

cplrt_fault *CPLRT(fault_raise)(cplrt_cx *cx, uint32_t kind, uint32_t clause_kind, uint32_t resource,
                                const char *message, size_t len, uint32_t span, const char *symbol,
                                const char *qualified_name)
{
    if (cx->fault != NULL) {
        CPLRT(fault_free)(cx->fault);
        cx->fault = NULL;
    }
    cplrt_fault *f = (cplrt_fault *)CPLRT(alloc)(cx->rt, sizeof *f + len + 1u);
    if (f == NULL) {
        CPLRT(oom)(cx);
        return NULL;
    }
    memset(f, 0, sizeof *f);
    char *m = (char *)(f + 1);
    if (len > 0)
        memcpy(m, message, len);
    m[len] = '\0';
    f->kind = kind;
    f->clause_kind = clause_kind;
    f->resource = resource;
    f->facet = CPLRT_FACET_NONE;
    f->span = span;
    f->owner = CPLRT_NONE;
    f->symbol = symbol;
    f->qualified_name = symbol != NULL ? (qualified_name != NULL ? qualified_name : symbol) : NULL;
    f->message = m;
    f->message_len = len;
    cx->fault = f;
    return f;
}

/* The datum of `key` in f, emptied for a new value (an earlier one is replaced, as Map::insert replaces it), or a new
   one. *slot NULL with CPLRT_OK when a static record has no room (the key is dropped); CPLRT_FAULT (the memory fault,
   f freed) when the array cannot grow. */
static int fault_slot(cplrt_cx *cx, cplrt_fault *f, const char *key, cplrt_datum **slot)
{
    *slot = NULL;
    for (uint32_t i = 0; i < f->ndata; i++) {
        if (f->data[i].key != NULL && strcmp(f->data[i].key, key) == 0) {
            cplrt_datum *d = &f->data[i];
            if (d->form == CPLRT_DATUM_JSON && !(f->flags & CPLRT_FAULT_STATIC))
                CPLRT(free)(d->json);
            d->json = NULL;
            d->len = 0;
            d->str = NULL;
            d->num = 0;
            *slot = d;
            return CPLRT_OK;
        }
    }
    if (f->ndata == f->data_cap) {
        if (f->flags & CPLRT_FAULT_STATIC)
            return CPLRT_OK;
        uint32_t cap = f->data_cap == 0 ? 8u : f->data_cap * 2u;
        cplrt_datum *grown = (cplrt_datum *)CPLRT(alloc)(cx->rt, (size_t)cap * sizeof *grown);
        if (grown == NULL)
            return CPLRT(oom)(cx);
        if (f->ndata > 0)
            memcpy(grown, f->data, (size_t)f->ndata * sizeof *grown);
        CPLRT(free)(f->data);
        f->data = grown;
        f->data_cap = cap;
    }
    cplrt_datum *d = &f->data[f->ndata++];
    memset(d, 0, sizeof *d);
    d->key = key;
    *slot = d;
    return CPLRT_OK;
}

int CPLRT(fault_set_str)(cplrt_cx *cx, cplrt_fault *f, const char *key, const char *str)
{
    cplrt_datum *d;
    if (fault_slot(cx, f, key, &d) != CPLRT_OK)
        return CPLRT_FAULT;
    if (d != NULL) {
        d->form = CPLRT_DATUM_STRING;
        d->str = str;
    }
    return CPLRT_OK;
}

int CPLRT(fault_set_num)(cplrt_cx *cx, cplrt_fault *f, const char *key, uint64_t num)
{
    cplrt_datum *d;
    if (fault_slot(cx, f, key, &d) != CPLRT_OK)
        return CPLRT_FAULT;
    if (d != NULL) {
        d->form = CPLRT_DATUM_NUMBER;
        d->num = num;
    }
    return CPLRT_OK;
}

int CPLRT(fault_set_json)(cplrt_cx *cx, cplrt_fault *f, const char *key, const char *json, size_t len)
{
    if (f->flags & CPLRT_FAULT_STATIC)
        return CPLRT_OK; /* a static record owns no text: the key is dropped */
    char *copy = (char *)CPLRT(alloc)(cx->rt, len + 1u);
    if (copy == NULL)
        return CPLRT(oom)(cx);
    if (len > 0)
        memcpy(copy, json, len);
    copy[len] = '\0';
    cplrt_datum *d;
    if (fault_slot(cx, f, key, &d) != CPLRT_OK) {
        CPLRT(free)(copy);
        return CPLRT_FAULT;
    }
    d->form = CPLRT_DATUM_JSON;
    d->json = copy;
    d->len = len;
    return CPLRT_OK;
}

int CPLRT(fault_push_frame)(cplrt_cx *cx, cplrt_fault *f, uint32_t def, uint32_t site)
{
    if (f->nframes == f->frames_cap) {
        if (f->flags & CPLRT_FAULT_STATIC)
            return CPLRT_OK; /* the frames a memory fault can record without allocating (3.3.1) */
        uint32_t cap = f->frames_cap == 0 ? 8u : f->frames_cap * 2u;
        if (cap < f->frames_cap)
            return CPLRT(oom)(cx);
        cplrt_btframe *grown = (cplrt_btframe *)CPLRT(alloc)(cx->rt, (size_t)cap * sizeof *grown);
        if (grown == NULL)
            return CPLRT(oom)(cx);
        if (f->nframes > 0)
            memcpy(grown, f->frames, (size_t)f->nframes * sizeof *grown);
        CPLRT(free)(f->frames);
        f->frames = grown;
        f->frames_cap = cap;
    }
    f->frames[f->nframes].def = def;
    f->frames[f->nframes].site = site;
    f->nframes++;
    return CPLRT_OK;
}

void CPLRT(fault_free)(cplrt_fault *f)
{
    if (f == NULL || (f->flags & CPLRT_FAULT_STATIC))
        return;
    for (uint32_t i = 0; i < f->ndata; i++)
        if (f->data[i].form == CPLRT_DATUM_JSON)
            CPLRT(free)(f->data[i].json);
    CPLRT(free)(f->data);
    CPLRT(free)(f->frames);
    CPLRT(free)(f);
}

int CPLRT(fault_copy)(cplrt_runtime *rt, const cplrt_fault *f, cplrt_fault **out)
{
    size_t len = f->message != NULL ? f->message_len : 0u;
    cplrt_fault *c = (cplrt_fault *)CPLRT(alloc)(rt, sizeof *c + len + 1u);
    if (c == NULL)
        return CPLRT_NOMEM;
    *c = *f;
    c->flags &= ~CPLRT_FAULT_STATIC;
    char *m = (char *)(c + 1);
    if (len > 0)
        memcpy(m, f->message, len);
    m[len] = '\0';
    c->message = m;
    c->message_len = len;
    c->frames = NULL;
    c->frames_cap = 0;
    c->nframes = 0;
    c->data = NULL;
    c->data_cap = 0;
    c->ndata = 0;
    if (f->nframes > 0) {
        c->frames = (cplrt_btframe *)CPLRT(alloc)(rt, (size_t)f->nframes * sizeof *c->frames);
        if (c->frames == NULL) {
            CPLRT(free)(c);
            return CPLRT_NOMEM;
        }
        memcpy(c->frames, f->frames, (size_t)f->nframes * sizeof *c->frames);
        c->nframes = c->frames_cap = f->nframes;
    }
    if (f->ndata > 0) {
        c->data = (cplrt_datum *)CPLRT(alloc)(rt, (size_t)f->ndata * sizeof *c->data);
        if (c->data == NULL) {
            CPLRT(fault_free)(c);
            return CPLRT_NOMEM;
        }
        c->data_cap = f->ndata;
        for (uint32_t i = 0; i < f->ndata; i++) {
            c->data[i] = f->data[i];
            if (f->data[i].form == CPLRT_DATUM_JSON) {
                c->data[i].json = NULL;
                char *j = (char *)CPLRT(alloc)(rt, f->data[i].len + 1u);
                if (j == NULL) {
                    c->ndata = i;
                    CPLRT(fault_free)(c);
                    return CPLRT_NOMEM;
                }
                if (f->data[i].len > 0)
                    memcpy(j, f->data[i].json, f->data[i].len);
                j[f->data[i].len] = '\0';
                c->data[i].json = j;
            }
            c->ndata = i + 1u;
        }
    }
    *out = c;
    return CPLRT_OK;
}

/* mirrors semantics S76 (spec/cabi.md 3.3.1, the memory record): no allocation */
int CPLRT(oom)(cplrt_cx *cx)
{
    cplrt_fault *m = cx->memory;
    if (cx->fault != NULL && cx->fault != m)
        CPLRT(fault_free)(cx->fault);
    cx->fault = NULL;
    if (m == NULL)
        return CPLRT_FAULT;
    const cplrt_def *d = fault_current(cx);
    m->kind = CPLRT_FK_RESOURCE_EXHAUSTION;
    m->clause_kind = 0;
    m->resource = CPLRT_RES_MEMORY;
    m->facet = CPLRT_FACET_NONE;
    m->span = CPLRT_NONE;
    m->flags = CPLRT_FAULT_STATIC;
    m->owner = CPLRT_NONE;
    m->symbol = d != NULL ? d->stable_id : NULL;
    m->qualified_name = d != NULL ? d->qualified_name : NULL;
    m->message = CPLRT_MSG_RESOURCE_MEMORY;
    m->message_len = sizeof(CPLRT_MSG_RESOURCE_MEMORY) - 1u;
    m->nframes = 0;
    if (m->data_cap >= 2u && m->data != NULL) {
        memset(m->data, 0, 2u * sizeof *m->data);
        m->data[0].key = "resource";
        m->data[0].form = CPLRT_DATUM_STRING;
        m->data[0].str = "memory";
        m->data[1].key = "limit";
        m->data[1].form = CPLRT_DATUM_NUMBER;
        m->data[1].num = cx->rt != NULL ? cx->rt->max_heap_bytes : 0u;
        m->ndata = 2;
    } else {
        m->ndata = 0;
    }
    cx->fault = m;
    return CPLRT_FAULT;
}

/* A new record blamed, as Interpreter::raise blames it, on the innermost active declaration (none at the top level;
   never for internal_error, whose symbol the caller passes NULL). */
static cplrt_fault *fault_new_here(cplrt_cx *cx, uint32_t kind, uint32_t ck, uint32_t resource, const char *message,
                                   size_t len, uint32_t span)
{
    const cplrt_def *d = kind == CPLRT_FK_INTERNAL_ERROR ? NULL : fault_current(cx);
    return CPLRT(fault_raise)(cx, kind, ck, resource, message, len, span, d != NULL ? d->stable_id : NULL,
                              d != NULL ? d->qualified_name : NULL);
}

/* Sets key to the JSON text of b and frees b; the memory fault when b failed. */
static int fault_set_sbuf(cplrt_cx *cx, cplrt_fault *f, const char *key, cplrt_sbuf *b)
{
    if (b->status != CPLRT_OK || b->ptr == NULL)
        return fault_sbuf_oom(cx, b);
    int st = CPLRT(fault_set_json)(cx, f, key, b->ptr, b->len);
    CPLRT(sbuf_free)(b);
    return st;
}

/* Sets key to a JSON string of the len bytes at s (copied). */
static int fault_set_text(cplrt_cx *cx, cplrt_fault *f, const char *key, const char *s, size_t len)
{
    cplrt_sbuf b;
    fault_sbuf_start(cx, &b);
    CPLRT(write_json_string)(&b, s, len);
    return fault_set_sbuf(cx, f, key, &b);
}

/* ---- value JSON objects --------------------------------------------------------------------------------------------- */

/* The order of n names: sorted[k] when the table gives it, else computed here (n is a parameter count). */
static uint32_t fault_nth_sorted(const char *const *names, const uint32_t *sorted, uint32_t n, uint32_t k)
{
    if (sorted != NULL)
        return sorted[k];
    /* the k-th smallest name, ties by index (the names are distinct) */
    for (uint32_t i = 0; i < n; i++) {
        uint32_t rank = 0;
        for (uint32_t j = 0; j < n; j++) {
            int c = strcmp(names[j], names[i]);
            if (c < 0 || (c == 0 && j < i))
                rank++;
        }
        if (rank == k)
            return i;
    }
    return k;
}

/* {"<name>": <value JSON>, ...} of n named words, keys in byte order (a serde_json Map), names[i] for values[i];
   only the first min(nnames, nvalues) pairs, as Rust's zip */
static void fault_write_object(cplrt_sbuf *b, const cplrt_program *p, const char *const *names, const uint32_t *sorted,
                               uint32_t nnames, const cplrt_w *values, uint32_t nvalues)
{
    CPLRT(sbuf_putc)(b, '{');
    int first = 1;
    for (uint32_t k = 0; k < nnames; k++) {
        uint32_t i = fault_nth_sorted(names, sorted, nnames, k);
        if (i >= nvalues || i >= nnames)
            continue;
        if (!first)
            CPLRT(sbuf_putc)(b, ',');
        first = 0;
        CPLRT(write_json_string)(b, names[i], strlen(names[i]));
        CPLRT(sbuf_putc)(b, ':');
        CPLRT(write_value_json)(b, p, values[i]);
    }
    CPLRT(sbuf_putc)(b, '}');
}

/* The value JSON of a visible local: a using parameter's entry is its capability reference, rendered with the type of
   its binding as value_to_json renders a handle ({"$handle": n, "$type": "<capability>"}). */
static void fault_write_local(cplrt_sbuf *b, const cplrt_program *p, const cplrt_binding *bd, cplrt_w w)
{
    if (bd->capability != CPLRT_NONE) {
        const cplrt_capref *h = (const cplrt_capref *)(uintptr_t)w;
        const cplrt_def *cap = fault_def(p, bd->capability);
        CPLRT(sbuf_puts)(b, "{\"$handle\":");
        CPLRT(sbuf_u64)(b, h != NULL ? h->handle : 0u);
        CPLRT(sbuf_puts)(b, ",\"$type\":");
        const char *t = cap != NULL && cap->type_name != NULL ? cap->type_name : "";
        CPLRT(write_json_string)(b, t, strlen(t));
        CPLRT(sbuf_putc)(b, '}');
        return;
    }
    CPLRT(write_value_json)(b, p, w);
}

/* The `bindings` array of assertion_failed and unresolved_hole (fault::binding_json, innermost first): name, type,
   kind, mutable (only true), value, written in byte order of the keys. */
static void fault_write_bindings(cplrt_sbuf *b, const cplrt_program *p, const cplrt_bsite *bs, const cplrt_w *visible,
                                 uint32_t n)
{
    CPLRT(sbuf_putc)(b, '[');
    uint32_t count = bs != NULL ? (bs->n < n ? bs->n : n) : 0u;
    for (uint32_t i = 0; i < count; i++) {
        const cplrt_binding *bd = &bs->bindings[i];
        if (i > 0)
            CPLRT(sbuf_putc)(b, ',');
        CPLRT(sbuf_puts)(b, "{\"kind\":");
        CPLRT(write_json_string)(b, bd->kind, strlen(bd->kind));
        if (bd->is_mutable)
            CPLRT(sbuf_puts)(b, ",\"mutable\":true");
        CPLRT(sbuf_puts)(b, ",\"name\":");
        CPLRT(write_json_string)(b, bd->name, strlen(bd->name));
        CPLRT(sbuf_puts)(b, ",\"type\":");
        CPLRT(write_json_string)(b, bd->type, strlen(bd->type));
        CPLRT(sbuf_puts)(b, ",\"value\":");
        fault_write_local(b, p, bd, visible[i]);
        CPLRT(sbuf_putc)(b, '}');
    }
    CPLRT(sbuf_putc)(b, ']');
}

/* The {<name>: <value JSON>} object of a loop violation's actual.bindings (Interpreter::bindings_object): one key per
   name, the innermost binding of each, in name order (the table's `object`). */
static void fault_write_bindings_object(cplrt_sbuf *b, const cplrt_program *p, const cplrt_bsite *bs,
                                        const cplrt_w *visible, uint32_t n)
{
    CPLRT(sbuf_putc)(b, '{');
    if (bs != NULL) {
        for (uint32_t k = 0; k < bs->nobject; k++) {
            uint32_t i = bs->object[k];
            if (i >= n || i >= bs->n)
                continue;
            const cplrt_binding *bd = &bs->bindings[i];
            if (k > 0)
                CPLRT(sbuf_putc)(b, ',');
            CPLRT(write_json_string)(b, bd->name, strlen(bd->name));
            CPLRT(sbuf_putc)(b, ':');
            fault_write_local(b, p, bd, visible[i]);
        }
    }
    CPLRT(sbuf_putc)(b, '}');
}

/* The `input` of a contract fault of a function activation (Interpreter::input_object): its positional parameters by
   name; nothing (CPLRT_OK) when the activation is not a function's (a test or property body has no input). */
static int fault_set_input(cplrt_cx *cx, cplrt_fault *f, const cplrt_frame *fr)
{
    if (fr == NULL)
        return CPLRT_OK;
    const cplrt_def *d = fault_def(cx->program, fr->def);
    if (d == NULL || d->kind != CPLRT_DEF_FN || (fr->params == NULL && fr->nparams > 0))
        return CPLRT_OK;
    cplrt_sbuf b;
    fault_sbuf_start(cx, &b);
    fault_write_object(&b, cx->program, d->members, d->sorted, d->nmembers, fr->params, fr->nparams);
    return fault_set_sbuf(cx, f, "input", &b);
}

/* ---- the raising helpers of cpl_rt_internal.h (R2) ------------------------------------------------------------------ */

/* mirrors cpl_eval::fault::Fault::division_by_zero, raised through Interpreter::raise at the operator */
int CPLRT(raise_division_by_zero)(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b)
{
    cplrt_fault *f = fault_new_here(cx, CPLRT_FK_DIVISION_BY_ZERO, 0, 0, CPLRT_MSG_DIVISION_BY_ZERO,
                                    sizeof(CPLRT_MSG_DIVISION_BY_ZERO) - 1u, site);
    if (f == NULL)
        return CPLRT_FAULT;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    CPLRT(sbuf_puts)(&s, "{\"a\":");
    CPLRT(int_write)(&s, a);
    CPLRT(sbuf_puts)(&s, ",\"b\":");
    CPLRT(int_write)(&s, b);
    CPLRT(sbuf_putc)(&s, '}');
    fault_set_sbuf(cx, f, "input", &s);
    return CPLRT_FAULT;
}

/* The common part of a contract_violation (Fault::contract_violation): message, facet contract, clause_kind,
   attributed_to, clause, status, the clause's span, and the blamed declaration. NULL after an allocation failure. */
static cplrt_fault *fault_contract(cplrt_cx *cx, uint32_t ck, const char *attributed, const char *clause_text,
                                   uint32_t span, const char *symbol, const char *qualified)
{
    static const char *const names[2] = { "clause_kind", "clause" };
    const char *values[2] = { fault_clause_kind_name(ck), clause_text };
    cplrt_sbuf m;
    if (fault_render(cx, &m, CPLRT_MSG_CONTRACT_VIOLATION, names, values, 2) != CPLRT_OK)
        return NULL;
    cplrt_fault *f =
        CPLRT(fault_raise)(cx, CPLRT_FK_CONTRACT_VIOLATION, ck, 0, m.ptr, m.len, span, symbol, qualified);
    CPLRT(sbuf_free)(&m);
    if (f == NULL)
        return NULL;
    f->facet = CPLRT_FACET_CONTRACT;
    if (CPLRT(fault_set_str)(cx, f, "clause_kind", fault_clause_kind_name(ck)) != CPLRT_OK ||
        CPLRT(fault_set_str)(cx, f, "attributed_to", attributed) != CPLRT_OK ||
        CPLRT(fault_set_str)(cx, f, "clause", clause_text) != CPLRT_OK ||
        CPLRT(fault_set_str)(cx, f, "status", "failed") != CPLRT_OK)
        return NULL;
    return f;
}

/* mirrors cpl_eval::builtins::requires_violation_at with the facts of requires_spec: the stub's in the program tables
   when the program declares it, else the cpl-rtspec fallback (CPLRT_BUILTINS) */
int CPLRT(raise_builtin_requires)(cplrt_cx *cx, uint32_t stub, const char *id, uint32_t clause, const cplrt_w *args,
                                  uint32_t n)
{
    const cplrt_program *p = cx->program;
    const cplrt_def *sd = fault_def(p, stub);
    const char *text = NULL, *symbol = NULL, *qualified = NULL;
    uint32_t span = CPLRT_NONE;
    const char *const *inputs = NULL;
    uint32_t ninputs = 0;
    if (sd != NULL && sd->kind == CPLRT_DEF_FN && clause < sd->nrequires && sd->requires != NULL &&
        sd->requires[clause] < p->nclauses) {
        const cplrt_clause *cl = &p->clauses[sd->requires[clause]];
        text = cl->text;
        span = cl->site;
        symbol = sd->stable_id;
        qualified = sd->qualified_name;
        inputs = sd->members;
        ninputs = sd->nmembers;
    } else {
        for (uint32_t i = 0; i < CPLRT_BUILTINS_N; i++) {
            const cplrt_builtin_spec *bs = &CPLRT_BUILTINS[i];
            if (strcmp(bs->id, id) != 0)
                continue;
            symbol = bs->fallback_symbol;
            qualified = bs->fallback_symbol; /* no definition has the identity: Fault::to_diagnostic uses it itself */
            if (clause < bs->nrequires) {
                text = bs->requires[clause].clause;
                inputs = bs->requires[clause].inputs;
                ninputs = bs->requires[clause].ninputs;
            }
            break;
        }
        if (text == NULL)
            text = "";
    }
    cplrt_fault *f = fault_contract(cx, CPLRT_CK_REQUIRES, "caller", text, span, symbol, qualified);
    if (f == NULL)
        return CPLRT_FAULT;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    /* a member stub's receiver is "self" already in the table (requires_spec) */
    fault_write_object(&s, p, inputs, NULL, ninputs, args, n);
    if (fault_set_sbuf(cx, f, "input", &s) != CPLRT_OK)
        return CPLRT_FAULT;
    const cplrt_def *caller = fault_current(cx);
    if (caller != NULL)
        CPLRT(fault_set_str)(cx, f, "caller", caller->qualified_name);
    return CPLRT_FAULT;
}

/* internal_error with a rendered detail (Fault::internal): no symbol, no facet; the span of site */
static int fault_internal_text(cplrt_cx *cx, uint32_t site, const char *kind, const char *detail)
{
    static const char *const names[2] = { "kind", "detail" };
    const char *values[2] = { kind, detail };
    cplrt_sbuf m;
    if (fault_render(cx, &m, CPLRT_MSG_INTERNAL_ERROR, names, values, 2) != CPLRT_OK)
        return CPLRT_FAULT;
    cplrt_fault *f = CPLRT(fault_raise)(cx, CPLRT_FK_INTERNAL_ERROR, 0, 0, m.ptr, m.len, site, NULL, NULL);
    CPLRT(sbuf_free)(&m);
    if (f == NULL)
        return CPLRT_FAULT;
    if (CPLRT(fault_set_str)(cx, f, "kind", kind) != CPLRT_OK)
        return CPLRT_FAULT;
    fault_set_text(cx, f, "detail", detail, strlen(detail));
    return CPLRT_FAULT;
}

int CPLRT(raise_internal)(cplrt_cx *cx, uint32_t site, const char *kind, const char *tmpl, const char *const *names,
                          const char *const *values, uint32_t n)
{
    cplrt_sbuf d;
    if (fault_render(cx, &d, tmpl, names, values, n) != CPLRT_OK)
        return CPLRT_FAULT;
    int st = fault_internal_text(cx, site, kind, d.ptr);
    CPLRT(sbuf_free)(&d);
    return st;
}

/* mirrors cpl_eval::interp::Interpreter::call_builtin's map_err: the call's span when the fault has none, and the
   frame {stub, span} */
int CPLRT(builtin_leave)(cplrt_cx *cx, uint32_t site, uint32_t stub, int st)
{
    if (st != CPLRT_FAULT || cx->cancelled || cx->fault == NULL)
        return st;
    /* the memory fault keeps no span (semantics S76) */
    if (cx->fault != cx->memory && cx->fault->span == CPLRT_NONE && site != CPLRT_NO_SITE)
        cx->fault->span = site;
    CPLRT(fault_push_frame)(cx, cx->fault, stub, site);
    return CPLRT_FAULT;
}

/* ---- the helpers of cpl_rt.h (emitted code) ------------------------------------------------------------------------ */

/* mirrors cpl_eval::interp::Interpreter::run_fn, its requires branch */
void cplrt_requires_failed(cplrt_cx *cx, uint32_t clause)
{
    const cplrt_clause *cl = fault_clause(cx, clause);
    if (cl == NULL) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such clause");
        return;
    }
    const cplrt_def *d = fault_def(cx->program, cl->owner);
    cplrt_fault *f = fault_contract(cx, CPLRT_CK_REQUIRES, "caller", cl->text, cl->site,
                                    d != NULL ? d->stable_id : cl->owner_id,
                                    d != NULL ? d->qualified_name : cl->owner_qualified);
    if (f == NULL)
        return;
    if (fault_set_input(cx, f, cx->top) != CPLRT_OK)
        return;
    /* the frame below this activation is the caller; the command line has none (S39 (c)) */
    if (cx->top != NULL && cx->top->prev != NULL) {
        const cplrt_def *caller = fault_def(cx->program, cx->top->prev->def);
        if (caller != NULL)
            CPLRT(fault_set_str)(cx, f, "caller", caller->qualified_name);
    }
}

/* Records a copy of the violation just raised (the evaluator's `violation` field), replacing an earlier one. */
static void fault_record_violation(cplrt_cx *cx, uint32_t owner)
{
    if (cx->fault == NULL || (cx->fault->flags & CPLRT_FAULT_STATIC))
        return;
    cplrt_fault *copy = NULL;
    if (CPLRT(fault_copy)(cx->rt, cx->fault, &copy) != CPLRT_OK) {
        CPLRT(oom)(cx);
        return;
    }
    copy->owner = owner;
    CPLRT(fault_free)(cx->violation);
    cx->violation = copy;
}

/* mirrors cpl_eval::interp::Interpreter::check_ensures */
void cplrt_ensures_failed(cplrt_cx *cx, uint32_t clause, cplrt_w result)
{
    const cplrt_clause *cl = fault_clause(cx, clause);
    if (cl == NULL) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such clause");
        return;
    }
    const cplrt_def *d = fault_def(cx->program, cl->owner);
    cplrt_fault *f = fault_contract(cx, CPLRT_CK_ENSURES, "body", cl->text, cl->site,
                                    d != NULL ? d->stable_id : cl->owner_id,
                                    d != NULL ? d->qualified_name : cl->owner_qualified);
    if (f == NULL)
        return;
    if (fault_set_input(cx, f, cx->top) != CPLRT_OK)
        return;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    CPLRT(write_value_json)(&s, cx->program, result);
    if (fault_set_sbuf(cx, f, "actual", &s) != CPLRT_OK)
        return;
    fault_record_violation(cx, cl->owner);
}

/* Constructor invariants are mandatory even when optional function contracts are disabled. */
void cplrt_constructor_failed(cplrt_cx *cx, uint32_t clause, cplrt_w result)
{
    const cplrt_clause *cl = fault_clause(cx, clause);
    if (cl == NULL) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such clause");
        return;
    }
    const cplrt_def *d = fault_def(cx->program, cl->owner);
    cplrt_fault *f = fault_contract(cx, CPLRT_CK_INVARIANT, "body", cl->text, cl->site,
                                    d != NULL ? d->stable_id : cl->owner_id,
                                    d != NULL ? d->qualified_name : cl->owner_qualified);
    if (f == NULL)
        return;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    CPLRT(write_value_json)(&s, cx->program, result);
    (void)fault_set_sbuf(cx, f, "actual", &s);
}

/* The actual object of a decreases violation: {"measure": m, "previous": p} (previous only when known and m is not
   negative), with the loop's bindings first when bs is given. */
static void fault_write_measure_actual(cplrt_sbuf *b, const cplrt_program *p, const cplrt_bsite *bs,
                                       const cplrt_w *visible, uint32_t n, int with_bindings, int has_measure,
                                       cplrt_w measure, cplrt_w previous)
{
    CPLRT(sbuf_putc)(b, '{');
    int comma = 0;
    if (with_bindings) {
        CPLRT(sbuf_puts)(b, "\"bindings\":");
        fault_write_bindings_object(b, p, bs, visible, n);
        comma = 1;
    }
    if (has_measure) {
        if (comma)
            CPLRT(sbuf_putc)(b, ',');
        CPLRT(sbuf_puts)(b, "\"measure\":");
        CPLRT(int_write)(b, measure);
        if (previous != 0) {
            CPLRT(sbuf_puts)(b, ",\"previous\":");
            CPLRT(int_write)(b, previous);
        }
    }
    CPLRT(sbuf_putc)(b, '}');
}

/* mirrors cpl_eval::interp::Interpreter::enter_measure (semantics S35) */
int cplrt_header_measure(cplrt_cx *cx, cplrt_frame *fr, uint32_t clause, cplrt_w measure)
{
    cplrt_w previous = 0;
    for (const cplrt_frame *g = fr->prev; g != NULL; g = g->prev) {
        if (g->def == fr->def) {
            previous = g->measure; /* the nearest enclosing activation of the same function: its record, maybe none */
            break;
        }
    }
    int negative = cplrt_int_lt_small(measure, 0);
    if (negative || (previous != 0 && cplrt_int_cmp(measure, previous) >= 0)) {
        const cplrt_clause *cl = fault_clause(cx, clause);
        if (cl == NULL)
            return CPLRT(raise_internal)(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such clause",
                                         NULL, NULL, 0);
        const cplrt_def *d = fault_def(cx->program, fr->def);
        cplrt_fault *f = fault_contract(cx, CPLRT_CK_DECREASES, "body", cl->text, cl->site,
                                        d != NULL ? d->stable_id : cl->owner_id,
                                        d != NULL ? d->qualified_name : cl->owner_qualified);
        if (f == NULL)
            return CPLRT_FAULT;
        cplrt_sbuf s;
        fault_sbuf_start(cx, &s);
        fault_write_measure_actual(&s, cx->program, NULL, NULL, 0, 0, 1, measure, negative ? 0 : previous);
        if (fault_set_sbuf(cx, f, "actual", &s) != CPLRT_OK)
            return CPLRT_FAULT;
        if (fault_set_input(cx, f, fr) != CPLRT_OK)
            return CPLRT_FAULT;
        fault_record_violation(cx, fr->def);
        return CPLRT_FAULT;
    }
    cplrt_retain(measure);
    cplrt_release(fr->measure);
    fr->measure = measure;
    return CPLRT_OK;
}

/* mirrors cpl_eval::interp::Interpreter::loop_violation */
static void fault_loop(cplrt_cx *cx, uint32_t ck, uint32_t clause, const cplrt_w *visible, uint32_t n,
                       uint64_t iteration, int has_measure, cplrt_w measure, cplrt_w previous)
{
    const cplrt_clause *cl = fault_clause(cx, clause);
    if (cl == NULL) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such clause");
        return;
    }
    const cplrt_def *d = fault_current(cx);
    cplrt_fault *f = fault_contract(cx, ck, "body", cl->text, cl->site, d != NULL ? d->stable_id : NULL,
                                    d != NULL ? d->qualified_name : NULL);
    if (f == NULL)
        return;
    if (CPLRT(fault_set_num)(cx, f, "iteration", iteration) != CPLRT_OK)
        return;
    const cplrt_bsite *bs = cl->bsite < cx->program->nbsites ? &cx->program->bsites[cl->bsite] : NULL;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    fault_write_measure_actual(&s, cx->program, bs, visible, n, 1, has_measure, measure, previous);
    if (fault_set_sbuf(cx, f, "actual", &s) != CPLRT_OK)
        return;
    if (d != NULL) {
        if (fault_set_input(cx, f, cx->top) != CPLRT_OK)
            return;
        fault_record_violation(cx, cx->top->def);
    }
}

void cplrt_invariant_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *visible, uint32_t n, uint64_t iteration)
{
    fault_loop(cx, CPLRT_CK_INVARIANT, clause, visible, n, iteration, 0, 0, 0);
}

/* mirrors the decreases branch of cpl_eval::interp::Interpreter::exec_while */
int cplrt_loop_measure(cplrt_cx *cx, uint32_t clause, const cplrt_w *visible, uint32_t n, uint64_t iteration,
                       cplrt_w measure, cplrt_w *previous)
{
    int negative = cplrt_int_lt_small(measure, 0);
    if (negative || (*previous != 0 && cplrt_int_cmp(measure, *previous) >= 0)) {
        fault_loop(cx, CPLRT_CK_DECREASES, clause, visible, n, iteration, 1, measure, negative ? 0 : *previous);
        return CPLRT_FAULT;
    }
    CPLRT_COPY(*previous, measure);
    return CPLRT_OK;
}

/* The operation of an operation clause: its capability's op entry. */
static const cplrt_op *fault_op(const cplrt_cx *cx, const cplrt_clause *cl)
{
    const cplrt_def *cap = fault_def(cx->program, cl->owner);
    if (cap == NULL || cap->kind != CPLRT_DEF_CAPABILITY || cap->ops == NULL || cl->op >= cap->nmembers)
        return NULL;
    return &cap->ops[cl->op];
}

/* mirrors cpl_eval::interp::Interpreter::check_op_requires */
void cplrt_op_requires_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *args, uint32_t n)
{
    const cplrt_clause *cl = fault_clause(cx, clause);
    const cplrt_op *op = cl != NULL ? fault_op(cx, cl) : NULL;
    if (op == NULL) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such operation clause");
        return;
    }
    /* an operation identity names no definition: its qualified_name is the identity itself (Fault::to_diagnostic) */
    cplrt_fault *f = fault_contract(cx, CPLRT_CK_REQUIRES, "caller", cl->text, cl->site, op->identity, op->identity);
    if (f == NULL)
        return;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    fault_write_object(&s, cx->program, op->params, op->sorted, op->nparams, args, n);
    if (fault_set_sbuf(cx, f, "input", &s) != CPLRT_OK)
        return;
    const cplrt_def *caller = fault_current(cx);
    if (caller != NULL)
        CPLRT(fault_set_str)(cx, f, "caller", caller->qualified_name);
}

/* mirrors cpl_eval::interp::Interpreter::check_op_ensures; never recorded as the violation (toolchain 1.7.3) */
void cplrt_op_ensures_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *args, uint32_t n, cplrt_w result)
{
    const cplrt_clause *cl = fault_clause(cx, clause);
    const cplrt_op *op = cl != NULL ? fault_op(cx, cl) : NULL;
    if (op == NULL) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_clause", "the clause table has no such operation clause");
        return;
    }
    cplrt_fault *f = fault_contract(cx, CPLRT_CK_ENSURES, "binding", cl->text, cl->site, op->identity, op->identity);
    if (f == NULL)
        return;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    fault_write_object(&s, cx->program, op->params, op->sorted, op->nparams, args, n);
    if (fault_set_sbuf(cx, f, "input", &s) != CPLRT_OK)
        return;
    fault_sbuf_start(cx, &s);
    CPLRT(write_value_json)(&s, cx->program, result);
    fault_set_sbuf(cx, f, "actual", &s);
}

/* mirrors cpl_eval::interp::Interpreter::eval_contract_expr (semantics S34, S70): the innermost clause a fault escapes
   claims it, never an internal_error, never during a cancellation unwind (3.6) */
void cplrt_claim(cplrt_cx *cx, uint32_t clause)
{
    if (cx->cancelled || cx->fault == NULL || cx->fault->kind == CPLRT_FK_INTERNAL_ERROR)
        return;
    if (cx->attributed) {
        CPLRT(cov_claim)(CPLRT_COV_ALREADY_CLAIMED);      /* S70: an inner clause claimed it */
        return;
    }
    const cplrt_clause *cl = fault_clause(cx, clause);
    cx->attributed = 1;
    if (cl == NULL)
        return;
    /* the kind of the claim site (3.8 coverage): requires, ensures, header_decreases, invariant, loop_decreases,
       where, op_requires, op_ensures, range */
    uint32_t site = 0;
    switch (cl->kind) {
    case CPLRT_CK_REQUIRES: site = cl->op != CPLRT_NONE ? 6u : 0u; break;
    case CPLRT_CK_ENSURES: site = cl->op != CPLRT_NONE ? 7u : 1u; break;
    case CPLRT_CK_INVARIANT: site = 3u; break;
    case CPLRT_CK_DECREASES: site = cl->bsite != CPLRT_NONE ? 4u : 2u; break;
    case CPLRT_CK_WHERE: site = 5u; break;
    default: site = 8u; break;
    }
    CPLRT(cov_claim)(site);
    cplrt_fault *f = cx->fault;
    if (cl->owner_id != NULL) {
        f->symbol = cl->owner_id;
        f->qualified_name = cl->owner_qualified != NULL ? cl->owner_qualified : cl->owner_id;
    }
    f->facet = CPLRT_FACET_CONTRACT;
    CPLRT(fault_set_str)(cx, f, "clause", cl->text);
}

/* mirrors cpl_eval::interp::Interpreter::eval_range_expr's claim (semantics S71): the property, with the range text
   the driver's request carries */
void cplrt_claim_range(cplrt_cx *cx, uint32_t property)
{
    if (cx->cancelled || cx->fault == NULL || cx->fault->kind == CPLRT_FK_INTERNAL_ERROR)
        return;
    if (cx->attributed) {
        CPLRT(cov_claim)(CPLRT_COV_ALREADY_CLAIMED);
        return;
    }
    cx->attributed = 1;
    CPLRT(cov_claim)(8u);
    cplrt_fault *f = cx->fault;
    const cplrt_def *d = fault_def(cx->program, property);
    if (d != NULL) {
        f->symbol = d->stable_id;
        f->qualified_name = d->qualified_name;
    }
    f->facet = CPLRT_FACET_CONTRACT;
    const char *text = cx->range_text != NULL ? cx->range_text : "";
    fault_set_text(cx, f, "clause", text, strlen(text));
}

/* mirrors cpl_eval::interp::Interpreter::eval_refine (T39) */
void cplrt_refine_unsound(cplrt_cx *cx, uint32_t site, uint32_t refinement, cplrt_w v)
{
    const cplrt_program *p = cx->program;
    const cplrt_refinement *r = refinement < p->nrefs ? &p->refinements[refinement] : NULL;
    const cplrt_def *d = r != NULL ? fault_def(p, r->def) : NULL;
    cplrt_sbuf vt;
    fault_sbuf_start(cx, &vt);
    CPLRT(write_value_text)(&vt, p, v);
    if (vt.status != CPLRT_OK || vt.ptr == NULL) {
        fault_sbuf_oom(cx, &vt);
        return;
    }
    static const char *const names[3] = { "value", "refinement", "predicate" };
    const char *values[3] = { vt.ptr, d != NULL && d->type_name != NULL ? d->type_name : "",
                              r != NULL && r->predicate != NULL ? r->predicate : "" };
    CPLRT(raise_internal)(cx, site, "refinement_proof_unsound", CPLRT_MSG_DETAIL_REFINEMENT_PROOF_UNSOUND, names, values,
                          3);
    CPLRT(sbuf_free)(&vt);
}

/* mirrors the Stmt::Assert arm of cpl_eval::interp::Interpreter::exec_stmt */
void cplrt_assert_failed(cplrt_cx *cx, uint32_t bsite, const cplrt_w *visible, uint32_t n)
{
    const cplrt_program *p = cx->program;
    const cplrt_bsite *bs = bsite < p->nbsites ? &p->bsites[bsite] : NULL;
    const char *text = bs != NULL && bs->text != NULL ? bs->text : "";
    static const char *const names[1] = { "clause" };
    const char *values[1] = { text };
    cplrt_sbuf m;
    if (fault_render(cx, &m, CPLRT_MSG_ASSERTION_FAILED, names, values, 1) != CPLRT_OK)
        return;
    cplrt_fault *f = fault_new_here(cx, CPLRT_FK_ASSERTION_FAILED, 0, 0, m.ptr, m.len,
                                    bs != NULL ? bs->site : CPLRT_NONE);
    CPLRT(sbuf_free)(&m);
    if (f == NULL)
        return;
    if (CPLRT(fault_set_str)(cx, f, "clause", text) != CPLRT_OK)
        return;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    fault_write_bindings(&s, p, bs, visible, n);
    fault_set_sbuf(cx, f, "bindings", &s);
}

/* mirrors cpl_eval::interp::Interpreter::raise_hole (semantics 3.5). (The hole table's element type is spelled through
   the table: cpl_rt.h defines the name cplrt_hole as the macro of this function.) */
void cplrt_hole(cplrt_cx *cx, uint32_t hole, const cplrt_w *visible, uint32_t n)
{
    const cplrt_program *p = cx->program;
    if (hole >= p->nholes) {
        cplrt_internal(cx, CPLRT_NO_SITE, "unknown_hole", "the hole table has no such hole");
        return;
    }
    const char *name = p->holes[hole].name, *expected = p->holes[hole].expected_type;
    uint32_t site = p->holes[hole].site, bsite = p->holes[hole].bsite;
    static const char *const names[1] = { "hole" };
    const char *values[1] = { name };
    cplrt_sbuf m;
    if (fault_render(cx, &m, CPLRT_MSG_UNRESOLVED_HOLE, names, values, 1) != CPLRT_OK)
        return;
    cplrt_fault *f = fault_new_here(cx, CPLRT_FK_UNRESOLVED_HOLE, 0, 0, m.ptr, m.len, site);
    CPLRT(sbuf_free)(&m);
    if (f == NULL)
        return;
    if (CPLRT(fault_set_str)(cx, f, "hole", name) != CPLRT_OK ||
        CPLRT(fault_set_str)(cx, f, "expected_type", expected) != CPLRT_OK)
        return;
    const cplrt_bsite *bs = bsite < p->nbsites ? &p->bsites[bsite] : NULL;
    cplrt_sbuf s;
    fault_sbuf_start(cx, &s);
    fault_write_bindings(&s, p, bs, visible, n);
    fault_set_sbuf(cx, f, "bindings", &s);
}

/* mirrors the no-arm branch of cpl_eval::interp::Interpreter::eval_match */
void cplrt_no_match(cplrt_cx *cx, uint32_t site, cplrt_w scrutinee)
{
    cplrt_sbuf vt;
    fault_sbuf_start(cx, &vt);
    CPLRT(write_value_text)(&vt, cx->program, scrutinee);
    if (vt.status != CPLRT_OK || vt.ptr == NULL) {
        fault_sbuf_oom(cx, &vt);
        return;
    }
    static const char *const names[1] = { "value" };
    const char *values[1] = { vt.ptr };
    CPLRT(raise_internal)(cx, site, "no_matching_arm", CPLRT_MSG_DETAIL_NO_MATCHING_ARM, names, values, 1);
    CPLRT(sbuf_free)(&vt);
}

void cplrt_internal(cplrt_cx *cx, uint32_t site, const char *kind, const char *detail)
{
    fault_internal_text(cx, site, kind, detail != NULL ? detail : "");
}

/* mirrors crates/cpl-eval/src/builtins/gen.rs: gen_range's arity, argument types, and precondition, then the
   generator's or the builtin type's internal_error, as call_builtin reports it */
int cplrt_bi_uncallable(cplrt_cx *cx, uint32_t site, uint32_t stub, const char *id, const cplrt_w *args, uint32_t n)
{
    int st;
    int generator = strncmp(id, "gen_", 4) == 0;
    if (strcmp(id, "gen_range") == 0 && n != 2) {
        char expected[4] = "2", found[24];
        int k = 0;
        uint64_t v = n;
        char digits[24];
        do {
            digits[k++] = (char)('0' + (int)(v % 10u));
            v /= 10u;
        } while (v != 0);
        int j = 0;
        while (k > 0)
            found[j++] = digits[--k];
        found[j] = '\0';
        static const char *const names[3] = { "id", "expected", "found" };
        const char *values[3] = { id, expected, found };
        st = CPLRT(raise_internal)(cx, CPLRT_NONE, "builtin_arity", CPLRT_MSG_DETAIL_BUILTIN_ARITY, names, values, 3);
    } else if (strcmp(id, "gen_range") == 0 && (!(cplrt_is_small(args[0]) || (cplrt_is_obj(args[0]) &&
                                                 cplrt_obj_of(args[0])->kind == CPLRT_BIGINT)) ||
                                               !(cplrt_is_small(args[1]) || (cplrt_is_obj(args[1]) &&
                                                 cplrt_obj_of(args[1])->kind == CPLRT_BIGINT)))) {
        int bad = !(cplrt_is_small(args[0]) || (cplrt_is_obj(args[0]) && cplrt_obj_of(args[0])->kind == CPLRT_BIGINT))
                      ? 0
                      : 1;
        static const char *const names[4] = { "index", "id", "found", "expected" };
        const char *values[4] = { bad == 0 ? "0" : "1", id, CPLRT(kind_name)(args[bad]), "an Int" };
        st = CPLRT(raise_internal)(cx, CPLRT_NONE, "type_confusion", CPLRT_MSG_DETAIL_TYPE_CONFUSION_BUILTIN, names,
                                   values, 4);
    } else if (strcmp(id, "gen_range") == 0 && cplrt_int_cmp(args[0], args[1]) > 0) {
        st = CPLRT(raise_builtin_requires)(cx, stub, "gen_range", 0, args, 2);
    } else {
        static const char *const names[1] = { "id" };
        const char *values[1] = { id };
        st = CPLRT(raise_internal)(cx, CPLRT_NONE, generator ? "generator_not_a_value" : "builtin_type_called",
                                   generator ? CPLRT_MSG_DETAIL_GENERATOR_NOT_A_VALUE
                                             : CPLRT_MSG_DETAIL_BUILTIN_TYPE_CALLED,
                                   names, values, 1);
    }
    return CPLRT(builtin_leave)(cx, site, stub, st);
}
