/* cpl_rt_api.c: the C API 0.1 (spec/cabi.md 4): the struct cpl_api table, the runtime lifecycle, error objects, the
   value, Int, Text, Bytes, List, composite, cancellation, capability, and callback functions, each checking its
   handles and arguments in the order of 4.5 and 4.8 and reporting through the statuses and interop codes of 4.6 and
   4.9. File-scope names begin with api_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- error objects (4.6) ------------------------------------------------------------------------------------------ */

typedef struct api_err {
    cpl_error pub;                 /* first: { api } */
    cpl_status status;
    uint32_t is_static;
    const char *code;              /* static */
    const char *category;          /* static */
    const char *message;
    size_t message_len;
    const char *json;
    size_t json_len;
    cpl_allocator allocator;       /* how the object is freed: allocator.alloc NULL is the C library's free */
    size_t size;
} api_err;

extern const cpl_api CPLRT(api_table);

#define API_NOMEM_JSON                                                                                                 \
    "{\"code\":\"interop_out_of_memory\",\"category\":\"interop\",\"severity\":\"error\",\"message\":\"memory was "   \
    "exhausted outside an evaluation\"}"
#define API_MEMFAULT_JSON                                                                                              \
    "{\"code\":\"resource_exhaustion\",\"category\":\"internal\",\"severity\":\"error\",\"message\":\"memory was "    \
    "exhausted\",\"resource\":\"memory\"}"
static const api_err api_nomem = { { &CPLRT(api_table) }, CPL_E_NOMEM, 1u, "interop_out_of_memory", "interop",
                                   "memory was exhausted outside an evaluation",
                                   sizeof "memory was exhausted outside an evaluation" - 1u, API_NOMEM_JSON,
                                   sizeof API_NOMEM_JSON - 1u, { 0u, 0u, NULL, NULL, NULL }, 0u };
static const api_err api_memfault = { { &CPLRT(api_table) }, CPL_FAULT, 1u, "resource_exhaustion", "internal",
                                      "memory was exhausted", sizeof "memory was exhausted" - 1u, API_MEMFAULT_JSON,
                                      sizeof API_MEMFAULT_JSON - 1u, { 0u, 0u, NULL, NULL, NULL }, 0u };

/* A new error object holding copies of the message and the JSON, allocated with the runtime's allocator (or malloc
   when rt is NULL) and freed through the copy of that allocator it keeps, so that it outlives its runtime (4.3).
   NULL when the allocation fails. */
static api_err *api_err_new(cplrt_runtime *rt, cpl_status status, const char *code, const char *category,
                            const char *message, size_t message_len, const char *json, size_t json_len)
{
    size_t size = sizeof(api_err) + message_len + 1u + json_len + 1u;
    void *raw;
    if (rt != NULL && rt->allocator.alloc != NULL)
        raw = rt->allocator.alloc(rt->allocator.user, size, _Alignof(max_align_t));
    else
        raw = malloc(size);
    if (raw == NULL)
        return NULL;
    api_err *e = (api_err *)raw;
    memset(e, 0, sizeof *e);
    e->pub.api = &CPLRT(api_table);
    e->status = status;
    e->code = code;
    e->category = category;
    char *m = (char *)(e + 1);
    memcpy(m, message, message_len);
    m[message_len] = '\0';
    char *j = m + message_len + 1u;
    memcpy(j, json, json_len);
    j[json_len] = '\0';
    e->message = m;
    e->message_len = message_len;
    e->json = j;
    e->json_len = json_len;
    if (rt != NULL)
        e->allocator = rt->allocator;
    e->size = size;
    return e;
}

static void api_err_release(cpl_error *pe)
{
    if (pe == NULL)
        return;
    api_err *e = (api_err *)(void *)pe;
    if (e->is_static)
        return;
    if (e->allocator.alloc != NULL) {
        if (e->allocator.free != NULL)
            e->allocator.free(e->allocator.user, e, e->size, _Alignof(max_align_t));
    } else {
        free(e);
    }
}

static cpl_status api_nomem_error(cpl_error **error)
{
    if (error != NULL)
        *error = (cpl_error *)(uintptr_t)&api_nomem.pub;
    return CPL_E_NOMEM;
}

/* A text buffer for an error object, before any runtime exists or outside the heap budget: the C library's. */
typedef struct api_text {
    char *ptr;
    size_t len, cap;
    int failed;
} api_text;

static void api_put(api_text *t, const char *s, size_t n)
{
    if (t->failed)
        return;
    if (t->len + n + 1u > t->cap) {
        size_t cap = t->cap == 0 ? 256u : t->cap;
        while (cap < t->len + n + 1u)
            cap *= 2u;
        char *grown = (char *)realloc(t->ptr, cap);
        if (grown == NULL) {
            t->failed = 1;
            return;
        }
        t->ptr = grown;
        t->cap = cap;
    }
    memcpy(t->ptr + t->len, s, n);
    t->len += n;
    t->ptr[t->len] = '\0';
}

static void api_puts(api_text *t, const char *s)
{
    api_put(t, s, strlen(s));
}

static void api_putu(api_text *t, uint64_t v)
{
    char digits[24];
    int k = 0;
    do {
        digits[k++] = (char)('0' + (int)(v % 10u));
        v /= 10u;
    } while (v != 0);
    char out[24];
    int j = 0;
    while (k > 0)
        out[j++] = digits[--k];
    api_put(t, out, (size_t)j);
}

/* A JSON string with serde_json's compact escapes (cpl_rt_json.c writes the same, into an sbuf). */
static void api_json_string(api_text *t, const char *s, size_t n)
{
    static const char hex[] = "0123456789abcdef";
    api_put(t, "\"", 1);
    size_t run = 0;
    for (size_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)s[i];
        const char *esc = NULL;
        char u[6];
        switch (c) {
        case '"': esc = "\\\""; break;
        case '\\': esc = "\\\\"; break;
        case '\b': esc = "\\b"; break;
        case '\t': esc = "\\t"; break;
        case '\n': esc = "\\n"; break;
        case '\f': esc = "\\f"; break;
        case '\r': esc = "\\r"; break;
        default:
            if (c < 0x20u) {
                u[0] = '\\';
                u[1] = 'u';
                u[2] = '0';
                u[3] = '0';
                u[4] = hex[c >> 4];
                u[5] = hex[c & 15u];
            }
            break;
        }
        if (esc == NULL && c >= 0x20u)
            continue;
        api_put(t, s + run, i - run);
        if (esc != NULL)
            api_puts(t, esc);
        else
            api_put(t, u, 6);
        run = i + 1u;
    }
    api_put(t, s + run, n - run);
    api_put(t, "\"", 1);
}

static const char *api_interop_message(uint32_t code)
{
    switch (code) {
    case CPLRT_INTEROP_VERSION_MISMATCH: return "the requested C API version is not supported by this library";
    case CPLRT_INTEROP_STRUCT_SIZE_UNSUPPORTED: return "a struct size is outside what this library supports";
    case CPLRT_INTEROP_HANDLE_INVALID: return "a handle is invalid";
    case CPLRT_INTEROP_HANDLE_RELEASED: return "a handle was used after it was released";
    case CPLRT_INTEROP_HANDLE_FOREIGN: return "a handle belongs to another runtime";
    case CPLRT_INTEROP_NULL_POINTER: return "a required pointer is NULL";
    case CPLRT_INTEROP_ARGUMENT_TYPE_MISMATCH: return "an argument is not a value of its declared type";
    case CPLRT_INTEROP_UTF8_INVALID: return "a text is not valid UTF-8";
    case CPLRT_INTEROP_BOOL_OUT_OF_RANGE: return "a Bool must be 0 or 1";
    case CPLRT_INTEROP_INT_OUT_OF_RANGE: return "an Int does not fit the machine integer type";
    case CPLRT_INTEROP_INDEX_OUT_OF_RANGE: return "an index is out of range";
    case CPLRT_INTEROP_VARIANT_OUT_OF_RANGE: return "a variant index is out of range";
    case CPLRT_INTEROP_ARITY_MISMATCH: return "the number of members does not match the declaration";
    case CPLRT_INTEROP_KIND_MISMATCH: return "a value or type is of another kind than required";
    case CPLRT_INTEROP_TYPE_UNKNOWN: return "the type is unknown to this package";
    case CPLRT_INTEROP_CAPABILITY_MISMATCH: return "a capability is of another type than the parameter";
    case CPLRT_INTEROP_JSON_INVALID: return "the JSON text does not decode as a value of the type";
    case CPLRT_INTEROP_RUNTIME_BUSY: return "the runtime has calls in flight";
    case CPLRT_INTEROP_RUNTIME_IN_USE: return "the runtime still has live handles, capabilities, or buffers";
    case CPLRT_INTEROP_RUNTIME_POISONED: return "the runtime is poisoned by an earlier internal error";
    case CPLRT_INTEROP_OPTION_UNSUPPORTED: return "an option is not supported in this edition";
    case CPLRT_INTEROP_OUT_OF_MEMORY: return "memory was exhausted outside an evaluation";
    case CPLRT_INTEROP_CANCELLED: return "the call was cancelled";
    case CPLRT_INTEROP_BIND_FAILED: return "the binding could not be granted";
    default: return "C API error";
    }
}

cpl_status CPLRT(error_interop)(cplrt_runtime *rt, cpl_error **error, uint32_t code, const cplrt_datum *data,
                                uint32_t n)
{
    return CPLRT(error_interop_note)(rt, error, code, data, n, NULL, 0);
}

cpl_status CPLRT(error_interop_note)(cplrt_runtime *rt, cpl_error **error, uint32_t code, const cplrt_datum *data,
                                     uint32_t n, const char *note, size_t note_len)
{
    cpl_status status = code < CPLRT_INTEROP_N ? (cpl_status)CPLRT_INTEROP[code].status : CPL_E_ARGUMENT;
    if (error == NULL)
        return status;
    const char *name = code < CPLRT_INTEROP_N ? CPLRT_INTEROP[code].code : "interop_internal";
    const char *message = api_interop_message(code);
    api_text t = { NULL, 0, 0, 0 };
    api_puts(&t, "{\"code\":");
    api_json_string(&t, name, strlen(name));
    api_puts(&t, ",\"category\":\"interop\",\"severity\":\"error\",\"message\":");
    api_json_string(&t, message, strlen(message));
    if (note != NULL) {                                 /* after message: no span, symbol, or facet (4.6) */
        api_puts(&t, ",\"notes\":[{\"message\":");
        api_json_string(&t, note, note_len);
        api_puts(&t, "}]");
    }
    /* the payload keys in byte order */
    uint32_t order[16];
    uint32_t m = n < 16u ? n : 16u;
    for (uint32_t i = 0; i < m; i++)
        order[i] = i;
    for (uint32_t i = 1; i < m; i++) {
        uint32_t x = order[i], j = i;
        while (j > 0 && strcmp(data[order[j - 1]].key, data[x].key) > 0) {
            order[j] = order[j - 1];
            j--;
        }
        order[j] = x;
    }
    for (uint32_t k = 0; k < m; k++) {
        const cplrt_datum *d = &data[order[k]];
        api_puts(&t, ",");
        api_json_string(&t, d->key, strlen(d->key));
        api_puts(&t, ":");
        switch (d->form) {
        case CPLRT_DATUM_STRING: api_json_string(&t, d->str != NULL ? d->str : "", d->str != NULL ? strlen(d->str) : 0); break;
        case CPLRT_DATUM_NUMBER: api_putu(&t, d->num); break;
        default: api_put(&t, d->json != NULL ? d->json : "null", d->json != NULL ? d->len : 4u); break;
        }
    }
    api_puts(&t, "}");
    if (t.failed) {
        free(t.ptr);
        return api_nomem_error(error);
    }
    api_err *e = api_err_new(rt, status, name, "interop", message, strlen(message), t.ptr, t.len);
    free(t.ptr);
    if (e == NULL)
        return api_nomem_error(error);
    *error = &e->pub;
    return status;
}

static const char *api_fault_code(uint32_t kind)
{
    static const char *const codes[8] = { "internal_error", "contract_violation", "assertion_failed",
                                          "unresolved_hole", "division_by_zero", "refinement_violation",
                                          "resource_exhaustion", "internal_error" };
    return kind < 8u ? codes[kind] : "internal_error";
}

static const char *api_fault_category(uint32_t kind)
{
    static const char *const cats[8] = { "internal", "contract", "test", "hole", "runtime", "runtime", "internal",
                                         "internal" };
    return kind < 8u ? cats[kind] : "internal";
}

cpl_status CPLRT(error_fault)(cplrt_runtime *rt, cpl_error **error, const cplrt_fault *f)
{
    if (error == NULL)
        return CPL_FAULT;
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(write_s49)(&b, rt->program, f);
    api_err *e = NULL;
    if (b.status == CPLRT_OK && b.ptr != NULL)
        e = api_err_new(rt, CPL_FAULT, api_fault_code(f->kind), api_fault_category(f->kind),
                        f->message != NULL ? f->message : "", f->message != NULL ? f->message_len : 0u, b.ptr, b.len);
    CPLRT(sbuf_free)(&b);
    if (e == NULL) {
        if (f->kind == CPLRT_FK_RESOURCE_EXHAUSTION && f->resource == CPLRT_RES_MEMORY) {
            *error = (cpl_error *)(uintptr_t)&api_memfault.pub;
            return CPL_FAULT;
        }
        return api_nomem_error(error);
    }
    *error = &e->pub;
    return CPL_FAULT;
}

cpl_status CPLRT(error_internal)(cplrt_runtime *rt, cpl_error **error, const char *detail)
{
    if (rt != NULL)
        atomic_store_explicit(&rt->poisoned, 1u, memory_order_relaxed);
    if (error == NULL)
        return CPL_E_INTERNAL;
    api_text m = { NULL, 0, 0, 0 }, t = { NULL, 0, 0, 0 };
    api_puts(&m, "internal error (runtime_invariant): ");
    api_puts(&m, detail);
    api_puts(&t, "{\"code\":\"internal_error\",\"category\":\"internal\",\"severity\":\"error\",\"message\":");
    if (!m.failed)
        api_json_string(&t, m.ptr, m.len);
    api_puts(&t, ",\"detail\":");
    api_json_string(&t, detail, strlen(detail));
    api_puts(&t, ",\"kind\":\"runtime_invariant\"}");
    api_err *e = NULL;
    if (!m.failed && !t.failed)
        e = api_err_new(rt, CPL_E_INTERNAL, "internal_error", "internal", m.ptr, m.len, t.ptr, t.len);
    free(m.ptr);
    free(t.ptr);
    if (e == NULL)
        return api_nomem_error(error);
    *error = &e->pub;
    return CPL_E_INTERNAL;
}

/* ---- small helpers for the payloads of 4.9 ---------------------------------------------------------------------- */

static cplrt_datum api_s(const char *key, const char *str)
{
    cplrt_datum d;
    memset(&d, 0, sizeof d);
    d.key = key;
    d.form = CPLRT_DATUM_STRING;
    d.str = str;
    return d;
}

static cplrt_datum api_n(const char *key, uint64_t num)
{
    cplrt_datum d;
    memset(&d, 0, sizeof d);
    d.key = key;
    d.form = CPLRT_DATUM_NUMBER;
    d.num = num;
    return d;
}

static cplrt_datum api_j(const char *key, const char *json)
{
    cplrt_datum d;
    memset(&d, 0, sizeof d);
    d.key = key;
    d.form = CPLRT_DATUM_JSON;
    d.json = (char *)(uintptr_t)json;
    d.len = strlen(json);
    return d;
}

static cpl_status api_err1(cplrt_runtime *rt, cpl_error **error, uint32_t code, const char *key, const char *str)
{
    cplrt_datum d = api_s(key, str);
    return CPLRT(error_interop)(rt, error, code, &d, 1);
}

static cpl_status api_null(cplrt_runtime *rt, cpl_error **error, const char *parameter)
{
    return api_err1(rt, error, CPLRT_INTEROP_NULL_POINTER, "parameter", parameter);
}

static void api_clear(cpl_error **error)
{
    if (error != NULL)
        *error = NULL;
}

static cplrt_runtime *api_rt(cpl_runtime *rt)
{
    return (cplrt_runtime *)(void *)rt;
}

/* A poisoned runtime refuses every status-returning function but the release functions and close (4.6). */
static int api_poisoned(cplrt_runtime *rt, cpl_error **error, cpl_status *st)
{
    if (atomic_load_explicit(&rt->poisoned, memory_order_relaxed)) {
        *st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_RUNTIME_POISONED, NULL, 0);
        return 1;
    }
    return 0;
}

/* The interop status of a failed handle check (4.5), with `parameter`. */
static cpl_status api_handle_error(cplrt_runtime *rt, cpl_error **error, int r, const char *parameter)
{
    uint32_t code = r == CPLRT_HANDLE_FOREIGN    ? CPLRT_INTEROP_HANDLE_FOREIGN
                    : r == CPLRT_HANDLE_RELEASED ? CPLRT_INTEROP_HANDLE_RELEASED
                                                 : CPLRT_INTEROP_HANDLE_INVALID;
    return api_err1(rt, error, code, "parameter", parameter);
}

/* The word of a value handle, retained for the caller (release it with cplrt_release), or the error status. */
static cpl_status api_value(cplrt_runtime *rt, cpl_value v, const char *parameter, cplrt_w *out, cpl_error **error)
{
    cplrt_slot e;
    int r = CPLRT(handle_get)(rt, v.bits, CPLRT_SLOT_VALUE, &e);
    if (r != CPLRT_HANDLE_OK)
        return api_handle_error(rt, error, r, parameter);
    *out = e.u.value;
    return CPL_OK;
}

/* Stores the owned word w in a new value handle; on failure releases w and reports CPL_E_NOMEM. */
static cpl_status api_new_handle(cplrt_runtime *rt, cplrt_w w, cpl_value *out, cpl_error **error)
{
    cplrt_slot e;
    memset(&e, 0, sizeof e);
    e.kind = CPLRT_SLOT_VALUE;
    e.u.value = w;
    if (CPLRT(handle_put)(rt, &e, out->bits) != CPLRT_OK) {
        cplrt_release(w);
        out->bits[0] = out->bits[1] = 0;
        return api_nomem_error(error);
    }
    api_clear(error);
    return CPL_OK;
}

static void api_zero(cpl_value *out)
{
    if (out != NULL)
        out->bits[0] = out->bits[1] = 0;
}

static const char *api_kind_word(cplrt_w w)
{
    if (cplrt_is_small(w))
        return "int";
    if (w == CPLRT_TRUE || w == CPLRT_FALSE)
        return "bool";
    if (!cplrt_is_obj(w))
        return "capability";
    switch (cplrt_obj_of(w)->kind) {
    case CPLRT_BIGINT: return "int";
    case CPLRT_TEXT: return "text";
    case CPLRT_BYTES: return "bytes";
    case CPLRT_LIST: return "list";
    case CPLRT_STRUCT: return "struct";
    case CPLRT_ENUM: return "enum";
    default: return "wrapper";
    }
}

static cpl_kind api_kind(cplrt_w w)
{
    if (cplrt_is_small(w))
        return CPL_KIND_INT;
    if (w == CPLRT_TRUE || w == CPLRT_FALSE)
        return CPL_KIND_BOOL;
    switch (cplrt_obj_of(w)->kind) {
    case CPLRT_BIGINT: return CPL_KIND_INT;
    case CPLRT_TEXT: return CPL_KIND_TEXT;
    case CPLRT_BYTES: return CPL_KIND_BYTES;
    case CPLRT_LIST: return CPL_KIND_LIST;
    case CPLRT_STRUCT: return CPL_KIND_STRUCT;
    case CPLRT_ENUM: return CPL_KIND_ENUM;
    default: return CPL_KIND_WRAPPER;
    }
}

static cpl_status api_kind_mismatch(cplrt_runtime *rt, cpl_error **error, const char *parameter, const char *expected,
                                    const char *found)
{
    cplrt_datum d[3];
    uint32_t n = 0;
    d[n++] = api_s("expected", expected);
    d[n++] = api_s("found", found);
    if (parameter != NULL)
        d[n++] = api_s("parameter", parameter);
    return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_KIND_MISMATCH, d, n);
}

static cpl_status api_utf8_error(cplrt_runtime *rt, cpl_error **error, const char *parameter, size_t offset)
{
    cplrt_datum d[2] = { api_n("offset", offset), api_s("parameter", parameter) };
    return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_UTF8_INVALID, d, 2);
}

/* ---- the runtime lifecycle (4.3) --------------------------------------------------------------------------------- */

/* The identity cells of this runtime copy: allocated with malloc, never freed, kept reachable (4.5). */
static void *api_identities;
static pthread_mutex_t api_identities_lock = PTHREAD_MUTEX_INITIALIZER;

static void *api_identity_new(void)
{
    void **cell = (void **)malloc(16);
    if (cell == NULL)
        return NULL;
    pthread_mutex_lock(&api_identities_lock);
    cell[0] = api_identities;
    cell[1] = NULL;
    api_identities = cell;
    pthread_mutex_unlock(&api_identities_lock);
    return cell;
}

static cpl_status api_struct_size(cpl_error **error, const char *name, uint64_t size, uint64_t minimum,
                                  uint64_t maximum)
{
    cplrt_datum d[4] = { api_s("struct", name), api_n("size", size), api_n("minimum", minimum),
                         api_n("maximum", maximum) };
    return CPLRT(error_interop)(NULL, error, CPLRT_INTEROP_STRUCT_SIZE_UNSUPPORTED, d, 4);
}

cpl_status cplrt_runtime_open(const cplrt_program *p, const cpl_runtime_options *options, cpl_runtime **out,
                              cpl_error **error)
{
    if (out == NULL)
        return api_null(NULL, error, "out");
    *out = NULL;
    cpl_runtime_options opts = CPL_RUNTIME_OPTIONS_INIT;
    if (options != NULL) {
        uint32_t size = options->struct_size;
        if (size < 8u)
            return api_struct_size(error, "cpl_runtime_options", size, 8u, sizeof(cpl_runtime_options));
        uint32_t major = options->api_version >> 16, minor = options->api_version & 0xFFFFu;
        if (major != CPL_API_VERSION_MAJOR || minor > CPL_API_VERSION_MINOR) {
            char lib[48], req[48];
            api_text a = { NULL, 0, 0, 0 };
            (void)a;
            snprintf(lib, sizeof lib, "{\"major\":%u,\"minor\":%u}", (unsigned)CPL_API_VERSION_MAJOR,
                     (unsigned)CPL_API_VERSION_MINOR);
            snprintf(req, sizeof req, "{\"major\":%u,\"minor\":%u}", (unsigned)major, (unsigned)minor);
            cplrt_datum d[2] = { api_j("library", lib), api_j("requested", req) };
            return CPLRT(error_interop)(NULL, error, CPLRT_INTEROP_VERSION_MISMATCH, d, 2);
        }
        if (size > sizeof(cpl_runtime_options))
            return api_struct_size(error, "cpl_runtime_options", size, 8u, sizeof(cpl_runtime_options));
        if (size >= offsetof(cpl_runtime_options, allocator) + sizeof options->allocator)
            opts.allocator = options->allocator;
        if (size >= offsetof(cpl_runtime_options, max_heap_bytes) + sizeof options->max_heap_bytes)
            opts.max_heap_bytes = options->max_heap_bytes;
        if (size >= offsetof(cpl_runtime_options, max_call_depth) + sizeof options->max_call_depth)
            opts.max_call_depth = options->max_call_depth;
        if (size >= offsetof(cpl_runtime_options, flags) + sizeof options->flags)
            opts.flags = options->flags;
        if (opts.flags != 0u)
            return api_err1(NULL, error, CPLRT_INTEROP_OPTION_UNSUPPORTED, "option", "flags");
        if (opts.allocator != NULL) {
            if (opts.allocator->struct_size < sizeof(cpl_allocator) || opts.allocator->struct_size > sizeof(cpl_allocator))
                return api_struct_size(error, "cpl_allocator", opts.allocator->struct_size, sizeof(cpl_allocator),
                                       sizeof(cpl_allocator));
            if (opts.allocator->alloc == NULL)
                return api_null(NULL, error, "options->allocator->alloc");
            if (opts.allocator->free == NULL)
                return api_null(NULL, error, "options->allocator->free");
        }
    }
    cplrt_runtime *rt;
    if (opts.allocator != NULL)
        rt = (cplrt_runtime *)opts.allocator->alloc(opts.allocator->user, sizeof *rt, _Alignof(max_align_t));
    else
        rt = (cplrt_runtime *)malloc(sizeof *rt);
    if (rt == NULL)
        return api_nomem_error(error);
    memset(rt, 0, sizeof *rt);
    rt->pub.api = &CPLRT(api_table);
    rt->program = p;
    rt->identity = api_identity_new();
    if (rt->identity == NULL) {
        if (opts.allocator != NULL)
            opts.allocator->free(opts.allocator->user, rt, sizeof *rt, _Alignof(max_align_t));
        else
            free(rt);
        return api_nomem_error(error);
    }
    if (opts.allocator != NULL)
        rt->allocator = *opts.allocator;
    rt->max_heap_bytes = opts.max_heap_bytes;
    atomic_init(&rt->live_bytes, 0u);
    atomic_init(&rt->peak_bytes, 0u);
    atomic_init(&rt->live_objects, 0u);
    atomic_init(&rt->active_calls, 0u);
    atomic_init(&rt->poisoned, 0u);
    rt->max_call_depth = opts.max_call_depth;
    rt->flags = opts.flags;
    pthread_mutex_init(&rt->lock, NULL);
    rt->free_head = CPLRT_NONE;
    rt->free_tail = CPLRT_NONE;
    *out = &rt->pub;
    api_clear(error);
    return CPL_OK;
}

/* The buffers handed out by the API (4.5): each block begins with this link, so that a forced close frees them. */
typedef struct api_buf {
    struct api_buf *prev, *next;
    size_t len;
} api_buf;
static api_buf *api_buffers_of(cplrt_runtime *rt)
{
    return (api_buf *)rt->buffers;
}

static cpl_status api_runtime_close(cpl_runtime *prt, uint32_t flags, cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    if (flags != 0u && flags != CPL_CLOSE_FORCE)
        return api_err1(rt, error, CPLRT_INTEROP_OPTION_UNSUPPORTED, "option", "flags");
    uint32_t active = atomic_load(&rt->active_calls);
    if (active != 0u) {
        cplrt_datum d = api_n("active_calls", active);
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_RUNTIME_BUSY, &d, 1);
    }
    pthread_mutex_lock(&rt->lock);
    uint64_t handles = rt->live_handles, caps = rt->live_caps, buffers = rt->live_buffers;
    pthread_mutex_unlock(&rt->lock);
    if (flags == 0u && (handles != 0u || caps != 0u || buffers != 0u)) {
        cplrt_datum d[3] = { api_n("live_handles", handles), api_n("live_caps", caps),
                             api_n("live_buffers", buffers) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_RUNTIME_IN_USE, d, 3);
    }
    /* every outstanding drop and fini, in reverse order of creation, while the runtime is still usable */
    CPLRT(caps_drop_all)(rt);
    /* then every live handle, then every live buffer */
    for (uint32_t i = 0; i < rt->nslots; i++) {
        cplrt_slot s = rt->slots[i];
        if (s.kind == CPLRT_SLOT_VALUE || s.kind == CPLRT_SLOT_CAP || s.kind == CPLRT_SLOT_CANCEL) {
            rt->slots[i].kind = CPLRT_SLOT_FREE;
            CPLRT(slot_unref)(&s);
        }
    }
    api_buf *b = api_buffers_of(rt);
    while (b != NULL) {
        api_buf *next = b->next;
        CPLRT(free)(b);
        b = next;
    }
    CPLRT(free)(rt->slots);
    rt->slots = NULL;
    CPLRT(workers_fini)(rt);
    pthread_mutex_destroy(&rt->lock);
    cpl_allocator a = rt->allocator;
    if (a.alloc != NULL)
        a.free(a.user, rt, sizeof *rt, _Alignof(max_align_t));
    else
        free(rt);
    api_clear(error);
    return CPL_OK;
}

static const cpl_library_info *api_runtime_info(cpl_runtime *prt)
{
    return &api_rt(prt)->program->info;
}

static void api_runtime_stats(cpl_runtime *prt, cpl_stats *out)
{
    cplrt_runtime *rt = api_rt(prt);
    if (out == NULL)
        return;
    cpl_stats s;
    memset(&s, 0, sizeof s);
    pthread_mutex_lock(&rt->lock);
    s.live_handles = rt->live_handles;
    s.live_caps = rt->live_caps;
    s.live_buffers = rt->live_buffers;
    pthread_mutex_unlock(&rt->lock);
    s.live_objects = atomic_load(&rt->live_objects);
    s.live_bytes = atomic_load(&rt->live_bytes);
    s.peak_bytes = atomic_load(&rt->peak_bytes);
    s.active_calls = atomic_load(&rt->active_calls);
    uint32_t size = out->struct_size;
    /* only the members the host's struct_size covers (4.2); struct_size and reserved are never written */
#define API_STAT(member)                                                                                               \
    if (size >= offsetof(cpl_stats, member) + sizeof s.member)                                                         \
        out->member = s.member;
    API_STAT(live_handles)
    API_STAT(live_caps)
    API_STAT(live_objects)
    API_STAT(live_bytes)
    API_STAT(peak_bytes)
    API_STAT(active_calls)
    API_STAT(live_buffers)
#undef API_STAT
}

static cpl_status api_type_lookup(cpl_runtime *prt, const char *name, size_t len, cpl_type *out, cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    cpl_status st;
    if (api_poisoned(rt, error, &st))
        return st;
    if (out == NULL)
        return api_null(rt, error, "out");
    if (name == NULL && len > 0)
        return api_null(rt, error, "name");
    const cplrt_program *p = rt->program;
    for (uint32_t i = 1; i < p->ntypes; i++) {
        const char *t = p->types[i].text;
        if (t != NULL && strlen(t) == len && (len == 0 || memcmp(t, name, len) == 0)) {
            *out = i;
            api_clear(error);
            return CPL_OK;
        }
    }
    /* the name, as text (it may not be NUL-terminated) */
    char buf[256];
    size_t n = len < sizeof buf - 1u ? len : sizeof buf - 1u;
    if (n > 0)
        memcpy(buf, name, n);
    buf[n] = '\0';
    return api_err1(rt, error, CPLRT_INTEROP_TYPE_UNKNOWN, "name", buf);
}

static cpl_status api_type_unknown(cplrt_runtime *rt, cpl_error **error, cpl_type type)
{
    cplrt_datum d = api_n("type", type);
    return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_TYPE_UNKNOWN, &d, 1);
}

static cpl_status api_type_name(cpl_runtime *prt, cpl_type type, cpl_strview *out, cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    cpl_status st;
    if (api_poisoned(rt, error, &st))
        return st;
    if (out == NULL)
        return api_null(rt, error, "out");
    if (type == 0 || type >= rt->program->ntypes)
        return api_type_unknown(rt, error, type);
    out->ptr = rt->program->types[type].text;
    out->len = strlen(out->ptr);
    api_clear(error);
    return CPL_OK;
}

/* ---- any value (4.8) ----------------------------------------------------------------------------------------------- */

#define API_BEGIN(out_check)                                                                                           \
    cplrt_runtime *rt = api_rt(prt);                                                                                   \
    cpl_status st_;                                                                                                    \
    if (api_poisoned(rt, error, &st_))                                                                                 \
        return st_;                                                                                                    \
    out_check

static cpl_status api_value_dup(cpl_runtime *prt, cpl_value v, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    return api_new_handle(rt, w, out, error);
}

static cpl_status api_value_release(cpl_runtime *prt, cpl_value v)
{
    cplrt_runtime *rt = api_rt(prt);
    if (v.bits[0] == 0 && v.bits[1] == 0)
        return CPL_OK;
    int r = CPLRT(handle_release)(rt, v.bits, CPLRT_SLOT_VALUE);
    return r == CPLRT_HANDLE_OK ? CPL_OK : CPL_E_ARGUMENT;
}

static cpl_status api_value_kind(cpl_runtime *prt, cpl_value v, cpl_kind *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    *out = api_kind(w);
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

/* The type id of a declaration: the table entry of its form whose text names the declaration alone. */
static uint32_t api_decl_type(const cplrt_program *p, uint32_t def)
{
    if (def >= p->ndefs)
        return 0;
    const char *name = p->defs[def].type_name;
    for (uint32_t i = 1; i < p->ntypes; i++) {
        const cplrt_type *t = &p->types[i];
        if ((t->form == CPLRT_TY_STRUCT || t->form == CPLRT_TY_ENUM || t->form == CPLRT_TY_WRAPPER) && t->def == def &&
            strcmp(t->text, name) == 0)
            return i;
    }
    for (uint32_t i = 1; i < p->ntypes; i++) {
        const cplrt_type *t = &p->types[i];
        if ((t->form == CPLRT_TY_STRUCT || t->form == CPLRT_TY_ENUM || t->form == CPLRT_TY_WRAPPER) && t->def == def)
            return i;
    }
    return 0;
}

static cpl_status api_value_type(cpl_runtime *prt, cpl_value v, cpl_type *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    cpl_kind k = api_kind(w);
    if (k != CPL_KIND_STRUCT && k != CPL_KIND_ENUM && k != CPL_KIND_WRAPPER) {
        st = api_kind_mismatch(rt, error, "v", "struct", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    uint32_t id = api_decl_type(rt->program, cplrt_obj_of(w)->u.live.def);
    cplrt_release(w);
    if (id == 0)
        return api_type_unknown(rt, error, 0);
    *out = id;
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_value_equal(cpl_runtime *prt, cpl_value a, cpl_value b, int32_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w x = 0, y = 0;
    cpl_status st = api_value(rt, a, "a", &x, error);
    if (st != CPL_OK)
        return st;
    st = api_value(rt, b, "b", &y, error);
    if (st != CPL_OK) {
        cplrt_release(x);
        return st;
    }
    int eq = 0;
    int r = CPLRT(value_equal)(rt, x, y, &eq);
    cplrt_release(x);
    cplrt_release(y);
    if (r != CPLRT_OK)
        return api_nomem_error(error);
    *out = eq ? 1 : 0;
    api_clear(error);
    return CPL_OK;
}

/* A buffer of the API (4.5): the text of b copied into a block linked into the runtime's buffers. */
static cpl_status api_buffer(cplrt_runtime *rt, const char *text, size_t len, cpl_buf *out, cpl_error **error)
{
    api_buf *blk = (api_buf *)CPLRT(alloc)(rt, sizeof(api_buf) + len + 1u);
    if (blk == NULL)
        return api_nomem_error(error);
    char *s = (char *)(blk + 1);
    if (len > 0)
        memcpy(s, text, len);
    s[len] = '\0';
    blk->len = len;
    pthread_mutex_lock(&rt->lock);
    blk->prev = NULL;
    blk->next = (api_buf *)rt->buffers;
    if (blk->next != NULL)
        blk->next->prev = blk;
    rt->buffers = blk;
    rt->live_buffers++;
    pthread_mutex_unlock(&rt->lock);
    out->ptr = s;
    out->len = len;
    api_clear(error);
    return CPL_OK;
}

static void api_buf_release(cpl_runtime *prt, cpl_buf *buf)
{
    cplrt_runtime *rt = api_rt(prt);
    if (buf == NULL || buf->ptr == NULL)
        return;
    api_buf *blk = (api_buf *)(void *)(buf->ptr - sizeof(api_buf));
    pthread_mutex_lock(&rt->lock);
    if (blk->prev != NULL)
        blk->prev->next = blk->next;
    else
        rt->buffers = blk->next;
    if (blk->next != NULL)
        blk->next->prev = blk->prev;
    rt->live_buffers--;
    pthread_mutex_unlock(&rt->lock);
    CPLRT(free)(blk);
    buf->ptr = NULL;
    buf->len = 0;
}

static cpl_status api_render(cpl_runtime *prt, cpl_value v, cpl_buf *out, cpl_error **error, int text)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    out->ptr = NULL;
    out->len = 0;
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_put)(&b, "", 0);
    if (text)
        CPLRT(write_value_text)(&b, rt->program, w);
    else
        CPLRT(write_value_json)(&b, rt->program, w);
    cplrt_release(w);
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        return api_nomem_error(error);
    }
    st = api_buffer(rt, b.ptr, b.len, out, error);
    CPLRT(sbuf_free)(&b);
    return st;
}

static cpl_status api_value_to_json(cpl_runtime *prt, cpl_value v, cpl_buf *out, cpl_error **error)
{
    return api_render(prt, v, out, error, 0);
}

static cpl_status api_value_to_text(cpl_runtime *prt, cpl_value v, cpl_buf *out, cpl_error **error)
{
    return api_render(prt, v, out, error, 1);
}

/* Whether a type has value JSON: no capability and no type parameter anywhere in it. */
static int api_has_json(const cplrt_program *p, uint32_t type)
{
    uint32_t stack[64];
    uint32_t n = 0;
    stack[n++] = type;
    uint32_t visited = 0;
    while (n > 0 && visited < 100000u) {
        visited++;
        uint32_t t = stack[--n];
        if (t == 0 || t >= p->ntypes)
            return 0;
        const cplrt_type *ty = &p->types[t];
        switch (ty->form) {
        case CPLRT_TY_CAPABILITY:
        case CPLRT_TY_PARAM: return 0;
        case CPLRT_TY_LIST:
        case CPLRT_TY_WRAPPER:
        case CPLRT_TY_REFINED:
            if (ty->elem != t && n < 64u)
                stack[n++] = ty->elem;
            break;
        case CPLRT_TY_STRUCT:
            for (uint32_t i = 0; i < ty->nmembers && n < 64u; i++)
                if (ty->members[i] != t)
                    stack[n++] = ty->members[i];
            break;
        case CPLRT_TY_ENUM:
            if (ty->members != NULL)
                for (uint32_t i = 0; i < ty->members[ty->nmembers] && n < 64u; i++)
                    if (ty->payloads[i] != t)
                        stack[n++] = ty->payloads[i];
            break;
        default: break;
        }
    }
    return 1;
}

static cpl_status api_value_from_json(cpl_runtime *prt, cpl_type type, const char *json, size_t len, cpl_value *out,
                                      cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (json == NULL && len > 0)
        return api_null(rt, error, "json");
    if (type == 0 || type >= rt->program->ntypes)
        return api_type_unknown(rt, error, type);
    if (!api_has_json(rt->program, type))
        return api_kind_mismatch(rt, error, "type", "a type with value JSON",
                                 rt->program->types[type].form == CPLRT_TY_CAPABILITY ? "capability" : "generic");
    cplrt_sbuf pointer, detail;
    cplrt_sbuf_init(&pointer, rt);
    cplrt_sbuf_init(&detail, rt);
    cplrt_w w = 0;
    int r = CPLRT(read_value_json)(rt, type, json != NULL ? json : "", len, &w, &pointer, &detail);
    if (r == CPLRT_NOMEM) {
        CPLRT(sbuf_free)(&pointer);
        CPLRT(sbuf_free)(&detail);
        return api_nomem_error(error);
    }
    if (r != CPLRT_OK) {
        cplrt_datum d[2] = { api_s("pointer", pointer.ptr != NULL ? pointer.ptr : ""),
                             api_s("detail", detail.ptr != NULL ? detail.ptr : "") };
        cpl_status st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_JSON_INVALID, d, 2);
        CPLRT(sbuf_free)(&pointer);
        CPLRT(sbuf_free)(&detail);
        return st;
    }
    CPLRT(sbuf_free)(&pointer);
    CPLRT(sbuf_free)(&detail);
    return api_new_handle(rt, w, out, error);
}

/* ---- Int and Bool ---------------------------------------------------------------------------------------------- */

static cpl_status api_int_result(cplrt_runtime *rt, int r, cplrt_w w, cpl_value *out, cpl_error **error)
{
    if (r != CPLRT_OK)
        return api_nomem_error(error);
    return api_new_handle(rt, w, out, error);
}

static cpl_status api_int_from_i64(cpl_runtime *prt, int64_t n, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    int r_ = CPLRT(int_from_i64)(rt, n, &w);
    return api_int_result(rt, r_, w, out, error);
}

static cpl_status api_int_from_u64(cpl_runtime *prt, uint64_t n, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    int r_ = CPLRT(int_from_u64)(rt, n, &w);
    return api_int_result(rt, r_, w, out, error);
}

static cpl_status api_int_from_decimal(cpl_runtime *prt, const char *s, size_t len, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (s == NULL && len > 0)
        return api_null(rt, error, "s");
    cplrt_w w = 0;
    int r = CPLRT(int_parse)(rt, s != NULL ? s : "", len, &w);
    if (r == CPLRT_INVALID) {
        cplrt_sbuf q;
        cplrt_sbuf_init(&q, rt);
        CPLRT(sbuf_put)(&q, "", 0);
        size_t valid = 0;
        if (len == 0 || CPLRT(utf8_valid)(s, len, &valid, NULL))
            CPLRT(write_quote_text)(&q, s != NULL ? s : "", len);
        else
            CPLRT(write_quote_text)(&q, s, valid);
        cplrt_datum d[4] = { api_s("parameter", "s"), api_s("pointer", ""), api_s("expected", "Int"),
                             api_s("found", q.ptr != NULL ? q.ptr : "") };
        cpl_status st = q.status == CPLRT_OK ? CPLRT(error_interop)(rt, error, CPLRT_INTEROP_ARGUMENT_TYPE_MISMATCH, d, 4)
                                             : api_nomem_error(error);
        CPLRT(sbuf_free)(&q);
        return st;
    }
    return api_int_result(rt, r, w, out, error);
}

static cpl_status api_int_from_words(cpl_runtime *prt, int32_t negative, const uint64_t *words, size_t n,
                                     cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (negative != 0 && negative != 1) {
        char found[16];
        snprintf(found, sizeof found, "%d", (int)negative);
        cplrt_datum d[2] = { api_s("parameter", "negative"), api_j("found", found) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_BOOL_OUT_OF_RANGE, d, 2);
    }
    if (words == NULL && n > 0)
        return api_null(rt, error, "words");
    cplrt_w w = 0;
    int r_ = CPLRT(int_from_words)(rt, negative, words, n, &w);
    return api_int_result(rt, r_, w, out, error);
}

static cpl_status api_int_range(cplrt_runtime *rt, cpl_error **error, cplrt_w w, const char *type, const char *position)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_put)(&b, "", 0);
    CPLRT(int_write)(&b, w);
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        return api_nomem_error(error);
    }
    cplrt_datum d[3] = { api_s("value", b.ptr), api_s("type", type), api_s("position", position) };
    cpl_status st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_INT_OUT_OF_RANGE, d, 3);
    CPLRT(sbuf_free)(&b);
    return st;
}

static int api_is_int(cplrt_w w)
{
    return cplrt_is_small(w) || (cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_BIGINT);
}

static cpl_status api_int_to_i64(cpl_runtime *prt, cpl_value v, int64_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!api_is_int(w)) {
        st = api_kind_mismatch(rt, error, "v", "int", api_kind_word(w));
    } else if (!CPLRT(int_to_i64)(w, out)) {
        st = api_int_range(rt, error, w, "int64", "out");
    } else {
        api_clear(error);
        st = CPL_OK;
    }
    cplrt_release(w);
    return st;
}

static cpl_status api_int_to_u64(cpl_runtime *prt, cpl_value v, uint64_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!api_is_int(w)) {
        st = api_kind_mismatch(rt, error, "v", "int", api_kind_word(w));
    } else if (!CPLRT(int_to_u64)(w, out)) {
        st = api_int_range(rt, error, w, "uint64", "out");
    } else {
        api_clear(error);
        st = CPL_OK;
    }
    cplrt_release(w);
    return st;
}

static cpl_status api_int_to_decimal(cpl_runtime *prt, cpl_value v, cpl_buf *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    out->ptr = NULL;
    out->len = 0;
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!api_is_int(w)) {
        st = api_kind_mismatch(rt, error, "v", "int", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_put)(&b, "", 0);
    CPLRT(int_write)(&b, w);
    cplrt_release(w);
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        return api_nomem_error(error);
    }
    st = api_buffer(rt, b.ptr, b.len, out, error);
    CPLRT(sbuf_free)(&b);
    return st;
}

static cpl_status api_int_to_words(cpl_runtime *prt, cpl_value v, int32_t *negative, uint64_t *words, size_t cap,
                                   size_t *needed, cpl_error **error)
{
    API_BEGIN(;)
    if (negative == NULL)
        return api_null(rt, error, "negative");
    if (needed == NULL)
        return api_null(rt, error, "needed");
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!api_is_int(w)) {
        st = api_kind_mismatch(rt, error, "v", "int", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    int neg = 0;
    size_t need = CPLRT(int_to_words)(w, &neg, NULL, 0);
    *needed = need;
    *negative = neg;
    if (words != NULL) {
        if (cap < need) {
            cplrt_release(w);
            cplrt_datum d[2] = { api_n("index", need - 1u), api_n("len", cap) };
            return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_INDEX_OUT_OF_RANGE, d, 2);
        }
        (void)CPLRT(int_to_words)(w, &neg, words, cap);
    }
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_bool_new(cpl_runtime *prt, int32_t b, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (b != 0 && b != 1) {
        char found[16];
        snprintf(found, sizeof found, "%d", (int)b);
        cplrt_datum d[2] = { api_s("parameter", "b"), api_j("found", found) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_BOOL_OUT_OF_RANGE, d, 2);
    }
    return api_new_handle(rt, cplrt_bool(b), out, error);
}

static cpl_status api_bool_get(cpl_runtime *prt, cpl_value v, int32_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (w != CPLRT_TRUE && w != CPLRT_FALSE) {
        st = api_kind_mismatch(rt, error, "v", "bool", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    *out = w == CPLRT_TRUE;
    api_clear(error);
    return CPL_OK;
}

/* ---- Text and Bytes -------------------------------------------------------------------------------------------- */

static cpl_status api_text_new(cpl_runtime *prt, const char *utf8, size_t len, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (utf8 == NULL && len > 0)
        return api_null(rt, error, "utf8");
    size_t valid = 0;
    if (len > 0 && !CPLRT(utf8_valid)(utf8, len, &valid, NULL))
        return api_utf8_error(rt, error, "utf8", valid);
    cplrt_w w = 0;
    if (CPLRT(text_make)(rt, utf8 != NULL ? utf8 : "", len, &w) != CPLRT_OK)
        return api_nomem_error(error);
    return api_new_handle(rt, w, out, error);
}

/* A view borrows from the object, which the handle keeps alive (4.5): the retain of handle_get is dropped at once. */
static cpl_status api_text_view(cpl_runtime *prt, cpl_value v, cpl_strview *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_TEXT) {
        st = api_kind_mismatch(rt, error, "v", "text", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    out->ptr = cplrt_text_of(w)->utf8;
    out->len = cplrt_text_of(w)->bytes;
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_text_scalars(cpl_runtime *prt, cpl_value v, size_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_TEXT) {
        st = api_kind_mismatch(rt, error, "v", "text", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    *out = cplrt_text_of(w)->scalars;
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_bytes_new(cpl_runtime *prt, const uint8_t *data, size_t len, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (data == NULL && len > 0)
        return api_null(rt, error, "data");
    cplrt_w w = 0;
    if (CPLRT(bytes_make)(rt, data, len, &w) != CPLRT_OK)
        return api_nomem_error(error);
    return api_new_handle(rt, w, out, error);
}

static cpl_status api_bytes_view(cpl_runtime *prt, cpl_value v, cpl_bytesview *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_BYTES) {
        st = api_kind_mismatch(rt, error, "v", "bytes", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    out->ptr = cplrt_bytes_of(w)->data;
    out->len = cplrt_bytes_of(w)->len;
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

/* ---- List -------------------------------------------------------------------------------------------------------- */

/* The words of n handles, each retained (release them with api_release_all), or the error of the first bad one. */
static cpl_status api_words(cplrt_runtime *rt, const cpl_value *items, size_t n, const char *name, cplrt_w *words,
                            cpl_error **error)
{
    char param[48];
    for (size_t i = 0; i < n; i++) {
        snprintf(param, sizeof param, "%s[%zu]", name, i);
        cpl_status st = api_value(rt, items[i], param, &words[i], error);
        if (st != CPL_OK) {
            for (size_t j = 0; j < i; j++)
                cplrt_release(words[j]);
            return st;
        }
    }
    return CPL_OK;
}

static void api_release_all(cplrt_w *words, size_t n)
{
    for (size_t i = 0; i < n; i++)
        cplrt_release(words[i]);
}

static cplrt_w *api_word_array(cplrt_runtime *rt, size_t n, cplrt_w *local, size_t nlocal)
{
    if (n <= nlocal)
        return local;
    if (n > SIZE_MAX / sizeof(cplrt_w))
        return NULL;
    return (cplrt_w *)CPLRT(alloc)(rt, n * sizeof(cplrt_w));
}

static cpl_status api_list_new(cpl_runtime *prt, const cpl_value *items, size_t n, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (items == NULL && n > 0)
        return api_null(rt, error, "items");
    if (n > UINT32_MAX)
        return api_nomem_error(error);
    cplrt_w local[16];
    cplrt_w *words = api_word_array(rt, n, local, 16);
    if (words == NULL)
        return api_nomem_error(error);
    cpl_status st = api_words(rt, items, n, "items", words, error);
    if (st == CPL_OK) {
        cplrt_w w = 0;
        int r = CPLRT(node_make)(rt, CPLRT_LIST, 0, 0, words, (uint32_t)n, &w);
        api_release_all(words, n);
        st = r == CPLRT_OK ? api_new_handle(rt, w, out, error) : api_nomem_error(error);
    }
    if (words != local)
        CPLRT(free)(words);
    return st;
}

static cpl_status api_list_len(cpl_runtime *prt, cpl_value v, size_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_LIST) {
        st = api_kind_mismatch(rt, error, "v", "list", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    *out = cplrt_len(w);
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_child(cplrt_runtime *rt, cplrt_w w, size_t index, size_t len, cplrt_w child, cpl_value *out,
                            cpl_error **error)
{
    if (index >= len) {
        cplrt_release(w);
        cplrt_datum d[2] = { api_n("index", index), api_n("len", len) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_INDEX_OUT_OF_RANGE, d, 2);
    }
    cplrt_retain(child);
    cplrt_release(w);
    return api_new_handle(rt, child, out, error);
}

static cpl_status api_list_get(cpl_runtime *prt, cpl_value v, size_t index, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_LIST) {
        st = api_kind_mismatch(rt, error, "v", "list", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    uint32_t len = cplrt_len(w);
    return api_child(rt, w, index, len, index < len ? cplrt_item(w, (uint32_t)index) : 0, out, error);
}

static cpl_status api_list_from_i64(cpl_runtime *prt, const int64_t *items, size_t n, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    if (items == NULL && n > 0)
        return api_null(rt, error, "items");
    if (n == 0)
        return api_new_handle(rt, CPLRT(empty_list)(), out, error);
    if (n > UINT32_MAX)
        return api_nomem_error(error);
    cplrt_node *list = CPLRT(list_alloc)(rt, (uint32_t)n);
    if (list == NULL)
        return api_nomem_error(error);
    for (size_t i = 0; i < n; i++) {
        cplrt_w x = 0;
        if (CPLRT(int_from_i64)(rt, items[i], &x) != CPLRT_OK) {
            cplrt_release(cplrt_word_of(&list->hdr));
            return api_nomem_error(error);
        }
        list->item[i] = x;
    }
    return api_new_handle(rt, cplrt_word_of(&list->hdr), out, error);
}

static cpl_status api_list_to_i64(cpl_runtime *prt, cpl_value v, int64_t *buf, size_t cap, size_t *needed,
                                  cpl_error **error)
{
    API_BEGIN(if (needed == NULL) return api_null(rt, error, "needed");)
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(w) || cplrt_obj_of(w)->kind != CPLRT_LIST) {
        st = api_kind_mismatch(rt, error, "v", "list", api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    uint32_t len = cplrt_len(w);
    *needed = len;
    if (buf != NULL) {
        for (uint32_t i = 0; i < len; i++) {
            cplrt_w x = cplrt_item(w, i);
            int64_t tmp;
            char param[32];
            snprintf(param, sizeof param, "buf[%u]", (unsigned)i);
            if (!api_is_int(x)) {
                st = api_kind_mismatch(rt, error, "v", "int", api_kind_word(x));
                cplrt_release(w);
                return st;
            }
            if (!CPLRT(int_to_i64)(x, &tmp)) {
                st = api_int_range(rt, error, x, "int64", param);
                cplrt_release(w);
                return st;
            }
        }
        if (cap < len) {
            cplrt_release(w);
            cplrt_datum d[2] = { api_n("index", len - 1u), api_n("len", cap) };
            return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_INDEX_OUT_OF_RANGE, d, 2);
        }
        for (uint32_t i = 0; i < len; i++)
            (void)CPLRT(int_to_i64)(cplrt_item(w, i), &buf[i]);
    }
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

/* ---- struct, enum, wrapper (4.5 shallow checks) ---------------------------------------------------------------- */

static const char *api_form_word(uint32_t form)
{
    switch (form) {
    case CPLRT_TY_INT: return "int";
    case CPLRT_TY_BOOL: return "bool";
    case CPLRT_TY_TEXT: return "text";
    case CPLRT_TY_BYTES: return "bytes";
    case CPLRT_TY_LIST: return "list";
    case CPLRT_TY_STRUCT: return "struct";
    case CPLRT_TY_ENUM: return "enum";
    case CPLRT_TY_WRAPPER: return "wrapper";
    case CPLRT_TY_REFINED: return "refinement";
    case CPLRT_TY_CAPABILITY: return "capability";
    default: return "type";
    }
}

/* The shallow check of a member (4.5): the child's kind and declaration against the declared member type, a type
   parameter accepting any value. 1 when it passes. */
static int api_shallow(const cplrt_program *p, uint32_t type, cplrt_w w)
{
    for (int guard = 0; guard < 64; guard++) {
        if (type == 0 || type >= p->ntypes)
            return 1;
        const cplrt_type *t = &p->types[type];
        switch (t->form) {
        case CPLRT_TY_INT: return api_is_int(w);
        case CPLRT_TY_BOOL: return w == CPLRT_TRUE || w == CPLRT_FALSE;
        case CPLRT_TY_TEXT: return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_TEXT;
        case CPLRT_TY_BYTES: return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_BYTES;
        case CPLRT_TY_LIST: return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_LIST;
        case CPLRT_TY_STRUCT:
            return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_STRUCT && cplrt_obj_of(w)->u.live.def == t->def;
        case CPLRT_TY_ENUM:
            return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_ENUM && cplrt_obj_of(w)->u.live.def == t->def;
        case CPLRT_TY_WRAPPER:
            return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_WRAPPER && cplrt_obj_of(w)->u.live.def == t->def;
        case CPLRT_TY_REFINED: type = t->elem; continue;
        case CPLRT_TY_PARAM: return 1;
        default: return 0;
        }
    }
    return 1;
}

/* Checks the type id and its form for a constructor; CPL_OK or the error. */
static cpl_status api_type_form(cplrt_runtime *rt, cpl_error **error, cpl_type type, uint32_t form, const char *expected)
{
    if (type == 0 || type >= rt->program->ntypes)
        return api_type_unknown(rt, error, type);
    uint32_t f = rt->program->types[type].form;
    if (f != form)
        return api_kind_mismatch(rt, error, "type", expected, api_form_word(f));
    return CPL_OK;
}

static cpl_status api_members(cplrt_runtime *rt, const cpl_value *items, size_t n, const char *name,
                              const uint32_t *types, cplrt_w *words, cpl_error **error)
{
    cpl_status st = api_words(rt, items, n, name, words, error);
    if (st != CPL_OK)
        return st;
    for (size_t i = 0; i < n; i++) {
        if (!api_shallow(rt->program, types[i], words[i])) {
            char param[48];
            snprintf(param, sizeof param, "%s[%zu]", name, i);
            const char *expected = types[i] < rt->program->ntypes ? rt->program->types[types[i]].text : "";
            st = api_kind_mismatch(rt, error, param, expected, api_kind_word(words[i]));
            api_release_all(words, n);
            return st;
        }
    }
    return CPL_OK;
}

static cpl_status api_arity(cplrt_runtime *rt, cpl_error **error, cpl_type type, uint64_t expected, uint64_t found)
{
    cplrt_datum d[3] = { api_s("type", rt->program->types[type].text), api_n("expected", expected),
                         api_n("found", found) };
    return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_ARITY_MISMATCH, d, 3);
}

static cpl_status api_struct_new(cpl_runtime *prt, cpl_type type, const cpl_value *fields, size_t n, cpl_value *out,
                                 cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cpl_status st = api_type_form(rt, error, type, CPLRT_TY_STRUCT, "struct");
    if (st != CPL_OK)
        return st;
    const cplrt_type *t = &rt->program->types[type];
    if (n != t->nmembers)
        return api_arity(rt, error, type, t->nmembers, n);
    if (fields == NULL && n > 0)
        return api_null(rt, error, "fields");
    cplrt_w local[16];
    cplrt_w *words = api_word_array(rt, n, local, 16);
    if (words == NULL)
        return api_nomem_error(error);
    st = api_members(rt, fields, n, "fields", t->members, words, error);
    if (st == CPL_OK) {
        cplrt_w w = 0;
        int r = CPLRT(node_make)(rt, CPLRT_STRUCT, t->def, 0, words, (uint32_t)n, &w);
        api_release_all(words, n);
        st = r == CPLRT_OK ? api_new_handle(rt, w, out, error) : api_nomem_error(error);
    }
    if (words != local)
        CPLRT(free)(words);
    return st;
}

static cpl_status api_composite(cplrt_runtime *rt, cpl_value v, uint8_t kind, const char *word, cplrt_w *w,
                                cpl_error **error)
{
    cpl_status st = api_value(rt, v, "v", w, error);
    if (st != CPL_OK)
        return st;
    if (!cplrt_is_obj(*w) || cplrt_obj_of(*w)->kind != kind) {
        st = api_kind_mismatch(rt, error, "v", word, api_kind_word(*w));
        cplrt_release(*w);
        return st;
    }
    return CPL_OK;
}

static cpl_status api_struct_field(cpl_runtime *prt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    cpl_status st = api_composite(rt, v, CPLRT_STRUCT, "struct", &w, error);
    if (st != CPL_OK)
        return st;
    uint32_t len = cplrt_len(w);
    return api_child(rt, w, index, len, index < len ? cplrt_field(w, index) : 0, out, error);
}

static cpl_status api_enum_new(cpl_runtime *prt, cpl_type type, uint32_t variant, const cpl_value *payload, size_t n,
                               cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cpl_status st = api_type_form(rt, error, type, CPLRT_TY_ENUM, "enum");
    if (st != CPL_OK)
        return st;
    const cplrt_type *t = &rt->program->types[type];
    if (variant >= t->nmembers || t->members == NULL) {
        cplrt_datum d[2] = { api_s("type", t->text), api_n("variant", variant) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_VARIANT_OUT_OF_RANGE, d, 2);
    }
    uint32_t lo = t->members[variant], count = t->members[variant + 1u] - lo;
    if (n != count)
        return api_arity(rt, error, type, count, n);
    if (payload == NULL && n > 0)
        return api_null(rt, error, "payload");
    cplrt_w local[16];
    cplrt_w *words = api_word_array(rt, n, local, 16);
    if (words == NULL)
        return api_nomem_error(error);
    st = api_members(rt, payload, n, "payload", t->payloads + lo, words, error);
    if (st == CPL_OK) {
        cplrt_w w = 0;
        int r = CPLRT(node_make)(rt, CPLRT_ENUM, t->def, variant, words, (uint32_t)n, &w);
        api_release_all(words, n);
        st = r == CPLRT_OK ? api_new_handle(rt, w, out, error) : api_nomem_error(error);
    }
    if (words != local)
        CPLRT(free)(words);
    return st;
}

static cpl_status api_enum_variant(cpl_runtime *prt, cpl_value v, uint32_t *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    cplrt_w w = 0;
    cpl_status st = api_composite(rt, v, CPLRT_ENUM, "enum", &w, error);
    if (st != CPL_OK)
        return st;
    *out = cplrt_variant(w);
    cplrt_release(w);
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_enum_payload(cpl_runtime *prt, cpl_value v, uint32_t index, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    cpl_status st = api_composite(rt, v, CPLRT_ENUM, "enum", &w, error);
    if (st != CPL_OK)
        return st;
    uint32_t len = cplrt_payload_len(w);
    return api_child(rt, w, index, len, index < len ? cplrt_payload(w, index) : 0, out, error);
}

static cpl_status api_wrapper_new(cpl_runtime *prt, cpl_type type, cpl_value inner, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cpl_status st = api_type_form(rt, error, type, CPLRT_TY_WRAPPER, "wrapper");
    if (st != CPL_OK)
        return st;
    const cplrt_type *t = &rt->program->types[type];
    cplrt_w w = 0;
    st = api_value(rt, inner, "inner", &w, error);
    if (st != CPL_OK)
        return st;
    if (!api_shallow(rt->program, t->elem, w)) {
        const char *expected = t->elem < rt->program->ntypes ? rt->program->types[t->elem].text : "";
        st = api_kind_mismatch(rt, error, "inner", expected, api_kind_word(w));
        cplrt_release(w);
        return st;
    }
    cplrt_w r = 0;
    int k = CPLRT(node_make)(rt, CPLRT_WRAPPER, t->def, 0, &w, 1, &r);
    cplrt_release(w);
    return k == CPLRT_OK ? api_new_handle(rt, r, out, error) : api_nomem_error(error);
}

static cpl_status api_wrapper_inner(cpl_runtime *prt, cpl_value v, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    cpl_status st = api_composite(rt, v, CPLRT_WRAPPER, "wrapper", &w, error);
    if (st != CPL_OK)
        return st;
    return api_child(rt, w, 0, 1, cplrt_inner(w), out, error);
}

static cpl_status api_static_variant(cplrt_runtime *rt, uint32_t def, uint32_t variant, cpl_value *out,
                                     cpl_error **error)
{
    cplrt_w w = CPLRT(enum_static)(rt->program, def, variant);
    if (w == 0 && CPLRT(node_make)(rt, CPLRT_ENUM, def, variant, NULL, 0, &w) != CPLRT_OK)
        return api_nomem_error(error);
    return api_new_handle(rt, w, out, error);
}

static cpl_status api_unit(cpl_runtime *prt, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    return api_static_variant(rt, rt->program->std.unit, 0, out, error);
}

static cpl_status api_option_none(cpl_runtime *prt, cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    return api_static_variant(rt, rt->program->std.option, CPL_OPTION_NONE, out, error);
}

static cpl_status api_wrap1(cpl_runtime *prt, cpl_value v, uint32_t def, uint32_t variant, const char *param,
                            cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, param, &w, error);
    if (st != CPL_OK)
        return st;
    cplrt_w r = 0;
    int k = CPLRT(node_make)(rt, CPLRT_ENUM, def, variant, &w, 1, &r);
    cplrt_release(w);
    return k == CPLRT_OK ? api_new_handle(rt, r, out, error) : api_nomem_error(error);
}

static cpl_status api_option_some(cpl_runtime *prt, cpl_value v, cpl_value *out, cpl_error **error)
{
    return api_wrap1(prt, v, api_rt(prt)->program->std.option, CPL_OPTION_SOME, "v", out, error);
}

static cpl_status api_result_ok(cpl_runtime *prt, cpl_value v, cpl_value *out, cpl_error **error)
{
    return api_wrap1(prt, v, api_rt(prt)->program->std.result, CPL_RESULT_OK, "v", out, error);
}

static cpl_status api_result_err(cpl_runtime *prt, cpl_value e, cpl_value *out, cpl_error **error)
{
    return api_wrap1(prt, e, api_rt(prt)->program->std.result, CPL_RESULT_ERR, "e", out, error);
}

static uint32_t api_member_index(const cplrt_program *p, uint32_t def, const char *name)
{
    if (def >= p->ndefs)
        return CPLRT_NONE;
    for (uint32_t i = 0; i < p->defs[def].nmembers; i++)
        if (strcmp(p->defs[def].members[i], name) == 0)
            return i;
    return CPLRT_NONE;
}

static cpl_status api_foreign_error_new(cpl_runtime *prt, cpl_foreign_kind kind, const char *message, size_t len,
                                        const cpl_strview *keys, const cpl_strview *values, size_t ndetails,
                                        cpl_value *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    api_zero(out);
    const cplrt_program *p = rt->program;
    if (kind < 0 || (uint32_t)kind >= CPLRT_FOREIGN_KINDS_N) {
        char variant[16];
        snprintf(variant, sizeof variant, "%d", (int)kind);
        cplrt_datum d[2] = { api_s("type", "std.foreign.ForeignErrorKind"), api_j("variant", variant) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_VARIANT_OUT_OF_RANGE, d, 2);
    }
    if (message == NULL && len > 0)
        return api_null(rt, error, "message");
    size_t valid = 0;
    if (len > 0 && !CPLRT(utf8_valid)(message, len, &valid, NULL))
        return api_utf8_error(rt, error, "message", valid);
    if ((keys == NULL || values == NULL) && ndetails > 0)
        return api_null(rt, error, keys == NULL ? "keys" : "values");
    for (size_t i = 0; i < ndetails; i++) {
        char param[32];
        if (keys[i].len > 0 && !CPLRT(utf8_valid)(keys[i].ptr, keys[i].len, &valid, NULL)) {
            snprintf(param, sizeof param, "keys[%zu]", i);
            return api_utf8_error(rt, error, param, valid);
        }
        if (values[i].len > 0 && !CPLRT(utf8_valid)(values[i].ptr, values[i].len, &valid, NULL)) {
            snprintf(param, sizeof param, "values[%zu]", i);
            return api_utf8_error(rt, error, param, valid);
        }
    }
    uint32_t fe = p->std.foreign_error, fd = p->std.foreign_detail, fk = p->std.foreign_error_kind;
    if (fe == CPLRT_NONE || fd == CPLRT_NONE || fk == CPLRT_NONE)
        return api_type_unknown(rt, error, CPL_T_FOREIGN_ERROR);
    uint32_t dk = api_member_index(p, fd, "key"), dv = api_member_index(p, fd, "value");
    uint32_t ek = api_member_index(p, fe, "kind"), em = api_member_index(p, fe, "message");
    uint32_t ed = api_member_index(p, fe, "details");
    if (dk > 1u || dv > 1u || ek > 2u || em > 2u || ed > 2u)
        return api_type_unknown(rt, error, CPL_T_FOREIGN_ERROR);
    cplrt_w details = CPLRT(empty_list)(), msg = 0, kv = 0, r = 0;
    int ok = 1;
    if (ndetails > 0) {
        cplrt_node *list = ndetails <= UINT32_MAX ? CPLRT(list_alloc)(rt, (uint32_t)ndetails) : NULL;
        if (list == NULL)
            return api_nomem_error(error);
        details = cplrt_word_of(&list->hdr);
        for (size_t i = 0; i < ndetails && ok; i++) {
            cplrt_w k = 0, v = 0, entry = 0, f[2];
            if (CPLRT(text_make)(rt, keys[i].ptr != NULL ? keys[i].ptr : "", keys[i].len, &k) != CPLRT_OK ||
                CPLRT(text_make)(rt, values[i].ptr != NULL ? values[i].ptr : "", values[i].len, &v) != CPLRT_OK) {
                cplrt_release(k);
                ok = 0;
                break;
            }
            f[dk] = k;
            f[dv] = v;
            if (CPLRT(node_make)(rt, CPLRT_STRUCT, fd, 0, f, 2, &entry) != CPLRT_OK)
                ok = 0;
            cplrt_release(k);
            cplrt_release(v);
            list->item[i] = entry;
        }
    }
    if (ok && CPLRT(text_make)(rt, message != NULL ? message : "", len, &msg) != CPLRT_OK)
        ok = 0;
    if (ok) {
        kv = CPLRT(enum_static)(p, fk, (uint32_t)kind);
        if (kv == 0 && CPLRT(node_make)(rt, CPLRT_ENUM, fk, (uint32_t)kind, NULL, 0, &kv) != CPLRT_OK)
            ok = 0;
    }
    if (ok) {
        cplrt_w f[3];
        f[ek] = kv;
        f[em] = msg;
        f[ed] = details;
        if (CPLRT(node_make)(rt, CPLRT_STRUCT, fe, 0, f, 3, &r) != CPLRT_OK)
            ok = 0;
    }
    cplrt_release(details);
    cplrt_release(msg);
    cplrt_release(kv);
    if (!ok)
        return api_nomem_error(error);
    return api_new_handle(rt, r, out, error);
}

/* ---- cancellation (3.6, 4.8) ----------------------------------------------------------------------------------- */

struct cplrt_token {
    _Atomic uint32_t rc;
    _Atomic uint32_t flag;
};

cplrt_token *CPLRT(token_new)(cplrt_runtime *rt)
{
    cplrt_token *t = (cplrt_token *)CPLRT(alloc)(rt, sizeof *t);
    if (t == NULL)
        return NULL;
    atomic_init(&t->rc, 1u);
    atomic_init(&t->flag, 0u);
    return t;
}

void CPLRT(token_retain)(cplrt_token *token)
{
    if (token != NULL)
        atomic_fetch_add_explicit(&token->rc, 1u, memory_order_relaxed);
}

void CPLRT(token_unref)(cplrt_token *token)
{
    if (token != NULL && atomic_fetch_sub_explicit(&token->rc, 1u, memory_order_acq_rel) == 1u)
        CPLRT(free)(token);
}

const _Atomic uint32_t *CPLRT(token_flag)(const cplrt_token *token)
{
    return &token->flag;
}

static cpl_status api_cancel_new(cpl_runtime *prt, cpl_cancel *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    out->bits[0] = out->bits[1] = 0;
    cplrt_token *t = CPLRT(token_new)(rt);
    if (t == NULL)
        return api_nomem_error(error);
    cplrt_slot e;
    memset(&e, 0, sizeof e);
    e.kind = CPLRT_SLOT_CANCEL;
    e.u.cancel = t;
    if (CPLRT(handle_put)(rt, &e, out->bits) != CPLRT_OK) {
        CPLRT(token_unref)(t);
        return api_nomem_error(error);
    }
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_cancel_request(cpl_runtime *prt, cpl_cancel token)
{
    cplrt_runtime *rt = api_rt(prt);
    if (atomic_load_explicit(&rt->poisoned, memory_order_relaxed))
        return CPL_E_STATE;
    cplrt_slot e;
    int r = CPLRT(handle_get)(rt, token.bits, CPLRT_SLOT_CANCEL, &e);
    if (r != CPLRT_HANDLE_OK)
        return CPL_E_ARGUMENT;
    atomic_store_explicit((_Atomic uint32_t *)(uintptr_t)CPLRT(token_flag)(e.u.cancel), 1u, memory_order_relaxed);
    CPLRT(slot_unref)(&e);
    return CPL_OK;
}

static cpl_status api_cancel_release(cpl_runtime *prt, cpl_cancel token)
{
    cplrt_runtime *rt = api_rt(prt);
    if (token.bits[0] == 0 && token.bits[1] == 0)
        return CPL_OK;
    return CPLRT(handle_release)(rt, token.bits, CPLRT_SLOT_CANCEL) == CPLRT_HANDLE_OK ? CPL_OK : CPL_E_ARGUMENT;
}

/* ---- capabilities and callbacks (5.7) -------------------------------------------------------------------------- */

static cpl_status api_cap_new_as(cpl_runtime *prt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                                 const char *label, cpl_cap *out, cpl_error **error);

static cpl_status api_cap_new(cpl_runtime *prt, cpl_type capability, const cpl_cap_ops *ops, void *user, cpl_cap *out,
                              cpl_error **error)
{
    return api_cap_new_as(prt, capability, ops, user, "host", out, error);
}

/* Milestone 3: cpl_cap_new with the binding name `label` (the Python extension's in-process grants, spec/python.md
   5.3), outside struct cpl_api. */
cpl_status CPLRT(api_cap_new_labeled)(cpl_runtime *prt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                                      const char *label, cpl_cap *out, cpl_error **error)
{
    return api_cap_new_as(prt, capability, ops, user, label != NULL && label[0] != '\0' ? label : "host", out, error);
}

static cpl_status api_cap_new_as(cpl_runtime *prt, cpl_type capability, const cpl_cap_ops *ops, void *user,
                                 const char *label, cpl_cap *out, cpl_error **error)
{
    API_BEGIN(if (out == NULL) return api_null(rt, error, "out");)
    out->bits[0] = out->bits[1] = 0;
    const cplrt_program *p = rt->program;
    if (capability == 0 || capability >= p->ntypes)
        return api_type_unknown(rt, error, capability);
    const cplrt_type *t = &p->types[capability];
    if (t->form != CPLRT_TY_CAPABILITY)
        return api_kind_mismatch(rt, error, "capability", "capability", api_form_word(t->form));
    if (ops == NULL)
        return api_null(rt, error, "ops");
    if (ops->struct_size < offsetof(cpl_cap_ops, drop) || ops->struct_size > sizeof(cpl_cap_ops)) {
        cplrt_datum d[4] = { api_s("struct", "cpl_cap_ops"), api_n("size", ops->struct_size),
                             api_n("minimum", offsetof(cpl_cap_ops, drop)), api_n("maximum", sizeof(cpl_cap_ops)) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_STRUCT_SIZE_UNSUPPORTED, d, 4);
    }
    uint32_t nops = t->def < p->ndefs ? p->defs[t->def].nmembers : 0;
    if (ops->nops != nops) {
        cplrt_datum d[3] = { api_s("type", t->text), api_n("expected", nops), api_n("found", ops->nops) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_ARITY_MISMATCH, d, 3);
    }
    if (ops->ops == NULL && nops > 0)
        return api_null(rt, error, "ops->ops");
    for (uint32_t i = 0; i < nops; i++) {
        if (ops->ops[i] == NULL) {
            char param[32];
            snprintf(param, sizeof param, "ops->ops[%u]", (unsigned)i);
            return api_null(rt, error, param);
        }
    }
    cplrt_capobj *c = CPLRT(cap_generic_new)(rt, t->def, ops, user, label);
    if (c == NULL)
        return api_nomem_error(error);
    cplrt_slot e;
    memset(&e, 0, sizeof e);
    e.kind = CPLRT_SLOT_CAP;
    e.u.cap = c;
    if (CPLRT(handle_put)(rt, &e, out->bits) != CPLRT_OK) {
        /* the host keeps user: the object's drop must not run */
        CPLRT(cap_run_drop_disarm)(c);
        CPLRT(cap_unref)(c);
        return api_nomem_error(error);
    }
    api_clear(error);
    return CPL_OK;
}

static cpl_status api_cap_release(cpl_runtime *prt, cpl_cap cap)
{
    cplrt_runtime *rt = api_rt(prt);
    if (cap.bits[0] == 0 && cap.bits[1] == 0)
        return CPL_OK;
    return CPLRT(handle_release)(rt, cap.bits, CPLRT_SLOT_CAP) == CPLRT_HANDLE_OK ? CPL_OK : CPL_E_ARGUMENT;
}

static cpl_runtime *api_call_runtime(cpl_call *call)
{
    return &CPLRT(call_rt)(call)->pub;
}

static uint32_t api_call_operation(cpl_call *call)
{
    return CPLRT(call_op)(call);
}

static int32_t api_call_cancelled(cpl_call *call)
{
    return CPLRT(call_is_cancelled)(call);
}

static cpl_status api_call_foreign_detail(cpl_call *call, const char *key, size_t key_len, const char *value,
                                          size_t value_len)
{
    return CPLRT(call_detail)(call, key, key_len, value, value_len);
}

static cpl_status api_call_foreign_error(cpl_call *call, cpl_foreign_kind kind, const char *message, size_t len)
{
    return CPLRT(call_fail)(call, kind, message, len);
}

/* ---- error objects ------------------------------------------------------------------------------------------------ */

static cpl_status api_error_status(const cpl_error *e)
{
    return ((const api_err *)(const void *)e)->status;
}

static const char *api_error_code(const cpl_error *e)
{
    return ((const api_err *)(const void *)e)->code;
}

static const char *api_error_category(const cpl_error *e)
{
    return ((const api_err *)(const void *)e)->category;
}

static cpl_strview api_error_message(const cpl_error *e)
{
    const api_err *x = (const api_err *)(const void *)e;
    cpl_strview v = { x->message, x->message_len };
    return v;
}

static cpl_strview api_error_json(const cpl_error *e)
{
    const api_err *x = (const api_err *)(const void *)e;
    cpl_strview v = { x->json, x->json_len };
    return v;
}

/* ---- the table (4.1): the order of Appendix A.1, frozen in crates/cpl-rt/api_v0.txt -------------------------------- */

const cpl_api CPLRT(api_table) = {
    sizeof(cpl_api),
    CPL_API_VERSION,
    api_runtime_close,
    api_runtime_info,
    api_runtime_stats,
    api_type_lookup,
    api_type_name,
    api_value_dup,
    api_value_release,
    api_value_kind,
    api_value_type,
    api_value_equal,
    api_value_to_json,
    api_value_from_json,
    api_value_to_text,
    api_buf_release,
    api_int_from_i64,
    api_int_from_u64,
    api_int_from_decimal,
    api_int_from_words,
    api_int_to_i64,
    api_int_to_u64,
    api_int_to_decimal,
    api_int_to_words,
    api_bool_new,
    api_bool_get,
    api_text_new,
    api_text_view,
    api_text_scalars,
    api_bytes_new,
    api_bytes_view,
    api_list_new,
    api_list_len,
    api_list_get,
    api_list_from_i64,
    api_list_to_i64,
    api_struct_new,
    api_struct_field,
    api_enum_new,
    api_enum_variant,
    api_enum_payload,
    api_wrapper_new,
    api_wrapper_inner,
    api_unit,
    api_option_some,
    api_option_none,
    api_result_ok,
    api_result_err,
    api_foreign_error_new,
    api_cancel_new,
    api_cancel_request,
    api_cancel_release,
    api_cap_new,
    api_cap_release,
    api_call_runtime,
    api_call_operation,
    api_call_cancelled,
    api_call_foreign_detail,
    api_call_foreign_error,
    api_error_status,
    api_error_code,
    api_error_category,
    api_error_message,
    api_error_json,
    api_err_release,
};

/* ---- exports (5.5) -------------------------------------------------------------------------------------------- */

/* Writes the word w (borrowed) into the output o by its form; CPL_OK, or the status of a conversion that failed. */
static cpl_status api_put_out(cplrt_runtime *rt, uint32_t form, cplrt_out o, cplrt_w w, const char *position,
                              cpl_error **error)
{
    switch (form) {
    case CPLRT_AF_HANDLE:
        cplrt_retain(w);
        return api_new_handle(rt, w, o.value, error);
    case CPLRT_AF_BOOL: *o.i32 = w == CPLRT_TRUE; return CPL_OK;
    case CPLRT_AF_I64:
        if (!CPLRT(int_to_i64)(w, o.i64))
            return api_int_range(rt, error, w, "int64", position);
        return CPL_OK;
    case CPLRT_AF_I32: {
        int64_t v = 0;
        if (!CPLRT(int_to_i64)(w, &v) || v < INT32_MIN || v > INT32_MAX)
            return api_int_range(rt, error, w, "int32", position);
        *o.i32 = (int32_t)v;
        return CPL_OK;
    }
    case CPLRT_AF_U64:
        if (!CPLRT(int_to_u64)(w, o.u64))
            return api_int_range(rt, error, w, "uint64", position);
        return CPL_OK;
    case CPLRT_AF_U32: {
        uint64_t v = 0;
        if (!CPLRT(int_to_u64)(w, &v) || v > UINT32_MAX)
            return api_int_range(rt, error, w, "uint32", position);
        *o.u32 = (uint32_t)v;
        return CPL_OK;
    }
    default: return CPL_OK;
    }
}

/* Zeroes an output by its form (4.6: an export writes every output). */
static void api_zero_out(uint32_t form, cplrt_out o)
{
    switch (form) {
    case CPLRT_AF_HANDLE:
        if (o.value != NULL)
            o.value->bits[0] = o.value->bits[1] = 0;
        break;
    case CPLRT_AF_BOOL:
    case CPLRT_AF_I32:
        if (o.i32 != NULL)
            *o.i32 = 0;
        break;
    case CPLRT_AF_I64:
        if (o.i64 != NULL)
            *o.i64 = 0;
        break;
    case CPLRT_AF_U64:
        if (o.u64 != NULL)
            *o.u64 = 0;
        break;
    case CPLRT_AF_U32:
        if (o.u32 != NULL)
            *o.u32 = 0;
        break;
    default: break;
    }
}

/* The body of every export (5.5); with `values` (the Python extension, spec/python.md 2) every argument and every
   output is a value handle, whatever the export's C forms. */
static cpl_status api_export(cpl_runtime *prt, uint32_t index, const cplrt_arg *args, const cpl_cap *caps,
                             const cplrt_out *outs, const cpl_call_options *options, cpl_error **error, int values)
{
    cplrt_runtime *rt = api_rt(prt);
    const cplrt_program *p = rt->program;
    const cplrt_export *x = &p->exports[index];
    const cplrt_def *fd = &p->defs[x->function];
    uint32_t nout = x->split ? 2u : 1u;
    uint32_t forms_out[2] = { values ? CPLRT_AF_HANDLE : x->ok_form,
                              x->split ? (values ? CPLRT_AF_HANDLE : x->err_form) : CPLRT_AF_UNIT };
    for (uint32_t k = 0; k < nout; k++)
        if (forms_out[k] != CPLRT_AF_UNIT)
            api_zero_out(forms_out[k], outs[k]);
    api_clear(error);
    cpl_status st;
    if (api_poisoned(rt, error, &st))
        return st;
    /* step 1: the options, then every C argument, every capability, every output pointer */
    cpl_call_options opts = CPL_CALL_OPTIONS_INIT;
    if (options != NULL) {
        if (options->struct_size < 8u || options->struct_size > sizeof(cpl_call_options)) {
            cplrt_datum d[4] = { api_s("struct", "cpl_call_options"), api_n("size", options->struct_size),
                                 api_n("minimum", 8u), api_n("maximum", sizeof(cpl_call_options)) };
            return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_STRUCT_SIZE_UNSUPPORTED, d, 4);
        }
        opts.flags = options->flags;
        if (options->struct_size >= offsetof(cpl_call_options, time_budget_ms) + sizeof options->time_budget_ms)
            opts.time_budget_ms = options->time_budget_ms;
        if (options->struct_size >= offsetof(cpl_call_options, stack_bytes) + sizeof options->stack_bytes)
            opts.stack_bytes = options->stack_bytes;
        if (options->struct_size >= offsetof(cpl_call_options, cancel) + sizeof options->cancel)
            opts.cancel = options->cancel;
        if (opts.flags != 0u)
            return api_err1(rt, error, CPLRT_INTEROP_OPTION_UNSUPPORTED, "option", "flags");
    }
    cplrt_token *token = NULL;
    if (opts.cancel.bits[0] != 0 || opts.cancel.bits[1] != 0) {
        cplrt_slot e;
        int r = CPLRT(handle_get)(rt, opts.cancel.bits, CPLRT_SLOT_CANCEL, &e);
        if (r != CPLRT_HANDLE_OK)
            return api_handle_error(rt, error, r, "options->cancel");
        token = e.u.cancel;
    }
    uint32_t n = x->nparams;
    cplrt_w local_words[16];
    cplrt_w *words = api_word_array(rt, n, local_words, 16);
    if (words == NULL) {
        CPLRT(token_unref)(token);
        return api_nomem_error(error);
    }
    memset(words, 0, (size_t)n * sizeof *words);
    const cplrt_capref *local_refs[8];
    cplrt_capobj *local_caps[8];
    const cplrt_capref **refs = local_refs;
    cplrt_capobj **capobjs = local_caps;
    if (x->nusing > 8u) {
        refs = (const cplrt_capref **)CPLRT(alloc)(rt, x->nusing * sizeof *refs);
        capobjs = (cplrt_capobj **)CPLRT(alloc)(rt, x->nusing * sizeof *capobjs);
    }
    uint32_t ncaps = 0;
    st = CPL_OK;
    if (refs == NULL || capobjs == NULL)
        st = api_nomem_error(error);
    for (uint32_t i = 0; i < n && st == CPL_OK; i++) {
        const char *param = i < fd->nmembers ? fd->members[i] : "argument";
        const cplrt_arg *a = &args[i];
        switch (values ? CPLRT_AF_HANDLE : x->forms[i]) {
        case CPLRT_AF_HANDLE: st = api_value(rt, a->value, param, &words[i], error); break;
        case CPLRT_AF_TEXT: {
            size_t valid = 0;
            if (a->span.ptr == NULL && a->span.len > 0)
                st = api_null(rt, error, param);
            else if (a->span.len > 0 && !CPLRT(utf8_valid)((const char *)a->span.ptr, a->span.len, &valid, NULL))
                st = api_utf8_error(rt, error, param, valid);
            else if (CPLRT(text_make)(rt, a->span.len > 0 ? (const char *)a->span.ptr : "", a->span.len, &words[i]) !=
                     CPLRT_OK)
                st = api_nomem_error(error);
            break;
        }
        case CPLRT_AF_BYTES:
            if (a->span.ptr == NULL && a->span.len > 0)
                st = api_null(rt, error, param);
            else if (CPLRT(bytes_make)(rt, (const uint8_t *)a->span.ptr, a->span.len, &words[i]) != CPLRT_OK)
                st = api_nomem_error(error);
            break;
        case CPLRT_AF_BOOL:
            if (a->i32 != 0 && a->i32 != 1) {
                char found[16];
                snprintf(found, sizeof found, "%d", (int)a->i32);
                cplrt_datum d[2] = { api_s("parameter", param), api_j("found", found) };
                st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_BOOL_OUT_OF_RANGE, d, 2);
            } else {
                words[i] = cplrt_bool(a->i32);
            }
            break;
        case CPLRT_AF_I64:
            if (CPLRT(int_from_i64)(rt, a->i64, &words[i]) != CPLRT_OK)
                st = api_nomem_error(error);
            break;
        case CPLRT_AF_I32:
            if (CPLRT(int_from_i64)(rt, a->i32, &words[i]) != CPLRT_OK)
                st = api_nomem_error(error);
            break;
        case CPLRT_AF_U64:
            if (CPLRT(int_from_u64)(rt, a->u64, &words[i]) != CPLRT_OK)
                st = api_nomem_error(error);
            break;
        case CPLRT_AF_U32:
            if (CPLRT(int_from_u64)(rt, a->u32, &words[i]) != CPLRT_OK)
                st = api_nomem_error(error);
            break;
        default: {
            cplrt_w u = CPLRT(enum_static)(p, p->std.unit, 0);
            words[i] = u;
            break;
        }
        }
    }
    for (uint32_t k = 0; k < x->nusing && st == CPL_OK; k++) {
        char param[32];
        snprintf(param, sizeof param, "caps[%u]", (unsigned)k);
        cplrt_slot e;
        int r = CPLRT(handle_get)(rt, caps[k].bits, CPLRT_SLOT_CAP, &e);
        if (r != CPLRT_HANDLE_OK) {
            st = api_handle_error(rt, error, r, param);
            break;
        }
        capobjs[ncaps++] = e.u.cap;
        refs[k] = CPLRT(cap_ref)(e.u.cap);
        if (refs[k]->capability != x->caps[k]) {
            cplrt_datum d[3] = { api_s("parameter", param), api_s("expected", p->defs[x->caps[k]].qualified_name),
                                 api_s("found", p->defs[refs[k]->capability].qualified_name) };
            st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_CAPABILITY_MISMATCH, d, 3);
        }
    }
    for (uint32_t k = 0; k < nout && st == CPL_OK; k++) {
        if (forms_out[k] != CPLRT_AF_UNIT && outs[k].value == NULL)
            st = api_null(rt, error, x->split ? (k == 0 ? "ok_out" : "err_out") : "out");
    }
    /* step 2: the deep type of every handle argument */
    for (uint32_t i = 0; i < n && st == CPL_OK; i++) {
        if (!values && x->forms[i] != CPLRT_AF_HANDLE)
            continue;
        cplrt_shape_failure sf;
        if (!CPLRT(shape_check)(rt, x->types[i], words[i], &sf)) {
            if (sf.pointer.status != CPLRT_OK) {
                st = api_nomem_error(error);
            } else {
                const char *param = i < fd->nmembers ? fd->members[i] : "argument";
                cplrt_datum d[4] = { api_s("parameter", param), api_s("pointer", sf.pointer.ptr != NULL ? sf.pointer.ptr : ""),
                                     api_s("expected", p->types[sf.type].text), api_s("found", sf.found) };
                st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_ARGUMENT_TYPE_MISMATCH, d, 4);
            }
        }
        CPLRT(sbuf_free)(&sf.pointer);
    }
    if (st == CPL_OK) {
        /* steps 3 and 4: the refined positions, then the call, one top-level evaluation */
        atomic_fetch_add(&rt->active_calls, 1u);
        cplrt_cx cx;
        cplrt_memory_record mem;
        if (CPLRT(cx_begin)(&cx, rt, &mem, UINT32_MAX, 1, opts.time_budget_ms, opts.stack_bytes,
                            token != NULL ? CPLRT(token_flag)(token) : NULL) != CPLRT_OK) {
            st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
        } else {
            int ok = CPLRT_OK;
            for (uint32_t i = 0; i < n && ok == CPLRT_OK; i++)
                ok = CPLRT(argument_refined)(&cx, x->function, i, x->types[i], words[i]);
            cplrt_w out = 0;
            if (ok == CPLRT_OK) {
                cplrt_fn_entry entry = p->code->fns[x->function];
                ok = entry(&cx, words, refs, &out);
            }
            if (ok == CPLRT_OK) {
                if (x->split) {
                    int is_ok = cplrt_variant(out) == CPL_RESULT_OK;
                    cplrt_w payload = cplrt_payload_len(out) > 0 ? cplrt_payload(out, 0) : 0;
                    uint32_t k = is_ok ? 0u : 1u;
                    st = forms_out[k] == CPLRT_AF_UNIT
                             ? CPL_OK
                             : api_put_out(rt, forms_out[k], outs[k], payload, k == 0 ? "ok_out" : "err_out", error);
                    if (st == CPL_OK && !is_ok)
                        st = CPL_ERR;
                } else if (forms_out[0] != CPLRT_AF_UNIT) {
                    st = api_put_out(rt, forms_out[0], outs[0], out, "out", error);
                }
            } else if (cx.cancelled) {
                st = CPLRT(error_interop)(rt, error, CPLRT_INTEROP_CANCELLED, NULL, 0);
            } else {
                st = cx.fault != NULL ? CPLRT(error_fault)(rt, error, cx.fault)
                                      : CPLRT(error_internal)(rt, error, "an export faulted without a fault record");
            }
            cplrt_release(out);
            CPLRT(cx_end)(&cx);
        }
        atomic_fetch_sub(&rt->active_calls, 1u);
    }
    if (st != CPL_OK && st != CPL_ERR) {
        for (uint32_t k = 0; k < nout; k++)
            if (forms_out[k] == CPLRT_AF_HANDLE && outs[k].value != NULL &&
                (outs[k].value->bits[0] != 0 || outs[k].value->bits[1] != 0)) {
                (void)CPLRT(handle_release)(rt, outs[k].value->bits, CPLRT_SLOT_VALUE);
                outs[k].value->bits[0] = outs[k].value->bits[1] = 0;
            } else if (forms_out[k] != CPLRT_AF_UNIT) {
                api_zero_out(forms_out[k], outs[k]);
            }
    }
    for (uint32_t i = 0; i < n; i++)
        cplrt_release(words[i]);
    if (words != local_words)
        CPLRT(free)(words);
    for (uint32_t k = 0; k < ncaps; k++)
        CPLRT(cap_unref)(capobjs[k]);
    if (refs != local_refs)
        CPLRT(free)((void *)refs);
    if (capobjs != local_caps)
        CPLRT(free)(capobjs);
    CPLRT(token_unref)(token);
    return st;
}

cpl_status cplrt_export_call(cpl_runtime *prt, uint32_t index, const cplrt_arg *args, const cpl_cap *caps,
                             const cplrt_out *outs, const cpl_call_options *options, cpl_error **error)
{
    return api_export(prt, index, args, caps, outs, options, error, 0);
}

/* ---- Milestone 3: the internal entries of the Python extension (spec/python.md 2), outside struct cpl_api ---------- */

/* Export `index` with every argument a value handle (nargs of them) and every output a value handle: *ok_out the
   result (an exported Result's Ok payload, the whole value otherwise), *err_out an exported Result's Err payload
   (status CPL_ERR); the checks, the evaluation, and the statuses of 5.5. */
cpl_status CPLRT(export_call_values)(cpl_runtime *prt, uint32_t index, const cpl_value *args, uint32_t nargs,
                                     const cpl_cap *caps, uint32_t ncaps, cpl_value *ok_out, cpl_value *err_out,
                                     const cpl_call_options *options, cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    const cplrt_program *p = rt->program;
    if (index >= p->nexports) {
        cplrt_datum d[2] = { api_n("index", index), api_n("len", p->nexports) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_INDEX_OUT_OF_RANGE, d, 2);
    }
    const cplrt_export *x = &p->exports[index];
    if (nargs != x->nparams || ncaps != x->nusing) {
        cplrt_datum d[3] = { api_s("type", p->defs[x->function].qualified_name), api_n("expected", x->nparams + x->nusing),
                             api_n("found", nargs + ncaps) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_ARITY_MISMATCH, d, 3);
    }
    cplrt_arg local[16];
    memset(local, 0, sizeof local);
    cplrt_arg *a = nargs <= 16u ? local : (cplrt_arg *)CPLRT(alloc)(rt, (size_t)nargs * sizeof *a);
    if (a == NULL)
        return api_nomem_error(error);
    for (uint32_t i = 0; i < nargs; i++)
        a[i].value = args[i];
    cplrt_out outs[2];
    outs[0].value = ok_out;
    outs[1].value = err_out;
    cpl_status st = api_export(prt, index, a, caps, outs, options, error, 1);
    if (a != local)
        CPLRT(free)(a);
    return st;
}

/* The value of a node-list text ({"nodes": [...]}, 3.8) as a new value handle; interop_json_invalid (`pointer` "",
   `detail`) for a text that is no node list of the program. */
cpl_status CPLRT(api_value_from_nodes)(cpl_runtime *prt, const char *text, size_t len, cpl_value *out,
                                       cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    if (out == NULL || (text == NULL && len > 0))
        return api_null(rt, error, out == NULL ? "out" : "text");
    out->bits[0] = out->bits[1] = 0;
    cplrt_jv *jv = NULL;
    size_t at = 0;
    int r = CPLRT(json_parse)(rt, text != NULL ? text : "", len, &jv, &at);
    if (r == CPLRT_NOMEM)
        return api_nomem_error(error);
    const char *detail = "the text is not JSON";
    cplrt_w w = 0;
    if (r == CPLRT_OK)
        r = CPLRT(nodes_read)(rt, jv, &w, &detail);
    CPLRT(json_free)(jv);
    if (r == CPLRT_NOMEM)
        return api_nomem_error(error);
    if (r != CPLRT_OK) {
        cplrt_datum d[2] = { api_s("pointer", ""), api_s("detail", detail) };
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_JSON_INVALID, d, 2);
    }
    return api_new_handle(rt, w, out, error);
}

/* The node-list text of a value ({"nodes": [...]}, 3.8, without renderings) in a buffer of the API (4.5). */
cpl_status CPLRT(api_value_to_nodes)(cpl_runtime *prt, cpl_value v, cpl_buf *out, cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    if (out == NULL)
        return api_null(rt, error, "out");
    out->ptr = NULL;
    out->len = 0;
    cplrt_w w = 0;
    cpl_status st = api_value(rt, v, "v", &w, error);
    if (st != CPL_OK)
        return st;
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_put)(&b, "", 0);
    CPLRT(nodes_write)(&b, rt, rt->program, w, 0);
    cplrt_release(w);
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        return api_nomem_error(error);
    }
    st = api_buffer(rt, b.ptr, b.len, out, error);
    CPLRT(sbuf_free)(&b);
    return st;
}

/* interop_runtime_busy with `active_calls` (at least 1): the Python extension's configure() while a call holds the
   module's runtime (spec/python.md 2), whether or not that call has entered the runtime yet. */
cpl_status CPLRT(api_busy)(cpl_runtime *prt, cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    uint32_t active = atomic_load(&rt->active_calls);
    cplrt_datum d = api_n("active_calls", active > 0u ? active : 1u);
    return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_RUNTIME_BUSY, &d, 1);
}

/* The interpreter and the "KEY=VALUE" environment entries of the runtime's later Python workers (spec/python.md
   6.5). */
cpl_status CPLRT(api_workers)(cpl_runtime *prt, const char *python, const char *const *env, uint32_t n,
                              cpl_error **error)
{
    cplrt_runtime *rt = api_rt(prt);
    if (CPLRT(workers_configure)(rt, python, env, n) != CPLRT_OK)
        return api_nomem_error(error);
    api_clear(error);
    return CPL_OK;
}
