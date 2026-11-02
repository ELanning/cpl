/* cpl_rt_value.c: the allocator wrappers and the heap budget, heap objects with atomic counts and immortal
   saturation, iterative release on the pending list, the value builders and their evaluation forms, `++`,
   structural equality on an explicit work stack, and the handle table (spec/cabi.md 2.3, 2.4, 2.8, 4.5, 4.7).
   Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <stdlib.h>
#include <string.h>

/* ---- allocation (2.4, 4.7) ------------------------------------------------------------------------------------- */

/* The hidden header before every block of CPLRT(alloc): its runtime and the size asked for, padded to the alignment
   of max_align_t so that the block after it keeps that alignment. */
typedef struct value_block {
    _Alignas(max_align_t) cplrt_runtime *rt;
    size_t size;
} value_block;
#define VALUE_ALIGN _Alignof(max_align_t)

void *CPLRT(alloc)(cplrt_runtime *rt, size_t size)
{
    if (size == 0)
        size = 1;
    if (size > SIZE_MAX - sizeof(value_block))
        return NULL;
    size_t total = size + sizeof(value_block);
    uint64_t live = atomic_load_explicit(&rt->live_bytes, memory_order_relaxed);
    if (rt->max_heap_bytes != 0) {
        do {
            if ((uint64_t)total > rt->max_heap_bytes || live > rt->max_heap_bytes - (uint64_t)total)
                return NULL;
        } while (!atomic_compare_exchange_weak_explicit(&rt->live_bytes, &live, live + (uint64_t)total,
                                                        memory_order_relaxed, memory_order_relaxed));
    } else {
        live = atomic_fetch_add_explicit(&rt->live_bytes, (uint64_t)total, memory_order_relaxed);
    }
    void *raw = rt->allocator.alloc ? rt->allocator.alloc(rt->allocator.user, total, VALUE_ALIGN) : malloc(total);
    if (raw == NULL) {
        atomic_fetch_sub_explicit(&rt->live_bytes, (uint64_t)total, memory_order_relaxed);
        return NULL;
    }
    uint64_t now = live + (uint64_t)total;
    uint64_t peak = atomic_load_explicit(&rt->peak_bytes, memory_order_relaxed);
    while (now > peak && !atomic_compare_exchange_weak_explicit(&rt->peak_bytes, &peak, now, memory_order_relaxed,
                                                                memory_order_relaxed)) {
    }
    value_block *b = (value_block *)raw;
    b->rt = rt;
    b->size = size;
    return (char *)raw + sizeof(value_block);
}

void CPLRT(free)(void *p)
{
    if (p == NULL)
        return;
    value_block *b = (value_block *)(void *)((char *)p - sizeof(value_block));
    cplrt_runtime *rt = b->rt;
    size_t total = b->size + sizeof(value_block);
    if (rt->allocator.alloc) {
        if (rt->allocator.free)
            rt->allocator.free(rt->allocator.user, b, total, VALUE_ALIGN);
    } else {
        free(b);
    }
    atomic_fetch_sub_explicit(&rt->live_bytes, (uint64_t)total, memory_order_relaxed);
}

size_t CPLRT(alloc_size)(const void *p)
{
    return ((const value_block *)(const void *)((const char *)p - sizeof(value_block)))->size;
}

static cplrt_runtime *value_runtime_of(const void *p)
{
    return ((const value_block *)(const void *)((const char *)p - sizeof(value_block)))->rt;
}

/* ---- heap objects and release (2.3, 2.4) ----------------------------------------------------------------------- */

cplrt_obj *CPLRT(obj_new)(cplrt_runtime *rt, uint8_t kind, uint32_t len, size_t size)
{
    cplrt_obj *o = (cplrt_obj *)CPLRT(alloc)(rt, size);
    if (o == NULL)
        return NULL;
    o->kind = kind;
    o->flags = 0;
    o->variant = 0;
    o->len = len;
    atomic_init(&o->u.live.rc, 1u);
    o->u.live.def = 0;
    atomic_fetch_add_explicit(&rt->live_objects, 1u, memory_order_relaxed);
    return o;
}

/* The last reference to o is gone. The dying objects form an intrusive list through u.next, and the children of each
   are decremented from that list, so release never recurses on the depth of a value (2.4). An immortal child is left
   alone, as cplrt_release leaves it. */
void CPLRT(release_obj)(cplrt_obj *o)
{
    cplrt_obj *pending = o;
    o->u.next = NULL;
    while (pending != NULL) {
        cplrt_obj *cur = pending;
        pending = cur->u.next;
        if (cur->kind >= CPLRT_LIST && cur->kind <= CPLRT_WRAPPER) {
            const cplrt_node *n = (const cplrt_node *)cur;
            for (uint32_t i = 0; i < cur->len; i++) {
                cplrt_w w = n->item[i];
                if (!cplrt_is_obj(w))
                    continue;
                cplrt_obj *c = cplrt_obj_of(w);
                if (atomic_load_explicit(&c->u.live.rc, memory_order_relaxed) < CPLRT_RC_SATURATED &&
                    atomic_fetch_sub_explicit(&c->u.live.rc, 1u, memory_order_acq_rel) == 1u) {
                    c->u.next = pending;
                    pending = c;
                }
            }
        }
        cplrt_runtime *rt = value_runtime_of(cur);
        atomic_fetch_sub_explicit(&rt->live_objects, 1u, memory_order_relaxed);
        CPLRT(free)(cur);
    }
}

/* ---- value builders (2.3) -------------------------------------------------------------------------------------- */

cplrt_w CPLRT(enum_static)(const cplrt_program *p, uint32_t def, uint32_t variant)
{
    if (p == NULL || def >= p->ndefs)
        return CPLRT_EMPTY;
    const cplrt_def *d = &p->defs[def];
    if (d->statics == NULL || variant >= d->nmembers || d->statics[variant] == NULL)
        return CPLRT_EMPTY;
    return cplrt_word_of(d->statics[variant]);
}

int CPLRT(node_make)(cplrt_runtime *rt, uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items, uint32_t n,
                     cplrt_w *out)
{
    if (n == 0) {
        cplrt_w w = CPLRT_EMPTY;
        if (kind == CPLRT_LIST)
            w = CPLRT(empty_list)();
        else if (kind == CPLRT_ENUM)
            w = CPLRT(enum_static)(rt->program, def, variant);
        if (w != CPLRT_EMPTY) {
            CPLRT_SET(*out, w);
            return CPLRT_OK;
        }
    }
    int wide = kind == CPLRT_ENUM && variant > 0xFFFFu;
    if (wide && n == UINT32_MAX)
        return CPLRT_NOMEM;
    uint32_t len = n + (wide ? 1u : 0u);
    cplrt_node *node = (cplrt_node *)CPLRT(obj_new)(rt, kind, len, sizeof(cplrt_node) + (size_t)len * sizeof(cplrt_w));
    if (node == NULL)
        return CPLRT_NOMEM;
    node->hdr.u.live.def = kind == CPLRT_LIST ? 0u : def;
    uint32_t at = 0;
    if (wide) {
        node->hdr.flags = CPLRT_F_WIDE;
        node->item[at++] = cplrt_small((int64_t)variant);
    } else if (kind == CPLRT_ENUM) {
        node->hdr.variant = (uint16_t)variant;
    }
    for (uint32_t i = 0; i < n; i++) {
        cplrt_retain(items[i]);
        node->item[at++] = items[i];
    }
    CPLRT_SET(*out, cplrt_word_of(&node->hdr));
    return CPLRT_OK;
}

/* The evaluation forms of cpl_rt.h: the builders with an allocation failure as the memory fault (S76). */
int cplrt_list_new(cplrt_cx *cx, const cplrt_w *items, uint32_t n, cplrt_w *out)
{
    return CPLRT(node_make)(cx->rt, CPLRT_LIST, 0, 0, items, n, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

int cplrt_struct_new(cplrt_cx *cx, uint32_t def, const cplrt_w *fields, uint32_t n, cplrt_w *out)
{
    return CPLRT(node_make)(cx->rt, CPLRT_STRUCT, def, 0, fields, n, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

int cplrt_enum_new(cplrt_cx *cx, uint32_t def, uint32_t variant, const cplrt_w *payload, uint32_t n, cplrt_w *out)
{
    return CPLRT(node_make)(cx->rt, CPLRT_ENUM, def, variant, payload, n, out) == CPLRT_OK ? CPLRT_OK
                                                                                           : CPLRT(oom)(cx);
}

int cplrt_wrap(cplrt_cx *cx, uint32_t def, cplrt_w inner, cplrt_w *out)
{
    return CPLRT(node_make)(cx->rt, CPLRT_WRAPPER, def, 0, &inner, 1, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

/* mirrors cpl_eval::value::Value::ok */
int cplrt_result_ok(cplrt_cx *cx, cplrt_w v, cplrt_w *out)
{
    return cplrt_enum_new(cx, cx->rt->program->std.result, CPL_RESULT_OK, &v, 1, out);
}

/* mirrors cpl_eval::value::Value::err */
int cplrt_result_err(cplrt_cx *cx, cplrt_w e, cplrt_w *out)
{
    return cplrt_enum_new(cx, cx->rt->program->std.result, CPL_RESULT_ERR, &e, 1, out);
}

/* mirrors cpl_eval::value::Value::some */
int cplrt_option_some(cplrt_cx *cx, cplrt_w v, cplrt_w *out)
{
    return cplrt_enum_new(cx, cx->rt->program->std.option, CPL_OPTION_SOME, &v, 1, out);
}

/* mirrors cpl_eval::builtins::constraint_error, wrapped as Value::err does */
int CPLRT(constraint_err)(cplrt_runtime *rt, const char *type_name, size_t type_len, const char *message,
                          size_t message_len, cplrt_w *out)
{
    const cplrt_program *p = rt->program;
    uint32_t def = p->std.constraint_error;
    cplrt_w fields[2] = { CPLRT_EMPTY, CPLRT_EMPTY };
    cplrt_w error = CPLRT_EMPTY;
    int st = CPLRT(text_make)(rt, type_name, type_len, &fields[0]);
    if (st == CPLRT_OK)
        st = CPLRT(text_make)(rt, message, message_len, &fields[1]);
    if (st == CPLRT_OK) {
        /* the evaluator swaps the two when the declaration lists `message` first */
        if (def < p->ndefs && p->defs[def].nmembers == 2 && p->defs[def].members != NULL &&
            strcmp(p->defs[def].members[0], "message") == 0) {
            cplrt_w t = fields[0];
            fields[0] = fields[1];
            fields[1] = t;
        }
        st = CPLRT(node_make)(rt, CPLRT_STRUCT, def, 0, fields, 2, &error);
    }
    if (st == CPLRT_OK)
        st = CPLRT(node_make)(rt, CPLRT_ENUM, p->std.result, CPL_RESULT_ERR, &error, 1, out);
    cplrt_release(fields[0]);
    cplrt_release(fields[1]);
    cplrt_release(error);
    return st;
}

/* mirrors cpl_eval::value::Value::kind_name */
const char *CPLRT(kind_name)(cplrt_w v)
{
    if (cplrt_is_small(v))
        return "Int";
    if (v == CPLRT_TRUE || v == CPLRT_FALSE)
        return "Bool";
    if (!cplrt_is_obj(v))
        return "capability";
    switch (cplrt_obj_of(v)->kind) {
    case CPLRT_BIGINT: return "Int";
    case CPLRT_TEXT: return "Text";
    case CPLRT_BYTES: return "Bytes";
    case CPLRT_LIST: return "List";
    case CPLRT_STRUCT: return "struct";
    case CPLRT_ENUM: return "enum";
    case CPLRT_WRAPPER: return "wrapper";
    default: return "capability";
    }
}

/* ---- `++` (2.6) ------------------------------------------------------------------------------------------------- */

/* mirrors cpl_eval::value::Value::concat */
int cplrt_concat(cplrt_cx *cx, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    uint8_t ka = cplrt_is_obj(a) ? cplrt_obj_of(a)->kind : 0u;
    uint8_t kb = cplrt_is_obj(b) ? cplrt_obj_of(b)->kind : 0u;
    int st;
    if (ka == kb && ka == CPLRT_LIST)
        st = CPLRT(list_concat)(cx->rt, a, b, out);
    else if (ka == kb && ka == CPLRT_TEXT)
        st = CPLRT(text_concat)(cx->rt, a, b, out);
    else if (ka == kb && ka == CPLRT_BYTES)
        st = CPLRT(bytes_concat)(cx->rt, a, b, out);
    else {
        const char *names[2] = { "lhs", "rhs" };
        const char *values[2] = { CPLRT(kind_name)(a), CPLRT(kind_name)(b) };
        return CPLRT(raise_internal)(cx, CPLRT_NO_SITE, "type_confusion", CPLRT_MSG_DETAIL_TYPE_CONFUSION_CONCAT, names,
                                     values, 2);
    }
    return st == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

/* ---- structural equality (2.8) --------------------------------------------------------------------------------- */

/* One pair of composites whose children are being compared, left to right. */
typedef struct value_eq_frame {
    const cplrt_node *a, *b;
    uint32_t i, n;
} value_eq_frame;

/* 1 when two values that are not the same word are equal without looking at children (leaves), 0 when they differ,
   2 when they are composites of the same kind, declaration, variant, and length whose children must be compared. */
static int value_shallow(cplrt_w a, cplrt_w b)
{
    if (!cplrt_is_obj(a) || !cplrt_is_obj(b))
        return 0; /* distinct immediates, or an immediate and an object: normalized Ints never coincide (2.5) */
    const cplrt_obj *x = cplrt_obj_of(a), *y = cplrt_obj_of(b);
    if (x->kind != y->kind)
        return 0;
    switch (x->kind) {
    case CPLRT_BIGINT:
        return x->flags == y->flags && x->len == y->len &&
               memcmp(((const cplrt_bigint *)x)->limb, ((const cplrt_bigint *)y)->limb, (size_t)x->len * 4u) == 0;
    case CPLRT_TEXT: return cplrt_text_eq(a, b);
    case CPLRT_BYTES: {
        const cplrt_bytes *p = (const cplrt_bytes *)x, *q = (const cplrt_bytes *)y;
        return p->len == q->len && (p->len == 0 || memcmp(p->data, q->data, p->len) == 0);
    }
    case CPLRT_LIST: return x->len == y->len ? 2 : 0;
    case CPLRT_STRUCT:
    case CPLRT_WRAPPER: return x->u.live.def == y->u.live.def && x->len == y->len ? 2 : 0;
    case CPLRT_ENUM:
        return x->u.live.def == y->u.live.def && cplrt_variant(a) == cplrt_variant(b) && x->len == y->len ? 2 : 0;
    default: return 0;
    }
}

/* mirrors cpl_eval::value::Value::structurally_equal */
int CPLRT(value_equal)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, int *eq)
{
    if (a == b) {
        *eq = 1;
        return CPLRT_OK;
    }
    int s = value_shallow(a, b);
    if (s != 2) {
        *eq = s;
        return CPLRT_OK;
    }
    value_eq_frame local[32];
    value_eq_frame *stack = local;
    size_t cap = sizeof local / sizeof local[0], depth = 0;
    stack[depth++] = (value_eq_frame){ (const cplrt_node *)cplrt_obj_of(a), (const cplrt_node *)cplrt_obj_of(b), 0,
                                       cplrt_obj_of(a)->len };
    int result = 1, st = CPLRT_OK;
    while (depth > 0) {
        value_eq_frame *f = &stack[depth - 1];
        if (f->i == f->n) {
            depth--;
            continue;
        }
        cplrt_w x = f->a->item[f->i], y = f->b->item[f->i];
        f->i++;
        if (x == y)
            continue; /* equal words, a shared child included, are equal without being walked */
        s = value_shallow(x, y);
        if (s == 0) {
            result = 0;
            break;
        }
        if (s == 1)
            continue;
        if (depth == cap) {
            size_t ncap = cap * 2;
            value_eq_frame *grown = (value_eq_frame *)CPLRT(alloc)(rt, ncap * sizeof *grown);
            if (grown == NULL) {
                st = CPLRT_NOMEM;
                break;
            }
            memcpy(grown, stack, depth * sizeof *grown);
            if (stack != local)
                CPLRT(free)(stack);
            stack = grown;
            cap = ncap;
        }
        stack[depth++] = (value_eq_frame){ (const cplrt_node *)cplrt_obj_of(x), (const cplrt_node *)cplrt_obj_of(y), 0,
                                           cplrt_obj_of(x)->len };
    }
    if (stack != local)
        CPLRT(free)(stack);
    if (st == CPLRT_OK)
        *eq = result;
    return st;
}

int cplrt_equal(cplrt_cx *cx, cplrt_w a, cplrt_w b, int *eq)
{
    return CPLRT(value_equal)(cx->rt, a, b, eq) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

/* ---- the handle table (4.5) ------------------------------------------------------------------------------------ */

static uint64_t value_handle_bits0(uint32_t slot, uint32_t generation)
{
    return ((uint64_t)generation << 32) | slot;
}

/* The reference an entry holds: retained for a copy given out, dropped when the table lets the entry go. */
static void value_entry_retain(const cplrt_slot *e)
{
    switch (e->kind) {
    case CPLRT_SLOT_VALUE:
    case CPLRT_SLOT_BORROWED: cplrt_retain(e->u.value); break;
    case CPLRT_SLOT_CAP: CPLRT(cap_retain)(e->u.cap); break;
    case CPLRT_SLOT_CANCEL: CPLRT(token_retain)(e->u.cancel); break;
    default: break;
    }
}

static void value_entry_drop(const cplrt_slot *e)
{
    switch (e->kind) {
    case CPLRT_SLOT_VALUE:
    case CPLRT_SLOT_BORROWED: cplrt_release(e->u.value); break;
    case CPLRT_SLOT_CAP: CPLRT(cap_unref)(e->u.cap); break;
    case CPLRT_SLOT_CANCEL: CPLRT(token_unref)(e->u.cancel); break;
    default: break;
    }
}

/* Under the lock: a free slot, reused first in, first out, else a new one; CPLRT_NONE when the table cannot grow. */
static uint32_t value_slot_take(cplrt_runtime *rt)
{
    if (rt->free_head != CPLRT_NONE) {
        uint32_t s = rt->free_head;
        rt->free_head = rt->slots[s].u.next;
        if (rt->free_head == CPLRT_NONE)
            rt->free_tail = CPLRT_NONE;
        return s;
    }
    if (rt->nslots == rt->slots_cap) {
        if (rt->slots_cap >= UINT32_MAX / 2u)
            return CPLRT_NONE;
        uint32_t ncap = rt->slots_cap == 0 ? 16u : rt->slots_cap * 2u;
        cplrt_slot *grown = (cplrt_slot *)CPLRT(alloc)(rt, (size_t)ncap * sizeof *grown);
        if (grown == NULL)
            return CPLRT_NONE;
        if (rt->nslots != 0)
            memcpy(grown, rt->slots, (size_t)rt->nslots * sizeof *grown);
        CPLRT(free)(rt->slots);
        rt->slots = grown;
        rt->slots_cap = ncap;
    }
    uint32_t s = rt->nslots++;
    rt->slots[s].kind = CPLRT_SLOT_FREE;
    rt->slots[s].generation = 0;
    return s;
}

/* Under the lock: the slot goes back to the free list with its generation advanced, or is retired when the
   generation would wrap, so that no handle that named it can name anything again (4.5). */
static void value_slot_give(cplrt_runtime *rt, uint32_t s)
{
    cplrt_slot *slot = &rt->slots[s];
    if (slot->generation == UINT32_MAX) {
        slot->kind = CPLRT_SLOT_RETIRED;
        return;
    }
    slot->generation++;
    slot->kind = CPLRT_SLOT_FREE;
    slot->u.next = CPLRT_NONE;
    if (rt->free_tail == CPLRT_NONE)
        rt->free_head = s;
    else
        rt->slots[rt->free_tail].u.next = s;
    rt->free_tail = s;
}

static void value_count(cplrt_runtime *rt, uint32_t kind, int delta)
{
    if (kind == CPLRT_SLOT_VALUE || kind == CPLRT_SLOT_CANCEL)
        rt->live_handles = delta > 0 ? rt->live_handles + 1u : rt->live_handles - 1u;
    else if (kind == CPLRT_SLOT_CAP)
        rt->live_caps = delta > 0 ? rt->live_caps + 1u : rt->live_caps - 1u;
}

int CPLRT(handle_put)(cplrt_runtime *rt, const cplrt_slot *entry, uint64_t bits[2])
{
    pthread_mutex_lock(&rt->lock);
    uint32_t s = value_slot_take(rt);
    if (s == CPLRT_NONE) {
        pthread_mutex_unlock(&rt->lock);
        return CPLRT_NOMEM;
    }
    cplrt_slot *slot = &rt->slots[s];
    slot->kind = entry->kind;
    slot->u = entry->u;
    value_count(rt, entry->kind, 1);
    bits[0] = value_handle_bits0(s, slot->generation);
    bits[1] = (uint64_t)(uintptr_t)rt->identity;
    pthread_mutex_unlock(&rt->lock);
    return CPLRT_OK;
}

/* Under the lock: the check order of 4.5 (null, identity, slot, generation) and then the kind; *index the slot. */
static int value_check(cplrt_runtime *rt, const uint64_t bits[2], uint32_t kind, int releasing, uint32_t *index)
{
    if (bits[0] == 0 && bits[1] == 0)
        return CPLRT_HANDLE_NULL;
    if (bits[1] != (uint64_t)(uintptr_t)rt->identity)
        return CPLRT_HANDLE_FOREIGN;
    uint32_t s = (uint32_t)(bits[0] & 0xFFFFFFFFu), generation = (uint32_t)(bits[0] >> 32);
    if (s >= rt->nslots)
        return CPLRT_HANDLE_INVALID;
    const cplrt_slot *slot = &rt->slots[s];
    if (slot->kind == CPLRT_SLOT_RETIRED || slot->generation != generation)
        return CPLRT_HANDLE_RELEASED; /* the handle named the slot before it moved on */
    if (slot->kind == CPLRT_SLOT_BORROWED)
        return kind == CPLRT_SLOT_VALUE && !releasing ? (*index = s, CPLRT_HANDLE_OK) : CPLRT_HANDLE_INVALID;
    if (slot->kind != kind)
        return CPLRT_HANDLE_INVALID; /* a handle of another kind, or a slot never handed out at this generation */
    *index = s;
    return CPLRT_HANDLE_OK;
}

int CPLRT(handle_get)(cplrt_runtime *rt, const uint64_t bits[2], uint32_t kind, cplrt_slot *entry)
{
    uint32_t s = 0;
    pthread_mutex_lock(&rt->lock);
    int r = value_check(rt, bits, kind, 0, &s);
    if (r == CPLRT_HANDLE_OK) {
        *entry = rt->slots[s];
        value_entry_retain(entry);
    }
    pthread_mutex_unlock(&rt->lock);
    return r;
}

int CPLRT(handle_release)(cplrt_runtime *rt, const uint64_t bits[2], uint32_t kind)
{
    uint32_t s = 0;
    cplrt_slot dropped;
    memset(&dropped, 0, sizeof dropped);
    pthread_mutex_lock(&rt->lock);
    int r = value_check(rt, bits, kind, 1, &s);
    if (r == CPLRT_HANDLE_OK) {
        dropped = rt->slots[s];
        value_count(rt, dropped.kind, -1);
        value_slot_give(rt, s);
    }
    pthread_mutex_unlock(&rt->lock);
    if (r == CPLRT_HANDLE_OK)
        value_entry_drop(&dropped); /* a capability's drop runs with no runtime lock held (5.7) */
    return r;
}

void CPLRT(slot_unref)(const cplrt_slot *entry)
{
    value_entry_drop(entry);
}

int CPLRT(handle_reserve)(cplrt_runtime *rt, uint32_t n, uint32_t *slots)
{
    pthread_mutex_lock(&rt->lock);
    for (uint32_t i = 0; i < n; i++) {
        uint32_t s = value_slot_take(rt);
        if (s == CPLRT_NONE) {
            for (uint32_t j = i; j > 0; j--)
                value_slot_give(rt, slots[j - 1]);
            pthread_mutex_unlock(&rt->lock);
            return CPLRT_NOMEM;
        }
        rt->slots[s].kind = CPLRT_SLOT_RESERVED;
        rt->slots[s].u.value = CPLRT_EMPTY;
        slots[i] = s;
    }
    pthread_mutex_unlock(&rt->lock);
    return CPLRT_OK;
}

cpl_value CPLRT(handle_borrow)(cplrt_runtime *rt, uint32_t slot, cplrt_w w)
{
    cpl_value h;
    pthread_mutex_lock(&rt->lock);
    rt->slots[slot].kind = CPLRT_SLOT_BORROWED;
    rt->slots[slot].u.value = w;
    h.bits[0] = value_handle_bits0(slot, rt->slots[slot].generation);
    h.bits[1] = (uint64_t)(uintptr_t)rt->identity;
    pthread_mutex_unlock(&rt->lock);
    return h;
}

void CPLRT(handle_unreserve)(cplrt_runtime *rt, const uint32_t *slots, uint32_t n)
{
    pthread_mutex_lock(&rt->lock);
    for (uint32_t i = 0; i < n; i++)
        value_slot_give(rt, slots[i]);
    pthread_mutex_unlock(&rt->lock);
}
