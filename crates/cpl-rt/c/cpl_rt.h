/* cpl_rt.h: the internal contract between the CPL C runtime and the C that cpl-emit-c emits (spec/cabi.md
   Appendix B.1). Internal: never installed for hosts and never an ABI; it changes with the runtime digest.
   C11 only. Every external-linkage name is namespaced by CPLRT() or CPLPKG() (spec/cabi.md 1.6). */
#ifndef CPL_RT_H
#define CPL_RT_H 1
#ifdef __cplusplus
#error "cpl_rt.h is C11: the runtime and the emitted code are compiled as C (spec/cabi.md 1.5)"
#endif
#if !defined(CPLRT_NS) || !defined(CPLRT) || !defined(CPLPKG)
#error "begin the unit with the namespace prologue of spec/cabi.md Appendix B.1 before including cpl_rt.h"
#endif
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include "cpl.h"

/* ---- names (1.6) -------------------------------------------------------------------------------------------- */
/* The prologue defines CPLRT(name) as cplrt_<prefix>__##name and CPLPKG(name) as cplpkg_<prefix>__##name, the prefix
   inside one token so that no macro named like it (linux, unix) can replace it; CPLRT_NS only marks the prologue. */
/* On the definition of every function the version script lists (<prefix>_api.c): the base flags make every
   other symbol hidden, and a version script cannot export a hidden symbol. */
#if defined(__GNUC__) || defined(__clang__)
#define CPLRT_EXPORT __attribute__((visibility("default")))
#else
#define CPLRT_EXPORT
#endif

/* ---- statuses and sentinels (1.2) --------------------------------------------------------------------------- */
#define CPLRT_OK 0                     /* the internal status of every emitted function and helper */
#define CPLRT_FAULT 1                  /* a fault is held in cx->fault (cancellation included, 3.6) */
#define CPLRT_NO_SITE UINT32_MAX       /* the site of a top-level activation */
#define CPLRT_NONE UINT32_MAX          /* "no entry" in a table reference */

/* ---- the word (2.3) ----------------------------------------------------------------------------------------- */
typedef uint64_t cplrt_w;
#define CPLRT_EMPTY ((cplrt_w)0)       /* a slot that holds nothing; never a value of the language */
#define CPLRT_FALSE ((cplrt_w)2)
#define CPLRT_TRUE ((cplrt_w)6)
#define CPLRT_SMALL_MIN (-(INT64_C(1) << 62))
#define CPLRT_SMALL_MAX ((INT64_C(1) << 62) - 1)
static inline int cplrt_is_small(cplrt_w w) { return (int)(w & 1u); }
static inline int cplrt_is_obj(cplrt_w w) { return w != 0u && (w & 7u) == 0u; }
static inline cplrt_w cplrt_small(int64_t v) { return ((uint64_t)v << 1) | 1u; }   /* v in [SMALL_MIN, SMALL_MAX] */
static inline int64_t cplrt_small_value(cplrt_w w)          /* portable: no implementation-defined shift */
{ return (int64_t)((w >> 1) ^ (UINT64_C(1) << 62)) - (INT64_C(1) << 62); }
static inline cplrt_w cplrt_bool(int b) { return b ? CPLRT_TRUE : CPLRT_FALSE; }
static inline int cplrt_truth(cplrt_w w) { return w == CPLRT_TRUE; }

/* ---- heap objects (2.3) ------------------------------------------------------------------------------------- */
#define CPLRT_BIGINT 1u
#define CPLRT_TEXT 2u
#define CPLRT_BYTES 3u
#define CPLRT_LIST 4u
#define CPLRT_STRUCT 5u
#define CPLRT_ENUM 6u
#define CPLRT_WRAPPER 7u
#define CPLRT_F_NEGATIVE 0x01u         /* a negative bigint */
#define CPLRT_F_WIDE 0x02u             /* an enum value whose variant index is the small Int in item[0] (2.3) */
#define CPLRT_RC_SATURATED UINT32_C(0x80000000)   /* every count at or above it is immortal (2.4) */
#define CPLRT_RC_IMMORTAL UINT32_C(0xC0000000)    /* the count of statics and of objects whose count saturated */
typedef struct cplrt_obj {
    uint8_t kind;                      /* CPLRT_BIGINT, _TEXT, _BYTES, _LIST, _STRUCT, _ENUM, _WRAPPER */
    uint8_t flags;
    uint16_t variant;                  /* enums */
    uint32_t len;                      /* children or limbs; Text and Bytes keep size_t lengths in their bodies */
    union {
        struct {
            _Atomic uint32_t rc;       /* CPLRT_RC_IMMORTAL for statics; >= CPLRT_RC_SATURATED: immortal */
            uint32_t def;              /* DefId.0 of a struct, enum, or wrapper */
        } live;
        struct cplrt_obj *next;        /* once dead: the pending-release link */
    } u;
} cplrt_obj;
_Static_assert(sizeof(cplrt_obj) == 16, "the object header is 16 bytes");
typedef struct cplrt_bigint { cplrt_obj hdr; uint32_t limb[]; } cplrt_bigint;        /* len limbs, least first */
typedef struct cplrt_text { cplrt_obj hdr; size_t bytes; size_t scalars; char utf8[]; } cplrt_text; /* NUL after */
typedef struct cplrt_bytes { cplrt_obj hdr; size_t len; uint8_t data[]; } cplrt_bytes;
typedef struct cplrt_node { cplrt_obj hdr; cplrt_w item[]; } cplrt_node;          /* List, struct, enum, wrapper */
static inline cplrt_obj *cplrt_obj_of(cplrt_w w) { return (cplrt_obj *)(uintptr_t)w; }
static inline cplrt_w cplrt_word_of(const cplrt_obj *o) { return (cplrt_w)(uintptr_t)o; }

/* ---- reference counts and the slot macros (1.3, 2.4) -------------------------------------------------------- */
void CPLRT(release_obj)(cplrt_obj *o);                     /* the last reference is gone: iterative release */
static inline void cplrt_retain(cplrt_w w)
{
    if (cplrt_is_obj(w)) {
        cplrt_obj *o = cplrt_obj_of(w);
        if (atomic_load_explicit(&o->u.live.rc, memory_order_relaxed) < CPLRT_RC_SATURATED &&
            atomic_fetch_add_explicit(&o->u.live.rc, 1u, memory_order_relaxed) >= CPLRT_RC_SATURATED - 1u)
            atomic_store_explicit(&o->u.live.rc, CPLRT_RC_IMMORTAL, memory_order_relaxed);   /* saturation */
    }
}
/* 0, immediates, and immortals: nothing. An acquire-release decrement and no standalone fence, which
   ThreadSanitizer does not model (2.4). */
static inline void cplrt_release(cplrt_w w)
{
    if (cplrt_is_obj(w)) {
        cplrt_obj *o = cplrt_obj_of(w);
        if (atomic_load_explicit(&o->u.live.rc, memory_order_relaxed) < CPLRT_RC_SATURATED &&
            atomic_fetch_sub_explicit(&o->u.live.rc, 1u, memory_order_acq_rel) == 1u)
            CPLRT(release_obj)(o);
    }
}
/* The only writes of a slot: store an owned word, store a retained borrowed word, move between slots. */
#define CPLRT_SET(slot, owned) \
    do { cplrt_w cplrt_set_ = (owned); cplrt_release(slot); (slot) = cplrt_set_; } while (0)
#define CPLRT_COPY(slot, borrowed) \
    do { cplrt_w cplrt_copy_ = (borrowed); cplrt_retain(cplrt_copy_); CPLRT_SET(slot, cplrt_copy_); } while (0)
#define CPLRT_MOVE(dst, src) \
    do { cplrt_w cplrt_move_ = (src); (src) = 0; cplrt_release(dst); (dst) = cplrt_move_; } while (0)

/* ---- borrowed access (2.3, 2.6); every result is borrowed from v ---------------------------------------------- */
static inline uint32_t cplrt_len(cplrt_w v) { return cplrt_obj_of(v)->len; }
static inline cplrt_w cplrt_item(cplrt_w v, uint32_t i) { return ((const cplrt_node *)cplrt_obj_of(v))->item[i]; }
static inline cplrt_w cplrt_field(cplrt_w v, uint32_t i) { return cplrt_item(v, i); }
static inline cplrt_w cplrt_inner(cplrt_w v) { return cplrt_item(v, 0); }
static inline uint32_t cplrt_variant(cplrt_w v)
{
    const cplrt_obj *o = cplrt_obj_of(v);
    return (o->flags & CPLRT_F_WIDE) ? (uint32_t)cplrt_small_value(cplrt_item(v, 0)) : o->variant;
}
static inline uint32_t cplrt_payload_len(cplrt_w v)
{ return cplrt_len(v) - ((cplrt_obj_of(v)->flags & CPLRT_F_WIDE) ? 1u : 0u); }
static inline cplrt_w cplrt_payload(cplrt_w v, uint32_t i)
{ return cplrt_item(v, i + ((cplrt_obj_of(v)->flags & CPLRT_F_WIDE) ? 1u : 0u)); }
static inline int cplrt_is_ok(cplrt_w v) { return cplrt_variant(v) == CPL_RESULT_OK; }
static inline const cplrt_text *cplrt_text_of(cplrt_w v) { return (const cplrt_text *)cplrt_obj_of(v); }
static inline const cplrt_bytes *cplrt_bytes_of(cplrt_w v) { return (const cplrt_bytes *)cplrt_obj_of(v); }
static inline int cplrt_int_sign_big(cplrt_w v) { return (cplrt_obj_of(v)->flags & CPLRT_F_NEGATIVE) ? -1 : 1; }
/* An Int against a small constant b (normalized bigints lie outside the small range, 2.5). */
static inline int cplrt_int_eq_small(cplrt_w a, int64_t b) { return a == cplrt_small(b); }
static inline int cplrt_int_ne_small(cplrt_w a, int64_t b) { return a != cplrt_small(b); }
static inline int cplrt_int_lt_small(cplrt_w a, int64_t b)
{ return cplrt_is_small(a) ? cplrt_small_value(a) < b : cplrt_int_sign_big(a) < 0; }
static inline int cplrt_int_gt_small(cplrt_w a, int64_t b)
{ return cplrt_is_small(a) ? cplrt_small_value(a) > b : cplrt_int_sign_big(a) > 0; }
static inline int cplrt_int_le_small(cplrt_w a, int64_t b) { return !cplrt_int_gt_small(a, b); }
static inline int cplrt_int_ge_small(cplrt_w a, int64_t b) { return !cplrt_int_lt_small(a, b); }
static inline int cplrt_text_eq(cplrt_w a, cplrt_w b)     /* byte equality: scalar equality for valid UTF-8 */
{
    const cplrt_text *x = cplrt_text_of(a), *y = cplrt_text_of(b);
    return a == b || (x->bytes == y->bytes && memcmp(x->utf8, y->utf8, x->bytes) == 0);
}

/* ---- capability references (2.6, 5.7) ----------------------------------------------------------------------- */
typedef struct cplrt_capref cplrt_capref;
typedef struct cplrt_capimpl {                  /* how the operations of one capability object reach their code */
    uint32_t nops;
    /* the binding layer of operation op (5.7, 6.2, 6.3): args are borrowed words in parameter order; on CPL_OK
       *result is an owned word; any other status follows the callback contract of 5.7 */
    cpl_status (*invoke)(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args, cplrt_w *result);
    void (*drop)(const cplrt_capref *cap);      /* once, after the last reference (5.7): the host's drop or fini */
} cplrt_capimpl;
struct cplrt_capref {                           /* the runtime's capability object begins with it */
    const cplrt_capimpl *impl;
    void *user;                                 /* the host's user data, or the adapter's state */
    uint32_t capability;                        /* DefId.0 of the capability declaration */
    uint32_t handle;                            /* the $handle number: creation order in the runtime, from 0 */
};

/* ---- activations and the evaluation context (1.2, 3.3-3.6, 4.3) --------------------------------------------- */
typedef struct cplrt_frame {
    struct cplrt_frame *prev;                   /* the enclosing activation; NULL at the top */
    uint32_t def;                               /* DefId.0 of the activation */
    uint32_t site;                              /* CPL_S of the call that entered it; CPLRT_NO_SITE at the top */
    const cplrt_w *params;                      /* function activations: the borrowed positional parameters */
    uint32_t nparams;
    uint32_t call;                              /* 1 for a CPL function call, which the depth limit counts */
    cplrt_w measure;                            /* the header decreases measure recorded on entry (S35), 0: none */
} cplrt_frame;
typedef struct cplrt_fault cplrt_fault;         /* a fault record (3.3.1); its layout is the runtime's */
typedef struct cplrt_runtime cplrt_runtime;     /* begins with the public cpl_runtime (4.1) */
typedef struct cplrt_program cplrt_program;
typedef struct cplrt_cx {                       /* one top-level evaluation, on the caller's C stack (4.3) */
    cplrt_runtime *rt;
    const cplrt_program *program;
    cplrt_frame *top;                           /* the innermost activation; NULL when none */
    uint32_t calls;                             /* active CPL function calls (S72) */
    uint32_t max_call_depth;
    int check_contracts;                        /* contract regions run (3.7) */
    uint32_t parallel_depth;
    const _Atomic uint32_t *parallel_failure;
    uint32_t parallel_index;
    int attributed;                             /* the propagating fault was claimed by a clause (S34, S70) */
    int cancelled;                              /* the fault path is a cancellation unwind (3.6) */
    uintptr_t stack_floor;                      /* the lowest frame address an activation may start at (3.4) */
    uint64_t deadline_ns;                       /* monotonic deadline (3.6); 0: no time budget */
    uint64_t budget_ms;                         /* the `limit` of a time fault */
    const _Atomic uint32_t *cancel;             /* the call's cancellation flag; NULL: none */
    const char *range_text;                     /* drivers: the clause text of the range bound being evaluated */
    cplrt_fault *fault;                         /* the fault being raised; NULL when none */
    cplrt_fault *violation;                     /* a copy of the last ensures, invariant, or decreases violation */
    cplrt_fault *memory;                        /* the preallocated resource_exhaustion (memory) record (4.7) */
} cplrt_cx;

/* ---- the program tables, written by cpl-emit-c into <prefix>_tables.c (3.2) ---------------------------------- */
#define CPLRT_DEF_FN 1u
#define CPLRT_DEF_WRAPPER 2u
#define CPLRT_DEF_REFINEMENT 3u
#define CPLRT_DEF_STRUCT 4u
#define CPLRT_DEF_ENUM 5u
#define CPLRT_DEF_CAPABILITY 6u
#define CPLRT_DEF_TEST 7u
#define CPLRT_DEF_PROPERTY 8u
#define CPLRT_DEF_BUILTIN_TYPE 9u
typedef struct cplrt_op {                       /* one capability operation */
    const char *name;
    const char *identity;                       /* operation_identity: reader.pages.PageRead.get */
    uint32_t nparams;
    const char *const *params;                  /* parameter names: the `input` keys */
    const uint32_t *sorted;                     /* parameter indices in byte order of their names */
    uint32_t ret;                               /* CPL_X of the declared return type (6.4) */
    uint32_t ret_foreign;                       /* 1 when the return type is Result<T, std.foreign.ForeignError> */
} cplrt_op;
typedef struct cplrt_def {                      /* CPL_D: one per DefId, std included */
    const char *stable_id;                      /* Def::stable_id: `symbol` */
    const char *qualified_name;                 /* Program::display_path: `qualified_name`, backtraces, `caller` */
    const char *type_name;                      /* types::type_name: `$type`, ConstraintError.type_name (S44) */
    const char *bare_name;                      /* types::bare_name: value text */
    uint32_t kind;                              /* CPLRT_DEF_* */
    uint32_t nmembers;                          /* fields, variants, positional parameters, or operations */
    const char *const *members;                 /* their names; a member stub's receiver is "self" */
    const uint32_t *sorted;                     /* member indices in byte order of their names (JSON keys) */
    cplrt_obj *const *statics;                  /* enum: per variant, its immortal value if payload-less, else NULL */
    const cplrt_op *ops;                        /* capability: nmembers operations */
    uint32_t nrequires;                         /* fn: its requires clauses (a builtin stub's from requires_spec) */
    const uint32_t *requires;                   /* CPL_CL indices */
    const uint32_t *ptypes;                     /* fn, in a driver: CPL_X of each positional parameter; else NULL */
    const uint32_t *psites;                     /* fn: CPL_S of each positional parameter (the span of a boundary
                                                   refinement_violation, 5.5 step 3); else NULL */
} cplrt_def;
typedef struct cplrt_site {                     /* CPL_S: every span a fault can report */
    uint32_t file;                              /* FileId.0, an index into cplrt_program.files */
    uint32_t start, end;                        /* byte offsets */
    uint32_t line, col, end_line, end_col;      /* SourceMap::location: the S49 span */
} cplrt_site;
#define CPLRT_CK_REQUIRES 1u
#define CPLRT_CK_ENSURES 2u
#define CPLRT_CK_INVARIANT 3u
#define CPLRT_CK_DECREASES 4u
#define CPLRT_CK_WHERE 5u                       /* a refinement predicate */
#define CPLRT_CK_RANGE 6u                       /* a property's for range (S71) */
typedef struct cplrt_clause {                   /* CPL_CL */
    const char *text;                           /* Clause::text (T56): `clause` */
    uint32_t kind;                              /* CPLRT_CK_* */
    uint32_t site;                              /* CPL_S of the clause */
    uint32_t owner;                             /* DefId.0 of the owning function, refinement, property, capability */
    uint32_t op;                                /* an operation clause: the operation index; else CPLRT_NONE */
    const char *owner_id;                       /* the `symbol` an S34 claim names */
    const char *owner_qualified;                /* its `qualified_name` */
    uint32_t bsite;                             /* a loop clause: CPL_A of the loop; else CPLRT_NONE */
} cplrt_clause;
typedef struct cplrt_binding {                  /* one visible local */
    const char *name;
    const char *type;                           /* display_ty_in with the enclosing generics */
    const char *kind;                           /* BindingKind::as_str */
    uint32_t is_mutable;                        /* 1 for a var */
    uint32_t capability;                        /* a using parameter: DefId.0 of its capability, else CPLRT_NONE */
} cplrt_binding;
typedef struct cplrt_bsite {                    /* CPL_A: the locals visible at an assert, a loop, or a hole */
    const char *text;                           /* an assert: Stmt::Assert::text; else NULL */
    uint32_t site;                              /* an assert: the asserted expression; else CPLRT_NONE */
    uint32_t n;
    const cplrt_binding *bindings;              /* innermost first: the order of a `visible` argument */
    uint32_t nobject;                           /* a loop: the actual.bindings object, one key per name */
    const uint32_t *object;                     /* indices into bindings (the innermost per name), in name order */
} cplrt_bsite;
typedef struct cplrt_hole {                     /* CPL_H, by HoleId */
    const char *name;                           /* without the ? */
    const char *expected_type;                  /* display_ty_in of the hole's type */
    uint32_t site;
    uint32_t bsite;
} cplrt_hole;
typedef struct cplrt_refinement {               /* CPL_R */
    uint32_t def;                               /* DefId.0 */
    const char *type_name;                      /* constraint_type_name */
    const char *declared_name;                  /* DeclName::display: the expected_type of a boundary violation */
    const char *predicate;                      /* the predicate's Clause::text */
    uint32_t clause;                            /* CPL_CL of the predicate */
    uint32_t base;                              /* CPL_X of the root base strip(R) (types::strip) */
    uint32_t nchain;                            /* check_order(def), root first, as CPL_R indices */
    const uint32_t *chain;
} cplrt_refinement;
#define CPLRT_TY_INT 1u
#define CPLRT_TY_BOOL 2u
#define CPLRT_TY_TEXT 3u
#define CPLRT_TY_BYTES 4u
#define CPLRT_TY_LIST 5u
#define CPLRT_TY_STRUCT 6u
#define CPLRT_TY_ENUM 7u
#define CPLRT_TY_WRAPPER 8u
#define CPLRT_TY_REFINED 9u
#define CPLRT_TY_CAPABILITY 10u
#define CPLRT_TY_PARAM 11u                      /* a type parameter of a generic declaration: matches any value */
typedef struct cplrt_type {                     /* CPL_X: the index is the package's cpl_type id */
    const char *text;                           /* the qualified type text (5.3): cpl_type_name, interop_* payloads */
    const char *display;                        /* Program::display_ty: the <expected_type> of stdlib S45 and semantics S80 */
    uint32_t form;                              /* CPLRT_TY_* */
    uint32_t def;                               /* struct, enum, wrapper, capability: DefId.0; refined: CPL_R */
    uint32_t elem;                              /* List: element; wrapper: inner; refined: root base; struct: invariant CPL_R + 1, or 0 */
    uint32_t nmembers;                          /* struct: fields; enum: variants */
    const uint32_t *members;                    /* struct: CPL_X per field; enum: nmembers + 1 payload offsets */
    const uint32_t *payloads;                   /* enum: the payload types of every variant, concatenated */
} cplrt_type;
typedef struct cplrt_std {                      /* DefId.0 of the std declarations the runtime constructs */
    uint32_t unit, option, result, constraint_error, foreign_error, foreign_error_kind, foreign_detail;
} cplrt_std;
#define CPLRT_AF_HANDLE 1u                      /* export argument forms (5.4) */
#define CPLRT_AF_I64 2u
#define CPLRT_AF_I32 3u
#define CPLRT_AF_U64 4u
#define CPLRT_AF_U32 5u
#define CPLRT_AF_BOOL 6u
#define CPLRT_AF_TEXT 7u
#define CPLRT_AF_BYTES 8u
#define CPLRT_AF_UNIT 9u                        /* no C parameter, no output */
typedef struct cplrt_export {
    const char *symbol;
    uint32_t function;                          /* DefId.0 */
    uint32_t nparams;
    const uint32_t *forms;                      /* per positional parameter: CPLRT_AF_* */
    const uint32_t *types;                      /* per positional parameter: CPL_X of its declared type */
    uint32_t nusing;
    const uint32_t *caps;                       /* per using parameter: DefId.0 of its capability */
    uint32_t split;                             /* 1: a Result return split into ok_out (CPL_OK), err_out (CPL_ERR) */
    uint32_t ok_form, err_form;                 /* the result's form (ok_form alone when not split): CPLRT_AF_* */
} cplrt_export;
typedef struct cplrt_grant {                    /* a backend-c binding (6.1) */
    const char *binding;                        /* the manifest table name */
    uint32_t capability;                        /* DefId.0 */
    const char *label;                          /* the adapter detail of a ForeignError: the binding name */
    cpl_status (*init)(cpl_runtime *rt, void **state, cpl_error **error);
    void (*fini)(void *state);
    const cplrt_capimpl *impl;                  /* the generated binding layer over the adapter's functions */
    uint32_t declarative;                       /* 1: a generated wrapper (6.3), whose init fails other than with
                                                   CPL_E_NOMEM only when [state] create returned NULL */
    uint32_t worker;                            /* Milestone 3 (spec/python.md 6): 0 in process; 1 a backend-python
                                                   binding (no init, fini, or impl: always a Python worker); 2 a
                                                   backend-c binding in the C adapter host (init, fini, and impl are
                                                   the host's own grant) */
    uint64_t timeout_ms;                        /* the binding's timeout_ms; 0: none */
} cplrt_grant;

/* ---- the entry points of <prefix>_prog.c (CPLPKG(code)) --------------------------------------------------- */
typedef int (*cplrt_fn_entry)(cplrt_cx *cx, const cplrt_w *args, const cplrt_capref *const *using, cplrt_w *out);
typedef int (*cplrt_pred_entry)(cplrt_cx *cx, uint32_t site, cplrt_w self, int *holds);
typedef int (*cplrt_test_entry)(cplrt_cx *cx, const cplrt_w *binders);
typedef int (*cplrt_bound_entry)(cplrt_cx *cx, cplrt_w *out);
typedef struct cplrt_bound { uint32_t property, binder, hi; cplrt_bound_entry entry; } cplrt_bound;
typedef struct cplrt_code {
    uint32_t ndefs;
    const cplrt_fn_entry *fns;                  /* by DefId.0: exports' trampolines; with CPL_WITH_TESTS, those of
                                                   every fn with a block body and every callable builtin stub */
    const cplrt_pred_entry *preds;              /* by DefId.0: cpl_p<n> of every refinement */
    const cplrt_test_entry *tests;              /* by DefId.0: cpl_t<n> (CPL_WITH_TESTS only; else NULL) */
    uint32_t nbounds;
    const cplrt_bound *bounds;                  /* cpl_b<n>_<binder>_lo and _hi (CPL_WITH_TESTS only) */
} cplrt_code;
struct cplrt_program {                          /* CPLPKG(program), in <prefix>_tables.c */
    cpl_library_info info;                      /* what <prefix>_library_info returns (4.2) */
    const cplrt_code *code;
    uint32_t ndefs;       const cplrt_def *defs;
    uint32_t nfiles;      const char *const *files;   /* project-relative paths, '/' separators */
    uint32_t nsites;      const cplrt_site *sites;
    uint32_t nclauses;    const cplrt_clause *clauses;
    uint32_t nbsites;     const cplrt_bsite *bsites;
    uint32_t nholes;      const cplrt_hole *holes;
    uint32_t nrefs;       const cplrt_refinement *refinements;
    uint32_t ntypes;      const cplrt_type *types;    /* [0] unused; [1..11] the well-known types of cpl.h */
    uint32_t nexports;    const cplrt_export *exports;
    uint32_t ngrants;     const cplrt_grant *grants;
    cplrt_std std;
    uint32_t max_call_depth;                    /* [run] max_call_depth at build time: the executable's default (8) */
    const char *python_worker;                  /* Milestone 3: the Python worker source (spec/python.md 6.1), NULL
                                                   when the program has no backend-python binding */
    size_t python_worker_len;
    const char *python_digest;                  /* the digest its banner names (64 lowercase hex), or NULL */
};
extern const cplrt_program CPLPKG(program);
extern const cplrt_code CPLPKG(code);

/* ---- coverage (1.4): only drivers emitted with EmitConfig::coverage, compiled with -DCPLRT_COVERAGE=1, count -- */
#if defined(CPLRT_COVERAGE) && CPLRT_COVERAGE
extern _Atomic uint64_t CPLPKG(cov)[];                    /* <prefix>_tables.c: one counter per lowered HIR node */
extern const char *const CPLPKG(cov_kinds)[];             /* its HIR variant: "ExprKind::Call", "Stmt::While", ... */
extern const uint32_t CPLPKG(ncov);
#define CPLRT_COV(k) ((void)atomic_fetch_add_explicit(&CPLPKG(cov)[k], 1u, memory_order_relaxed))
void CPLRT(cov_bi)(const char *id);                       /* one call of the builtin `id` (cpl_rt_eval.c) */
#else
#define CPLRT_COV(k) ((void)0)
#endif

/* ---- helpers called by emitted code. Every int-returning helper returns CPLRT_OK or CPLRT_FAULT; every
   `cplrt_w *out` has CPLRT_SET semantics; word arguments are borrowed. ------------------------------------------ */
#define cplrt_enter_fn CPLRT(enter_fn)
#define cplrt_enter_act CPLRT(enter_act)
#define cplrt_leave CPLRT(leave)
#define cplrt_tick CPLRT(tick)
/* depth (S72), stack guard, time, cancellation, then the frame push; refusal leaves no frame (3.4) */
int cplrt_enter_fn(cplrt_cx *cx, cplrt_frame *fr, uint32_t def, uint32_t site, const cplrt_w *params, uint32_t n);
int cplrt_enter_act(cplrt_cx *cx, cplrt_frame *fr, uint32_t def, uint32_t site);  /* the same without the depth */
int cplrt_leave(cplrt_cx *cx, cplrt_frame *fr, int st);    /* pop; on a fault append {def, site} to the backtrace */
typedef int (*cplrt_parallel_task)(cplrt_cx *, cplrt_w, const cplrt_w *, cplrt_w *);
#define cplrt_parallel_map CPLRT(parallel_map)
int cplrt_parallel_map(cplrt_cx *, uint32_t, cplrt_w, cplrt_parallel_task, const cplrt_w *, cplrt_w *);
int cplrt_tick(cplrt_cx *cx, uint32_t site);               /* before a loop condition: time and cancellation */

#define cplrt_int_add CPLRT(int_add)
#define cplrt_int_sub CPLRT(int_sub)
#define cplrt_int_mul CPLRT(int_mul)
#define cplrt_int_neg CPLRT(int_neg)
#define cplrt_int_divrem CPLRT(int_divrem)
#define cplrt_int_cmp CPLRT(int_cmp)
int cplrt_int_add(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out);
int cplrt_int_sub(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out);
int cplrt_int_mul(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out);
int cplrt_int_neg(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w *out);
/* the only division (T9): quotient toward zero, remainder a - q * b (S37); either output may be NULL;
   a zero divisor raises division_by_zero at site with input {"a", "b"} */
int cplrt_int_divrem(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *q, cplrt_w *r);
static inline int cplrt_int_div(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{ return cplrt_int_divrem(cx, site, a, b, out, NULL); }
static inline int cplrt_int_rem(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{ return cplrt_int_divrem(cx, site, a, b, NULL, out); }
int cplrt_int_cmp(cplrt_w a, cplrt_w b);                   /* -1, 0, 1; never fails */

#define cplrt_equal CPLRT(equal)
#define cplrt_concat CPLRT(concat)
#define cplrt_list_new CPLRT(list_new)
#define cplrt_struct_new CPLRT(struct_new)
#define cplrt_enum_new CPLRT(enum_new)
#define cplrt_wrap CPLRT(wrap)
#define cplrt_result_ok CPLRT(result_ok)
#define cplrt_result_err CPLRT(result_err)
#define cplrt_option_some CPLRT(option_some)
#define cplrt_check_failure CPLRT(check_failure)
int cplrt_equal(cplrt_cx *cx, cplrt_w a, cplrt_w b, int *eq);           /* structural ==, iterative (2.8) */
int cplrt_concat(cplrt_cx *cx, cplrt_w a, cplrt_w b, cplrt_w *out);     /* ++ on List, Text, Bytes */
int cplrt_list_new(cplrt_cx *cx, const cplrt_w *items, uint32_t n, cplrt_w *out);
int cplrt_struct_new(cplrt_cx *cx, uint32_t def, const cplrt_w *fields, uint32_t n, cplrt_w *out);
int cplrt_enum_new(cplrt_cx *cx, uint32_t def, uint32_t variant, const cplrt_w *payload, uint32_t n, cplrt_w *out);
int cplrt_wrap(cplrt_cx *cx, uint32_t def, cplrt_w inner, cplrt_w *out);
int cplrt_result_ok(cplrt_cx *cx, cplrt_w v, cplrt_w *out);
int cplrt_result_err(cplrt_cx *cx, cplrt_w e, cplrt_w *out);
int cplrt_option_some(cplrt_cx *cx, cplrt_w v, cplrt_w *out);
/* check<R>: Err(ConstraintError) for the refinement CPL_R `refinement` whose predicate was false for v */
int cplrt_check_failure(cplrt_cx *cx, uint32_t refinement, cplrt_w v, cplrt_w *out);

#define cplrt_requires_failed CPLRT(requires_failed)
#define cplrt_ensures_failed CPLRT(ensures_failed)
#define cplrt_constructor_failed CPLRT(constructor_failed)
#define cplrt_header_measure CPLRT(header_measure)
#define cplrt_invariant_failed CPLRT(invariant_failed)
#define cplrt_loop_measure CPLRT(loop_measure)
#define cplrt_op_requires_failed CPLRT(op_requires_failed)
#define cplrt_op_ensures_failed CPLRT(op_ensures_failed)
#define cplrt_claim CPLRT(claim)
#define cplrt_claim_range CPLRT(claim_range)
#define cplrt_refine_unsound CPLRT(refine_unsound)
#define cplrt_assert_failed CPLRT(assert_failed)
#define cplrt_hole CPLRT(hole)
#define cplrt_no_match CPLRT(no_match)
#define cplrt_internal CPLRT(internal)
#define cplrt_bi_uncallable CPLRT(bi_uncallable)
/* Each *_failed, hole, and internal helper sets cx->fault (or the memory record) and returns nothing. A `visible`
   array (1.4) holds the words of a binding site's locals, innermost first; the entry of a binding whose
   cplrt_binding.capability is not CPLRT_NONE (a using parameter) is (cplrt_w)(uintptr_t) of its
   const cplrt_capref *, which the runtime reads only for its handle number and never retains or releases. */
void cplrt_requires_failed(cplrt_cx *cx, uint32_t clause);      /* input: the frame; caller: the frame below */
void cplrt_ensures_failed(cplrt_cx *cx, uint32_t clause, cplrt_w result);
void cplrt_constructor_failed(cplrt_cx *cx, uint32_t clause, cplrt_w result);
int cplrt_header_measure(cplrt_cx *cx, cplrt_frame *fr, uint32_t clause, cplrt_w measure);   /* S35 */
void cplrt_invariant_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *visible, uint32_t n, uint64_t iteration);
/* a loop measure after the condition held: >= 0 and below *previous, else the decreases violation; on success
   *previous holds the measure */
int cplrt_loop_measure(cplrt_cx *cx, uint32_t clause, const cplrt_w *visible, uint32_t n, uint64_t iteration,
                       cplrt_w measure, cplrt_w *previous);
void cplrt_op_requires_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *args, uint32_t n);
void cplrt_op_ensures_failed(cplrt_cx *cx, uint32_t clause, const cplrt_w *args, uint32_t n, cplrt_w result);
void cplrt_claim(cplrt_cx *cx, uint32_t clause);           /* S34 and S70: a fault escaped the clause */
void cplrt_claim_range(cplrt_cx *cx, uint32_t property);   /* S71: with cx->range_text */
void cplrt_refine_unsound(cplrt_cx *cx, uint32_t site, uint32_t refinement, cplrt_w v);   /* T39 */
void cplrt_assert_failed(cplrt_cx *cx, uint32_t bsite, const cplrt_w *visible, uint32_t n);
void cplrt_hole(cplrt_cx *cx, uint32_t hole, const cplrt_w *visible, uint32_t n);
/* internal_error no_matching_arm at the match `site`: "no arm of the match matches <value text of scrutinee>" */
void cplrt_no_match(cplrt_cx *cx, uint32_t site, cplrt_w scrutinee);
void cplrt_internal(cplrt_cx *cx, uint32_t site, const char *kind, const char *detail);
/* a call of the stub `stub` whose builtin `id` is not callable (1.4): gen_range's precondition first, then
   builtin_type_called or generator_not_a_value with the evaluator's detail and the frame {stub, site}; always
   CPLRT_FAULT */
int cplrt_bi_uncallable(cplrt_cx *cx, uint32_t site, uint32_t stub, const char *id, const cplrt_w *args, uint32_t n);

#define cplrt_cap_invoke CPLRT(cap_invoke)
/* a capability operation's host call: one reserved handle slot per handle argument (5.7), the binding layer, the
   foreign-failure mapping (6.5), and validation (6.4) */
int cplrt_cap_invoke(cplrt_cx *cx, uint32_t site, const cplrt_capref *h, uint32_t op,
                     const cplrt_w *args, uint32_t n, cplrt_w *out);

/* ---- the 46 callable builtins (2.7): cplrt_bi_<id>(cx, site, stub, arguments..., out) ----------------------- */
#define CPLRT_BI1(id) int CPLRT(bi_##id)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
#define CPLRT_BI2(id) \
    int CPLRT(bi_##id)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
CPLRT_BI1(flatten); CPLRT_BI1(int_to_text); CPLRT_BI1(int_abs); CPLRT_BI2(int_min); CPLRT_BI2(int_max);
CPLRT_BI1(text_len); CPLRT_BI1(text_is_empty); CPLRT_BI1(bytes_len); CPLRT_BI1(bytes_is_empty);
CPLRT_BI1(list_len); CPLRT_BI1(list_is_empty); CPLRT_BI2(list_take); CPLRT_BI2(list_drop); CPLRT_BI1(list_drop_last);
CPLRT_BI2(list_push); CPLRT_BI2(list_get); CPLRT_BI1(list_first); CPLRT_BI1(list_last); CPLRT_BI1(list_reverse);
CPLRT_BI2(list_contains); CPLRT_BI1(option_is_some); CPLRT_BI1(option_is_none); CPLRT_BI2(option_unwrap_or);
CPLRT_BI2(option_ok_or); CPLRT_BI1(result_is_ok); CPLRT_BI1(result_is_err); CPLRT_BI2(result_unwrap_or);
CPLRT_BI1(result_ok); CPLRT_BI1(result_err); CPLRT_BI1(text_trim); CPLRT_BI1(text_trim_start);
CPLRT_BI1(text_trim_end);
CPLRT_BI2(text_starts_with); CPLRT_BI2(text_ends_with); CPLRT_BI2(text_contains); CPLRT_BI2(text_split);
CPLRT_BI2(text_join); CPLRT_BI1(text_to_upper); CPLRT_BI1(text_to_lower); CPLRT_BI1(text_parse_int);
CPLRT_BI1(text_from_int); CPLRT_BI1(text_chars); CPLRT_BI1(bytes_from_list); CPLRT_BI1(bytes_to_list);
CPLRT_BI1(bytes_from_text); CPLRT_BI1(bytes_to_text);
#undef CPLRT_BI1
#undef CPLRT_BI2
#if defined(CPLRT_COVERAGE) && CPLRT_COVERAGE                /* a coverage driver counts each call by builtin id */
#define cplrt_bi(id) (CPLRT(cov_bi)(#id), CPLRT(bi_##id))
#else
#define cplrt_bi(id) CPLRT(bi_##id)                         /* emitted code calls cplrt_bi(list_take)(cx, ...) */
#endif

/* ---- helpers called by <prefix>_api.c (4.2, 5.3-5.7, 6.1) ---------------------------------------------------- */
typedef union cplrt_arg {                       /* one C argument of an export, by its form */
    cpl_value value;
    int64_t i64;
    int32_t i32;
    uint64_t u64;
    uint32_t u32;
    struct { const void *ptr; size_t len; } span;               /* CPLRT_AF_TEXT and CPLRT_AF_BYTES */
} cplrt_arg;
typedef union cplrt_out {                       /* one output pointer of an export, by its form */
    cpl_value *value;
    int64_t *i64;
    int32_t *i32;
    uint64_t *u64;
    uint32_t *u32;
} cplrt_out;
#define cplrt_runtime_open CPLRT(runtime_open)
#define cplrt_export_call CPLRT(export_call)
#define cplrt_cap_create CPLRT(cap_create)
#define cplrt_call_borrow CPLRT(call_borrow)
#define cplrt_call_take CPLRT(call_take)
#define cplrt_cap_ops CPLRT(cap_ops)
#define cplrt_grant_call CPLRT(grant_call)
cpl_status cplrt_runtime_open(const cplrt_program *p, const cpl_runtime_options *options, cpl_runtime **out,
                              cpl_error **error);
/* 5.5 steps 1-4 for export `index`; args and caps in parameter order, outs: [ok_out, err_out] or [out] */
cpl_status cplrt_export_call(cpl_runtime *rt, uint32_t index, const cplrt_arg *args, const cpl_cap *caps,
                             const cplrt_out *outs, const cpl_call_options *options, cpl_error **error);
/* <prefix>_<capability>_new (5.7): the typed ops struct (size: its sizeof in the library; op_offsets: where its
   operation pointers are, declaration order) is validated and copied; user is taken only on CPL_OK */
cpl_status cplrt_cap_create(cpl_runtime *rt, uint32_t capability, const cplrt_capimpl *impl, const void *ops,
                            size_t size, const size_t *op_offsets, void *user, cpl_cap *out, cpl_error **error);
const void *cplrt_cap_ops(const cplrt_capref *cap);        /* the runtime's copy of the typed ops struct */
/* a borrowed handle, valid until the callback returns, in a slot that cplrt_cap_invoke reserved before the call
   (5.7), so it cannot fail */
cpl_value cplrt_call_borrow(cpl_call *call, cplrt_w w);
cpl_status cplrt_call_take(cpl_call *call, cpl_status st, cpl_value result, cplrt_w *out);   /* 5.7 result rule */
cpl_status cplrt_grant_call(cpl_runtime *rt, uint32_t grant, cpl_cap *out, cpl_error **error);   /* 6.1 */
/* Milestone 3, the worker transport (spec/python.md 6): the binding layer of every worker grant; the C adapter host's
   main (CPL_WORKER_MODE=1); and the flag the driver's and the executable's mains set, by which a C adapter host is
   this executable re-executed */
extern const cplrt_capimpl CPLRT(worker_impl);
int CPLRT(worker_main)(void);
extern int CPLRT(worker_self);
/* Milestone 3, the internal entries of the Python extension (spec/python.md 2), outside struct cpl_api: an export with
   every argument and output a value handle; a value from and to its node-list text (3.8); the workers' configuration */
cpl_status CPLRT(export_call_values)(cpl_runtime *rt, uint32_t index, const cpl_value *args, uint32_t nargs,
                                     const cpl_cap *caps, uint32_t ncaps, cpl_value *ok_out, cpl_value *err_out,
                                     const cpl_call_options *options, cpl_error **error);
cpl_status CPLRT(api_value_from_nodes)(cpl_runtime *rt, const char *text, size_t len, cpl_value *out, cpl_error **error);
cpl_status CPLRT(api_value_to_nodes)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
cpl_status CPLRT(api_workers)(cpl_runtime *rt, const char *python, const char *const *env, uint32_t n,
                              cpl_error **error);
cpl_status CPLRT(api_busy)(cpl_runtime *rt, cpl_error **error);
cpl_status CPLRT(api_cap_new_labeled)(cpl_runtime *rt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                                      const char *label, cpl_cap *out, cpl_error **error);

/* Poll at every loop condition without an out-of-line call when no deadline or cancellation needs handling.
   The slow path retains the runtime's fault attribution. Atomic cancellation reads must never be hoisted out
   of the loop, including when a parallel task inherits no time budget. */
static inline int cplrt_fast_tick(cplrt_cx *cx, uint32_t site)
{
    if (cx->deadline_ns != 0 ||
        (cx->parallel_failure != NULL && cx->parallel_index > atomic_load_explicit(cx->parallel_failure, memory_order_acquire)) ||
        (cx->cancel != NULL && atomic_load_explicit(cx->cancel, memory_order_relaxed) != 0u))
        return cplrt_tick(cx, site);
    return CPLRT_OK;
}

/* Optimized lowering's fast paths preserve the arbitrary-precision fallback and its fault site. */
static inline int cplrt_fast_add(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t n = cplrt_small_value(a) + cplrt_small_value(b);
        if (n >= CPLRT_SMALL_MIN && n <= CPLRT_SMALL_MAX) { CPLRT_SET(*out, cplrt_small(n)); return CPLRT_OK; }
    }
    return cplrt_int_add(cx, site, a, b, out);
}
static inline int cplrt_fast_sub(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t n = cplrt_small_value(a) - cplrt_small_value(b);
        if (n >= CPLRT_SMALL_MIN && n <= CPLRT_SMALL_MAX) { CPLRT_SET(*out, cplrt_small(n)); return CPLRT_OK; }
    }
    return cplrt_int_sub(cx, site, a, b, out);
}
static inline int cplrt_fast_mul(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
#if defined(__GNUC__) || defined(__clang__)
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t n;
        if (!__builtin_mul_overflow(cplrt_small_value(a), cplrt_small_value(b), &n) && n >= CPLRT_SMALL_MIN && n <= CPLRT_SMALL_MAX) {
            CPLRT_SET(*out, cplrt_small(n)); return CPLRT_OK;
        }
    }
#endif
    return cplrt_int_mul(cx, site, a, b, out);
}
static inline int cplrt_fast_div(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b) && cplrt_small_value(b) != 0) {
        /* Small operands cannot overflow int64_t division; MIN / -1 can exceed the small representation. */
        int64_t n = cplrt_small_value(a) / cplrt_small_value(b);
        if (n <= CPLRT_SMALL_MAX) { CPLRT_SET(*out, cplrt_small(n)); return CPLRT_OK; }
    }
    return cplrt_int_div(cx, site, a, b, out);
}
static inline int cplrt_fast_rem(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b) && cplrt_small_value(b) != 0) {
        CPLRT_SET(*out, cplrt_small(cplrt_small_value(a) % cplrt_small_value(b))); return CPLRT_OK;
    }
    return cplrt_int_rem(cx, site, a, b, out);
}
#define cplrt_list_push_owned CPLRT(list_push_owned)
int cplrt_list_push_owned(cplrt_cx *, uint32_t, uint32_t, cplrt_w *, cplrt_w);
#define cplrt_list_concat_owned CPLRT(list_concat_owned)
int cplrt_list_concat_owned(cplrt_cx *, cplrt_w *, cplrt_w);
#define cplrt_text_concat_owned CPLRT(text_concat_owned)
int cplrt_text_concat_owned(cplrt_cx *, cplrt_w *, cplrt_w);
#define cplrt_bytes_concat_owned CPLRT(bytes_concat_owned)
int cplrt_bytes_concat_owned(cplrt_cx *, cplrt_w *, cplrt_w);

#endif
