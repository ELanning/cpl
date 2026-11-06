/* cpl_rt_boundary.c: the type-directed shape checks and the refined positions in S74 order (spec/cabi.md 5.5 steps 2
   and 3, 6.4; semantics S74, S77, S81), for export arguments and capability results. One iterative walk serves both:
   it mirrors the work list of cpl_eval::boundary::walk (crates/cpl-eval/src/boundary.rs) over the type table of the
   program (cplrt_type): list elements by index, struct fields in declaration order, payloads by position, a wrapper's
   inner value at /$payload/0, a refinement-typed position checked at its root base and then recorded as one refined
   position whose chain runs root first. File-scope names begin with boundary_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <string.h>

#define BOUNDARY_VISIT 0u
#define BOUNDARY_LEAVE 1u
#define BOUNDARY_REFINED 2u
#define BOUNDARY_SEG_NONE 0u
#define BOUNDARY_SEG_INDEX 1u
#define BOUNDARY_SEG_FIELD 2u
#define BOUNDARY_SEG_PAYLOAD 3u

typedef struct boundary_work {
    uint32_t op;       /* VISIT, LEAVE (pop `index` segments), REFINED (the CPL_R named at the position in `type`) */
    uint32_t type;     /* VISIT: the CPL_X to check the value against */
    uint32_t declared; /* VISIT: the CPL_X a mismatch reports (a refinement's base reports the refinement) */
    uint32_t seg;      /* VISIT: the segment the position adds to its parent's pointer */
    uint64_t index;    /* INDEX and PAYLOAD: the number; LEAVE: the count of segments */
    const char *name;  /* FIELD: the field's name */
    cplrt_w value;     /* borrowed from the walked value */
} boundary_work;

typedef struct boundary_seg {
    uint32_t kind;     /* INDEX, FIELD, or PAYLOAD (two segments: $payload and the number) */
    uint64_t index;
    const char *name;
} boundary_seg;

typedef struct boundary_walk {
    cplrt_runtime *rt;
    boundary_work *work;
    size_t nwork, wcap;
    boundary_work wlocal[32];
    boundary_seg *path;
    size_t npath, pcap;
    boundary_seg plocal[32];
    /* the (object, type) positions whose children are already pushed: a value from another process may share a node
       at many positions (a few hundred bytes of wire nodes can describe more leaves than memory holds), so each shared
       composite is checked once against a type, and the walk is linear in the nodes, not in the expanded tree */
    struct boundary_seen { const void *obj; uint32_t type; } *seen;
    size_t nseen, seencap;
    int nomem;
} boundary_walk;

/* Whether the composite v is visited at type the first time (1), or was before (0); 0 also out of memory. */
static int boundary_first(boundary_walk *w, cplrt_w v, uint32_t type)
{
    if (w->nseen * 2u >= w->seencap) {
        size_t cap = w->seencap == 0 ? 64u : w->seencap * 2u;
        if (cap > SIZE_MAX / sizeof *w->seen) {
            w->nomem = 1;
            return 0;
        }
        struct boundary_seen *grown = (struct boundary_seen *)CPLRT(alloc)(w->rt, cap * sizeof *grown);
        if (grown == NULL) {
            w->nomem = 1;
            return 0;
        }
        memset(grown, 0, cap * sizeof *grown);
        for (size_t i = 0; i < w->seencap; i++) {
            if (w->seen[i].obj == NULL)
                continue;
            size_t h = (((uintptr_t)w->seen[i].obj >> 3) * 0x9E3779B97F4A7C15ull ^ w->seen[i].type) & (cap - 1u);
            while (grown[h].obj != NULL)
                h = (h + 1u) & (cap - 1u);
            grown[h] = w->seen[i];
        }
        CPLRT(free)(w->seen);
        w->seen = grown;
        w->seencap = cap;
    }
    const void *obj = cplrt_obj_of(v);
    size_t h = (((uintptr_t)obj >> 3) * 0x9E3779B97F4A7C15ull ^ type) & (w->seencap - 1u);
    while (w->seen[h].obj != NULL) {
        if (w->seen[h].obj == obj && w->seen[h].type == type)
            return 0;
        h = (h + 1u) & (w->seencap - 1u);
    }
    w->seen[h].obj = obj;
    w->seen[h].type = type;
    w->nseen++;
    return 1;
}

static int boundary_grow(boundary_walk *w, void **items, size_t *cap, size_t need, size_t elem, void *local)
{
    if (need <= *cap)
        return 1;
    size_t cap2 = *cap * 2u;
    if (cap2 < need)
        cap2 = need;
    if (cap2 > SIZE_MAX / elem) {
        w->nomem = 1;
        return 0;
    }
    void *grown = CPLRT(alloc)(w->rt, cap2 * elem);
    if (grown == NULL) {
        w->nomem = 1;
        return 0;
    }
    memcpy(grown, *items, *cap * elem);
    if (*items != local)
        CPLRT(free)(*items);
    *items = grown;
    *cap = cap2;
    return 1;
}

static int boundary_push(boundary_walk *w, const boundary_work *item)
{
    if (!boundary_grow(w, (void **)&w->work, &w->wcap, w->nwork + 1u, sizeof *w->work, w->wlocal))
        return 0;
    w->work[w->nwork++] = *item;
    return 1;
}

static void boundary_init(boundary_walk *w, cplrt_runtime *rt)
{
    w->rt = rt;
    w->work = w->wlocal;
    w->nwork = 0;
    w->wcap = sizeof w->wlocal / sizeof w->wlocal[0];
    w->path = w->plocal;
    w->npath = 0;
    w->pcap = sizeof w->plocal / sizeof w->plocal[0];
    w->seen = NULL;
    w->nseen = 0;
    w->seencap = 0;
    w->nomem = 0;
}

static void boundary_fini(boundary_walk *w)
{
    if (w->work != w->wlocal)
        CPLRT(free)(w->work);
    if (w->path != w->plocal)
        CPLRT(free)(w->path);
    CPLRT(free)(w->seen);
}

/* The RFC 6901 pointer of the current path (boundary::render_pointer). */
static void boundary_pointer(const boundary_walk *w, cplrt_sbuf *b)
{
    CPLRT(sbuf_put)(b, "", 0);
    for (size_t i = 0; i < w->npath; i++) {
        const boundary_seg *s = &w->path[i];
        char digits[24];
        size_t k = 0;
        if (s->kind == BOUNDARY_SEG_FIELD) {
            CPLRT(sbuf_pointer_segment)(b, s->name, strlen(s->name));
            continue;
        }
        if (s->kind == BOUNDARY_SEG_PAYLOAD)
            CPLRT(sbuf_pointer_segment)(b, "$payload", 8u);
        uint64_t v = s->index;
        char text[24];
        do {
            digits[k++] = (char)('0' + (int)(v % 10u));
            v /= 10u;
        } while (v != 0);
        size_t j = 0;
        while (k > 0)
            text[j++] = digits[--k];
        CPLRT(sbuf_pointer_segment)(b, text, j);
    }
}

static int boundary_is_int(cplrt_w v)
{
    return cplrt_is_small(v) || (cplrt_is_obj(v) && cplrt_obj_of(v)->kind == CPLRT_BIGINT);
}

static int boundary_is_kind(cplrt_w v, uint8_t kind)
{
    return cplrt_is_obj(v) && cplrt_obj_of(v)->kind == kind;
}

/* The `found` word of a mismatch: the value's kind, or its declaration's qualified name. */
static const char *boundary_found(const cplrt_program *p, cplrt_w v)
{
    if (boundary_is_int(v))
        return "int";
    if (v == CPLRT_TRUE || v == CPLRT_FALSE)
        return "bool";
    if (!cplrt_is_obj(v))
        return "value";
    const cplrt_obj *o = cplrt_obj_of(v);
    switch (o->kind) {
    case CPLRT_TEXT: return "text";
    case CPLRT_BYTES: return "bytes";
    case CPLRT_LIST: return "list";
    case CPLRT_STRUCT:
    case CPLRT_ENUM:
    case CPLRT_WRAPPER:
        if (o->u.live.def < p->ndefs && p->defs[o->u.live.def].qualified_name != NULL)
            return p->defs[o->u.live.def].qualified_name;
        return o->kind == CPLRT_STRUCT ? "struct" : o->kind == CPLRT_ENUM ? "enum" : "wrapper";
    default: return "value";
    }
}

/* One step of the walk for a VISIT item: 1 when the value matches the item's type (its children pushed), else 0. */
static int boundary_visit(boundary_walk *w, const cplrt_program *p, const boundary_work *it)
{
    if (it->type >= p->ntypes)
        return 0;
    const cplrt_type *ty = &p->types[it->type];
    cplrt_w v = it->value;
    switch (ty->form) {
    case CPLRT_TY_REFINED: {
        if (ty->elem >= p->ntypes || p->types[ty->elem].form == CPLRT_TY_REFINED)
            return 0;
        boundary_work rec = { BOUNDARY_REFINED, ty->def, 0, 0, 0, NULL, v };
        boundary_work base = { BOUNDARY_VISIT, ty->elem, it->declared, BOUNDARY_SEG_NONE, 0, NULL, v };
        return boundary_push(w, &rec) && boundary_push(w, &base) ? 1 : 1;
    }
    case CPLRT_TY_INT: return boundary_is_int(v);
    case CPLRT_TY_BOOL: return v == CPLRT_TRUE || v == CPLRT_FALSE;
    case CPLRT_TY_TEXT: return boundary_is_kind(v, CPLRT_TEXT);
    case CPLRT_TY_BYTES: return boundary_is_kind(v, CPLRT_BYTES);
    case CPLRT_TY_LIST: {
        if (!boundary_is_kind(v, CPLRT_LIST))
            return 0;
        if (!boundary_first(w, v, it->type))
            return 1;
        uint32_t n = cplrt_len(v);
        for (uint32_t i = n; i-- > 0;) {
            boundary_work e = { BOUNDARY_VISIT, ty->elem, ty->elem, BOUNDARY_SEG_INDEX, i, NULL, cplrt_item(v, i) };
            if (!boundary_push(w, &e))
                return 1;
        }
        return 1;
    }
    case CPLRT_TY_STRUCT: {
        if (!boundary_is_kind(v, CPLRT_STRUCT) || cplrt_obj_of(v)->u.live.def != ty->def ||
            cplrt_len(v) != ty->nmembers || ty->def >= p->ndefs)
            return 0;
        const cplrt_def *d = &p->defs[ty->def];
        if (!boundary_first(w, v, it->type))
            return 1;
        if (ty->elem > 0) {
            boundary_work invariant = { BOUNDARY_REFINED, ty->elem - 1u, 0, 0, 0, NULL, v };
            if (!boundary_push(w, &invariant))
                return 1;
        }
        for (uint32_t i = ty->nmembers; i-- > 0;) {
            const char *name = d->members != NULL && i < d->nmembers ? d->members[i] : "";
            boundary_work e = { BOUNDARY_VISIT, ty->members[i], ty->members[i], BOUNDARY_SEG_FIELD, 0, name,
                                cplrt_item(v, i) };
            if (!boundary_push(w, &e))
                return 1;
        }
        return 1;
    }
    case CPLRT_TY_ENUM: {
        if (!boundary_is_kind(v, CPLRT_ENUM) || cplrt_obj_of(v)->u.live.def != ty->def)
            return 0;
        uint32_t variant = cplrt_variant(v);
        if (variant >= ty->nmembers || ty->members == NULL)
            return 0;
        uint32_t lo = ty->members[variant], count = ty->members[variant + 1u] - lo;
        if (cplrt_payload_len(v) != count)
            return 0;
        if (count > 0 && !boundary_first(w, v, it->type))
            return 1;
        for (uint32_t i = count; i-- > 0;) {
            uint32_t t = ty->payloads[lo + i];
            boundary_work e = { BOUNDARY_VISIT, t, t, BOUNDARY_SEG_PAYLOAD, i, NULL, cplrt_payload(v, i) };
            if (!boundary_push(w, &e))
                return 1;
        }
        return 1;
    }
    case CPLRT_TY_WRAPPER: {
        if (!boundary_is_kind(v, CPLRT_WRAPPER) || cplrt_obj_of(v)->u.live.def != ty->def)
            return 0;
        if (!boundary_first(w, v, it->type))
            return 1;
        boundary_work e = { BOUNDARY_VISIT, ty->elem, ty->elem, BOUNDARY_SEG_PAYLOAD, 0, NULL, cplrt_inner(v) };
        boundary_push(w, &e);
        return 1;
    }
    case CPLRT_TY_PARAM: return 1;
    default: return 0; /* a capability (no word is one, semantics S27), or an unknown form */
    }
}

/* Enters the position of a VISIT item: pushes its segments onto the path and a LEAVE for them. */
static int boundary_enter(boundary_walk *w, const boundary_work *it)
{
    if (it->seg == BOUNDARY_SEG_NONE)
        return 1;
    if (!boundary_grow(w, (void **)&w->path, &w->pcap, w->npath + 1u, sizeof *w->path, w->plocal))
        return 0;
    boundary_seg s = { it->seg, it->index, it->name };
    w->path[w->npath++] = s;
    boundary_work leave = { BOUNDARY_LEAVE, 0, 0, 0, 1, NULL, 0 };
    return boundary_push(w, &leave);
}

int CPLRT(values_typed)(cplrt_runtime *rt, const uint32_t *types, uint32_t expected, const cplrt_w *v, uint32_t n)
{
    if (n != expected)
        return CPLRT_INVALID;
    for (uint32_t i = 0; types != NULL && i < n; i++) {
        if (types[i] == 0)
            continue;
        cplrt_shape_failure sf;
        int ok = CPLRT(shape_check)(rt, types[i], v[i], &sf);
        int nomem = !ok && sf.pointer.status != CPLRT_OK;
        CPLRT(sbuf_free)(&sf.pointer);
        if (nomem)
            return CPLRT_NOMEM;
        if (!ok)
            return CPLRT_INVALID;
    }
    return CPLRT_OK;
}

/* Whether type is a leaf (Int, Bool, Text, Bytes): its value has no positions below it and no refinement. */
static int boundary_leaf(const cplrt_program *p, uint32_t type)
{
    if (type >= p->ntypes)
        return 0;
    switch (p->types[type].form) {
    case CPLRT_TY_INT:
    case CPLRT_TY_BOOL:
    case CPLRT_TY_TEXT:
    case CPLRT_TY_BYTES: return 1;
    default: return 0;
    }
}

int CPLRT(shape_check)(cplrt_runtime *rt, uint32_t type, cplrt_w v, cplrt_shape_failure *failure)
{
    const cplrt_program *p = rt->program;
    cplrt_sbuf_init(&failure->pointer, rt);
    failure->type = type;
    failure->reserved = 0;
    failure->found = NULL;
    /* a leaf of the right form needs no walk (one of the wrong form takes it, for the failure's details) */
    if (boundary_leaf(p, type)) {
        boundary_work leaf = { BOUNDARY_VISIT, type, type, BOUNDARY_SEG_NONE, 0, NULL, v };
        if (boundary_visit(NULL, p, &leaf))
            return 1;
    }
    boundary_walk w;
    boundary_init(&w, rt);
    boundary_work root = { BOUNDARY_VISIT, type, type, BOUNDARY_SEG_NONE, 0, NULL, v };
    boundary_push(&w, &root);
    int ok = 1;
    while (w.nwork > 0 && !w.nomem) {
        boundary_work it = w.work[--w.nwork];
        if (it.op == BOUNDARY_LEAVE) {
            w.npath -= (size_t)it.index;
            continue;
        }
        if (it.op == BOUNDARY_REFINED)
            continue;
        if (!boundary_enter(&w, &it))
            break;
        if (!boundary_visit(&w, p, &it)) {
            ok = 0;
            failure->type = it.declared;
            failure->found = boundary_found(p, it.value);
            boundary_pointer(&w, &failure->pointer);
            break;
        }
    }
    if (w.nomem) {
        ok = 0;
        failure->pointer.status = CPLRT_NOMEM;
    }
    boundary_fini(&w);
    return ok;
}

int CPLRT(refined_check)(cplrt_cx *cx, uint32_t site, uint32_t type, cplrt_w v, cplrt_refined_failure *failure)
{
    const cplrt_program *p = cx->program;
    cplrt_sbuf_init(&failure->pointer, cx->rt);
    failure->refinement = CPLRT_NONE;
    failure->declared = CPLRT_NONE;
    failure->actual = 0;
    if (boundary_leaf(p, type))
        return CPLRT_OK; /* no refined position */
    boundary_walk w;
    boundary_init(&w, cx->rt);
    boundary_work root = { BOUNDARY_VISIT, type, type, BOUNDARY_SEG_NONE, 0, NULL, v };
    boundary_push(&w, &root);
    int st = CPLRT_OK;
    while (w.nwork > 0 && !w.nomem) {
        boundary_work it = w.work[--w.nwork];
        if (it.op == BOUNDARY_LEAVE) {
            w.npath -= (size_t)it.index;
            continue;
        }
        if (it.op == BOUNDARY_REFINED) {
            uint32_t declared = it.type;
            if (declared >= p->nrefs)
                continue;
            const cplrt_refinement *r = &p->refinements[declared];
            uint32_t failed = CPLRT_NONE;
            for (uint32_t k = 0; k < r->nchain && failed == CPLRT_NONE; k++) {
                uint32_t ri = r->chain[k];
                if (ri >= p->nrefs)
                    continue;
                uint32_t def = p->refinements[ri].def;
                cplrt_pred_entry pred = p->code != NULL && p->code->preds != NULL && def < p->code->ndefs
                                            ? p->code->preds[def]
                                            : NULL;
                if (pred == NULL) {
                    cplrt_internal(cx, site, "not_a_refinement", "the program has no predicate for a refinement");
                    st = CPLRT_FAULT;
                    break;
                }
                int holds = 0;
                if (pred(cx, site, it.value, &holds) != CPLRT_OK) {
                    st = CPLRT_FAULT;
                    break;
                }
                if (!holds)
                    failed = ri;
            }
            if (st != CPLRT_OK)
                break;
            if (failed != CPLRT_NONE) {
                failure->refinement = failed;
                failure->declared = declared;
                failure->actual = it.value;
                boundary_pointer(&w, &failure->pointer);
                if (failure->pointer.status != CPLRT_OK)
                    w.nomem = 1;
                break;
            }
            continue;
        }
        if (!boundary_enter(&w, &it))
            break;
        (void)boundary_visit(&w, p, &it); /* the shape was checked first: a mismatch only stops the descent */
    }
    if (w.nomem && st == CPLRT_OK) {
        CPLRT(sbuf_free)(&failure->pointer);
        st = CPLRT(oom)(cx);
    }
    boundary_fini(&w);
    return st;
}

/* mirrors cpl_eval::run::check_arguments for one argument (5.5 step 3, semantics S39 (a), S81): every refined
   position of argument `index` of function `def`, of type `type`, root base first and then the chain, each predicate an
   activation entered with CPLRT_NO_SITE; the first false predicate is refinement_violation blamed on the function at
   the parameter's span with expected_type, actual, clause, parameter, and pointer. CPLRT_OK when all hold, else
   CPLRT_FAULT with the fault in cx->fault. */
int CPLRT(argument_refined)(cplrt_cx *cx, uint32_t def, uint32_t index, uint32_t type, cplrt_w v)
{
    const cplrt_program *p = cx->program;
    const cplrt_def *d = &p->defs[def];
    cplrt_refined_failure rf;
    if (CPLRT(refined_check)(cx, CPLRT_NO_SITE, type, v, &rf) != CPLRT_OK) {
        CPLRT(sbuf_free)(&rf.pointer);
        return CPLRT_FAULT;
    }
    if (rf.refinement == CPLRT_NONE) {
        CPLRT(sbuf_free)(&rf.pointer);
        return CPLRT_OK;
    }
    const cplrt_refinement *dec = &p->refinements[rf.declared];
    const cplrt_refinement *bad = &p->refinements[rf.refinement];
    static const char *const names[2] = { "expected_type", "clause" };
    const char *values[2] = { dec->declared_name, bad->predicate };
    cplrt_sbuf m;
    cplrt_sbuf_init(&m, cx->rt);
    CPLRT(sbuf_put)(&m, "", 0);
    CPLRT(write_template)(&m, CPLRT_MSG_REFINEMENT_VIOLATION, names, values, 2);
    if (m.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&m);
        CPLRT(sbuf_free)(&rf.pointer);
        return CPLRT(oom)(cx);
    }
    uint32_t span = d->psites != NULL && index < d->nmembers ? d->psites[index] : CPLRT_NONE;
    cplrt_fault *f = CPLRT(fault_raise)(cx, CPLRT_FK_REFINEMENT_VIOLATION, 0, 0, m.ptr, m.len, span, d->stable_id,
                                        d->qualified_name);
    CPLRT(sbuf_free)(&m);
    if (f != NULL) {
        const char *keys[4] = { "expected_type", "clause", "parameter", "pointer" };
        const char *strs[4] = { dec->declared_name, bad->predicate, index < d->nmembers ? d->members[index] : "",
                                rf.pointer.ptr != NULL ? rf.pointer.ptr : "" };
        for (int k = 0; k < 4 && cx->fault == f; k++) {
            cplrt_sbuf j;
            cplrt_sbuf_init(&j, cx->rt);
            CPLRT(sbuf_put)(&j, "", 0);
            CPLRT(write_json_string)(&j, strs[k], strlen(strs[k]));
            if (j.status == CPLRT_OK)
                (void)CPLRT(fault_set_json)(cx, f, keys[k], j.ptr, j.len);
            else
                CPLRT(oom)(cx);
            CPLRT(sbuf_free)(&j);
        }
        if (cx->fault == f) {
            cplrt_sbuf j;
            cplrt_sbuf_init(&j, cx->rt);
            CPLRT(sbuf_put)(&j, "", 0);
            CPLRT(write_value_json)(&j, p, rf.actual);
            if (j.status == CPLRT_OK)
                (void)CPLRT(fault_set_json)(cx, f, "actual", j.ptr, j.len);
            else
                CPLRT(oom)(cx);
            CPLRT(sbuf_free)(&j);
        }
    }
    CPLRT(sbuf_free)(&rf.pointer);
    return CPLRT_FAULT;
}
