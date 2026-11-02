/* cpl.h: the CPL C API 0.1 (spec/cabi.md 4 and Appendix A.1). One file, identical in every package.
   Only fixed-width integers, size_t, and pointers appear in the ABI: no C enum, no bool, no bit-field, no long.
   Every function below is a static inline wrapper that calls the entry of the same name in the function table
   (struct cpl_api) that the object it takes carries as its first member, so a call always reaches the runtime
   copy that owns the object. No unwinding may cross a function of this header or a callback it registers:
   a callback returns normally (spec/cabi.md 4.7). */
#ifndef CPL_H
#define CPL_H 1
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* ---- versions --------------------------------------------------------------------------------------------- */
#define CPL_API_VERSION_MAJOR 0u
#define CPL_API_VERSION_MINOR 1u
#define CPL_API_VERSION ((CPL_API_VERSION_MAJOR << 16) | CPL_API_VERSION_MINOR)   /* usable in #if */

/* Layout assertions (spec/cabi.md 1.7): no ABI struct has implicit padding on an LP64 target, and this header and
   every generated header assert their structs' sizes there, so a #pragma pack left active cannot go unnoticed. */
#if defined(__cplusplus)
#if __cplusplus >= 201103L && defined(__LP64__)
#define CPL_LAYOUT_ASSERT(e) static_assert(e, "CPL ABI struct layout (spec/cabi.md 1.7)");
#endif
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L && defined(__LP64__)
#define CPL_LAYOUT_ASSERT(e) _Static_assert(e, "CPL ABI struct layout (spec/cabi.md 1.7)");
#endif
#ifndef CPL_LAYOUT_ASSERT
#define CPL_LAYOUT_ASSERT(e)
#endif

/* ---- statuses (spec/cabi.md 4.6) ---------------------------------------------------------------------------- */
typedef int32_t cpl_status;
#define CPL_OK             0   /* success; outputs set */
#define CPL_ERR            1   /* the export returned Err(e): *err_out holds e; no error object */
#define CPL_FAULT          2   /* an execution fault (semantics 6.7): *error holds its S49 diagnostic */
#define CPL_E_ARGUMENT     3   /* host misuse or ill-typed input: bad, released, or foreign handle; NULL; bad UTF-8 */
#define CPL_E_RANGE        4   /* a checked machine-integer conversion does not fit */
#define CPL_E_NOMEM        5   /* allocation failed outside a CPL evaluation */
#define CPL_E_VERSION      6   /* API version or struct size not supported */
#define CPL_E_STATE        7   /* runtime busy, still in use, or poisoned; an adapter that cannot be granted */
#define CPL_E_CANCELLED    8   /* the call observed its cancellation token */
#define CPL_E_UNSUPPORTED  9   /* option or feature not available in this library or edition */
#define CPL_E_INTERNAL    10   /* a runtime invariant failed: a toolchain bug; the runtime is poisoned */
#define CPL_FOREIGN_ERROR 11   /* only as a capability callback's return value, after cpl_call_foreign_error */
/* 12 is reserved for CPL_PENDING (asynchronous completion, a later minor version) */

/* ---- handles and plain types (spec/cabi.md 4.5) ------------------------------------------------------------- */
typedef struct cpl_value  { uint64_t bits[2]; } cpl_value;    /* owned value handle; {{0, 0}} is "no value" */
typedef struct cpl_cap    { uint64_t bits[2]; } cpl_cap;      /* owned capability handle */
typedef struct cpl_cancel { uint64_t bits[2]; } cpl_cancel;   /* owned cancellation token */
typedef uint32_t cpl_type;                          /* type id: well-known below, package ids from <prefix>_type_* */
typedef uint32_t cpl_kind;
#define CPL_KIND_INT     1u
#define CPL_KIND_BOOL    2u
#define CPL_KIND_TEXT    3u
#define CPL_KIND_BYTES   4u
#define CPL_KIND_LIST    5u
#define CPL_KIND_STRUCT  6u
#define CPL_KIND_ENUM    7u
#define CPL_KIND_WRAPPER 8u
typedef int32_t cpl_foreign_kind;                             /* std.foreign.ForeignErrorKind, declaration order */
#define CPL_FOREIGN_EXCEPTION        0
#define CPL_FOREIGN_INVALID_ARGUMENT 1
#define CPL_FOREIGN_INVALID_RESULT   2
#define CPL_FOREIGN_TRANSPORT        3
#define CPL_FOREIGN_TIMEOUT          4
#define CPL_FOREIGN_CANCELLED        5
#define CPL_FOREIGN_UNAVAILABLE      6
/* well-known type ids, equal in every package (declarations; instantiations such as List<Int> get package ids) */
#define CPL_T_INT                1u
#define CPL_T_BOOL               2u
#define CPL_T_TEXT               3u
#define CPL_T_BYTES              4u
#define CPL_T_UNIT               5u
#define CPL_T_OPTION             6u
#define CPL_T_RESULT             7u
#define CPL_T_CONSTRAINT_ERROR   8u
#define CPL_T_FOREIGN_ERROR      9u
#define CPL_T_FOREIGN_ERROR_KIND 10u
#define CPL_T_FOREIGN_DETAIL     11u
#define CPL_OPTION_SOME 0u                                    /* variant indices of the prelude enums */
#define CPL_OPTION_NONE 1u
#define CPL_RESULT_OK   0u
#define CPL_RESULT_ERR  1u
#define CPL_CONSTRAINT_ERROR_F_TYPE_NAME 0u                   /* field indices, declaration order */
#define CPL_CONSTRAINT_ERROR_F_MESSAGE   1u
#define CPL_FOREIGN_ERROR_F_KIND    0u
#define CPL_FOREIGN_ERROR_F_MESSAGE 1u
#define CPL_FOREIGN_ERROR_F_DETAILS 2u
#define CPL_FOREIGN_DETAIL_F_KEY    0u
#define CPL_FOREIGN_DETAIL_F_VALUE  1u
typedef struct cpl_strview   { const char *ptr; size_t len; } cpl_strview;      /* borrowed UTF-8; ptr[len] == '\0' */
typedef struct cpl_bytesview { const uint8_t *ptr; size_t len; } cpl_bytesview;  /* borrowed */
typedef struct cpl_buf       { char *ptr; size_t len; } cpl_buf;                 /* owned text; cpl_buf_release */
typedef struct cpl_api cpl_api;                                                  /* the function table (4.1) */
typedef struct cpl_runtime { const cpl_api *api; } cpl_runtime;                  /* only the first member is public */
typedef struct cpl_call    { const cpl_api *api; } cpl_call;                     /* a capability callback's context */
typedef struct cpl_error   { const cpl_api *api; } cpl_error;

/* ---- options, information, statistics (spec/cabi.md 4.2, 4.3, 4.8) ----------------------------------------- */
typedef struct cpl_allocator {    /* copied at open; called from any thread, concurrently (spec/cabi.md 4.7) */
    uint32_t struct_size;            /* sizeof(cpl_allocator) in the host's cpl.h */
    uint32_t reserved;               /* padding; never read */
    void *user;
    void *(*alloc)(void *user, size_t size, size_t align);                   /* NULL on failure */
    void  (*free)(void *user, void *ptr, size_t size, size_t align);
} cpl_allocator;
typedef struct cpl_runtime_options {
    uint32_t struct_size;            /* sizeof(cpl_runtime_options) in the host's cpl.h */
    uint32_t api_version;            /* CPL_API_VERSION in the host's cpl.h */
    const cpl_allocator *allocator;  /* NULL: the C library's malloc and free */
    uint64_t max_heap_bytes;         /* 0: unlimited; otherwise a heap budget (spec/cabi.md 2.4) */
    uint32_t max_call_depth;         /* semantics S38; 10000 in CPL_RUNTIME_OPTIONS_INIT; 0 refuses every call */
    uint32_t flags;                  /* none in 0.1; nonzero is CPL_E_UNSUPPORTED */
} cpl_runtime_options;
#define CPL_RUNTIME_OPTIONS_INIT { sizeof(cpl_runtime_options), CPL_API_VERSION, NULL, 0u, 10000u, 0u }
typedef struct cpl_call_options {
    uint32_t struct_size;            /* sizeof(cpl_call_options) */
    uint32_t flags;                  /* CPL_CALL_UNCHECKED_CONTRACTS is reserved for Milestone 5: CPL_E_UNSUPPORTED */
    uint64_t time_budget_ms;         /* 0: none; the budget of this top-level evaluation */
    uint64_t stack_bytes;            /* 0: the calling thread's own bounds */
    cpl_cancel cancel;               /* {{0, 0}}: none */
} cpl_call_options;
#define CPL_CALL_OPTIONS_INIT { sizeof(cpl_call_options), 0u, 0u, 0u, { { 0u, 0u } } }
#define CPL_CALL_UNCHECKED_CONTRACTS 1u
#define CPL_CLOSE_FORCE 1u
typedef struct cpl_library_info {
    uint32_t struct_size, api_version, abi, release;  /* release: built by cpl build --release */
    uint32_t holes, reserved;                         /* holes: unresolved holes linked (0 in a release build) */
    const char *package, *prefix, *edition, *toolchain, *runtime_version, *runtime_digest,
               *program_digest, *exports_digest, *target, *compiler, *profile;
} cpl_library_info;
typedef struct cpl_stats {
    uint32_t struct_size;            /* sizeof(cpl_stats) in the host's cpl.h, set before the call */
    uint32_t reserved;               /* padding; never read or written */
    uint64_t live_handles, live_caps, live_objects, live_bytes, peak_bytes, active_calls, live_buffers;
} cpl_stats;
#define CPL_STATS_INIT { sizeof(cpl_stats), 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u }

/* ---- capability callbacks (the generic form; <prefix>.h has a typed ops struct per capability; 5.7) -------- */
typedef cpl_status (*cpl_op_fn)(void *user, cpl_call *call,
                                const cpl_value *args, size_t nargs,    /* borrowed for the call */
                                cpl_value *result);                     /* owned; transferred on CPL_OK */
typedef struct cpl_cap_ops {
    uint32_t struct_size;
    uint32_t nops;                   /* must equal the capability's operation count */
    const cpl_op_fn *ops;            /* declaration order */
    void (*drop)(void *user);        /* optional; once, after the last reference is gone and no call is in flight */
} cpl_cap_ops;
CPL_LAYOUT_ASSERT(sizeof(cpl_value) == 16 && sizeof(cpl_cap) == 16 && sizeof(cpl_cancel) == 16)
CPL_LAYOUT_ASSERT(sizeof(cpl_strview) == 16 && sizeof(cpl_bytesview) == 16 && sizeof(cpl_buf) == 16)
CPL_LAYOUT_ASSERT(sizeof(cpl_allocator) == 32 && sizeof(cpl_runtime_options) == 32 && sizeof(cpl_call_options) == 40)
CPL_LAYOUT_ASSERT(sizeof(cpl_library_info) == 112 && sizeof(cpl_stats) == 64 && sizeof(cpl_cap_ops) == 24)

/* ---- the function table (spec/cabi.md 4.1): append-only within a major version ---------------------------- */
struct cpl_api {
    uint32_t struct_size;            /* sizeof(struct cpl_api) in the library */
    uint32_t api_version;            /* CPL_API_VERSION of the library */
    /* runtime */
    cpl_status (*cpl_runtime_close)(cpl_runtime *rt, uint32_t flags, cpl_error **error);
    const cpl_library_info *(*cpl_runtime_info)(cpl_runtime *rt);
    void       (*cpl_runtime_stats)(cpl_runtime *rt, cpl_stats *out);
    cpl_status (*cpl_type_lookup)(cpl_runtime *rt, const char *name, size_t len, cpl_type *out, cpl_error **error);
    cpl_status (*cpl_type_name)(cpl_runtime *rt, cpl_type type, cpl_strview *out, cpl_error **error);
    /* any value */
    cpl_status (*cpl_value_dup)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_value_release)(cpl_runtime *rt, cpl_value v);
    cpl_status (*cpl_value_kind)(cpl_runtime *rt, cpl_value v, cpl_kind *out, cpl_error **error);
    cpl_status (*cpl_value_type)(cpl_runtime *rt, cpl_value v, cpl_type *out, cpl_error **error);
    cpl_status (*cpl_value_equal)(cpl_runtime *rt, cpl_value a, cpl_value b, int32_t *out, cpl_error **error);
    cpl_status (*cpl_value_to_json)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
    cpl_status (*cpl_value_from_json)(cpl_runtime *rt, cpl_type type, const char *json, size_t len,
                                      cpl_value *out, cpl_error **error);
    cpl_status (*cpl_value_to_text)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
    void       (*cpl_buf_release)(cpl_runtime *rt, cpl_buf *buf);
    /* Int and Bool */
    cpl_status (*cpl_int_from_i64)(cpl_runtime *rt, int64_t n, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_from_u64)(cpl_runtime *rt, uint64_t n, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_from_decimal)(cpl_runtime *rt, const char *s, size_t len, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_from_words)(cpl_runtime *rt, int32_t negative, const uint64_t *words, size_t n,
                                     cpl_value *out, cpl_error **error);
    cpl_status (*cpl_int_to_i64)(cpl_runtime *rt, cpl_value v, int64_t *out, cpl_error **error);
    cpl_status (*cpl_int_to_u64)(cpl_runtime *rt, cpl_value v, uint64_t *out, cpl_error **error);
    cpl_status (*cpl_int_to_decimal)(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error);
    cpl_status (*cpl_int_to_words)(cpl_runtime *rt, cpl_value v, int32_t *negative, uint64_t *words, size_t cap,
                                   size_t *needed, cpl_error **error);
    cpl_status (*cpl_bool_new)(cpl_runtime *rt, int32_t b, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_bool_get)(cpl_runtime *rt, cpl_value v, int32_t *out, cpl_error **error);
    /* Text and Bytes */
    cpl_status (*cpl_text_new)(cpl_runtime *rt, const char *utf8, size_t len, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_text_view)(cpl_runtime *rt, cpl_value v, cpl_strview *out, cpl_error **error);
    cpl_status (*cpl_text_scalars)(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error);
    cpl_status (*cpl_bytes_new)(cpl_runtime *rt, const uint8_t *data, size_t len, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_bytes_view)(cpl_runtime *rt, cpl_value v, cpl_bytesview *out, cpl_error **error);
    /* List */
    cpl_status (*cpl_list_new)(cpl_runtime *rt, const cpl_value *items, size_t n, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_list_len)(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error);
    cpl_status (*cpl_list_get)(cpl_runtime *rt, cpl_value v, size_t index, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_list_from_i64)(cpl_runtime *rt, const int64_t *items, size_t n, cpl_value *out,
                                    cpl_error **error);
    cpl_status (*cpl_list_to_i64)(cpl_runtime *rt, cpl_value v, int64_t *buf, size_t cap, size_t *needed,
                                  cpl_error **error);
    /* struct, enum, wrapper, and the prelude values */
    cpl_status (*cpl_struct_new)(cpl_runtime *rt, cpl_type type, const cpl_value *fields, size_t n,
                                 cpl_value *out, cpl_error **error);
    cpl_status (*cpl_struct_field)(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_enum_new)(cpl_runtime *rt, cpl_type type, uint32_t variant, const cpl_value *payload, size_t n,
                               cpl_value *out, cpl_error **error);
    cpl_status (*cpl_enum_variant)(cpl_runtime *rt, cpl_value v, uint32_t *out, cpl_error **error);
    cpl_status (*cpl_enum_payload)(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_wrapper_new)(cpl_runtime *rt, cpl_type type, cpl_value inner, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_wrapper_inner)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_unit)(cpl_runtime *rt, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_option_some)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_option_none)(cpl_runtime *rt, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_result_ok)(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_result_err)(cpl_runtime *rt, cpl_value e, cpl_value *out, cpl_error **error);
    cpl_status (*cpl_foreign_error_new)(cpl_runtime *rt, cpl_foreign_kind kind, const char *message, size_t len,
                                        const cpl_strview *keys, const cpl_strview *values, size_t ndetails,
                                        cpl_value *out, cpl_error **error);
    /* cancellation */
    cpl_status (*cpl_cancel_new)(cpl_runtime *rt, cpl_cancel *out, cpl_error **error);
    cpl_status (*cpl_cancel_request)(cpl_runtime *rt, cpl_cancel token);
    cpl_status (*cpl_cancel_release)(cpl_runtime *rt, cpl_cancel token);
    /* capabilities and callbacks */
    cpl_status (*cpl_cap_new)(cpl_runtime *rt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                              cpl_cap *out, cpl_error **error);
    cpl_status (*cpl_cap_release)(cpl_runtime *rt, cpl_cap cap);
    cpl_runtime *(*cpl_call_runtime)(cpl_call *call);
    uint32_t   (*cpl_call_operation)(cpl_call *call);
    int32_t    (*cpl_call_cancelled)(cpl_call *call);
    cpl_status (*cpl_call_foreign_detail)(cpl_call *call, const char *key, size_t key_len,
                                          const char *value, size_t value_len);
    cpl_status (*cpl_call_foreign_error)(cpl_call *call, cpl_foreign_kind kind, const char *message, size_t len);
    /* errors */
    cpl_status  (*cpl_error_status)(const cpl_error *e);
    const char *(*cpl_error_code)(const cpl_error *e);
    const char *(*cpl_error_category)(const cpl_error *e);
    cpl_strview (*cpl_error_message)(const cpl_error *e);
    cpl_strview (*cpl_error_json)(const cpl_error *e);
    void        (*cpl_error_release)(cpl_error *e);
};
CPL_LAYOUT_ASSERT(sizeof(struct cpl_api) == 8 + 8 * 63)   /* the 63 entries of C API 0.1 */

/* True when the library's table has the entry `member` (for entries added after 0.1; every 0.1 entry is present). */
#define CPL_API_HAS(api, member) \
    ((api)->struct_size >= offsetof(struct cpl_api, member) + sizeof((api)->member))

/* ---- runtime ------------------------------------------------------------------------------------------------ */
/* opened by the package:
   cpl_status <prefix>_runtime_open(const cpl_runtime_options *, cpl_runtime **, cpl_error **) */
static inline cpl_status cpl_runtime_close(cpl_runtime *rt, uint32_t flags, cpl_error **error)
{ return rt->api->cpl_runtime_close(rt, flags, error); }
static inline const cpl_library_info *cpl_runtime_info(cpl_runtime *rt)
{ return rt->api->cpl_runtime_info(rt); }
/* writes the counters that out->struct_size covers: initialize *out with CPL_STATS_INIT */
static inline void cpl_runtime_stats(cpl_runtime *rt, cpl_stats *out)
{ rt->api->cpl_runtime_stats(rt, out); }
static inline cpl_status cpl_type_lookup(cpl_runtime *rt, const char *name, size_t len, cpl_type *out,
                                         cpl_error **error)
{ return rt->api->cpl_type_lookup(rt, name, len, out, error); }
static inline cpl_status cpl_type_name(cpl_runtime *rt, cpl_type type, cpl_strview *out, cpl_error **error)
{ return rt->api->cpl_type_name(rt, type, out, error); }

/* ---- any value ---------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_value_dup(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_value_dup(rt, v, out, error); }
/* CPL_E_ARGUMENT for a stale or foreign handle */
static inline cpl_status cpl_value_release(cpl_runtime *rt, cpl_value v)
{ return rt->api->cpl_value_release(rt, v); }
static inline cpl_status cpl_value_kind(cpl_runtime *rt, cpl_value v, cpl_kind *out, cpl_error **error)
{ return rt->api->cpl_value_kind(rt, v, out, error); }
/* struct, enum, wrapper */
static inline cpl_status cpl_value_type(cpl_runtime *rt, cpl_value v, cpl_type *out, cpl_error **error)
{ return rt->api->cpl_value_type(rt, v, out, error); }
static inline cpl_status cpl_value_equal(cpl_runtime *rt, cpl_value a, cpl_value b, int32_t *out, cpl_error **error)
{ return rt->api->cpl_value_equal(rt, a, b, out, error); }
/* value JSON, semantics S40 */
static inline cpl_status cpl_value_to_json(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ return rt->api->cpl_value_to_json(rt, v, out, error); }
/* strict, toolchain 7.3 */
static inline cpl_status cpl_value_from_json(cpl_runtime *rt, cpl_type type, const char *json, size_t len,
                                             cpl_value *out, cpl_error **error)
{ return rt->api->cpl_value_from_json(rt, type, json, len, out, error); }
/* value text, semantics S40 */
static inline cpl_status cpl_value_to_text(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ return rt->api->cpl_value_to_text(rt, v, out, error); }
static inline void cpl_buf_release(cpl_runtime *rt, cpl_buf *buf)
{ rt->api->cpl_buf_release(rt, buf); }

/* ---- Int and Bool ------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_int_from_i64(cpl_runtime *rt, int64_t n, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_int_from_i64(rt, n, out, error); }
static inline cpl_status cpl_int_from_u64(cpl_runtime *rt, uint64_t n, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_int_from_u64(rt, n, out, error); }
/* the grammar of stdlib.md S25 */
static inline cpl_status cpl_int_from_decimal(cpl_runtime *rt, const char *s, size_t len, cpl_value *out,
                                              cpl_error **error)
{ return rt->api->cpl_int_from_decimal(rt, s, len, out, error); }
/* little-endian */
static inline cpl_status cpl_int_from_words(cpl_runtime *rt, int32_t negative, const uint64_t *words, size_t n,
                                            cpl_value *out, cpl_error **error)
{ return rt->api->cpl_int_from_words(rt, negative, words, n, out, error); }
/* CPL_E_RANGE */
static inline cpl_status cpl_int_to_i64(cpl_runtime *rt, cpl_value v, int64_t *out, cpl_error **error)
{ return rt->api->cpl_int_to_i64(rt, v, out, error); }
/* CPL_E_RANGE */
static inline cpl_status cpl_int_to_u64(cpl_runtime *rt, cpl_value v, uint64_t *out, cpl_error **error)
{ return rt->api->cpl_int_to_u64(rt, v, out, error); }
static inline cpl_status cpl_int_to_decimal(cpl_runtime *rt, cpl_value v, cpl_buf *out, cpl_error **error)
{ return rt->api->cpl_int_to_decimal(rt, v, out, error); }
/* words NULL: count only */
static inline cpl_status cpl_int_to_words(cpl_runtime *rt, cpl_value v, int32_t *negative, uint64_t *words,
                                          size_t cap, size_t *needed, cpl_error **error)
{ return rt->api->cpl_int_to_words(rt, v, negative, words, cap, needed, error); }
/* 0 or 1 */
static inline cpl_status cpl_bool_new(cpl_runtime *rt, int32_t b, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_bool_new(rt, b, out, error); }
static inline cpl_status cpl_bool_get(cpl_runtime *rt, cpl_value v, int32_t *out, cpl_error **error)
{ return rt->api->cpl_bool_get(rt, v, out, error); }

/* ---- Text and Bytes ----------------------------------------------------------------------------------------- */
/* copied */
static inline cpl_status cpl_text_new(cpl_runtime *rt, const char *utf8, size_t len, cpl_value *out,
                                      cpl_error **error)
{ return rt->api->cpl_text_new(rt, utf8, len, out, error); }
/* until v is released */
static inline cpl_status cpl_text_view(cpl_runtime *rt, cpl_value v, cpl_strview *out, cpl_error **error)
{ return rt->api->cpl_text_view(rt, v, out, error); }
/* Text.len */
static inline cpl_status cpl_text_scalars(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error)
{ return rt->api->cpl_text_scalars(rt, v, out, error); }
static inline cpl_status cpl_bytes_new(cpl_runtime *rt, const uint8_t *data, size_t len, cpl_value *out,
                                       cpl_error **error)
{ return rt->api->cpl_bytes_new(rt, data, len, out, error); }
static inline cpl_status cpl_bytes_view(cpl_runtime *rt, cpl_value v, cpl_bytesview *out, cpl_error **error)
{ return rt->api->cpl_bytes_view(rt, v, out, error); }

/* ---- List --------------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_list_new(cpl_runtime *rt, const cpl_value *items, size_t n, cpl_value *out,
                                      cpl_error **error)
{ return rt->api->cpl_list_new(rt, items, n, out, error); }
static inline cpl_status cpl_list_len(cpl_runtime *rt, cpl_value v, size_t *out, cpl_error **error)
{ return rt->api->cpl_list_len(rt, v, out, error); }
static inline cpl_status cpl_list_get(cpl_runtime *rt, cpl_value v, size_t index, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_list_get(rt, v, index, out, error); }
static inline cpl_status cpl_list_from_i64(cpl_runtime *rt, const int64_t *items, size_t n, cpl_value *out,
                                           cpl_error **error)
{ return rt->api->cpl_list_from_i64(rt, items, n, out, error); }
/* buf NULL: count only */
static inline cpl_status cpl_list_to_i64(cpl_runtime *rt, cpl_value v, int64_t *buf, size_t cap, size_t *needed,
                                         cpl_error **error)
{ return rt->api->cpl_list_to_i64(rt, v, buf, cap, needed, error); }

/* ---- struct, enum, wrapper, and the prelude values ---------------------------------------------------------- */
/* declaration order */
static inline cpl_status cpl_struct_new(cpl_runtime *rt, cpl_type type, const cpl_value *fields, size_t n,
                                        cpl_value *out, cpl_error **error)
{ return rt->api->cpl_struct_new(rt, type, fields, n, out, error); }
static inline cpl_status cpl_struct_field(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out,
                                          cpl_error **error)
{ return rt->api->cpl_struct_field(rt, v, index, out, error); }
static inline cpl_status cpl_enum_new(cpl_runtime *rt, cpl_type type, uint32_t variant, const cpl_value *payload,
                                      size_t n, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_enum_new(rt, type, variant, payload, n, out, error); }
static inline cpl_status cpl_enum_variant(cpl_runtime *rt, cpl_value v, uint32_t *out, cpl_error **error)
{ return rt->api->cpl_enum_variant(rt, v, out, error); }
static inline cpl_status cpl_enum_payload(cpl_runtime *rt, cpl_value v, uint32_t index, cpl_value *out,
                                          cpl_error **error)
{ return rt->api->cpl_enum_payload(rt, v, index, out, error); }
static inline cpl_status cpl_wrapper_new(cpl_runtime *rt, cpl_type type, cpl_value inner, cpl_value *out,
                                         cpl_error **error)
{ return rt->api->cpl_wrapper_new(rt, type, inner, out, error); }
static inline cpl_status cpl_wrapper_inner(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_wrapper_inner(rt, v, out, error); }
static inline cpl_status cpl_unit(cpl_runtime *rt, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_unit(rt, out, error); }
static inline cpl_status cpl_option_some(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_option_some(rt, v, out, error); }
static inline cpl_status cpl_option_none(cpl_runtime *rt, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_option_none(rt, out, error); }
static inline cpl_status cpl_result_ok(cpl_runtime *rt, cpl_value v, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_result_ok(rt, v, out, error); }
static inline cpl_status cpl_result_err(cpl_runtime *rt, cpl_value e, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_result_err(rt, e, out, error); }
static inline cpl_status cpl_foreign_error_new(cpl_runtime *rt, cpl_foreign_kind kind, const char *message,
                                               size_t len, const cpl_strview *keys, const cpl_strview *values,
                                               size_t ndetails, cpl_value *out, cpl_error **error)
{ return rt->api->cpl_foreign_error_new(rt, kind, message, len, keys, values, ndetails, out, error); }

/* ---- cancellation ------------------------------------------------------------------------------------------- */
static inline cpl_status cpl_cancel_new(cpl_runtime *rt, cpl_cancel *out, cpl_error **error)
{ return rt->api->cpl_cancel_new(rt, out, error); }
/* any thread; not async-signal-safe (spec/cabi.md 4.4) */
static inline cpl_status cpl_cancel_request(cpl_runtime *rt, cpl_cancel token)
{ return rt->api->cpl_cancel_request(rt, token); }
static inline cpl_status cpl_cancel_release(cpl_runtime *rt, cpl_cancel token)
{ return rt->api->cpl_cancel_release(rt, token); }

/* ---- capabilities and callbacks ----------------------------------------------------------------------------- */
/* takes user only on success */
static inline cpl_status cpl_cap_new(cpl_runtime *rt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                                     cpl_cap *out, cpl_error **error)
{ return rt->api->cpl_cap_new(rt, capability, ops, user, out, error); }
static inline cpl_status cpl_cap_release(cpl_runtime *rt, cpl_cap cap)
{ return rt->api->cpl_cap_release(rt, cap); }
static inline cpl_runtime *cpl_call_runtime(cpl_call *call)
{ return call->api->cpl_call_runtime(call); }
/* operation index, declaration order */
static inline uint32_t cpl_call_operation(cpl_call *call)
{ return call->api->cpl_call_operation(call); }
/* the running call's token was requested */
static inline int32_t cpl_call_cancelled(cpl_call *call)
{ return call->api->cpl_call_cancelled(call); }
static inline cpl_status cpl_call_foreign_detail(cpl_call *call, const char *key, size_t key_len, const char *value,
                                                 size_t value_len)
{ return call->api->cpl_call_foreign_detail(call, key, key_len, value, value_len); }
/* returns CPL_FOREIGN_ERROR; CPL_E_ARGUMENT or CPL_E_NOMEM when the failure cannot be recorded */
static inline cpl_status cpl_call_foreign_error(cpl_call *call, cpl_foreign_kind kind, const char *message,
                                                size_t len)
{ return call->api->cpl_call_foreign_error(call, kind, message, len); }

/* ---- errors (usable after their runtime closed) ------------------------------------------------------------- */
static inline cpl_status cpl_error_status(const cpl_error *e)
{ return e->api->cpl_error_status(e); }
/* "contract_violation", "interop_handle_released", ... (static) */
static inline const char *cpl_error_code(const cpl_error *e)
{ return e->api->cpl_error_code(e); }
/* "contract", "interop", ... (static) */
static inline const char *cpl_error_category(const cpl_error *e)
{ return e->api->cpl_error_category(e); }
static inline cpl_strview cpl_error_message(const cpl_error *e)
{ return e->api->cpl_error_message(e); }
/* the S49 diagnostic object, compact JSON */
static inline cpl_strview cpl_error_json(const cpl_error *e)
{ return e->api->cpl_error_json(e); }
/* NULL and static error objects: no-op */
static inline void cpl_error_release(cpl_error *e)
{ if (e != NULL) e->api->cpl_error_release(e); }

#ifdef __cplusplus
}
#endif
#endif
