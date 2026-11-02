/* cpl_rt_internal.h: the runtime's own cross-file contract (spec/cabi.md 2.1). Internal like cpl_rt.h, and narrower:
   only the runtime's units include it (every runtime .c file and both mains, behind the runtime prologue of
   spec/cabi.md 2.2: `#define _GNU_SOURCE 1`, then the namespace prologue of Appendix B.1), and emitted code never
   does. It holds the two layouts that several runtime files share, the fault record (cplrt_fault: the fields of
   spec/cabi.md 3.3.1) and the runtime object (cplrt_runtime: the handle table, the allocator, the heap counters, the
   capability registry, and the defaults of 4.3), and the declaration of every function that one runtime file calls in
   another, in three sections by the files that define them:
     R1  cpl_rt_value.c, cpl_rt_int.c, cpl_rt_text.c, cpl_rt_list.c, cpl_rt_json.c, cpl_rt_render.c,
         cpl_rt_platform.h, cpl_rt_platform_linux.c
     R2  cpl_rt_eval.c, cpl_rt_fault.c, cpl_rt_builtins.c, cpl_rt_boundary.c
     R3  cpl_rt_api.c, cpl_rt_cap.c, cpl_rt_driver.c, cpl_rt_exe.c
   The owners of those files append the declarations they add to their own section; what is here already is frozen
   (it changes only through the contracts engineer, like cpl.h and cpl_rt.h). Every external-linkage name is CPLRT()
   (spec/cabi.md 1.6). Conventions of every declaration below:
   - A function that takes a `cplrt_cx *` is called inside an evaluation and returns CPLRT_OK or CPLRT_FAULT, an
     allocation failure having become the memory fault of semantics S76 (CPLRT(oom)). A function that takes only a
     `cplrt_runtime *` returns CPLRT_OK, CPLRT_NOMEM (an allocation failed: the allocator returned NULL or the heap
     budget would be exceeded; nothing changed), or CPLRT_INVALID (an input is not well-formed, as documented), and its
     caller maps a failure to a fault or to a C API status.
   - Every `cplrt_w *out` has CPLRT_SET semantics, as in cpl_rt.h: on success the slot's old content is released and it
     holds one new reference; on failure it is unchanged. Word arguments are borrowed.
   - Nothing recurses on the depth of a value (spec/cabi.md 2.4, 2.8): deep work runs on explicit work stacks. */
#ifndef CPL_RT_INTERNAL_H
#define CPL_RT_INTERNAL_H 1
#include "cpl_rt.h"
#include <pthread.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>

/* ---- internal statuses, beside CPLRT_OK (0) and CPLRT_FAULT (1) of cpl_rt.h ------------------------------------ */
#define CPLRT_NOMEM 2                  /* an allocation failed; nothing changed */
#define CPLRT_INVALID 3                /* an input is not well-formed for the function, as its documentation says */

/* ---- the fault record (spec/cabi.md 3.3.1): the fields of cpl_eval::Fault ---------------------------------------- */
#define CPLRT_FK_CONTRACT_VIOLATION 1u   /* contract_violation, with clause_kind */
#define CPLRT_FK_ASSERTION_FAILED 2u     /* assertion_failed */
#define CPLRT_FK_UNRESOLVED_HOLE 3u      /* unresolved_hole */
#define CPLRT_FK_DIVISION_BY_ZERO 4u     /* division_by_zero */
#define CPLRT_FK_REFINEMENT_VIOLATION 5u /* refinement_violation */
#define CPLRT_FK_RESOURCE_EXHAUSTION 6u  /* resource_exhaustion, with resource */
#define CPLRT_FK_INTERNAL_ERROR 7u       /* internal_error */
#define CPLRT_RES_STACK 1u               /* resource_exhaustion's `resource` */
#define CPLRT_RES_MEMORY 2u
#define CPLRT_RES_TIME 3u
#define CPLRT_RES_STEPS 4u
#define CPLRT_FACET_NONE 0u              /* unset: the diagnostic says body when there is a symbol (Fault::to_diagnostic) */
#define CPLRT_FACET_CONTRACT 1u          /* every contract violation, builtin precondition fault, and claim */
#define CPLRT_FAULT_STATIC 1u            /* flags: neither the record nor its arrays are the allocator's (memory record) */
#define CPLRT_DATUM_STRING 1u            /* a payload value written as a JSON string: str */
#define CPLRT_DATUM_NUMBER 2u            /* a payload value written as a JSON number: num */
#define CPLRT_DATUM_JSON 3u              /* a payload value given as compact JSON text: json, len */
typedef struct cplrt_datum {             /* one payload key of a fault or of an error object (toolchain 1.7.3; 4.9) */
    const char *key;                     /* static: "clause", "input", "actual", "parameter", ... */
    uint32_t form;                       /* CPLRT_DATUM_* */
    uint32_t reserved;
    const char *str;                     /* STRING: NUL-terminated; in a fault record static (the record owns no str) */
    uint64_t num;                        /* NUMBER */
    char *json;                          /* JSON: NUL-terminated after len bytes; in a fault record owned by the record */
    size_t len;
} cplrt_datum;
typedef struct cplrt_btframe {           /* one backtrace entry (3.3.1) */
    uint32_t def;                        /* DefId.0 of the activation; a stub's for a builtin */
    uint32_t site;                       /* CPL_S of the call that entered it; CPLRT_NO_SITE: no span */
} cplrt_btframe;
struct cplrt_fault {                     /* declared in cpl_rt.h; owned by its evaluation context until the evaluation ends */
    uint32_t kind;                       /* CPLRT_FK_*: code and category follow, as FaultKind::code and ::category */
    uint32_t clause_kind;                /* a contract_violation: CPLRT_CK_REQUIRES .. CPLRT_CK_DECREASES; else 0 */
    uint32_t resource;                   /* a resource_exhaustion: CPLRT_RES_*; else 0 */
    uint32_t facet;                      /* CPLRT_FACET_* */
    uint32_t span;                       /* CPL_S of the span; CPLRT_NONE: none */
    uint32_t flags;                      /* CPLRT_FAULT_STATIC */
    uint32_t owner;                      /* in cx->violation: DefId.0 of the declaration the violation is of (the `def` of
                                            a driver's violation answer, 3.8); CPLRT_NONE otherwise */
    uint32_t reserved;
    const char *symbol;                  /* the blamed declaration's stable id; NULL: none (never one for internal_error).
                                            Static: a cplrt_def.stable_id, cplrt_clause.owner_id, cplrt_op.identity, or a
                                            builtin's fallback symbol */
    const char *qualified_name;          /* the symbol's qualified_name (static); NULL with symbol */
    const char *message;                 /* rendered once from its template (2.9) when raised; NUL-terminated; owned by
                                            the record unless CPLRT_FAULT_STATIC */
    size_t message_len;
    cplrt_btframe *frames;               /* the backtrace, innermost first: frames_cap entries, nframes used */
    uint32_t nframes, frames_cap;
    cplrt_datum *data;                   /* the payload keys in the order they were set, one per key; the writers order
                                            them (3.3.2): data_cap entries, ndata used */
    uint32_t ndata, data_cap;
};
#define CPLRT_MEMORY_FRAMES 64u          /* the backtrace frames a memory fault records without allocating (3.3.1) */
typedef struct cplrt_memory_record {     /* the preallocated resource_exhaustion (memory) record of one evaluation */
    cplrt_fault fault;                   /* CPLRT_FAULT_STATIC; data and frames point into this bundle */
    cplrt_datum data[2];                 /* resource "memory", limit (the heap budget in bytes, 0 when none) */
    cplrt_btframe frames[CPLRT_MEMORY_FRAMES];
} cplrt_memory_record;

/* ---- the handle table (spec/cabi.md 4.5) ------------------------------------------------------------------------- */
/* A handle's bits[0] is (uint64_t)generation << 32 | slot, and its bits[1] the address of the issuing runtime's
   identity cell; {{0, 0}} is "no value". */
#define CPLRT_SLOT_FREE 0u               /* on the free list: u.next */
#define CPLRT_SLOT_VALUE 1u              /* a value handle: u.value, one owned reference */
#define CPLRT_SLOT_CAP 2u                /* a capability handle: u.cap, one reference of the capability object */
#define CPLRT_SLOT_CANCEL 3u             /* a cancellation token: u.cancel, one reference of the token */
#define CPLRT_SLOT_RESERVED 4u           /* reserved by cplrt_cap_invoke for a callback's argument (5.7) */
#define CPLRT_SLOT_BORROWED 5u           /* a callback's borrowed argument: u.value, not owned; cannot be released */
#define CPLRT_SLOT_RETIRED 6u            /* its generation would wrap: never reused (4.5) */
#define CPLRT_HANDLE_OK 0                /* the results of a handle check, in the check order of 4.5 */
#define CPLRT_HANDLE_NULL 1              /* {{0, 0}} */
#define CPLRT_HANDLE_FOREIGN 2           /* another runtime's identity: interop_handle_foreign */
#define CPLRT_HANDLE_INVALID 3           /* no such slot, or one of another kind: interop_handle_invalid */
#define CPLRT_HANDLE_RELEASED 4          /* the slot's generation moved on: interop_handle_released */
typedef struct cplrt_capobj cplrt_capobj; /* a capability object (R3, cpl_rt_cap.c); begins with its cplrt_capref */
typedef struct cplrt_token cplrt_token;   /* a cancellation token (R3, cpl_rt_api.c) */
typedef struct cplrt_slot {
    uint32_t kind;                       /* CPLRT_SLOT_* */
    uint32_t generation;                 /* the generation of the handle that names the slot now */
    union {
        cplrt_w value;                   /* VALUE, BORROWED */
        cplrt_capobj *cap;               /* CAP */
        cplrt_token *cancel;             /* CANCEL */
        uint32_t next;                   /* FREE: the next free slot, CPLRT_NONE at the end of the list */
    } u;
} cplrt_slot;

/* ---- the runtime object (spec/cabi.md 4.3) ------------------------------------------------------------------------ */
struct cplrt_runtime {                   /* declared in cpl_rt.h; what <prefix>_runtime_open creates */
    cpl_runtime pub;                     /* first: the public { const cpl_api *api } (4.1) */
    const cplrt_program *program;        /* CPLPKG(program): the tables and the immortal statics of the package */
    void *identity;                      /* the identity cell: 16 bytes of the C library's malloc, never freed (4.5) */
    cpl_allocator allocator;             /* the copy of *options->allocator taken at open (4.7); else malloc and free */
    uint64_t max_heap_bytes;             /* the heap budget of 2.4; 0: none */
    _Atomic uint64_t live_bytes;         /* bytes allocated through the allocator after the runtime object and not yet
                                            freed: cpl_stats.live_bytes and the quantity the budget bounds */
    _Atomic uint64_t peak_bytes;
    _Atomic uint64_t live_objects;       /* live heap objects, immortals excluded */
    _Atomic uint32_t active_calls;       /* evaluations in flight: interop_runtime_busy */
    _Atomic uint32_t poisoned;           /* 1 after CPL_E_INTERNAL (4.6) */
    uint32_t max_call_depth;             /* cpl_runtime_options.max_call_depth: the default limit of every evaluation */
    uint32_t flags;                      /* cpl_runtime_options.flags (none in 0.1) */
    pthread_mutex_t lock;                /* the handle-table mutex (4.4): guards the members below */
    cplrt_slot *slots;                   /* the handle table: slots_cap entries, nslots in use */
    uint32_t nslots, slots_cap;
    uint32_t free_head, free_tail;       /* the free slots, reused first in, first out (4.5); CPLRT_NONE: none */
    uint64_t live_handles;               /* value handles and cancellation tokens (cpl_stats.live_handles) */
    uint64_t live_caps;                  /* capability handles (cpl_stats.live_caps) */
    uint64_t live_buffers;               /* cpl_buf buffers not yet released (cpl_stats.live_buffers) */
    uint32_t next_cap_handle;            /* the $handle number of the next capability object: creation order from 0 */
    uint32_t reserved;
    cplrt_capobj *caps_first;            /* the capability registry: every live capability object, in creation order, */
    cplrt_capobj *caps_last;             /* which CPL_CLOSE_FORCE walks in reverse (4.3) */
    void *buffers;                       /* the live cpl_buf blocks of the API, linked (R3, cpl_rt_api.c), which
                                            CPL_CLOSE_FORCE frees (4.3); guarded by lock */
    char *worker_python;                 /* Milestone 3 (spec/python.md 6.5): the Python workers' interpreter, NULL:
                                            CPL_PYTHON, else python3; guarded by lock */
    char **worker_env;                   /* "KEY=VALUE" entries set in every Python worker's environment */
    uint32_t nworker_env;
};

/* ---- a growable text buffer, the target of every writer --------------------------------------------------------- */
typedef struct cplrt_sbuf {
    cplrt_runtime *rt;                   /* whose allocator holds the text */
    char *ptr;                           /* NUL-terminated after len bytes once anything was written; NULL before */
    size_t len, cap;
    int status;                          /* CPLRT_OK; CPLRT_NOMEM or CPLRT_INVALID once a write failed: sticky, and
                                            every later write does nothing */
} cplrt_sbuf;
static inline void cplrt_sbuf_init(cplrt_sbuf *b, cplrt_runtime *rt)
{
    b->rt = rt;
    b->ptr = NULL;
    b->len = 0;
    b->cap = 0;
    b->status = CPLRT_OK;
}

/* ---- the protocol reader's JSON tree (spec/cabi.md 3.8) ---------------------------------------------------------- */
#define CPLRT_JSON_NULL 1u
#define CPLRT_JSON_FALSE 2u
#define CPLRT_JSON_TRUE 3u
#define CPLRT_JSON_NUMBER 4u             /* str, len: the number's text as written */
#define CPLRT_JSON_STRING 5u             /* str, len: the decoded UTF-8, NUL-terminated (U+0000 may occur: len counts) */
#define CPLRT_JSON_ARRAY 6u              /* items: n values */
#define CPLRT_JSON_OBJECT 7u             /* items: 2n entries, each key (a STRING) followed by its value */
#define CPLRT_JSON_MAX_DEPTH 64u         /* a protocol line nests no deeper; deep values travel as node lists */
typedef struct cplrt_jv {
    uint32_t type;                       /* CPLRT_JSON_* */
    uint32_t n;
    const char *str;
    size_t len;
    struct cplrt_jv *items;
} cplrt_jv;

/* ---- R1: values, Int, Text, Bytes, List, JSON, render, platform ---- */

/* Allocation (2.4, 4.7). Every block the runtime allocates after the runtime object goes through CPLRT(alloc): taken
   from rt->allocator aligned to alignof(max_align_t), charged to rt->live_bytes (and peak_bytes) and to the heap
   budget, and preceded by a hidden header that records its runtime and size, so that CPLRT(free) and
   CPLRT(release_obj), which receive no runtime, return it through the same allocator. NULL (nothing charged) when the
   allocator returns NULL or max_heap_bytes would be exceeded; size is never 0. A block must be freed before its
   runtime is: error objects, which outlive their runtime (4.3), are allocated by R3 so that they can be. */
void *CPLRT(alloc)(cplrt_runtime *rt, size_t size);
void CPLRT(free)(void *p);                                  /* NULL: nothing */

/* Heap objects (2.3). A new object of `size` bytes, its 16-byte header included, of `kind`, with `len` children or
   limbs, flags, variant, and def 0, and a count of 1; it counts in rt->live_objects until CPLRT(release_obj) frees it.
   NULL as CPLRT(alloc). */
cplrt_obj *CPLRT(obj_new)(cplrt_runtime *rt, uint8_t kind, uint32_t len, size_t size);

/* Value builders, for the C API outside an evaluation and for the runtime's own files (the evaluation forms of
   cpl_rt.h, cplrt_list_new(cx, ...) and its family, are these plus CPLRT(oom)). */
/* A List (CPLRT_LIST), struct, enum, or wrapper of the n borrowed words, each retained, carrying def (and variant for
   an enum): [] is the runtime's immortal empty list, an enum value with an empty payload the program's immortal static
   of its variant (cplrt_def.statics), and a variant index above 65535 the wide form (CPLRT_F_WIDE, 2.3). CPLRT_OK or
   CPLRT_NOMEM (also for n beyond the 2^32 - 1 children a header can count). */
int CPLRT(node_make)(cplrt_runtime *rt, uint8_t kind, uint32_t def, uint32_t variant, const cplrt_w *items, uint32_t n,
                     cplrt_w *out);
/* A Text of len bytes of well-formed UTF-8 (the caller validated it, CPLRT(utf8_valid)), its scalars counted, a NUL
   after the last byte; "" is the runtime's immortal empty text. CPLRT_OK or CPLRT_NOMEM. */
int CPLRT(text_make)(cplrt_runtime *rt, const char *utf8, size_t len, cplrt_w *out);
/* A Bytes of a copy of len bytes; the empty one is the runtime's immortal. CPLRT_OK or CPLRT_NOMEM. */
int CPLRT(bytes_make)(cplrt_runtime *rt, const uint8_t *data, size_t len, cplrt_w *out);
/* 1 when [s, s + len) is well-formed UTF-8 by Unicode Table 3-7 (Rust's str::from_utf8: no surrogates, no overlong
   forms, nothing above U+10FFFF), then *scalars its scalar count; 0 otherwise, then *valid_up_to the offset of the
   first invalid sequence (Utf8Error::valid_up_to). Either output may be NULL. */
int CPLRT(utf8_valid)(const char *s, size_t len, size_t *valid_up_to, size_t *scalars);

/* Int (2.5): conversions with machine integers and words, the one decimal reader, the canonical decimal writer. */
int CPLRT(int_from_i64)(cplrt_runtime *rt, int64_t v, cplrt_w *out);                 /* CPLRT_OK or CPLRT_NOMEM */
int CPLRT(int_from_u64)(cplrt_runtime *rt, uint64_t v, cplrt_w *out);
/* The Int of the n little-endian 64-bit words of a magnitude with the sign `negative` (0 or 1); a negative zero is 0 */
int CPLRT(int_from_words)(cplrt_runtime *rt, int negative, const uint64_t *words, size_t n, cplrt_w *out);
int CPLRT(int_to_i64)(cplrt_w v, int64_t *out);            /* 1 and *out when the Int v fits int64_t, else 0 */
int CPLRT(int_to_u64)(cplrt_w v, uint64_t *out);           /* 1 and *out when the Int v fits uint64_t, else 0 */
/* The number of 64-bit words of |v| (0 for 0), and *negative; the words, little-endian, only when cap is at least it */
size_t CPLRT(int_to_words)(cplrt_w v, int *negative, uint64_t *words, size_t cap);
/* stdlib.md S25, for std.text.parse_int and cpl_int_from_decimal: ("+" | "-")? [0-9]+, leading zeros accepted, -0 is
   0, no length limit; CPLRT_INVALID for any other text; CPLRT_NOMEM */
int CPLRT(int_parse)(cplrt_runtime *rt, const char *s, size_t len, cplrt_w *out);
void CPLRT(int_write)(cplrt_sbuf *b, cplrt_w v);           /* the canonical decimal: -, digits, no leading zero, 0 */

/* Structural equality outside an evaluation (2.8): *eq 1 when a == b, else 0; CPLRT_OK or CPLRT_NOMEM (its work
   stack). cplrt_equal of cpl_rt.h is this with the memory fault. */
int CPLRT(value_equal)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, int *eq);

/* The handle table (4.5), under rt->lock. CPLRT(handle_put) stores `entry` (kind VALUE, CAP, or CANCEL; its reference
   passes to the table) in a free slot, first in, first out, and writes the handle: CPLRT_OK, or CPLRT_NOMEM when the
   table cannot grow (the reference then stays the caller's). CPLRT(handle_get) checks bits in the order of 4.5 against
   the slot kind `kind` and returns a CPLRT_HANDLE_* result; on CPLRT_HANDLE_OK *entry is a copy of the slot whose
   reference was retained for the caller under the lock (CPLRT(slot_unref) releases it). CPLRT(handle_release) checks
   the same way, frees the slot (its generation advances, or it is retired), and drops the table's reference after the
   lock is released. A BORROWED slot is CPLRT_HANDLE_INVALID to handle_release. */
int CPLRT(handle_put)(cplrt_runtime *rt, const cplrt_slot *entry, uint64_t bits[2]);
int CPLRT(handle_get)(cplrt_runtime *rt, const uint64_t bits[2], uint32_t kind, cplrt_slot *entry);
int CPLRT(handle_release)(cplrt_runtime *rt, const uint64_t bits[2], uint32_t kind);
void CPLRT(slot_unref)(const cplrt_slot *entry);          /* the reference a handle_get retained */
/* The slots of a callback's borrowed arguments (5.7): reserve n slots (CPLRT_OK or CPLRT_NOMEM, nothing reserved
   then), fill reserved slot `slot` with the borrowed word w and return its handle (cannot fail), and free the n slots
   again (their generations advance) when the callback has returned. */
int CPLRT(handle_reserve)(cplrt_runtime *rt, uint32_t n, uint32_t *slots);
cpl_value CPLRT(handle_borrow)(cplrt_runtime *rt, uint32_t slot, cplrt_w w);
void CPLRT(handle_unreserve)(cplrt_runtime *rt, const uint32_t *slots, uint32_t n);

/* Text buffers. A failed growth sets b->status to CPLRT_NOMEM; the buffer is freed with sbuf_free. */
void CPLRT(sbuf_put)(cplrt_sbuf *b, const char *s, size_t n);
void CPLRT(sbuf_puts)(cplrt_sbuf *b, const char *s);       /* NUL-terminated s */
void CPLRT(sbuf_putc)(cplrt_sbuf *b, char c);
void CPLRT(sbuf_u64)(cplrt_sbuf *b, uint64_t v);           /* decimal */
void CPLRT(sbuf_free)(cplrt_sbuf *b);                      /* frees ptr and empties the buffer; status is kept */

/* The C renderings (3.3.2), byte for byte the Rust ones, each iterative. */
/* a JSON string with serde_json's compact escapes: \" \\ \b \t \n \f \r, \u00xx (lowercase) below U+0020, else UTF-8 */
void CPLRT(write_json_string)(cplrt_sbuf *b, const char *s, size_t len);
/* value JSON: serde_json::to_string(value_to_json_untyped), object keys in byte order (semantics S40) */
void CPLRT(write_value_json)(cplrt_sbuf *b, const cplrt_program *p, cplrt_w v);
/* value text: value_text_untyped (semantics S40, with quote_text's escapes) */
void CPLRT(write_value_text)(cplrt_sbuf *b, const cplrt_program *p, cplrt_w v);
/* cpl_diag::quote_text: "..." with \\ \" \n \r \t and \u{h} (minimal lowercase hex) for the other controls and U+007F */
void CPLRT(write_quote_text)(cplrt_sbuf *b, const char *s, size_t len);
/* The {name} renderer (2.9): cpl_rtspec::messages::render of the template tmpl (a CPLRT_MSG_* of cpl_rt_gen.h) with the
   n arguments names[i] = values[i]; a placeholder without an argument sets b->status to CPLRT_INVALID */
void CPLRT(write_template)(cplrt_sbuf *b, const char *tmpl, const char *const *names, const char *const *values,
                           uint32_t n);
/* The S49 diagnostic of a fault record: serde_json::to_string(Fault::to_diagnostic(p).to_json(sm)) (3.3.2) */
void CPLRT(write_s49)(cplrt_sbuf *b, const cplrt_program *p, const cplrt_fault *f);
/* The payload of a fault record as one JSON object, keys in byte order: the `data` of a driver's fault record (3.8) */
void CPLRT(write_payload)(cplrt_sbuf *b, const cplrt_fault *f);

/* The strict value-JSON reader (4.8; toolchain 7.3 without its nesting limit, iterative): the value of the type-table
   entry `type` (a CPL_X) that the text decodes, a refined position decoded at its root base and its predicates not
   evaluated. CPLRT_INVALID for a JSON or decoding failure, a duplicate key included, with the RFC 6901 position written
   to *pointer and the reason to *detail (interop_json_invalid); CPLRT_NOMEM. */
int CPLRT(read_value_json)(cplrt_runtime *rt, uint32_t type, const char *json, size_t len, cplrt_w *out,
                           cplrt_sbuf *pointer, cplrt_sbuf *detail);
/* The protocol reader of the driver (3.8): one JSON text as a tree of cplrt_jv (strings decoded, numbers as their
   text); CPLRT_INVALID with *error_offset for malformed JSON, a duplicate key, or nesting beyond CPLRT_JSON_MAX_DEPTH;
   CPLRT_NOMEM. CPLRT(json_member) is the value of key in an OBJECT, NULL when it has none or v is no OBJECT. */
int CPLRT(json_parse)(cplrt_runtime *rt, const char *text, size_t len, cplrt_jv **out, size_t *error_offset);
void CPLRT(json_free)(cplrt_jv *v);                        /* NULL: nothing */
const cplrt_jv *CPLRT(json_member)(const cplrt_jv *v, const char *key);

/* The platform (cpl_rt_platform_linux.c: threads, the monotonic clock, per-thread cached stack bounds). */
uint64_t CPLRT(clock_ns)(void);                            /* the monotonic clock, nanoseconds (3.6) */
/* The calling thread's stack [*lo, *hi), queried once per thread and cached in a _Thread_local record (3.4):
   CPLRT_OK, or CPLRT_NOMEM when the query fails (pthread_getattr_np with ENOMEM), which is not cached. */
int CPLRT(stack_bounds)(uintptr_t *lo, uintptr_t *hi);
/* The evaluation-thread launcher of the driver and the native executable (3.4, 3.8, 8): run fn(arg) on a new thread
   with `stack` bytes of stack reserved (CPLRT_LIMIT_EVAL_STACK_SIZE, 1 GiB) and wait for it to end: CPLRT_OK, or
   CPLRT_NOMEM when the thread cannot be created. */
int CPLRT(run_on_thread)(size_t stack, void (*fn)(void *arg), void *arg);

/* R1 additions (the R1 owner's), by the conventions above. */
/* The runtime's immortal statics (2.3) as words: "", [], and the empty Bytes (cpl_rt_text.c, cpl_rt_list.c). */
cplrt_w CPLRT(empty_text)(void);
cplrt_w CPLRT(empty_bytes)(void);
cplrt_w CPLRT(empty_list)(void);
/* The program's immortal static of the payload-less variant `variant` of the enum `def` (cplrt_def.statics), or 0 when
   the tables have none: Unit, None, Unavailable, ... (cpl_rt_value.c). */
cplrt_w CPLRT(enum_static)(const cplrt_program *p, uint32_t def, uint32_t variant);
/* Fresh bodies for the runtime's own builders, each an object with a count of 1, NULL as CPLRT(alloc): a Text of
   `bytes` (> 0) bytes holding `scalars` scalars, its NUL written and its bytes the caller's to write; a Bytes of `len`
   (> 0) bytes, the caller's to write; a List of n (> 0) items, all 0, in which the caller stores owned words (a
   partly filled list may be released as it is, 0 holding nothing). */
cplrt_text *CPLRT(text_alloc)(cplrt_runtime *rt, size_t bytes, size_t scalars);
cplrt_bytes *CPLRT(bytes_alloc)(cplrt_runtime *rt, size_t len);
cplrt_node *CPLRT(list_alloc)(cplrt_runtime *rt, uint32_t n);
/* `++` of two Texts, two Bytes, or two Lists (Value::concat): an operand when the other is empty. CPLRT_OK or
   CPLRT_NOMEM. cplrt_concat of cpl_rt.h dispatches to them. */
int CPLRT(text_concat)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, cplrt_w *out);
int CPLRT(bytes_concat)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, cplrt_w *out);
int CPLRT(list_concat)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, cplrt_w *out);
/* The canonical decimal of the Int v as a Text: int_to_text and text_from_int (2.7). CPLRT_OK or CPLRT_NOMEM. */
int CPLRT(int_text)(cplrt_runtime *rt, cplrt_w v, cplrt_w *out);
/* Err(ConstraintError { type_name, message }), the fields placed as builtins::constraint_error places them (stdlib.md
   S44): the value of a failed parse_int, bytes.to_text, and check<R> (2.7). CPLRT_OK or CPLRT_NOMEM. */
int CPLRT(constraint_err)(cplrt_runtime *rt, const char *type_name, size_t type_len, const char *message,
                          size_t message_len, cplrt_w *out);
/* Value::kind_name for internal-error details: "Int", "Bool", "Text", "Bytes", "List", "struct", "enum", "wrapper". */
const char *CPLRT(kind_name)(cplrt_w v);
/* One step of an RFC 6901 pointer: "/" and the segment with "~" written "~0" and "/" written "~1". */
void CPLRT(sbuf_pointer_segment)(cplrt_sbuf *b, const char *s, size_t len);
/* The bytes as lowercase hex, two digits each (value JSON and value text of Bytes). */
void CPLRT(sbuf_hex)(cplrt_sbuf *b, const uint8_t *data, size_t len);

/* ---- R2: eval, fault, builtins, boundary ---- */

/* Evaluation contexts (4.3; the evaluator's begin_evaluation). CPLRT(cx_begin) starts a top-level evaluation in *cx on
   the calling thread: rt and its program; no frame and no call; max_call_depth (UINT32_MAX: rt->max_call_depth);
   check_contracts; attribution and cancellation cleared; the stack floor of 3.4 (from the thread's cached bounds, or
   from the entry stack pointer and stack_bytes when that is not 0, and never below the floor of an evaluation of the
   same runtime copy already active on this thread); the deadline of budget_ms from now (0: no budget; saturating);
   the cancellation flag cancel (NULL: none); no fault and no violation; and *memory, which the caller keeps alive
   until CPLRT(cx_end), initialized as the evaluation's memory record with limit rt->max_heap_bytes. CPLRT_OK, or
   CPLRT_NOMEM when the thread's stack bounds cannot be queried (the export then returns CPL_E_NOMEM, 3.4).
   CPLRT(cx_end) frees cx->fault and cx->violation (never the memory record) and ends the evaluation. */
int CPLRT(cx_begin)(cplrt_cx *cx, cplrt_runtime *rt, cplrt_memory_record *memory, uint32_t max_call_depth,
                    int check_contracts, uint64_t budget_ms, uint64_t stack_bytes, const _Atomic uint32_t *cancel);
void CPLRT(cx_end)(cplrt_cx *cx);

/* Raising (3.3.1). CPLRT(fault_raise) makes a new record cx->fault (which is NULL then) of kind, clause_kind, and
   resource, with the message (copied), the span site (CPLRT_NONE: none), and the static symbol and qualified_name
   (NULL: none), and returns it; it returns NULL after an allocation failure, the memory record then being the fault.
   The fault_set_* functions set payload key `key` of f, replacing an earlier one, to a static string, a number, or a
   copy of compact JSON text; CPLRT(fault_push_frame) appends the next outer activation to the backtrace. Each returns
   CPLRT_OK, or CPLRT_FAULT after an allocation failure, f then being freed and the memory record the fault. */
cplrt_fault *CPLRT(fault_raise)(cplrt_cx *cx, uint32_t kind, uint32_t clause_kind, uint32_t resource,
                                const char *message, size_t len, uint32_t span, const char *symbol,
                                const char *qualified_name);
int CPLRT(fault_set_str)(cplrt_cx *cx, cplrt_fault *f, const char *key, const char *str);
int CPLRT(fault_set_num)(cplrt_cx *cx, cplrt_fault *f, const char *key, uint64_t num);
int CPLRT(fault_set_json)(cplrt_cx *cx, cplrt_fault *f, const char *key, const char *json, size_t len);
int CPLRT(fault_push_frame)(cplrt_cx *cx, cplrt_fault *f, uint32_t def, uint32_t site);
void CPLRT(fault_free)(cplrt_fault *f);                    /* NULL and a CPLRT_FAULT_STATIC record: nothing */
/* A deep copy of f (the violation record of 3.3.1, the answers of the driver): CPLRT_OK or CPLRT_NOMEM */
int CPLRT(task_tick)(cplrt_cx *, uint32_t);
int CPLRT(fault_copy)(cplrt_runtime *rt, const cplrt_fault *f, cplrt_fault **out);
/* The memory fault of semantics S76 (3.3.1): drops a record being built, fills the memory record without allocating
   (symbol the innermost frame's declaration, no span, the frames it can hold), makes it cx->fault, and returns
   CPLRT_FAULT. Every allocation failure inside an evaluation ends here. */
int CPLRT(oom)(cplrt_cx *cx);
/* division_by_zero at the operator's site with input {"a", "b"}, blamed on the innermost frame's declaration (2.5) */
int CPLRT(raise_division_by_zero)(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b);
/* The precondition fault of requires clause `clause` (0-based) of builtin `id` called through the stub `stub` (a
   DefId.0, or CPLRT_NONE for a program without the stub) with the n arguments (2.7; stdlib.md 10.4): symbol, clause
   text, clause span, and input names from the stub's facts in the program tables, else from CPLRT_BUILTINS; caller the
   innermost frame's declaration (absent at the top level). Always CPLRT_FAULT. */
int CPLRT(raise_builtin_requires)(cplrt_cx *cx, uint32_t stub, const char *id, uint32_t clause, const cplrt_w *args,
                                  uint32_t n);
/* internal_error `kind` at site with its detail rendered from the template tmpl and n arguments (2.9; the
   cplrt_internal of cpl_rt.h takes the detail already rendered). Always CPLRT_FAULT. */
int CPLRT(raise_internal)(cplrt_cx *cx, uint32_t site, const char *kind, const char *tmpl, const char *const *names,
                          const char *const *values, uint32_t n);
/* What a builtin does as a fault leaves it (1.2, call_builtin): with st CPLRT_FAULT, the span of the call's site when
   the fault has none and the frame {stub, site}, or with CPLRT_NO_SITE no span and the frame {stub}; returns st. */
int CPLRT(builtin_leave)(cplrt_cx *cx, uint32_t site, uint32_t stub, int st);

/* Boundary checks (5.5 steps 2 and 3, 6.4; semantics S74, S77, S81), for export arguments and capability results. */
typedef struct cplrt_shape_failure {     /* the first position whose value is not of its declared type */
    cplrt_sbuf pointer;                  /* its RFC 6901 position in the value */
    uint32_t type;                       /* the CPL_X expected there: .text is `expected`, .display the S77/S80 text */
    uint32_t reserved;
    const char *found;                   /* static: the value's kind word (int, bool, text, bytes, list, struct, enum,
                                            wrapper) or, for a struct, enum, or wrapper, its declaration's qualified name */
} cplrt_shape_failure;
/* The deep shape check of w against the type-table entry `type`, in the order value_from_json decodes (struct fields
   in declaration order, list elements by index, payloads by position): 1 when w is of the type; else 0 with *failure
   written (its pointer initialized by the callee with rt). CPLRT_NOMEM is reported through failure->pointer.status. */
int CPLRT(shape_check)(cplrt_runtime *rt, uint32_t type, cplrt_w w, cplrt_shape_failure *failure);
typedef struct cplrt_refined_failure {   /* the first refined position whose chain does not hold */
    uint32_t refinement;                 /* CPL_R of the first false predicate's refinement, root first; CPLRT_NONE: all hold */
    uint32_t declared;                   /* CPL_R of the refinement named at the position: its declared_name is the
                                            `expected_type` */
    cplrt_sbuf pointer;                  /* its RFC 6901 position in the value */
    cplrt_w actual;                      /* the value at the position, borrowed from w */
} cplrt_refined_failure;
/* Every refined position of w at `type` in S74 order (the positions of a refinement's root base strip(R) first, then
   its whole chain root first), each predicate an activation of its cpl_p<def> entered at `site` (CPLRT_NO_SITE at an
   export's entry, the operation call's site for a capability result): CPLRT_OK with *failure written (refinement
   CPLRT_NONE when every predicate holds), or CPLRT_FAULT for a predicate's fault (reported per S34) or memory. */
int CPLRT(refined_check)(cplrt_cx *cx, uint32_t site, uint32_t type, cplrt_w w, cplrt_refined_failure *failure);

/* run_function's check_arguments for argument `index` of function `def` at type `type` (5.5 step 3): CPLRT_OK when
   every refined position holds, else CPLRT_FAULT with the refinement_violation (or a predicate's fault) raised. */
int CPLRT(argument_refined)(cplrt_cx *cx, uint32_t def, uint32_t index, uint32_t type, cplrt_w v);

/* ---- coverage (1.4, 3.8): the builtin calls by id and the claims by the kind of their claim site, counted only
   with -DCPLRT_COVERAGE=1 (cpl_rt_eval.c) ---- */
#define CPLRT_COV_CLAIMS 10u
extern const char *const CPLRT(cov_claim_names)[CPLRT_COV_CLAIMS];  /* requires ... range, then already_claimed */
#define CPLRT_COV_ALREADY_CLAIMED 9u
void CPLRT(cov_claim)(uint32_t index);
/* the counted builtins: their number, and the id and count of entry i */
uint32_t CPLRT(cov_nbi)(void);
const char *CPLRT(cov_bi_at)(uint32_t i, uint64_t *count);
uint64_t CPLRT(cov_claims_at)(uint32_t i);

/* ---- R3: api, capabilities, driver, executable ---- */

/* Error objects (4.6). Each writes a new error object to *error when error is not NULL and returns its status:
   CPLRT(error_interop) one of the interop code CPLRT_INTEROP[code] (cpl_rt_gen.h; its status) with the n payload keys
   of data (4.9; a STRING str need only outlive the call) and a message of the runtime's wording; CPLRT(error_fault) the
   CPL_FAULT object of a fault record (its S49 rendering, CPLRT(write_s49)); CPLRT(error_internal) the CPL_E_INTERNAL
   object, internal_error kind runtime_invariant with `detail`, and it poisons rt. When the object cannot be allocated
   the status is CPL_E_NOMEM with the static interop_out_of_memory object, except for the memory fault, which stays
   CPL_FAULT with the static resource_exhaustion object (4.6, 4.7). rt NULL (no runtime yet: <prefix>_runtime_open)
   allocates with the C library's malloc. */
cpl_status CPLRT(error_interop)(cplrt_runtime *rt, cpl_error **error, uint32_t code, const cplrt_datum *data,
                                uint32_t n);
/* CPLRT(error_interop) with one note (its message `note`, note_len bytes of strict UTF-8; NULL: none) in `notes`. */
cpl_status CPLRT(error_interop_note)(cplrt_runtime *rt, cpl_error **error, uint32_t code, const cplrt_datum *data,
                                     uint32_t n, const char *note, size_t note_len);
cpl_status CPLRT(error_fault)(cplrt_runtime *rt, cpl_error **error, const cplrt_fault *f);
cpl_status CPLRT(error_internal)(cplrt_runtime *rt, cpl_error **error, const char *detail);

/* Capability objects and cancellation tokens are counted like values (2.4, 5.7). The last CPLRT(cap_unref) runs the
   object's drop or fini, with no runtime lock held, then frees it; the last CPLRT(token_unref) frees the token.
   CPLRT(token_flag) is the flag an evaluation polls (cplrt_cx.cancel). */
void CPLRT(cap_retain)(cplrt_capobj *cap);
void CPLRT(cap_unref)(cplrt_capobj *cap);
const cplrt_capref *CPLRT(cap_ref)(const cplrt_capobj *cap);   /* what emitted code receives for a using parameter */
/* A grant's adapter init for its capability object, whose handle the caller allocated first (cpl_rt_cap.c). */
cpl_status CPLRT(grant_init)(cplrt_runtime *rt, const cplrt_grant *g, cplrt_capobj *c, cpl_error **error);
void CPLRT(token_retain)(cplrt_token *token);
void CPLRT(token_unref)(cplrt_token *token);
const _Atomic uint32_t *CPLRT(token_flag)(const cplrt_token *token);

/* R3 additions (the R3 owner's), by the conventions above. */
/* The C API's function table (cpl_rt_api.c), which every runtime, call, and error object carries (4.1). */
extern const cpl_api CPLRT(api_table);
/* Capability objects (cpl_rt_cap.c). A new object with one reference, registered last, numbered by creation order;
   `label` (the binding name of semantics 6.9) is copied and `ops` (a CPLRT(alloc) block or NULL) passes to it; NULL
   after an allocation failure (ops then stays the caller's). */
cplrt_capobj *CPLRT(cap_new_obj)(cplrt_runtime *rt, uint32_t capability, const cplrt_capimpl *impl, void *user,
                                 const char *label, void *ops);
const char *CPLRT(cap_label)(const cplrt_capref *cap);
void CPLRT(cap_run_drop)(cplrt_capobj *cap);               /* the drop or fini, once */
void CPLRT(cap_run_drop_disarm)(cplrt_capobj *cap);        /* the drop counts as done without running */
void CPLRT(caps_drop_all)(cplrt_runtime *rt);             /* every outstanding drop, in reverse creation order */
cplrt_capobj *CPLRT(cap_generic_new)(cplrt_runtime *rt, uint32_t capability, const cpl_cap_ops *ops, void *user,
                                     const char *label);
/* The callback context (5.7): its runtime, operation, cancellation, and the failure it reports. */
cplrt_runtime *CPLRT(call_rt)(cpl_call *call);
uint32_t CPLRT(call_op)(cpl_call *call);
int32_t CPLRT(call_is_cancelled)(cpl_call *call);
cpl_status CPLRT(call_detail)(cpl_call *call, const char *key, size_t key_len, const char *value, size_t value_len);
cpl_status CPLRT(call_fail)(cpl_call *call, int32_t kind, const char *message, size_t len);
/* The binding layer alone, for the driver's cap_call request (3.8, 7.2): CPLRT_OK with *out; CPLRT_INVALID with
   *foreign the JSON object of a reported failure; CPLRT_FAULT with the binding layer's fault in cx->fault. */
int CPLRT(cap_call_raw)(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, uint32_t op, const cplrt_w *args,
                        uint32_t n, cplrt_w *out, cplrt_sbuf *foreign);
/* A scripted host's call that no response matches: raises the binding_protocol fault of semantics S80 at the call and
   returns CPL_E_INTERNAL for the binding layer to return. */
cpl_status CPLRT(call_no_response)(cpl_call *call, const char *arguments);
/* ---- cpl_rt_wire.c: values as node lists (3.8; spec/python.md 6.2) -------------------------------------------- */
/* {"nodes": [...]} of root (borrowed) into b, every distinct word once, post-order, the root last; with renderings,
   then "json" and "text" (the C writer's value JSON and value text, built in buffers of `scratch`). A failed
   allocation sets b->status to CPLRT_NOMEM. */
void CPLRT(nodes_write)(cplrt_sbuf *b, cplrt_runtime *scratch, const cplrt_program *p, cplrt_w root, int renderings);
/* The value of a node list {"nodes": [...]} built in rt: CPLRT_OK and *out (owned), CPLRT_INVALID with a static
   *detail, or CPLRT_NOMEM. nodes_read_all reads an array of them into *out (malloc, owned words). */
int CPLRT(nodes_read)(cplrt_runtime *rt, const cplrt_jv *v, cplrt_w *out, const char **detail);
int CPLRT(nodes_read_all)(cplrt_runtime *rt, const cplrt_jv *arr, cplrt_w **out, uint32_t *n, const char **detail);
void CPLRT(words_release)(cplrt_w *words, uint32_t n);          /* releases each word, then frees the array */
int CPLRT(jv_u64)(const cplrt_jv *v, uint64_t *out);            /* 1 for a nonnegative integer that fits */
int CPLRT(jv_u32)(const cplrt_jv *v, uint32_t *out);

/* ---- cpl_rt_worker.c: the worker transport (spec/python.md 6) ------------------------------------------------- */
/* The interpreter and the environment entries ("KEY=VALUE") of the Python workers rt starts from now on (python NULL:
   the default); CPLRT_OK or CPLRT_NOMEM. workers_fini frees them when rt closes. */
int CPLRT(workers_configure)(cplrt_runtime *rt, const char *python, const char *const *env, uint32_t n);
void CPLRT(workers_fini)(cplrt_runtime *rt);
/* The grant of a worker binding (g->worker != 0): starts the worker and binds; c's user is the worker. */
cpl_status CPLRT(worker_grant)(cplrt_runtime *rt, const cplrt_grant *g, cplrt_capobj *c, cpl_error **error);
/* The grant in process whatever g->worker says (the C adapter host's own grant). */
cpl_status CPLRT(grant_init_local)(cplrt_runtime *rt, const cplrt_grant *g, cplrt_capobj *c, cpl_error **error);
/* The capability implementation a grant's object uses: the worker's for a worker grant. */
const cplrt_capimpl *CPLRT(grant_impl)(const cplrt_grant *g);

/* the worker client's failures (spec/python.md 6.4): a host that ended (Transport, or binding_host_failed), and an
   answer that is not cpl-worker protocol 1 (binding_protocol) */
cpl_status CPLRT(call_host_ended)(cpl_call *call, const char *message, size_t len);
cpl_status CPLRT(call_worker_protocol)(cpl_call *call, const char *reason);
cplrt_cx *CPLRT(call_cx)(cpl_call *call);
/* Cancellation tokens (cpl_rt_api.c): a new token with one reference, not requested; NULL after an allocation failure. */
cplrt_token *CPLRT(token_new)(cplrt_runtime *rt);

#endif
