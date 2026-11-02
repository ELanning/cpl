/* cpl_rt_cap.c: capability objects, the callback protocol, result validation, and ForeignError construction
   (spec/cabi.md 5.7, 6.4, 6.5; semantics S77, S80; stdlib.md S45). The C counterpart of crates/cpl-eval/src/boundary.rs:
   cplrt_cap_invoke reserves the handle slots of a callback's arguments, runs the binding layer, maps what it reports,
   and validates its result before CPL code sees it. File-scope names begin with cap_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdlib.h>
#include <string.h>

/* ---- capability objects (5.7) --------------------------------------------------------------------------------- */

struct cplrt_capobj {
    cplrt_capref ref;              /* first: what emitted code receives for a using parameter */
    _Atomic uint32_t rc;
    uint32_t dropped;              /* the drop or fini ran (CPL_CLOSE_FORCE runs it before the last reference) */
    cplrt_runtime *rt;
    char *label;                   /* the binding name of semantics 6.9 (owned) */
    void *ops;                     /* the runtime's copy of the typed or generic operation table, or NULL */
    cplrt_capobj *prev, *next;     /* the capability registry, in creation order */
};

/* The binding name of a capability: "host" for one created through the C API, the grant's binding name, or the label
   of a driver's scripted host (cabi.md 6.4). */
const char *CPLRT(cap_label)(const cplrt_capref *cap)
{
    return ((const cplrt_capobj *)(const void *)cap)->label;
}

/* A new capability object with one reference: registered last in the runtime's registry, numbered by creation order
   from 0. `label` is copied; `ops` (allocated by CPLRT(alloc), possibly NULL) passes to the object. NULL when an
   allocation fails (ops then stays the caller's). */
cplrt_capobj *CPLRT(cap_new_obj)(cplrt_runtime *rt, uint32_t capability, const cplrt_capimpl *impl, void *user,
                                 const char *label, void *ops)
{
    cplrt_capobj *c = (cplrt_capobj *)CPLRT(alloc)(rt, sizeof *c);
    if (c == NULL)
        return NULL;
    size_t n = strlen(label);
    c->label = (char *)CPLRT(alloc)(rt, n + 1u);
    if (c->label == NULL) {
        CPLRT(free)(c);
        return NULL;
    }
    memcpy(c->label, label, n + 1u);
    c->ref.impl = impl;
    c->ref.user = user;
    c->ref.capability = capability;
    atomic_init(&c->rc, 1u);
    c->dropped = 0;
    c->rt = rt;
    c->ops = ops;
    pthread_mutex_lock(&rt->lock);
    c->ref.handle = rt->next_cap_handle++;
    c->prev = rt->caps_last;
    c->next = NULL;
    if (rt->caps_last != NULL)
        rt->caps_last->next = c;
    else
        rt->caps_first = c;
    rt->caps_last = c;
    pthread_mutex_unlock(&rt->lock);
    return c;
}

void CPLRT(cap_retain)(cplrt_capobj *cap)
{
    if (cap != NULL)
        atomic_fetch_add_explicit(&cap->rc, 1u, memory_order_relaxed);
}

/* Runs the object's drop or fini once, with no runtime lock held (5.7). */
void CPLRT(cap_run_drop)(cplrt_capobj *cap)
{
    if (cap->dropped)
        return;
    cap->dropped = 1;
    if (cap->ref.impl != NULL && cap->ref.impl->drop != NULL)
        cap->ref.impl->drop(&cap->ref);
}

/* Marks the drop as done without running it: a constructor that fails after creating the object keeps the host's
   user data (5.7). */
void CPLRT(cap_run_drop_disarm)(cplrt_capobj *cap)
{
    cap->dropped = 1;
}

void CPLRT(cap_unref)(cplrt_capobj *cap)
{
    if (cap == NULL)
        return;
    if (atomic_fetch_sub_explicit(&cap->rc, 1u, memory_order_acq_rel) != 1u)
        return;
    cplrt_runtime *rt = cap->rt;
    CPLRT(cap_run_drop)(cap);
    pthread_mutex_lock(&rt->lock);
    if (cap->prev != NULL)
        cap->prev->next = cap->next;
    else
        rt->caps_first = cap->next;
    if (cap->next != NULL)
        cap->next->prev = cap->prev;
    else
        rt->caps_last = cap->prev;
    pthread_mutex_unlock(&rt->lock);
    CPLRT(free)(cap->ops);
    CPLRT(free)(cap->label);
    CPLRT(free)(cap);
}

const cplrt_capref *CPLRT(cap_ref)(const cplrt_capobj *cap)
{
    return &cap->ref;
}

const void *cplrt_cap_ops(const cplrt_capref *cap)
{
    return ((const cplrt_capobj *)(const void *)cap)->ops;
}

/* ---- the call of one capability operation (5.7) ---------------------------------------------------------------- */

typedef struct cap_detail {
    char *key, *value;
    size_t key_len, value_len;
} cap_detail;

typedef struct cap_call {
    cpl_call pub;                  /* first: { api } */
    cplrt_runtime *rt;
    cplrt_cx *cx;
    uint32_t op;
    uint32_t site;                 /* the operation call's site */
    const cplrt_capref *cap;
    int raised;                    /* the binding layer raised its own fault (a scripted host without a response) */
    uint32_t *slots;               /* the handle slots cplrt_cap_invoke reserved for the arguments */
    uint32_t nslots, used;
    int failed;                    /* cpl_call_foreign_error was called */
    int32_t kind;
    char *message;
    size_t message_len;
    cap_detail *details;
    uint32_t ndetails, details_cap;
} cap_call;

extern const cpl_api CPLRT(api_table);

cplrt_runtime *CPLRT(call_rt)(cpl_call *call)
{
    return ((cap_call *)(void *)call)->rt;
}

uint32_t CPLRT(call_op)(cpl_call *call)
{
    return ((cap_call *)(void *)call)->op;
}

int32_t CPLRT(call_is_cancelled)(cpl_call *call)
{
    const cap_call *c = (const cap_call *)(const void *)call;
    if (c->cx == NULL || c->cx->cancel == NULL)
        return 0;
    return atomic_load_explicit((_Atomic uint32_t *)(uintptr_t)c->cx->cancel, memory_order_relaxed) != 0u;
}

cpl_value cplrt_call_borrow(cpl_call *call, cplrt_w w)
{
    cap_call *c = (cap_call *)(void *)call;
    if (c->used >= c->nslots) {
        cpl_value none = { { 0u, 0u } };
        return none;
    }
    return CPLRT(handle_borrow)(c->rt, c->slots[c->used++], w);
}

/* The 5.7 result rule: on CPL_OK the result must be an owned value handle of the runtime, which passes to the
   runtime; on any other status a valid handle left in it is released and nothing else is read. Returns the status
   cplrt_cap_invoke interprets: CPL_OK with *out, or the callback's status (CPL_E_INTERNAL for CPL_OK without a valid
   result). */
cpl_status cplrt_call_take(cpl_call *call, cpl_status st, cpl_value result, cplrt_w *out)
{
    cap_call *c = (cap_call *)(void *)call;
    cplrt_slot entry;
    int r = CPLRT(handle_get)(c->rt, result.bits, CPLRT_SLOT_VALUE, &entry);
    if (st != CPL_OK) {
        if (r == CPLRT_HANDLE_OK) {
            CPLRT(slot_unref)(&entry);
            (void)CPLRT(handle_release)(c->rt, result.bits, CPLRT_SLOT_VALUE);
        }
        return st;
    }
    if (r != CPLRT_HANDLE_OK)
        return CPL_E_INTERNAL; /* CPL_OK without a valid owned result: binding_protocol */
    cplrt_w w = entry.u.value; /* retained by handle_get: that reference becomes the result */
    if (CPLRT(handle_release)(c->rt, result.bits, CPLRT_SLOT_VALUE) != CPLRT_HANDLE_OK) {
        /* a borrowed argument handed back as the result: the reference handle_get took is the result */
    }
    CPLRT_SET(*out, w);
    return CPL_OK;
}

cpl_status CPLRT(call_detail)(cpl_call *call, const char *key, size_t key_len, const char *value, size_t value_len)
{
    cap_call *c = (cap_call *)(void *)call;
    if ((key == NULL && key_len > 0) || (value == NULL && value_len > 0))
        return CPL_E_ARGUMENT;
    if ((key_len > 0 && !CPLRT(utf8_valid)(key, key_len, NULL, NULL)) ||
        (value_len > 0 && !CPLRT(utf8_valid)(value, value_len, NULL, NULL)))
        return CPL_E_ARGUMENT;
    if (c->ndetails == c->details_cap) {
        uint32_t cap = c->details_cap == 0 ? 4u : c->details_cap * 2u;
        cap_detail *grown = (cap_detail *)CPLRT(alloc)(c->rt, (size_t)cap * sizeof *grown);
        if (grown == NULL)
            return CPL_E_NOMEM;
        if (c->ndetails > 0)
            memcpy(grown, c->details, (size_t)c->ndetails * sizeof *grown);
        CPLRT(free)(c->details);
        c->details = grown;
        c->details_cap = cap;
    }
    char *k = (char *)CPLRT(alloc)(c->rt, key_len + 1u);
    char *v = (char *)CPLRT(alloc)(c->rt, value_len + 1u);
    if (k == NULL || v == NULL) {
        CPLRT(free)(k);
        CPLRT(free)(v);
        return CPL_E_NOMEM;
    }
    if (key_len > 0)
        memcpy(k, key, key_len);
    k[key_len] = '\0';
    if (value_len > 0)
        memcpy(v, value, value_len);
    v[value_len] = '\0';
    c->details[c->ndetails].key = k;
    c->details[c->ndetails].value = v;
    c->details[c->ndetails].key_len = key_len;
    c->details[c->ndetails].value_len = value_len;
    c->ndetails++;
    return CPL_OK;
}

cpl_status CPLRT(call_fail)(cpl_call *call, int32_t kind, const char *message, size_t len)
{
    cap_call *c = (cap_call *)(void *)call;
    if (kind < 0 || (uint32_t)kind >= CPLRT_FOREIGN_KINDS_N)
        return CPL_E_ARGUMENT;
    if ((message == NULL && len > 0) || (len > 0 && !CPLRT(utf8_valid)(message, len, NULL, NULL)))
        return CPL_E_ARGUMENT;
    char *m = (char *)CPLRT(alloc)(c->rt, len + 1u);
    if (m == NULL)
        return CPL_E_NOMEM;
    if (len > 0)
        memcpy(m, message, len);
    m[len] = '\0';
    CPLRT(free)(c->message);
    c->message = m;
    c->message_len = len;
    c->kind = kind;
    c->failed = 1;
    return CPL_FOREIGN_ERROR;
}

static void cap_call_free(cap_call *c)
{
    CPLRT(free)(c->message);
    for (uint32_t i = 0; i < c->ndetails; i++) {
        CPLRT(free)(c->details[i].key);
        CPLRT(free)(c->details[i].value);
    }
    CPLRT(free)(c->details);
    c->message = NULL;
    c->details = NULL;
    c->ndetails = 0;
}

/* ---- building ForeignError values (stdlib.md S45) ------------------------------------------------------------- */

/* A Text of the len bytes at s (valid UTF-8). */
static int cap_text(cplrt_cx *cx, const char *s, size_t len, cplrt_w *out)
{
    return CPLRT(text_make)(cx->rt, s, len, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

/* The index of the member `name` of definition def, or CPLRT_NONE. */
static uint32_t cap_member(const cplrt_program *p, uint32_t def, const char *name)
{
    if (def >= p->ndefs)
        return CPLRT_NONE;
    const cplrt_def *d = &p->defs[def];
    for (uint32_t i = 0; i < d->nmembers; i++)
        if (strcmp(d->members[i], name) == 0)
            return i;
    return CPLRT_NONE;
}

/* One key/value pair of the details, in order. */
typedef struct cap_pair {
    const char *key, *value;
    size_t key_len, value_len;
} cap_pair;

/* Err(ForeignError { kind, message, details }) (boundary::foreign_error with Value::err): the fields placed by name
   in the declarations of std.foreign. */
static int cap_foreign_value(cplrt_cx *cx, uint32_t kind, const char *message, size_t message_len,
                             const cap_pair *pairs, uint32_t npairs, cplrt_w *out)
{
    const cplrt_program *p = cx->program;
    uint32_t fe = p->std.foreign_error, fd = p->std.foreign_detail, fk = p->std.foreign_error_kind;
    if (fe == CPLRT_NONE || fd == CPLRT_NONE || fk == CPLRT_NONE) {
        cplrt_internal(cx, CPLRT_NO_SITE, "binding_error_unrepresentable", "the program declares no std.foreign");
        return CPLRT_FAULT;
    }
    uint32_t dk = cap_member(p, fd, "key"), dv = cap_member(p, fd, "value");
    uint32_t ek = cap_member(p, fe, "kind"), em = cap_member(p, fe, "message"), ed = cap_member(p, fe, "details");
    if (dk > 1u || dv > 1u || ek > 2u || em > 2u || ed > 2u) {
        cplrt_internal(cx, CPLRT_NO_SITE, "binding_error_unrepresentable", "std.foreign has an unexpected shape");
        return CPLRT_FAULT;
    }
    int st = CPLRT_OK;
    cplrt_node *list = NULL;
    cplrt_w details = 0, msg = 0, kv = 0, error = 0, key = 0, value = 0;
    if (npairs > 0) {
        list = CPLRT(list_alloc)(cx->rt, npairs);
        if (list == NULL)
            return CPLRT(oom)(cx);
        details = cplrt_word_of(&list->hdr);
        for (uint32_t i = 0; i < npairs && st == CPLRT_OK; i++) {
            if (cap_text(cx, pairs[i].key, pairs[i].key_len, &key) != CPLRT_OK ||
                cap_text(cx, pairs[i].value, pairs[i].value_len, &value) != CPLRT_OK) {
                st = CPLRT_FAULT;
                break;
            }
            cplrt_w fields[2];
            fields[dk] = key;
            fields[dv] = value;
            cplrt_w entry = 0;
            if (CPLRT(node_make)(cx->rt, CPLRT_STRUCT, fd, 0, fields, 2, &entry) != CPLRT_OK) {
                st = CPLRT(oom)(cx);
                break;
            }
            list->item[i] = entry;
            CPLRT_SET(key, 0);
            CPLRT_SET(value, 0);
        }
    } else {
        details = CPLRT(empty_list)();
    }
    if (st == CPLRT_OK && cap_text(cx, message, message_len, &msg) != CPLRT_OK)
        st = CPLRT_FAULT;
    if (st == CPLRT_OK) {
        kv = CPLRT(enum_static)(p, fk, kind);
        if (kv == 0 && CPLRT(node_make)(cx->rt, CPLRT_ENUM, fk, kind, NULL, 0, &kv) != CPLRT_OK)
            st = CPLRT(oom)(cx);
    }
    if (st == CPLRT_OK) {
        cplrt_w fields[3];
        fields[ek] = kv;
        fields[em] = msg;
        fields[ed] = details;
        if (CPLRT(node_make)(cx->rt, CPLRT_STRUCT, fe, 0, fields, 3, &error) != CPLRT_OK)
            st = CPLRT(oom)(cx);
    }
    if (st == CPLRT_OK && cplrt_result_err(cx, error, out) != CPLRT_OK)
        st = CPLRT_FAULT;
    cplrt_release(key);
    cplrt_release(value);
    cplrt_release(details);
    cplrt_release(msg);
    cplrt_release(kv);
    cplrt_release(error);
    return st;
}

/* The runtime's details first (adapter, capability, operation), then `extra` in order (cabi.md 6.5). */
static int cap_failure_value(cplrt_cx *cx, const cplrt_capref *h, const cplrt_op *op, uint32_t kind,
                             const char *message, size_t message_len, const cap_pair *extra, uint32_t nextra,
                             cplrt_w *out)
{
    const cplrt_program *p = cx->program;
    cap_pair local[16];
    cap_pair *pairs = local;
    uint32_t n = 3u + nextra;
    if (n > 16u) {
        pairs = (cap_pair *)CPLRT(alloc)(cx->rt, (size_t)n * sizeof *pairs);
        if (pairs == NULL)
            return CPLRT(oom)(cx);
    }
    const char *label = CPLRT(cap_label)(h);
    const char *capq = h->capability < p->ndefs ? p->defs[h->capability].qualified_name : "";
    pairs[0] = (cap_pair){ "adapter", label, 7u, strlen(label) };
    pairs[1] = (cap_pair){ "capability", capq, 10u, strlen(capq) };
    pairs[2] = (cap_pair){ "operation", op->identity, 9u, strlen(op->identity) };
    for (uint32_t i = 0; i < nextra; i++)
        pairs[3u + i] = extra[i];
    int st = cap_foreign_value(cx, kind, message, message_len, pairs, n, out);
    if (pairs != local)
        CPLRT(free)(pairs);
    return st;
}

/* An internal_error of the binding layer at the operation call (semantics S80): kind, detail, operation, binding. */
static int cap_binding_fault(cplrt_cx *cx, uint32_t site, const char *kind, const char *tmpl, const char *const *names,
                             const char *const *values, uint32_t n, const cplrt_op *op, const char *binding,
                             const char *pointer)
{
    CPLRT(raise_internal)(cx, site, kind, tmpl, names, values, n);
    cplrt_fault *f = cx->fault;
    if (f == NULL || f == cx->memory)
        return CPLRT_FAULT;
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, cx->rt);
    CPLRT(sbuf_put)(&b, "", 0);
    CPLRT(write_json_string)(&b, op->identity, strlen(op->identity));
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        return CPLRT(oom)(cx);
    }
    if (CPLRT(fault_set_json)(cx, f, "operation", b.ptr, b.len) != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        return CPLRT_FAULT;
    }
    CPLRT(sbuf_free)(&b);
    cplrt_sbuf_init(&b, cx->rt);
    CPLRT(sbuf_put)(&b, "", 0);
    CPLRT(write_json_string)(&b, binding, strlen(binding));
    if (b.status != CPLRT_OK || CPLRT(fault_set_json)(cx, f, "binding", b.ptr, b.len) != CPLRT_OK) {
        int oom = b.status != CPLRT_OK;
        CPLRT(sbuf_free)(&b);
        return oom ? CPLRT(oom)(cx) : CPLRT_FAULT;
    }
    CPLRT(sbuf_free)(&b);
    if (pointer != NULL) {
        cplrt_sbuf_init(&b, cx->rt);
        CPLRT(sbuf_put)(&b, "", 0);
        CPLRT(write_json_string)(&b, pointer, strlen(pointer));
        if (b.status != CPLRT_OK || CPLRT(fault_set_json)(cx, f, "pointer", b.ptr, b.len) != CPLRT_OK) {
            int oom = b.status != CPLRT_OK;
            CPLRT(sbuf_free)(&b);
            return oom ? CPLRT(oom)(cx) : CPLRT_FAULT;
        }
        CPLRT(sbuf_free)(&b);
    }
    return CPLRT_FAULT;
}

/* The outcome of a failure the binding reported (kind, message, details in order). */
static int cap_reported(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, const cplrt_op *op, uint32_t kind,
                        const char *message, size_t message_len, const cap_pair *details, uint32_t ndetails,
                        cplrt_w *out)
{
    if (op->ret_foreign)
        return cap_failure_value(cx, h, op, kind, message, message_len, details, ndetails, out);
    /* semantics S80: the return type cannot carry the failure */
    cplrt_sbuf q;
    cplrt_sbuf_init(&q, cx->rt);
    CPLRT(sbuf_put)(&q, "", 0);
    CPLRT(write_quote_text)(&q, message, message_len);
    if (q.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&q);
        return CPLRT(oom)(cx);
    }
    const char *binding = CPLRT(cap_label)(h);
    static const char *const names[4] = { "binding", "kind", "operation", "message" };
    const char *values[4] = { binding, kind < CPLRT_FOREIGN_KINDS_N ? CPLRT_FOREIGN_KINDS[kind] : "Exception",
                              op->identity, q.ptr };
    int st = cap_binding_fault(cx, site, "binding_error_unrepresentable", CPLRT_MSG_DETAIL_BINDING_ERROR_UNREPRESENTABLE,
                               names, values, 4, op, binding, NULL);
    CPLRT(sbuf_free)(&q);
    return st;
}

/* mirrors cpl_eval::boundary::validate_result: the shape, then the refined positions (semantics S77) */
static int cap_validate(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, const cplrt_op *op, cplrt_w *result)
{
    const cplrt_program *p = cx->program;
    const char *binding = CPLRT(cap_label)(h);
    cplrt_shape_failure sf;
    if (!CPLRT(shape_check)(cx->rt, op->ret, *result, &sf)) {
        if (sf.pointer.status != CPLRT_OK) {
            CPLRT(sbuf_free)(&sf.pointer);
            return CPLRT(oom)(cx);
        }
        const char *pointer = sf.pointer.ptr != NULL ? sf.pointer.ptr : "";
        const char *expected = sf.type < p->ntypes ? p->types[sf.type].display : "";
        static const char *const names[3] = { "operation", "expected_type", "pointer" };
        const char *values[3] = { op->identity, expected, pointer };
        const char *tmpl = pointer[0] == '\0' ? CPLRT_MSG_RESULT_ILL_TYPED_WHOLE : CPLRT_MSG_RESULT_ILL_TYPED;
        int st;
        if (op->ret_foreign) {
            cplrt_sbuf m;
            cplrt_sbuf_init(&m, cx->rt);
            CPLRT(sbuf_put)(&m, "", 0);
            CPLRT(write_template)(&m, tmpl, names, values, 3);
            if (m.status != CPLRT_OK) {
                CPLRT(sbuf_free)(&m);
                CPLRT(sbuf_free)(&sf.pointer);
                return CPLRT(oom)(cx);
            }
            cap_pair extra[2] = { { "pointer", pointer, 7u, strlen(pointer) },
                                  { "expected_type", expected, 13u, strlen(expected) } };
            cplrt_w sub = 0;
            st = cap_failure_value(cx, h, op, CPL_FOREIGN_INVALID_RESULT, m.ptr, m.len, extra, 2, &sub);
            CPLRT(sbuf_free)(&m);
            if (st == CPLRT_OK)
                CPLRT_SET(*result, sub);
        } else {
            st = cap_binding_fault(cx, site, "binding_result_ill_typed", tmpl, names, values, 3, op, binding, pointer);
        }
        CPLRT(sbuf_free)(&sf.pointer);
        return st;
    }
    CPLRT(sbuf_free)(&sf.pointer);
    cplrt_refined_failure rf;
    if (CPLRT(refined_check)(cx, site, op->ret, *result, &rf) != CPLRT_OK) {
        CPLRT(sbuf_free)(&rf.pointer);
        return CPLRT_FAULT;
    }
    if (rf.refinement == CPLRT_NONE) {
        CPLRT(sbuf_free)(&rf.pointer);
        return CPLRT_OK;
    }
    const char *pointer = rf.pointer.ptr != NULL ? rf.pointer.ptr : "";
    const char *expected = rf.declared < p->nrefs ? p->refinements[rf.declared].declared_name : "";
    const char *clause = rf.refinement < p->nrefs ? p->refinements[rf.refinement].predicate : "";
    int st;
    if (op->ret_foreign) {
        static const char *const names[4] = { "operation", "expected_type", "pointer", "clause" };
        const char *values[4] = { op->identity, expected, pointer, clause };
        cplrt_sbuf m;
        cplrt_sbuf_init(&m, cx->rt);
        CPLRT(sbuf_put)(&m, "", 0);
        CPLRT(write_template)(&m, pointer[0] == '\0' ? CPLRT_MSG_RESULT_REFINEMENT_WHOLE : CPLRT_MSG_RESULT_REFINEMENT,
                              names, values, 4);
        if (m.status != CPLRT_OK) {
            CPLRT(sbuf_free)(&m);
            CPLRT(sbuf_free)(&rf.pointer);
            return CPLRT(oom)(cx);
        }
        cap_pair extra[2] = { { "pointer", pointer, 7u, strlen(pointer) },
                              { "expected_type", expected, 13u, strlen(expected) } };
        cplrt_w sub = 0;
        st = cap_failure_value(cx, h, op, CPL_FOREIGN_INVALID_RESULT, m.ptr, m.len, extra, 2, &sub);
        CPLRT(sbuf_free)(&m);
        if (st == CPLRT_OK)
            CPLRT_SET(*result, sub);
        CPLRT(sbuf_free)(&rf.pointer);
        return st;
    }
    /* refinement_violation blamed on the operation (semantics S77) */
    static const char *const names[2] = { "expected_type", "clause" };
    const char *values[2] = { expected, clause };
    cplrt_sbuf m;
    cplrt_sbuf_init(&m, cx->rt);
    CPLRT(sbuf_put)(&m, "", 0);
    CPLRT(write_template)(&m, CPLRT_MSG_REFINEMENT_VIOLATION, names, values, 2);
    if (m.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&m);
        CPLRT(sbuf_free)(&rf.pointer);
        return CPLRT(oom)(cx);
    }
    cplrt_fault *f = CPLRT(fault_raise)(cx, CPLRT_FK_REFINEMENT_VIOLATION, 0, 0, m.ptr, m.len, site, op->identity,
                                        op->identity);
    CPLRT(sbuf_free)(&m);
    st = CPLRT_FAULT;
    if (f != NULL) {
        cplrt_sbuf b;
        int ok = 1;
        const char *keys[5] = { "expected_type", "clause", "pointer", "attributed_to", "binding" };
        const char *strs[5] = { expected, clause, pointer, "binding", binding };
        for (int i = 0; i < 5 && ok; i++) {
            cplrt_sbuf_init(&b, cx->rt);
            CPLRT(sbuf_put)(&b, "", 0);
            CPLRT(write_json_string)(&b, strs[i], strlen(strs[i]));
            if (b.status != CPLRT_OK) {
                CPLRT(sbuf_free)(&b);
                CPLRT(oom)(cx);
                ok = 0;
                break;
            }
            if (CPLRT(fault_set_json)(cx, f, keys[i], b.ptr, b.len) != CPLRT_OK)
                ok = 0;
            CPLRT(sbuf_free)(&b);
        }
        if (ok) {
            cplrt_sbuf_init(&b, cx->rt);
            CPLRT(sbuf_put)(&b, "", 0);
            CPLRT(write_value_json)(&b, p, rf.actual);
            if (b.status != CPLRT_OK) {
                CPLRT(sbuf_free)(&b);
                CPLRT(oom)(cx);
            } else {
                (void)CPLRT(fault_set_json)(cx, f, "actual", b.ptr, b.len);
                CPLRT(sbuf_free)(&b);
            }
        }
    }
    CPLRT(sbuf_free)(&rf.pointer);
    return st;
}

/* A binding_protocol fault (semantics S80). */
static int cap_protocol(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, const cplrt_op *op, const char *tmpl,
                        int32_t status)
{
    char code[16];
    int k = 0;
    int64_t v = status;
    int neg = v < 0;
    uint64_t u = neg ? (uint64_t)(-(v + 1)) + 1u : (uint64_t)v;
    char digits[24];
    do {
        digits[k++] = (char)('0' + (int)(u % 10u));
        u /= 10u;
    } while (u != 0);
    int j = 0;
    if (neg)
        code[j++] = '-';
    while (k > 0)
        code[j++] = digits[--k];
    code[j] = '\0';
    const char *binding = CPLRT(cap_label)(h);
    static const char *const names[3] = { "binding", "code", "operation" };
    const char *values[3] = { binding, code, op->identity };
    return cap_binding_fault(cx, site, "binding_protocol", tmpl, names, values, 3, op, binding, NULL);
}

/* mirrors cpl_eval::interp::Interpreter::call_operation's host call and validation (5.7, 6.4, 6.5) */
int cplrt_cap_invoke(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, uint32_t op, const cplrt_w *args,
                     uint32_t n, cplrt_w *out)
{
    const cplrt_program *p = cx->program;
    if (h == NULL || h->capability >= p->ndefs || p->defs[h->capability].ops == NULL ||
        op >= p->defs[h->capability].nmembers) {
        cplrt_internal(cx, site, "unbound_capability", "an operation was called on a handle the host did not bind");
        return CPLRT_FAULT;
    }
    const cplrt_op *opd = &p->defs[h->capability].ops[op];
    uint32_t local_slots[8];
    uint32_t *slots = local_slots;
    if (n > 8u) {
        slots = (uint32_t *)CPLRT(alloc)(cx->rt, (size_t)n * sizeof *slots);
        if (slots == NULL)
            return CPLRT(oom)(cx);
    }
    if (n > 0 && CPLRT(handle_reserve)(cx->rt, n, slots) != CPLRT_OK) {
        if (slots != local_slots)
            CPLRT(free)(slots);
        return CPLRT(oom)(cx);
    }
    cap_call call;
    memset(&call, 0, sizeof call);
    call.pub.api = &CPLRT(api_table);
    call.rt = cx->rt;
    call.cx = cx;
    call.op = op;
    call.site = site;
    call.cap = h;
    call.slots = slots;
    call.nslots = n;
    cplrt_w result = 0;
    cpl_status st = h->impl != NULL && h->impl->invoke != NULL
                        ? h->impl->invoke(h, &call.pub, op, args, &result)
                        : CPL_E_INTERNAL;
    if (n > 0)
        CPLRT(handle_unreserve)(cx->rt, slots, n);
    if (slots != local_slots)
        CPLRT(free)(slots);
    if (call.raised) {
        cplrt_release(result);
        cap_call_free(&call);
        return CPLRT_FAULT;
    }
    int rs;
    if (st == CPL_OK && result != 0) {
        cap_call_free(&call);
        rs = cap_validate(cx, site, h, opd, &result);
        if (rs == CPLRT_OK) {
            CPLRT_MOVE(*out, result);
            return CPLRT_OK;
        }
        cplrt_release(result);
        return rs;
    }
    cplrt_release(result);
    if (st == CPL_FOREIGN_ERROR && call.failed) {
        cap_pair local[16];
        cap_pair *pairs = local;
        if (call.ndetails > 16u) {
            pairs = (cap_pair *)CPLRT(alloc)(cx->rt, (size_t)call.ndetails * sizeof *pairs);
            if (pairs == NULL) {
                cap_call_free(&call);
                return CPLRT(oom)(cx);
            }
        }
        for (uint32_t i = 0; i < call.ndetails; i++)
            pairs[i] = (cap_pair){ call.details[i].key, call.details[i].value, call.details[i].key_len,
                                   call.details[i].value_len };
        cplrt_w v = 0;
        rs = cap_reported(cx, site, h, opd, (uint32_t)call.kind, call.message != NULL ? call.message : "",
                          call.message_len, pairs, call.ndetails, &v);
        if (pairs != local)
            CPLRT(free)(pairs);
        cap_call_free(&call);
        if (rs == CPLRT_OK)
            CPLRT_MOVE(*out, v);
        return rs;
    }
    cap_call_free(&call);
    if (st == CPL_E_NOMEM)
        return CPLRT(oom)(cx);
    if (st == CPL_OK || st == CPL_E_INTERNAL)
        return cap_protocol(cx, site, h, opd, CPLRT_MSG_DETAIL_BINDING_PROTOCOL_NO_RESULT, st);
    if (st == CPL_FOREIGN_ERROR)
        return cap_protocol(cx, site, h, opd, CPLRT_MSG_DETAIL_BINDING_PROTOCOL_NO_FAILURE, st);
    return cap_protocol(cx, site, h, opd, CPLRT_MSG_DETAIL_BINDING_PROTOCOL_STATUS, st);
}

/* The binding layer alone, for the driver's cap_call request (cabi.md 3.8, 7.2): no clauses and no validation.
   CPLRT_OK with *out the owned result; CPLRT_INVALID with *foreign the JSON object {"kind", "message", "details"} of
   a reported failure (its details after the runtime's three keys, as the adapter gave them); CPLRT_FAULT with the
   binding layer's fault (binding_protocol, memory) in cx->fault. */
int CPLRT(cap_call_raw)(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, uint32_t op, const cplrt_w *args,
                        uint32_t n, cplrt_w *out, cplrt_sbuf *foreign)
{
    const cplrt_program *p = cx->program;
    const cplrt_op *opd = &p->defs[h->capability].ops[op];
    uint32_t local_slots[8];
    uint32_t *slots = local_slots;
    if (n > 8u) {
        slots = (uint32_t *)CPLRT(alloc)(cx->rt, (size_t)n * sizeof *slots);
        if (slots == NULL)
            return CPLRT(oom)(cx);
    }
    if (n > 0 && CPLRT(handle_reserve)(cx->rt, n, slots) != CPLRT_OK) {
        if (slots != local_slots)
            CPLRT(free)(slots);
        return CPLRT(oom)(cx);
    }
    cap_call call;
    memset(&call, 0, sizeof call);
    call.pub.api = &CPLRT(api_table);
    call.rt = cx->rt;
    call.cx = cx;
    call.op = op;
    call.site = site;
    call.cap = h;
    call.slots = slots;
    call.nslots = n;
    cplrt_w result = 0;
    cpl_status st = h->impl != NULL && h->impl->invoke != NULL
                        ? h->impl->invoke(h, &call.pub, op, args, &result)
                        : CPL_E_INTERNAL;
    if (n > 0)
        CPLRT(handle_unreserve)(cx->rt, slots, n);
    if (slots != local_slots)
        CPLRT(free)(slots);
    if (call.raised) {
        cplrt_release(result);
        cap_call_free(&call);
        return CPLRT_FAULT;
    }
    if (st == CPL_OK && result != 0) {
        cap_call_free(&call);
        CPLRT_MOVE(*out, result);
        return CPLRT_OK;
    }
    cplrt_release(result);
    if (st == CPL_FOREIGN_ERROR && call.failed) {
        CPLRT(sbuf_puts)(foreign, "{\"kind\":");
        const char *kind = (uint32_t)call.kind < CPLRT_FOREIGN_KINDS_N ? CPLRT_FOREIGN_KINDS[call.kind] : "Exception";
        CPLRT(write_json_string)(foreign, kind, strlen(kind));
        CPLRT(sbuf_puts)(foreign, ",\"message\":");
        CPLRT(write_json_string)(foreign, call.message != NULL ? call.message : "", call.message_len);
        CPLRT(sbuf_puts)(foreign, ",\"details\":[");
        for (uint32_t i = 0; i < call.ndetails; i++) {
            if (i > 0)
                CPLRT(sbuf_putc)(foreign, ',');
            CPLRT(sbuf_putc)(foreign, '[');
            CPLRT(write_json_string)(foreign, call.details[i].key, call.details[i].key_len);
            CPLRT(sbuf_putc)(foreign, ',');
            CPLRT(write_json_string)(foreign, call.details[i].value, call.details[i].value_len);
            CPLRT(sbuf_putc)(foreign, ']');
        }
        CPLRT(sbuf_puts)(foreign, "]}");
        cap_call_free(&call);
        if (foreign->status != CPLRT_OK)
            return CPLRT(oom)(cx);
        return CPLRT_INVALID;
    }
    cap_call_free(&call);
    if (st == CPL_E_NOMEM)
        return CPLRT(oom)(cx);
    if (st == CPL_OK || st == CPL_E_INTERNAL)
        return cap_protocol(cx, site, h, opd, CPLRT_MSG_DETAIL_BINDING_PROTOCOL_NO_RESULT, st);
    if (st == CPL_FOREIGN_ERROR)
        return cap_protocol(cx, site, h, opd, CPLRT_MSG_DETAIL_BINDING_PROTOCOL_NO_FAILURE, st);
    return cap_protocol(cx, site, h, opd, CPLRT_MSG_DETAIL_BINDING_PROTOCOL_STATUS, st);
}

/* A scripted host's call that no response matches (cabi.md 3.8; semantics S80): the binding_protocol fault with the
   detail "<binding> has no scripted response for <operation>(<arguments>)", raised at the operation call. Returns
   CPL_E_INTERNAL, which the binding layer returns; cplrt_cap_invoke then reports the raised fault. */
cpl_status CPLRT(call_no_response)(cpl_call *call, const char *arguments)
{
    cap_call *c = (cap_call *)(void *)call;
    const cplrt_program *p = c->cx->program;
    const cplrt_op *opd = &p->defs[c->cap->capability].ops[c->op];
    const char *binding = CPLRT(cap_label)(c->cap);
    static const char *const names[3] = { "binding", "operation", "arguments" };
    const char *values[3] = { binding, opd->identity, arguments };
    (void)cap_binding_fault(c->cx, c->site, "binding_protocol", CPLRT_MSG_DETAIL_BINDING_PROTOCOL_NO_RESPONSE, names,
                            values, 3, opd, binding, NULL);
    c->raised = 1;
    return CPL_E_INTERNAL;
}

/* A worker that ended during the call (spec/python.md 6.4; stdlib S45, S48; semantics S82): with a foreign-error return
   type, the failure Transport with `message` (recorded as cpl_call_foreign_error records one; returns
   CPL_FOREIGN_ERROR, which the binding layer returns); otherwise the fault internal_error kind binding_host_failed
   whose detail is `message`, raised at the operation call (returns CPL_E_INTERNAL). CPL_E_NOMEM when the failure
   cannot be recorded. */
cpl_status CPLRT(call_host_ended)(cpl_call *call, const char *message, size_t len)
{
    cap_call *c = (cap_call *)(void *)call;
    const cplrt_program *p = c->cx->program;
    const cplrt_op *opd = &p->defs[c->cap->capability].ops[c->op];
    if (opd->ret_foreign)
        return CPLRT(call_fail)(call, CPL_FOREIGN_TRANSPORT, message, len);
    const char *binding = CPLRT(cap_label)(c->cap);
    (void)cap_binding_fault(c->cx, c->site, "binding_host_failed", "{message}", (const char *const[]){ "message" },
                            (const char *const[]){ message }, 1, opd, binding, NULL);
    c->raised = 1;
    return CPL_E_INTERNAL;
}

/* A worker answer that is not cpl-worker protocol 1 (semantics S82): the fault binding_protocol with the detail
   "<binding> sent an answer that is not cpl-worker protocol 1 for <operation>: <reason>", raised at the operation
   call. Returns CPL_E_INTERNAL. */
cpl_status CPLRT(call_worker_protocol)(cpl_call *call, const char *reason)
{
    cap_call *c = (cap_call *)(void *)call;
    const cplrt_program *p = c->cx->program;
    const cplrt_op *opd = &p->defs[c->cap->capability].ops[c->op];
    const char *binding = CPLRT(cap_label)(c->cap);
    static const char *const names[3] = { "binding", "operation", "reason" };
    const char *values[3] = { binding, opd->identity, reason };
    (void)cap_binding_fault(c->cx, c->site, "binding_protocol", CPLRT_MSG_DETAIL_BINDING_PROTOCOL_WORKER, names, values,
                            3, opd, binding, NULL);
    c->raised = 1;
    return CPL_E_INTERNAL;
}

/* The evaluation context of a callback (the driver's scripted host compares values in it). */
cplrt_cx *CPLRT(call_cx)(cpl_call *call)
{
    return ((cap_call *)(void *)call)->cx;
}

/* ---- the generic operation table of cpl_cap_new (5.7) ---------------------------------------------------------- */

typedef struct cap_generic {
    cpl_cap_ops ops;               /* the copy of the host's table; ops.ops points at fns */
    void (*drop)(void *user);
    cpl_op_fn fns[];
} cap_generic;

static cpl_status cap_generic_invoke(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args,
                                     cplrt_w *result)
{
    const cap_generic *g = (const cap_generic *)cplrt_cap_ops(cap);
    const cap_call *c = (const cap_call *)(const void *)call;
    const cplrt_program *p = c->rt->program;
    uint32_t nargs = p->defs[cap->capability].ops[op].nparams;
    cpl_value local[8];
    memset(local, 0, sizeof local);
    cpl_value *handles = local;
    if (nargs > 8u) {
        handles = (cpl_value *)CPLRT(alloc)(c->rt, (size_t)nargs * sizeof *handles);
        if (handles == NULL)
            return CPL_E_NOMEM;
    }
    for (uint32_t i = 0; i < nargs; i++)
        handles[i] = cplrt_call_borrow(call, args[i]);
    cpl_value res = { { 0u, 0u } };
    cpl_status st = g->fns[op](cap->user, call, handles, nargs, &res);
    if (handles != local)
        CPLRT(free)(handles);
    return cplrt_call_take(call, st, res, result);
}

static void cap_generic_drop(const cplrt_capref *cap)
{
    const cap_generic *g = (const cap_generic *)cplrt_cap_ops(cap);
    if (g != NULL && g->drop != NULL)
        g->drop(cap->user);
}

static const cplrt_capimpl cap_generic_impl = { 0u, cap_generic_invoke, cap_generic_drop };

/* cpl_cap_new's object: the table copied (its ops array included) and the generic binding layer; `label` is the binding
   name of semantics 6.9 ("host" for cpl_cap_new, a Python binding's name for the Python extension's in-process grants). */
cplrt_capobj *CPLRT(cap_generic_new)(cplrt_runtime *rt, uint32_t capability, const cpl_cap_ops *ops, void *user,
                                     const char *label)
{
    uint32_t nops = ops->nops;
    cap_generic *g = (cap_generic *)CPLRT(alloc)(rt, sizeof *g + (size_t)nops * sizeof(cpl_op_fn));
    if (g == NULL)
        return NULL;
    memset(&g->ops, 0, sizeof g->ops);
    g->ops.struct_size = (uint32_t)sizeof g->ops;
    g->ops.nops = nops;
    for (uint32_t i = 0; i < nops; i++)
        g->fns[i] = ops->ops[i];
    g->ops.ops = g->fns;
    g->drop = ops->struct_size >= offsetof(cpl_cap_ops, drop) + sizeof ops->drop ? ops->drop : NULL;
    cplrt_capobj *c = CPLRT(cap_new_obj)(rt, capability, &cap_generic_impl, user, label, g);
    if (c == NULL)
        CPLRT(free)(g);
    return c;
}

/* Every outstanding drop and fini, in reverse order of creation (4.3, CPL_CLOSE_FORCE): each found under the lock and
   run without it, since a drop may release other capabilities. */
void CPLRT(caps_drop_all)(cplrt_runtime *rt)
{
    for (;;) {
        cplrt_capobj *victim = NULL;
        pthread_mutex_lock(&rt->lock);
        for (cplrt_capobj *c = rt->caps_last; c != NULL; c = c->prev) {
            if (!c->dropped) {
                victim = c;
                break;
            }
        }
        if (victim != NULL)
            atomic_fetch_add_explicit(&victim->rc, 1u, memory_order_relaxed);
        pthread_mutex_unlock(&rt->lock);
        if (victim == NULL)
            return;
        CPLRT(cap_run_drop)(victim);
        CPLRT(cap_unref)(victim);
    }
}

/* <prefix>_<capability>_new (5.7): the typed ops struct validated (its size covering every operation pointer, each
   pointer not NULL) and copied; user is taken only on CPL_OK. */
cpl_status cplrt_cap_create(cpl_runtime *prt, uint32_t capability, const cplrt_capimpl *impl, const void *ops,
                            size_t size, const size_t *op_offsets, void *user, cpl_cap *out, cpl_error **error)
{
    cplrt_runtime *rt = (cplrt_runtime *)(void *)prt;
    cplrt_datum d[4];
    memset(d, 0, sizeof d);
    if (atomic_load_explicit(&rt->poisoned, memory_order_relaxed))
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_RUNTIME_POISONED, NULL, 0);
    if (out == NULL || ops == NULL) {
        d[0].key = "parameter";
        d[0].form = CPLRT_DATUM_STRING;
        d[0].str = out == NULL ? "out" : "ops";
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_NULL_POINTER, d, 1);
    }
    out->bits[0] = out->bits[1] = 0;
    uint32_t struct_size = *(const uint32_t *)ops;
    size_t minimum = 8u;
    for (uint32_t i = 0; i < impl->nops; i++)
        if (op_offsets[i] + sizeof(void (*)(void)) > minimum)
            minimum = op_offsets[i] + sizeof(void (*)(void));
    if (struct_size < minimum || struct_size > size) {
        d[0].key = "struct";
        d[0].form = CPLRT_DATUM_STRING;
        d[0].str = "ops";
        d[1].key = "size";
        d[1].form = CPLRT_DATUM_NUMBER;
        d[1].num = struct_size;
        d[2].key = "minimum";
        d[2].form = CPLRT_DATUM_NUMBER;
        d[2].num = minimum;
        d[3].key = "maximum";
        d[3].form = CPLRT_DATUM_NUMBER;
        d[3].num = size;
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_STRUCT_SIZE_UNSUPPORTED, d, 4);
    }
    for (uint32_t i = 0; i < impl->nops; i++) {
        void (*fn)(void);
        memcpy(&fn, (const char *)ops + op_offsets[i], sizeof fn);
        if (fn == NULL) {
            d[0].key = "parameter";
            d[0].form = CPLRT_DATUM_STRING;
            d[0].str = "ops";
            return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_NULL_POINTER, d, 1);
        }
    }
    void *copy = CPLRT(alloc)(rt, size);
    if (copy == NULL)
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    memset(copy, 0, size);
    memcpy(copy, ops, struct_size);
    *(uint32_t *)copy = struct_size;
    cplrt_capobj *c = CPLRT(cap_new_obj)(rt, capability, impl, user, "host", copy);
    if (c == NULL) {
        CPLRT(free)(copy);
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    }
    cplrt_slot e;
    memset(&e, 0, sizeof e);
    e.kind = CPLRT_SLOT_CAP;
    e.u.cap = c;
    if (CPLRT(handle_put)(rt, &e, out->bits) != CPLRT_OK) {
        c->dropped = 1; /* the host keeps user */
        CPLRT(cap_unref)(c);
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    }
    if (error != NULL)
        *error = NULL;
    return CPL_OK;
}

/* The adapter's init for the capability object c of grant g, whose handle the caller has allocated (6.2: a grant
   never fails after a successful init). CPL_OK: c holds the state. Otherwise c's fini is disarmed (it never runs after
   a failed init), the caller drops its handle and reference, and *error (when not NULL) is interop_out_of_memory for
   CPL_E_NOMEM, else interop_bind_failed with `binding`, `kind` "Unavailable" for a generated wrapper (its [state]
   create returned NULL, stdlib S45), and the message of the error object init stored as a note. That error object is
   released either way. */
cpl_status CPLRT(grant_init)(cplrt_runtime *rt, const cplrt_grant *g, cplrt_capobj *c, cpl_error **error)
{
    if (g->worker != 0u)
        return CPLRT(worker_grant)(rt, g, c, error);  /* spec/python.md 6: the adapter runs in a worker */
    return CPLRT(grant_init_local)(rt, g, c, error);
}

const cplrt_capimpl *CPLRT(grant_impl)(const cplrt_grant *g)
{
    return g->worker != 0u ? &CPLRT(worker_impl) : g->impl;
}

cpl_status CPLRT(grant_init_local)(cplrt_runtime *rt, const cplrt_grant *g, cplrt_capobj *c, cpl_error **error)
{
    void *state = NULL;
    cpl_error *adapter_error = NULL;
    cpl_status st = g->init != NULL ? g->init(&rt->pub, &state, &adapter_error) : CPL_OK;
    if (st == CPL_OK) {
        if (adapter_error != NULL)
            adapter_error->api->cpl_error_release(adapter_error);
        c->ref.user = state;
        return CPL_OK;
    }
    CPLRT(cap_run_drop_disarm)(c);
    cpl_status r;
    if (st == CPL_E_NOMEM) {
        r = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    } else {
        cplrt_datum d[2];
        memset(d, 0, sizeof d);
        uint32_t n = 0;
        d[n].key = "binding";
        d[n].form = CPLRT_DATUM_STRING;
        d[n++].str = g->binding;
        if (g->declarative) {
            d[n].key = "kind";
            d[n].form = CPLRT_DATUM_STRING;
            d[n++].str = "Unavailable";
        }
        const char *note = NULL;
        size_t note_len = 0;
        if (adapter_error != NULL) {
            cpl_strview m = adapter_error->api->cpl_error_message(adapter_error);
            note = m.ptr != NULL ? m.ptr : "";
            note_len = m.ptr != NULL ? m.len : 0u;
        }
        r = CPLRT(error_interop_note)(rt, error, CPLRT_INTEROP_BIND_FAILED, d, n, note, note_len);
    }
    if (adapter_error != NULL)
        adapter_error->api->cpl_error_release(adapter_error);
    return r;
}

/* <prefix>_bind_<binding> (5.7, 6.1, 6.2): the capability object and its handle, then the adapter's init; the
   object's fini runs once, after the last reference. */
cpl_status cplrt_grant_call(cpl_runtime *prt, uint32_t grant, cpl_cap *out, cpl_error **error)
{
    cplrt_runtime *rt = (cplrt_runtime *)(void *)prt;
    cplrt_datum d;
    memset(&d, 0, sizeof d);
    if (atomic_load_explicit(&rt->poisoned, memory_order_relaxed))
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_RUNTIME_POISONED, NULL, 0);
    if (out == NULL) {
        d.key = "parameter";
        d.form = CPLRT_DATUM_STRING;
        d.str = "out";
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_NULL_POINTER, &d, 1);
    }
    out->bits[0] = out->bits[1] = 0;
    const cplrt_grant *g = &rt->program->grants[grant];
    cplrt_capobj *c = CPLRT(cap_new_obj)(rt, g->capability, CPLRT(grant_impl)(g), NULL, g->binding, NULL);
    if (c == NULL)
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    cplrt_slot e;
    memset(&e, 0, sizeof e);
    e.kind = CPLRT_SLOT_CAP;
    e.u.cap = c;
    uint64_t bits[2] = { 0, 0 };
    if (CPLRT(handle_put)(rt, &e, bits) != CPLRT_OK) {
        CPLRT(cap_run_drop_disarm)(c);                  /* no init ran: nothing to fini */
        CPLRT(cap_unref)(c);
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    }
    cpl_status st = CPLRT(grant_init)(rt, g, c, error);
    if (st != CPL_OK) {
        (void)CPLRT(handle_release)(rt, bits, CPLRT_SLOT_CAP);   /* the object goes with the table's reference */
        return st;
    }
    out->bits[0] = bits[0];                             /* published only now, with its state */
    out->bits[1] = bits[1];
    if (error != NULL)
        *error = NULL;
    return CPL_OK;
}
