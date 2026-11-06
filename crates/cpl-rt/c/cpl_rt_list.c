/* cpl_rt_list.c: List (spec/cabi.md 2.3, 2.6, 2.7): contiguous item arrays and views of them, the immortal [], `++`,
   every list builtin, and flatten, each transcribing crates/cpl-eval/src/builtins/prelude.rs: take and drop compare the
   count with the length before any conversion, so a huge count clamps (stdlib.md 4.1), and view the operand's buffer
   instead of copying it, as drop_last does; get is None for a negative or huge index; a result equal to the operand
   shares it. Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <string.h>

static struct {
    cplrt_obj hdr;
} list_empty = { { CPLRT_LIST, 0u, 0u, 0u, { .live = { CPLRT_RC_IMMORTAL, 0u } } } };

cplrt_w CPLRT(empty_list)(void)
{
    return cplrt_word_of(&list_empty.hdr);
}

cplrt_node *CPLRT(list_alloc)(cplrt_runtime *rt, uint32_t n)
{
    cplrt_node *list = (cplrt_node *)CPLRT(obj_new)(rt, CPLRT_LIST, n, sizeof(cplrt_node) + (size_t)n * sizeof(cplrt_w));
    if (list != NULL)
        memset(list->item, 0, (size_t)n * sizeof(cplrt_w));
    return list;
}

/* A new list of the n borrowed words at items (each retained); [] for none. CPLRT_OK or CPLRT_NOMEM. */
static int list_of(cplrt_runtime *rt, const cplrt_w *items, uint32_t n, cplrt_w *out)
{
    return CPLRT(node_make)(rt, CPLRT_LIST, 0, 0, items, n, out);
}

static const cplrt_w *list_items(cplrt_w xs)
{
    return cplrt_items(xs);
}

/* A result of at most this many items is copied rather than viewed: as cheap as a view, and it holds no larger
   buffer alive. (Tests may define it as 0, so that every slice is a view.) */
#ifndef CPLRT_LIST_VIEW_MIN
#define CPLRT_LIST_VIEW_MIN 8u
#endif

/* The items from..to of xs: xs itself for all of them, a copy of a few, else a view of the buffer they live in (a
   view of a view views the same base), so walking a list with first() and drop(1) is linear. CPLRT_OK or
   CPLRT_NOMEM. */
static int list_slice(cplrt_runtime *rt, cplrt_w xs, uint32_t from, uint32_t to, cplrt_w *out)
{
    if (from == 0 && to == cplrt_len(xs)) {
        CPLRT_COPY(*out, xs);
        return CPLRT_OK;
    }
    if (to - from <= CPLRT_LIST_VIEW_MIN || to == from)
        return list_of(rt, list_items(xs) + from, to - from, out);
    const cplrt_obj *o = cplrt_obj_of(xs);
    cplrt_w base = xs;
    uint32_t start = from;
    if (o->flags & CPLRT_F_VIEW) {
        base = ((const cplrt_listview *)o)->base;
        start += o->u.live.def;
    }
    cplrt_listview *v = (cplrt_listview *)CPLRT(obj_new)(rt, CPLRT_LIST, to - from, sizeof(cplrt_listview));
    if (v == NULL)
        return CPLRT_NOMEM;
    v->hdr.flags = CPLRT_F_VIEW;
    v->hdr.u.live.def = start;
    cplrt_retain(base);
    v->base = base;
    CPLRT_SET(*out, cplrt_word_of(&v->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::value::Value::concat (List) */
int CPLRT(list_concat)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    uint32_t na = cplrt_len(a), nb = cplrt_len(b);
    if (nb == 0) {
        CPLRT_COPY(*out, a);
        return CPLRT_OK;
    }
    if (na == 0) {
        CPLRT_COPY(*out, b);
        return CPLRT_OK;
    }
    if (na > UINT32_MAX - nb)
        return CPLRT_NOMEM; /* more children than a header can count (2.3) */
    cplrt_node *r = CPLRT(list_alloc)(rt, na + nb);
    if (r == NULL)
        return CPLRT_NOMEM;
    for (uint32_t i = 0; i < na; i++) {
        cplrt_retain(list_items(a)[i]);
        r->item[i] = list_items(a)[i];
    }
    for (uint32_t i = 0; i < nb; i++) {
        cplrt_retain(list_items(b)[i]);
        r->item[na + i] = list_items(b)[i];
    }
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

static int list_oom(cplrt_cx *cx, uint32_t site, uint32_t stub)
{
    return CPLRT(builtin_leave)(cx, site, stub, CPLRT(oom)(cx));
}

/* mirrors cpl_eval::builtins::prelude::flatten: the inner lists in order; [] for none, the one non-empty inner list
   itself when it is the only one */
int CPLRT(bi_flatten)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    uint32_t n = cplrt_len(a0), nonempty = 0, only = 0;
    uint64_t total = 0;
    for (uint32_t i = 0; i < n; i++) {
        uint32_t k = cplrt_len(list_items(a0)[i]);
        if (k != 0) {
            nonempty++;
            only = i;
            total += k;
        }
    }
    if (nonempty == 0) {
        CPLRT_SET(*out, CPLRT(empty_list)());
        return CPLRT_OK;
    }
    if (nonempty == 1) {
        CPLRT_COPY(*out, list_items(a0)[only]);
        return CPLRT_OK;
    }
    if (total > UINT32_MAX)
        return list_oom(cx, site, stub);
    cplrt_node *r = CPLRT(list_alloc)(cx->rt, (uint32_t)total);
    if (r == NULL)
        return list_oom(cx, site, stub);
    uint32_t at = 0;
    for (uint32_t i = 0; i < n; i++) {
        cplrt_w inner = list_items(a0)[i];
        for (uint32_t j = 0; j < cplrt_len(inner); j++) {
            cplrt_retain(list_items(inner)[j]);
            r->item[at++] = list_items(inner)[j];
        }
    }
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::list_len */
int CPLRT(bi_list_len)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    CPLRT_SET(*out, cplrt_small((int64_t)cplrt_len(a0)));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::list_is_empty */
int CPLRT(bi_list_is_empty)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    CPLRT_SET(*out, cplrt_bool(cplrt_len(a0) == 0));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::clamp_count: min(n, len) for n >= 0, compared before any conversion */
static uint32_t list_clamp(cplrt_w n, uint32_t len)
{
    if (!cplrt_is_small(n))
        return len; /* a positive bigint exceeds every length */
    int64_t v = cplrt_small_value(n);
    return (uint64_t)v < len ? (uint32_t)v : len;
}

/* The precondition `n >= 0` of take and drop, checked first in every contract mode (stdlib.md S39): the fault, with
   `input` {self, n}. */
static int list_count_negative(cplrt_cx *cx, uint32_t site, uint32_t stub, const char *id, cplrt_w xs, cplrt_w n)
{
    cplrt_w args[2] = { xs, n };
    return CPLRT(builtin_leave)(cx, site, stub, CPLRT(raise_builtin_requires)(cx, stub, id, 0, args, 2));
}

/* mirrors cpl_eval::builtins::prelude::list_take */
int CPLRT(bi_list_take)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    if (cplrt_int_lt_small(a1, 0))
        return list_count_negative(cx, site, stub, "list_take", a0, a1);
    uint32_t len = cplrt_len(a0), k = list_clamp(a1, len);
    return list_slice(cx->rt, a0, 0, k, out) == CPLRT_OK ? CPLRT_OK : list_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::prelude::list_drop */
int CPLRT(bi_list_drop)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    if (cplrt_int_lt_small(a1, 0))
        return list_count_negative(cx, site, stub, "list_drop", a0, a1);
    uint32_t len = cplrt_len(a0), k = list_clamp(a1, len);
    return list_slice(cx->rt, a0, k, len, out) == CPLRT_OK ? CPLRT_OK : list_oom(cx, site, stub);
}

/* xs = xs.drop(k) for an owned local (cpl_rt.h): a unique view keeps viewing its base from further along, as long as
   list_slice would make a view of the rest too; otherwise exactly list_drop. */
int cplrt_list_drop_owned(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w *xs, cplrt_w k)
{
    cplrt_obj *o = cplrt_obj_of(*xs);
    if (cplrt_is_small(k) && cplrt_small_value(k) > 0 && (o->flags & CPLRT_F_VIEW) &&
        atomic_load_explicit(&o->u.live.rc, memory_order_acquire) == 1u) {
        uint32_t len = o->len, n = list_clamp(k, len);
        if (len - n > CPLRT_LIST_VIEW_MIN) {
            o->u.live.def += n;
            o->len = len - n;
            return CPLRT_OK;
        }
    }
    cplrt_w out = 0;
    if (CPLRT(bi_list_drop)(cx, site, stub, *xs, k, &out) != CPLRT_OK)
        return CPLRT_FAULT;
    CPLRT_MOVE(*xs, out);
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::list_drop_last */
int CPLRT(bi_list_drop_last)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    uint32_t len = cplrt_len(a0);
    return list_slice(cx->rt, a0, 0, len == 0 ? 0 : len - 1u, out) == CPLRT_OK ? CPLRT_OK : list_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::prelude::list_push */
int CPLRT(bi_list_push)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    uint32_t len = cplrt_len(a0);
    if (len == UINT32_MAX)
        return list_oom(cx, site, stub);
    cplrt_node *r = CPLRT(list_alloc)(cx->rt, len + 1u);
    if (r == NULL)
        return list_oom(cx, site, stub);
    for (uint32_t i = 0; i < len; i++) {
        cplrt_retain(list_items(a0)[i]);
        r->item[i] = list_items(a0)[i];
    }
    cplrt_retain(a1);
    r->item[len] = a1;
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::element: Some(xs[i]) when i names an element, else None */
static int list_element(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w xs, int64_t i, cplrt_w *out)
{
    const cplrt_program *p = cx->rt->program;
    if (i >= 0 && (uint64_t)i < cplrt_len(xs)) {
        cplrt_w x = list_items(xs)[i];
        return CPLRT(node_make)(cx->rt, CPLRT_ENUM, p->std.option, CPL_OPTION_SOME, &x, 1, out) == CPLRT_OK
                   ? CPLRT_OK
                   : list_oom(cx, site, stub);
    }
    return CPLRT(node_make)(cx->rt, CPLRT_ENUM, p->std.option, CPL_OPTION_NONE, NULL, 0, out) == CPLRT_OK
               ? CPLRT_OK
               : list_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::prelude::list_get: no indexing from the end; a huge index is None */
int CPLRT(bi_list_get)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    int64_t i = cplrt_is_small(a1) ? cplrt_small_value(a1) : -1;
    return list_element(cx, site, stub, a0, i, out);
}

/* mirrors cpl_eval::builtins::prelude::list_first */
int CPLRT(bi_list_first)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return list_element(cx, site, stub, a0, 0, out);
}

/* mirrors cpl_eval::builtins::prelude::list_last */
int CPLRT(bi_list_last)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return list_element(cx, site, stub, a0, (int64_t)cplrt_len(a0) - 1, out);
}

/* mirrors cpl_eval::builtins::prelude::list_reverse */
int CPLRT(bi_list_reverse)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    uint32_t len = cplrt_len(a0);
    if (len == 0) {
        CPLRT_SET(*out, CPLRT(empty_list)());
        return CPLRT_OK;
    }
    cplrt_node *r = CPLRT(list_alloc)(cx->rt, len);
    if (r == NULL)
        return list_oom(cx, site, stub);
    for (uint32_t i = 0; i < len; i++) {
        cplrt_w x = list_items(a0)[len - 1u - i];
        cplrt_retain(x);
        r->item[i] = x;
    }
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::list_contains: some element == v by structural equality (2.8), left to right */
int CPLRT(bi_list_contains)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    uint32_t len = cplrt_len(a0);
    int found = 0;
    for (uint32_t i = 0; i < len && !found; i++)
        if (CPLRT(value_equal)(cx->rt, list_items(a0)[i], a1, &found) != CPLRT_OK)
            return list_oom(cx, site, stub);
    CPLRT_SET(*out, cplrt_bool(found));
    return CPLRT_OK;
}

/* An owned local's xs = xs.push(item). Unique lists grow geometrically; snapshots still share immutable values.
   The spare-capacity flag and def word are list-internal: lists have no nominal definition id. */
#define LIST_CAPACITY CPLRT_F_CAPACITY

/* An owned local's list *xs made ready for `extra` more items at its end: *room is the node to append them to (at
   room->hdr.len), and *view a view of it to lengthen with it, or NULL when *xs is that node. A unique node with spare
   capacity grows in place; so does a unique view that ends where the base it alone holds ends, while that base has
   room and the base's dead prefix (the items before the view) is no longer than the view, so a queue that drops from
   the front and pushes at the back stays linear. Anything else is rebuilt as a node of geometric capacity: its items
   moved when nothing else holds them (a moved view's dead prefix is released with its base), else retained. */
static int list_reserve_owned(cplrt_runtime *rt, cplrt_w *xs, uint32_t extra, cplrt_node **room, cplrt_listview **view)
{
    cplrt_obj *old = cplrt_obj_of(*xs);
    uint32_t len = old->len;
    if (len > UINT32_MAX - extra)
        return CPLRT_NOMEM;
    uint32_t needed = len + extra;
    int unique = atomic_load_explicit(&old->u.live.rc, memory_order_acquire) == 1u;
    cplrt_obj *owner = old; /* the object whose items are the list's: old, or the base of a view */
    uint32_t start = 0;
    int movable = unique;
    *view = NULL;
    if (old->flags & CPLRT_F_VIEW) {
        owner = cplrt_obj_of(((cplrt_listview *)old)->base);
        start = old->u.live.def;
        movable = unique && atomic_load_explicit(&owner->u.live.rc, memory_order_acquire) == 1u &&
                  start + len == owner->len;
        uint32_t cap = (owner->flags & LIST_CAPACITY) ? owner->u.live.def : owner->len;
        if (movable && start <= len && extra <= cap - owner->len) {
            *room = (cplrt_node *)owner;
            *view = (cplrt_listview *)old;
            return CPLRT_OK;
        }
    } else {
        uint32_t cap = (old->flags & LIST_CAPACITY) ? old->u.live.def : len;
        if (unique && needed <= cap) {
            *room = (cplrt_node *)old;
            return CPLRT_OK;
        }
    }
    uint32_t next = len < 8u ? 8u : (len <= UINT32_MAX / 2u ? len * 2u : UINT32_MAX);
    if (next < needed) next = needed;
    if (unique && !(old->flags & CPLRT_F_VIEW)) {
        /* a unique node grows its own block: realloc can move a large one without copying its items */
        cplrt_node *g = (cplrt_node *)CPLRT(obj_grow)(rt, old, sizeof(cplrt_node) + (size_t)next * sizeof(cplrt_w));
        if (g == NULL)
            return CPLRT_NOMEM;
        g->hdr.flags |= LIST_CAPACITY;
        g->hdr.u.live.def = next;
        *xs = cplrt_word_of(&g->hdr); /* the old block is this one, moved: nothing to release */
        *room = g;
        return CPLRT_OK;
    }
    /* the items past len are never read (release and every reader stop at len): no need to clear them */
    cplrt_node *list = (cplrt_node *)CPLRT(obj_new)(rt, CPLRT_LIST, len, sizeof(cplrt_node) + (size_t)next * sizeof(cplrt_w));
    if (list == NULL)
        return CPLRT_NOMEM;
    list->hdr.flags |= LIST_CAPACITY;
    list->hdr.u.live.def = next;
    list->hdr.len = len;
    const cplrt_w *items = ((const cplrt_node *)owner)->item + start;
    for (uint32_t i = 0; i < len; i++) {
        cplrt_w v = items[i];
        if (!movable) cplrt_retain(v);
        list->item[i] = v;
    }
    if (movable) owner->len = start; /* children moved; the old buffer releases only what is before them */
    CPLRT_SET(*xs, cplrt_word_of(&list->hdr));
    *room = list;
    return CPLRT_OK;
}

/* mirrors cpl_eval::interp::Interpreter::exec_stmt: reuse uniquely owned list capacity for a push assignment. */
int cplrt_list_push_owned(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w *xs, cplrt_w item)
{
    /* An argument may alias the old buffer or one of its children. Own it before reserve can release that buffer. */
    cplrt_retain(item);
    cplrt_node *list;
    cplrt_listview *view;
    if (list_reserve_owned(cx->rt, xs, 1u, &list, &view) != CPLRT_OK) {
        cplrt_release(item);
        return list_oom(cx, site, stub);
    }
    list->item[list->hdr.len++] = item;
    if (view != NULL)
        view->hdr.len++;
    return CPLRT_OK;
}

/* ---- the scalar tier's list operations (cpl_rt.h): never a fault, CPLRT_NOMEM instead ------------------------- */

/* A flat array's room doubled (8 elements at first), within the heap budget and a list's length limit; the elements
   kept. Past CPLRT_Q_BUF_MAX bytes it fails, and the scalar form bails: the generic form, whose lists grow at the pace
   of its own steps, builds a list that large (so that a time budget meets it with as much memory in use as without
   the scalar tier). */
#define CPLRT_Q_BUF_MAX ((size_t)64 << 20)
int cplrt_q_buf_grow(cplrt_cx *cx, cplrt_qbuf *b, size_t elem)
{
    uint32_t cap = b->cap == 0u ? 8u : b->cap;
    if (b->cap != 0u) {
        if (b->cap > UINT32_MAX / 2u)
            return CPLRT_NOMEM;
        cap = b->cap * 2u;
    }
    if (elem == 0u || (size_t)cap > CPLRT_Q_BUF_MAX / elem)
        return CPLRT_NOMEM;
    /* grown in place where the allocator can (realloc: a large block moves without a copy) */
    void *a = b->a == NULL ? CPLRT(alloc)(cx->rt, (size_t)cap * elem)
                           : (void *)CPLRT(obj_grow)(cx->rt, (cplrt_obj *)b->a, (size_t)cap * elem);
    if (a == NULL)
        return CPLRT_NOMEM;
    b->a = a;
    b->cap = cap;
    return CPLRT_OK;
}

void cplrt_q_buf_free(cplrt_qbuf *b)
{
    CPLRT(free)(b->a);
    b->a = NULL;
    b->n = b->cap = 0u;
}

int cplrt_q_list(cplrt_cx *cx, const cplrt_w *items, uint32_t n, cplrt_w *out)
{
    if (n == 0u) {
        CPLRT_SET(*out, CPLRT(empty_list)());
        return CPLRT_OK;
    }
    return list_of(cx->rt, items, n, out);
}

int cplrt_q_push(cplrt_cx *cx, cplrt_w xs, cplrt_w item, cplrt_w *out)
{
    cplrt_w one = 0;
    if (list_of(cx->rt, &item, 1u, &one) != CPLRT_OK)
        return CPLRT_NOMEM;
    int st = CPLRT(list_concat)(cx->rt, xs, one, out);
    cplrt_release(one);
    return st;
}

int cplrt_q_push_owned(cplrt_cx *cx, cplrt_w *xs, cplrt_w item)
{
    cplrt_retain(item);
    cplrt_node *list;
    cplrt_listview *view;
    if (list_reserve_owned(cx->rt, xs, 1u, &list, &view) != CPLRT_OK) {
        cplrt_release(item);
        return CPLRT_NOMEM;
    }
    list->item[list->hdr.len++] = item;
    if (view != NULL)
        view->hdr.len++;
    return CPLRT_OK;
}

int cplrt_q_slice(cplrt_cx *cx, cplrt_w xs, uint32_t from, uint32_t to, cplrt_w *out)
{
    return list_slice(cx->rt, xs, from, to, out);
}

int cplrt_q_drop_owned(cplrt_cx *cx, cplrt_w *xs, uint32_t n)
{
    cplrt_obj *o = cplrt_obj_of(*xs);
    uint32_t len = o->len;
    if (n == 0u)
        return CPLRT_OK;
    if ((o->flags & CPLRT_F_VIEW) && len - n > CPLRT_LIST_VIEW_MIN &&
        atomic_load_explicit(&o->u.live.rc, memory_order_acquire) == 1u) {
        o->u.live.def += n;
        o->len = len - n;
        return CPLRT_OK;
    }
    cplrt_w out = 0;
    if (list_slice(cx->rt, *xs, n, len, &out) != CPLRT_OK)
        return CPLRT_NOMEM;
    CPLRT_MOVE(*xs, out);
    return CPLRT_OK;
}

int cplrt_q_concat(cplrt_cx *cx, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    return CPLRT(list_concat)(cx->rt, a, b, out);
}

/* mirrors cpl_eval::interp::Interpreter::exec_stmt: xs = xs ++ other, after other has evaluated without
   reassigning xs. Shared snapshots still copy. */
int cplrt_list_concat_owned(cplrt_cx *cx, cplrt_w *xs, cplrt_w other)
{
    uint32_t n = cplrt_len(other);
    if (n == 0u) return CPLRT_OK;
    cplrt_retain(other); /* includes xs ++ xs and a borrowed child of xs */
    cplrt_node *list;
    cplrt_listview *view;
    if (list_reserve_owned(cx->rt, xs, n, &list, &view) != CPLRT_OK) {
        cplrt_release(other);
        return CPLRT(oom)(cx);
    }
    uint32_t len = list->hdr.len;
    for (uint32_t i = 0; i < n; i++) {
        cplrt_w item = list_items(other)[i];
        cplrt_retain(item);
        list->item[len + i] = item;
    }
    list->hdr.len = len + n;
    if (view != NULL)
        view->hdr.len += n;
    cplrt_release(other);
    return CPLRT_OK;
}
