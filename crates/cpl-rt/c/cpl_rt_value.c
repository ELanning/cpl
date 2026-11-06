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
   of max_align_t so that the block after it keeps that alignment. A slot of the object pool (below) has the same
   header, with the low bit of rt set; a free slot links the next free slot through size. */
typedef struct value_block {
    _Alignas(max_align_t) cplrt_runtime *rt;
    size_t size;
} value_block;
#define VALUE_ALIGN _Alignof(max_align_t)

static inline value_block *value_block_of(const void *p)
{
    return (value_block *)(void *)((char *)(uintptr_t)p - sizeof(value_block));
}

static inline cplrt_runtime *value_block_rt(const value_block *b)
{
    return (cplrt_runtime *)((uintptr_t)b->rt & ~(uintptr_t)1u);
}

static inline int value_block_pooled(const value_block *b)
{
    return ((uintptr_t)b->rt & 1u) != 0u;
}

/* Count total more bytes in rt->live_bytes, within the budget, and the peak: 0 when the budget refuses them. */
static int value_charge(cplrt_runtime *rt, uint64_t total)
{
    uint64_t live = atomic_load_explicit(&rt->live_bytes, memory_order_relaxed);
    if (rt->max_heap_bytes != 0) {
        do {
            if (total > rt->max_heap_bytes || live > rt->max_heap_bytes - total)
                return 0;
        } while (!atomic_compare_exchange_weak_explicit(&rt->live_bytes, &live, live + total, memory_order_relaxed,
                                                        memory_order_relaxed));
    } else {
        live = atomic_fetch_add_explicit(&rt->live_bytes, total, memory_order_relaxed);
    }
    uint64_t now = live + total;
    uint64_t peak = atomic_load_explicit(&rt->peak_bytes, memory_order_relaxed);
    while (now > peak && !atomic_compare_exchange_weak_explicit(&rt->peak_bytes, &peak, now, memory_order_relaxed,
                                                                memory_order_relaxed)) {
    }
    return 1;
}

static void pool_release_block(value_block *b);

void *CPLRT(alloc)(cplrt_runtime *rt, size_t size)
{
    if (size == 0)
        size = 1;
    if (size > SIZE_MAX - sizeof(value_block))
        return NULL;
    size_t total = size + sizeof(value_block);
    if (!value_charge(rt, (uint64_t)total))
        return NULL;
    void *raw = rt->allocator.alloc ? rt->allocator.alloc(rt->allocator.user, total, VALUE_ALIGN) : malloc(total);
    if (raw == NULL) {
        atomic_fetch_sub_explicit(&rt->live_bytes, (uint64_t)total, memory_order_relaxed);
        return NULL;
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
    value_block *b = value_block_of(p);
    if (value_block_pooled(b)) {
        pool_release_block(b);
        return;
    }
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

/* Grow the block of the unique object o to size bytes: in place where the allocator can (realloc, with the default
   allocator; a large block then moves without a copy), else a new block with the old bytes copied and the old one
   freed (a pool slot always moves out of the pool). The budget and the statistics count the difference, as alloc and
   free would. The object, perhaps moved, or NULL with o unchanged. */
cplrt_obj *CPLRT(obj_grow)(cplrt_runtime *rt, cplrt_obj *o, size_t size)
{
    value_block *b = value_block_of(o);
    size_t old = b->size;
    if (size <= old)
        return o;
    if (rt->allocator.alloc != NULL || value_block_pooled(b)) {
        cplrt_obj *n = (cplrt_obj *)CPLRT(alloc)(rt, size);
        if (n == NULL)
            return NULL;
        memcpy(n, o, old);
        if (value_block_pooled(b)) /* the object now counts as a general block, and its slot as freed */
            atomic_fetch_add_explicit(&rt->live_objects, 1u, memory_order_relaxed);
        CPLRT(free)(o);
        return n;
    }
    if (size > SIZE_MAX - sizeof(value_block))
        return NULL;
    uint64_t delta = (uint64_t)(size - old);
    if (!value_charge(rt, delta))
        return NULL;
    value_block *g = (value_block *)realloc(b, size + sizeof(value_block));
    if (g == NULL) {
        atomic_fetch_sub_explicit(&rt->live_bytes, delta, memory_order_relaxed);
        return NULL;
    }
    g->size = size;
    return (cplrt_obj *)(void *)((char *)g + sizeof(value_block));
}

size_t CPLRT(alloc_size)(const void *p)
{
    return value_block_of(p)->size;
}

/* ---- the object pool (2.4) ------------------------------------------------------------------------------------- */

/* While an evaluation runs, its thread owns the runtime's object pool, and every object of up to POOL_CLASSES * 16
   bytes (a struct, an Option, a short list, a small text) it makes is a slot of a 64 KiB page of the pool, one page
   per 16-byte size class at a time: taken from the page's free slots or the untouched rest of it, with no malloc and
   no atomic counter update, and freed onto its page's free slots by the owner the same way. A page counts whole in
   rt->live_bytes (the budget sees it at once), and the objects of the pool count in rt->live_objects through
   rt->pool_net, which the owner adds when its ownership ends, so the statistics between calls are exact. A page left
   with no objects is kept as a spare for the next pages, uncounted, while the spares are fewer than the most pages
   in use since the ownership began (at least POOL_SPARE, at most POOL_SPARE_MAX: the memory of a heap that shrinks
   and grows again is not returned to the system and faulted back in each time), and freed otherwise; when an
   ownership ends, the spares past what it used are freed.

   One thread owns the pool at a time: the outermost evaluation of the runtime on a thread, claimed when no other
   thread holds it, or a release outside any evaluation for its own duration; the owner check is the thread pointer
   (value_thread), not a thread-local, whose access from a shared library is a call. A thread that frees a slot while
   another owns the pool (a parallel section's worker, a concurrent call) pushes it on its page's remote stack, which
   the owner takes back when the page's other slots run out and, flagged by rt->pool_remote, when its ownership ends. The pool makes
   nothing for a runtime with a host allocator (which sees every block) or a small heap budget (whose page-sized
   steps would move the point at which an evaluation runs out of memory). Under AddressSanitizer a free slot is
   poisoned, so that a use after free is still caught until the slot is reused. */
#if defined(__SANITIZE_ADDRESS__)
#define POOL_ASAN 1
#elif defined(__has_feature)
#if __has_feature(address_sanitizer)
#define POOL_ASAN 1
#endif
#endif
#ifdef POOL_ASAN
#include <sanitizer/asan_interface.h>
#define POOL_POISON(p, n) ASAN_POISON_MEMORY_REGION((p), (n))
#define POOL_UNPOISON(p, n) ASAN_UNPOISON_MEMORY_REGION((p), (n))
#else
#define POOL_POISON(p, n) ((void)(p), (void)(n))
#define POOL_UNPOISON(p, n) ((void)(p), (void)(n))
#endif

#define POOL_CLASSES 4u
#define POOL_PAGE ((size_t)1 << 16)
#define POOL_SPARE 4u
#define POOL_SPARE_MAX 1024u /* 64 MiB */
#define POOL_MIN_BUDGET (UINT64_C(1) << 26)

typedef struct cplrt_pool_page {
    struct cplrt_pool_page *prev, *next;   /* rt->pool_pages */
    struct cplrt_pool_page *pprev, *pnext; /* rt->pool_partial[cls], while listed */
    value_block *free;                     /* the owner's free slots, linked through size */
    _Atomic uintptr_t remote;              /* the slots other threads freed, linked through size */
    char *bump, *end;                      /* the slots never used: [bump, end) */
    uint32_t used;                         /* slots taken and not yet freed onto free (remote ones included) */
    uint32_t slot;                         /* the size of a slot: the header and 16 * (cls + 1) bytes */
    uint8_t cls, listed;
} pool_page;
#define POOL_FIRST ((sizeof(pool_page) + 63u) & ~(size_t)63u)
_Static_assert(POOL_FIRST % sizeof(value_block) == 0, "slots keep the alignment of a block");

static inline pool_page *pool_page_of(const value_block *b)
{
    return (pool_page *)((uintptr_t)b & ~(uintptr_t)(POOL_PAGE - 1u));
}

/* The calling thread's identity: its thread pointer where one instruction reads it (the TLS ABI of x86-64 and AArch64
   Linux keeps it per thread and fixed for the thread's life), else pthread_self. */
static inline uintptr_t value_thread(void)
{
#if defined(__linux__) && defined(__x86_64__)
    uintptr_t t;
    __asm__("mov %%fs:0, %0" : "=r"(t));
    return t;
#elif defined(__linux__) && defined(__aarch64__)
    uintptr_t t;
    __asm__("mrs %0, tpidr_el0" : "=r"(t));
    return t;
#else
    return (uintptr_t)pthread_self();
#endif
}

static inline int pool_mine(cplrt_runtime *rt)
{
    return atomic_load_explicit(&rt->cache_owner, memory_order_relaxed) == value_thread();
}

static inline int pool_owned_by(cplrt_runtime *rt, uintptr_t self)
{
    return atomic_load_explicit(&rt->cache_owner, memory_order_relaxed) == self;
}

/* Whether the calling thread may make objects in rt's pool (checked at each use: a budget lowered during an
   evaluation turns the pool off from then on). */
static inline int pool_alloc_ok(cplrt_runtime *rt)
{
    return pool_mine(rt) && rt->allocator.alloc == NULL &&
           (rt->max_heap_bytes == 0 || rt->max_heap_bytes >= POOL_MIN_BUDGET);
}

static void pool_list(cplrt_runtime *rt, pool_page *pg)
{
    pool_page **head = &rt->pool_partial[pg->cls];
    pg->pprev = NULL;
    pg->pnext = *head;
    if (*head != NULL)
        (*head)->pprev = pg;
    *head = pg;
    pg->listed = 1;
}

static void pool_unlist(cplrt_runtime *rt, pool_page *pg)
{
    if (pg->pprev != NULL)
        pg->pprev->pnext = pg->pnext;
    else
        rt->pool_partial[pg->cls] = pg->pnext;
    if (pg->pnext != NULL)
        pg->pnext->pprev = pg->pprev;
    pg->listed = 0;
}

/* How many spare pages to keep: the most pages in use lately, within [POOL_SPARE, POOL_SPARE_MAX]. */
static inline uint32_t pool_spare_limit(const cplrt_runtime *rt)
{
    uint32_t n = rt->pool_peak;
    return n < POOL_SPARE ? POOL_SPARE : n > POOL_SPARE_MAX ? POOL_SPARE_MAX : n;
}

/* A page with no objects left, not a class's current one: freed, or kept as a spare. */
static void pool_page_drop(cplrt_runtime *rt, pool_page *pg)
{
    if (pg->listed)
        pool_unlist(rt, pg);
    if (pg->prev != NULL)
        pg->prev->next = pg->next;
    else
        rt->pool_pages = pg->next;
    if (pg->next != NULL)
        pg->next->prev = pg->prev;
    atomic_fetch_sub_explicit(&rt->live_bytes, (uint64_t)POOL_PAGE, memory_order_relaxed);
    rt->pool_npages--;
    if (rt->pool_nspare < pool_spare_limit(rt)) {
        pg->next = rt->pool_spare;
        rt->pool_spare = pg;
        rt->pool_nspare++;
    } else {
        POOL_UNPOISON(pg, POOL_PAGE);
        free(pg);
    }
}

/* A page for class c, a spare or a new one, counted and linked: NULL when the budget or the system refuses it. */
static pool_page *pool_page_new(cplrt_runtime *rt, uint32_t c)
{
    if (!value_charge(rt, (uint64_t)POOL_PAGE))
        return NULL;
    pool_page *pg = rt->pool_spare;
    if (pg != NULL) {
        rt->pool_spare = pg->next;
        rt->pool_nspare--;
    } else {
        void *m = NULL;
        if (posix_memalign(&m, POOL_PAGE, POOL_PAGE) != 0 || m == NULL) {
            atomic_fetch_sub_explicit(&rt->live_bytes, (uint64_t)POOL_PAGE, memory_order_relaxed);
            return NULL;
        }
        pg = (pool_page *)m;
    }
    pg->free = NULL;
    atomic_init(&pg->remote, 0u);
    pg->bump = (char *)pg + POOL_FIRST;
    pg->end = (char *)pg + POOL_PAGE;
    pg->used = 0;
    pg->slot = (uint32_t)(sizeof(value_block) + 16u * (c + 1u));
    pg->cls = (uint8_t)c;
    pg->listed = 0;
    pg->pprev = pg->pnext = NULL;
    pg->prev = NULL;
    pg->next = rt->pool_pages;
    if (rt->pool_pages != NULL)
        rt->pool_pages->prev = pg;
    rt->pool_pages = pg;
    if (++rt->pool_npages > rt->pool_peak)
        rt->pool_peak = rt->pool_npages;
    POOL_POISON(pg->bump, (size_t)(pg->end - pg->bump));
    return pg;
}

static inline value_block *pool_take(pool_page *pg)
{
    value_block *b = pg->free;
    if (b != NULL) {
        pg->free = (value_block *)(uintptr_t)b->size;
        pg->used++;
        return b;
    }
    if ((size_t)(pg->end - pg->bump) >= pg->slot) {
        b = (value_block *)(void *)pg->bump;
        pg->bump += pg->slot;
        pg->used++;
        POOL_UNPOISON(b, sizeof *b);
        return b;
    }
    return NULL;
}

/* The slots other threads freed on pg, onto its free slots. */
static void pool_collect(cplrt_runtime *rt, pool_page *pg)
{
    uintptr_t h = atomic_exchange_explicit(&pg->remote, (uintptr_t)0, memory_order_acquire);
    uint32_t n = 0;
    while (h != 0) {
        value_block *b = (value_block *)h;
        h = (uintptr_t)b->size;
        b->size = (size_t)(uintptr_t)pg->free;
        pg->free = b;
        n++;
    }
    pg->used -= n;
    rt->pool_net -= (int64_t)n;
}

/* A slot of class c when the current page has none: its remote slots, another page with free slots, a new page. */
static value_block *pool_refill(cplrt_runtime *rt, uint32_t c)
{
    pool_page *pg = rt->pool_cur[c];
    value_block *b;
    if (pg != NULL && atomic_load_explicit(&pg->remote, memory_order_relaxed) != 0) {
        pool_collect(rt, pg);
        if ((b = pool_take(pg)) != NULL)
            return b;
    }
    /* the page being left is full: a free onto it lists it again */
    while ((pg = rt->pool_partial[c]) != NULL) {
        pool_unlist(rt, pg);
        rt->pool_cur[c] = pg;
        if ((b = pool_take(pg)) != NULL)
            return b;
    }
    pg = pool_page_new(rt, c);
    if (pg == NULL)
        return NULL;
    rt->pool_cur[c] = pg;
    return pool_take(pg);
}

/* The owner frees slot b. */
static void pool_free_local(cplrt_runtime *rt, value_block *b)
{
    pool_page *pg = pool_page_of(b);
    POOL_POISON(b + 1, pg->slot - sizeof *b);
    b->size = (size_t)(uintptr_t)pg->free;
    pg->free = b;
    pg->used--;
    rt->pool_net--;
    if (pg != rt->pool_cur[pg->cls]) {
        if (pg->used == 0)
            pool_page_drop(rt, pg);
        else if (!pg->listed)
            pool_list(rt, pg);
    }
}

/* Another thread than the owner frees slot b: onto its page's remote stack. Until then b counts in pg->used, so the
   page outlives the push. */
static void pool_free_remote(cplrt_runtime *rt, value_block *b)
{
    pool_page *pg = pool_page_of(b);
    POOL_POISON(b + 1, pg->slot - sizeof *b);
    uintptr_t h = atomic_load_explicit(&pg->remote, memory_order_relaxed);
    do {
        b->size = (size_t)h;
    } while (!atomic_compare_exchange_weak_explicit(&pg->remote, &h, (uintptr_t)b, memory_order_release,
                                                    memory_order_relaxed));
    atomic_store_explicit(&rt->pool_remote, 1u, memory_order_release);
}

static int pool_claim(cplrt_runtime *rt)
{
    uintptr_t none = 0;
    if (!atomic_compare_exchange_strong_explicit(&rt->cache_owner, &none, value_thread(), memory_order_acquire,
                                                 memory_order_relaxed))
        return 0;
    rt->cache_depth = 1;
    return 1;
}

/* The owner's ownership ends: the remote slots taken back if any were pushed, the empty current pages dropped, and
   the pool's objects counted in live_objects; then the pool is no thread's. */
static void pool_unclaim(cplrt_runtime *rt)
{
    if (atomic_load_explicit(&rt->pool_remote, memory_order_relaxed) != 0u &&
        atomic_exchange_explicit(&rt->pool_remote, 0u, memory_order_acquire) != 0u) {
        pool_page *pg = rt->pool_pages;
        while (pg != NULL) {
            pool_page *next = pg->next;
            if (atomic_load_explicit(&pg->remote, memory_order_relaxed) != 0) {
                pool_collect(rt, pg);
                if (pg != rt->pool_cur[pg->cls]) {
                    if (pg->used == 0)
                        pool_page_drop(rt, pg);
                    else if (!pg->listed)
                        pool_list(rt, pg);
                }
            }
            pg = next;
        }
    }
    for (uint32_t c = 0; c < POOL_CLASSES; c++) {
        pool_page *pg = rt->pool_cur[c];
        if (pg != NULL && pg->used == 0) {
            rt->pool_cur[c] = NULL;
            pool_page_drop(rt, pg);
        }
    }
    if (rt->pool_net != 0) {
        atomic_fetch_add_explicit(&rt->live_objects, (uint64_t)rt->pool_net, memory_order_relaxed);
        rt->pool_net = 0;
    }
    /* the spares past what this ownership used are freed; the next one starts from the pages in use */
    uint32_t keep = pool_spare_limit(rt);
    while (rt->pool_nspare > keep) {
        pool_page *pg = rt->pool_spare;
        rt->pool_spare = pg->next;
        rt->pool_nspare--;
        POOL_UNPOISON(pg, POOL_PAGE);
        free(pg);
    }
    rt->pool_peak = rt->pool_npages;
    rt->cache_depth = 0;
    atomic_store_explicit(&rt->cache_owner, 0, memory_order_release);
}

/* A slot freed outside release_obj's loop (CPLRT(free), obj_grow). */
static void pool_release_block(value_block *b)
{
    cplrt_runtime *rt = value_block_rt(b);
    if (pool_mine(rt)) {
        pool_free_local(rt, b);
    } else if (pool_claim(rt)) {
        pool_free_local(rt, b);
        pool_unclaim(rt);
    } else {
        pool_free_remote(rt, b);
    }
}

/* An evaluation of rt begins on this thread (CPLRT(cx_begin)). */
void CPLRT(value_cache_enter)(cplrt_runtime *rt)
{
    if (pool_mine(rt))
        rt->cache_depth++;
    else
        (void)pool_claim(rt);
}

/* An evaluation of rt ends (CPLRT(cx_end)); the owner's outermost gives the pool up. */
void CPLRT(value_cache_leave)(cplrt_runtime *rt)
{
    if (!pool_mine(rt))
        return;
    if (rt->cache_depth > 1) {
        rt->cache_depth--;
        return;
    }
    pool_unclaim(rt);
}

/* The statistics made exact (cpl_runtime_stats, a close): the remote slots taken back, when no thread owns the pool. */
void CPLRT(value_settle)(cplrt_runtime *rt)
{
    if (atomic_load_explicit(&rt->pool_remote, memory_order_relaxed) != 0u && pool_claim(rt))
        pool_unclaim(rt);
}

/* The runtime closes: every page freed, whatever it still holds. */
void CPLRT(value_pool_fini)(cplrt_runtime *rt)
{
    pool_page *pg = rt->pool_pages;
    while (pg != NULL) {
        pool_page *next = pg->next;
        atomic_fetch_sub_explicit(&rt->live_bytes, (uint64_t)POOL_PAGE, memory_order_relaxed);
        POOL_UNPOISON(pg, POOL_PAGE);
        free(pg);
        pg = next;
    }
    pg = rt->pool_spare;
    while (pg != NULL) {
        pool_page *next = pg->next;
        POOL_UNPOISON(pg, POOL_PAGE);
        free(pg);
        pg = next;
    }
    rt->pool_pages = rt->pool_spare = NULL;
    rt->pool_nspare = rt->pool_npages = rt->pool_peak = 0;
    for (uint32_t c = 0; c < POOL_CLASSES; c++)
        rt->pool_cur[c] = rt->pool_partial[c] = NULL;
}

/* ---- heap objects and release (2.3, 2.4) ----------------------------------------------------------------------- */

cplrt_obj *CPLRT(obj_new)(cplrt_runtime *rt, uint8_t kind, uint32_t len, size_t size)
{
    cplrt_obj *o = NULL;
    if (size <= POOL_CLASSES * 16u && pool_alloc_ok(rt)) {
        uint32_t c = size == 0 ? 0u : (uint32_t)((size - 1u) / 16u);
        pool_page *pg = rt->pool_cur[c];
        value_block *b = pg != NULL ? pool_take(pg) : NULL;
        if (b == NULL)
            b = pool_refill(rt, c);
        if (b != NULL) {
            b->rt = (cplrt_runtime *)((uintptr_t)rt | 1u);
            b->size = (size_t)(c + 1u) * 16u;
            POOL_UNPOISON(b + 1, b->size);
            rt->pool_net++;
            o = (cplrt_obj *)(void *)(b + 1);
            o->kind = kind;
            o->flags = 0;
            o->variant = 0;
            o->len = len;
            atomic_init(&o->u.live.rc, 1u);
            o->u.live.def = 0;
            return o;
        }
        return NULL; /* no page: the budget, or the system, refused it */
    }
    o = (cplrt_obj *)CPLRT(alloc)(rt, size);
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

/* Whether the dying object c holds no object: a text, bytes, a bigint, or a node whose children are all immediates. */
static inline int value_leaf(const cplrt_obj *c)
{
    if (c->kind < CPLRT_LIST || c->kind > CPLRT_WRAPPER)
        return 1;
    if (c->flags & CPLRT_F_VIEW)
        return 0;
    const cplrt_w *items = ((const cplrt_node *)c)->item;
    for (uint32_t i = 0; i < c->len; i++)
        if (cplrt_is_obj(items[i]))
            return 0;
    return 1;
}

/* The last reference to o is gone. The dying objects form an intrusive list through u.next, and the children of each
   are decremented from that list, so release never recurses on the depth of a value (2.4). A dying child that holds
   no object is freed at once, while it is in the cache (a list of a million structs is one pass, not two). An
   immortal child is left alone, as cplrt_release leaves it. Outside an evaluation, the first slot of a pool claims
   the pool for the rest. */
void CPLRT(release_obj)(cplrt_obj *o)
{
    cplrt_obj *pending = o;
    cplrt_runtime *claimed = NULL;
    uintptr_t self = 0; /* the calling thread, once a pool slot needs it */
    o->u.next = NULL;
    while (pending != NULL) {
        cplrt_obj *cur = pending;
        pending = cur->u.next;
        if (cur->kind >= CPLRT_LIST && cur->kind <= CPLRT_WRAPPER) {
            /* a List view owns one reference: its base */
            int view = (cur->flags & CPLRT_F_VIEW) != 0;
            const cplrt_w *children = view ? &((const cplrt_listview *)cur)->base : ((const cplrt_node *)cur)->item;
            uint32_t count = view ? 1u : cur->len;
            for (uint32_t i = 0; i < count; i++) {
                cplrt_w w = children[i];
                if (!cplrt_is_obj(w))
                    continue;
                cplrt_obj *c = cplrt_obj_of(w);
                uint32_t rc = atomic_load_explicit(&c->u.live.rc, memory_order_relaxed);
                if (rc >= CPLRT_RC_SATURATED)
                    continue;
                int last;
                if (!(c->flags & CPLRT_F_SHARED)) {
                    last = rc == 1u;
                    if (!last)
                        atomic_store_explicit(&c->u.live.rc, rc - 1u, memory_order_relaxed);
                } else {
                    last = atomic_fetch_sub_explicit(&c->u.live.rc, 1u, memory_order_acq_rel) == 1u;
                }
                if (last) {
                    if (value_leaf(c)) {
                        value_block *cb = value_block_of(c);
                        cplrt_runtime *crt = value_block_rt(cb);
                        if (value_block_pooled(cb)) {
                            if (self == 0)
                                self = value_thread();
                            if (pool_owned_by(crt, self)) {
                                pool_free_local(crt, cb);
                                continue;
                            }
                        } else {
                            atomic_fetch_sub_explicit(&crt->live_objects, 1u, memory_order_relaxed);
                            CPLRT(free)(c);
                            continue;
                        }
                    }
                    c->u.next = pending;
                    pending = c;
                }
            }
        }
        value_block *b = value_block_of(cur);
        cplrt_runtime *rt = value_block_rt(b);
        if (value_block_pooled(b)) {
            if (self == 0)
                self = value_thread();
            if (pool_owned_by(rt, self)) {
                pool_free_local(rt, b);
            } else if (claimed == NULL && pool_claim(rt)) {
                claimed = rt;
                pool_free_local(rt, b);
            } else {
                pool_free_remote(rt, b);
            }
            continue;
        }
        atomic_fetch_sub_explicit(&rt->live_objects, 1u, memory_order_relaxed);
        CPLRT(free)(cur);
    }
    if (claimed != NULL)
        pool_unclaim(claimed);
}

/* Mark w and everything it reaches CPLRT_F_SHARED (cpl_rt.h), before it can reach another thread. Marking stops at an
   object already shared (all it reaches is) and at immortals. CPLRT_OK, or CPLRT_NOMEM with only part marked, which
   is harmless: a shared object's count is merely updated atomically. */
int CPLRT(value_share)(cplrt_runtime *rt, cplrt_w w)
{
    if (!cplrt_is_obj(w))
        return CPLRT_OK;
    /* the work stack starts on the C stack: sharing a small value allocates nothing */
    cplrt_obj *local[64];
    size_t cap = sizeof local / sizeof local[0], n = 0;
    cplrt_obj **work = local;
    work[n++] = cplrt_obj_of(w);
    int st = CPLRT_OK;
    while (n > 0) {
        cplrt_obj *o = work[--n];
        if ((o->flags & CPLRT_F_SHARED) ||
            atomic_load_explicit(&o->u.live.rc, memory_order_relaxed) >= CPLRT_RC_SATURATED)
            continue;
        o->flags |= CPLRT_F_SHARED;
        if (o->kind < CPLRT_LIST || o->kind > CPLRT_WRAPPER)
            continue;
        int view = (o->flags & CPLRT_F_VIEW) != 0;
        const cplrt_w *children = view ? &((const cplrt_listview *)o)->base : ((const cplrt_node *)o)->item;
        uint32_t count = view ? 1u : o->len;
        for (uint32_t i = 0; i < count; i++) {
            if (!cplrt_is_obj(children[i]))
                continue;
            if (n == cap) {
                cplrt_obj **more = (cplrt_obj **)CPLRT(alloc)(rt, cap * 2u * sizeof *work);
                if (more == NULL) {
                    st = CPLRT_NOMEM;
                    n = 0;
                    break;
                }
                memcpy(more, work, n * sizeof *work);
                if (work != local)
                    CPLRT(free)(work);
                work = more;
                cap *= 2u;
            }
            work[n++] = cplrt_obj_of(children[i]);
        }
    }
    if (work != local)
        CPLRT(free)(work);
    return st;
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

/* node_make, taking the references of the items whose bits `moved` sets instead of retaining them (on success). */
static int value_node_make_m(cplrt_runtime *rt, uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items,
                             uint32_t n, uint64_t moved, cplrt_w *out);

int CPLRT(node_make)(cplrt_runtime *rt, uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items, uint32_t n,
                     cplrt_w *out)
{
    return value_node_make_m(rt, kind, def, variant, items, n, 0u, out);
}

static int value_node_make_m(cplrt_runtime *rt, uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items,
                             uint32_t n, uint64_t moved, cplrt_w *out)
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
        if (i >= 64u || !((moved >> i) & 1u))
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

int cplrt_struct_new_m(cplrt_cx *cx, uint32_t def, const cplrt_w *fields, uint32_t n, uint64_t moved, cplrt_w *out)
{
    return value_node_make_m(cx->rt, CPLRT_STRUCT, def, 0, fields, n, moved, out) == CPLRT_OK ? CPLRT_OK
                                                                                                : CPLRT(oom)(cx);
}

int cplrt_enum_new_m(cplrt_cx *cx, uint32_t def, uint32_t variant, const cplrt_w *payload, uint32_t n, uint64_t moved,
                     cplrt_w *out)
{
    return value_node_make_m(cx->rt, CPLRT_ENUM, def, variant, payload, n, moved, out) == CPLRT_OK ? CPLRT_OK
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
    const cplrt_w *a, *b;
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
    stack[depth++] = (value_eq_frame){ cplrt_items(a), cplrt_items(b), 0, cplrt_obj_of(a)->len };
    int result = 1, st = CPLRT_OK;
    while (depth > 0) {
        value_eq_frame *f = &stack[depth - 1];
        if (f->i == f->n) {
            depth--;
            continue;
        }
        cplrt_w x = f->a[f->i], y = f->b[f->i];
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
        stack[depth++] = (value_eq_frame){ cplrt_items(x), cplrt_items(y), 0, cplrt_obj_of(x)->len };
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
    /* a handle can be used from any thread: what it holds is shared from now on */
    if ((entry->kind == CPLRT_SLOT_VALUE || entry->kind == CPLRT_SLOT_BORROWED) &&
        CPLRT(value_share)(rt, entry->u.value) != CPLRT_OK)
        return CPLRT_NOMEM;
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
