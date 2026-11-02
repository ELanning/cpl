/* cpl_adapter.h: what the adapter of a backend-"c" binding is written against (spec/cabi.md 6.2 and
   Appendix A.2). It is cpl.h plus CPL_ADAPTER and four static inline helpers built only on the functions of
   cpl.h; it adds no entry to struct cpl_api. */
#ifndef CPL_ADAPTER_H
#define CPL_ADAPTER_H 1
#include <string.h>
#include "cpl.h"
#ifdef __cplusplus
extern "C" {
#endif

/* The visibility of the adapter contract's functions: hidden, so they are never exported from the package. */
#if defined(__GNUC__) || defined(__clang__)
#define CPL_ADAPTER __attribute__((visibility("hidden")))
#else
#define CPL_ADAPTER
#endif

/* A checked conversion of the Int argument v to [lo, hi]. Outside it (or outside int64_t) the call reports
   InvalidArgument and the helper returns CPL_FOREIGN_ERROR, which the operation function returns in turn.
   A bad handle returns that status (a binding protocol error, spec/cabi.md 5.7). */
static inline cpl_status cpl_adapter_int_arg(cpl_call *call, cpl_value v, int64_t lo, int64_t hi, int64_t *out)
{
    static const char message[] = "an integer argument is outside the range of the C parameter";
    int64_t n = 0;
    cpl_status st = cpl_int_to_i64(cpl_call_runtime(call), v, &n, NULL);
    if (st == CPL_OK && n >= lo && n <= hi) {
        *out = n;
        return CPL_OK;
    }
    if (st != CPL_OK && st != CPL_E_RANGE)
        return st;
    return cpl_call_foreign_error(call, CPL_FOREIGN_INVALID_ARGUMENT, message, sizeof message - 1);
}

/* A NUL-terminated copy of the UTF-8 span [ptr, ptr + len) into buf (cap bytes). An interior U+0000, which a C
   string cannot carry, or a buffer shorter than len + 1 reports InvalidArgument and returns CPL_FOREIGN_ERROR. */
static inline cpl_status cpl_adapter_cstr(cpl_call *call, const char *ptr, size_t len, char *buf, size_t cap)
{
    static const char interior[] = "a text argument contains U+0000, which a C string cannot carry";
    static const char too_long[] = "a text argument is longer than the C buffer";
    if (len > 0 && memchr(ptr, 0, len) != NULL)
        return cpl_call_foreign_error(call, CPL_FOREIGN_INVALID_ARGUMENT, interior, sizeof interior - 1);
    if (cap == 0 || len > cap - 1)
        return cpl_call_foreign_error(call, CPL_FOREIGN_INVALID_ARGUMENT, too_long, sizeof too_long - 1);
    if (len > 0)
        memcpy(buf, ptr, len);
    buf[len] = '\0';
    return CPL_OK;
}

/* Adds the detail foreign_code (the decimal text of code) and reports a failure of kind `kind` with the
   NUL-terminated message, which must be NULL (an empty message) or strict UTF-8. Returns CPL_FOREIGN_ERROR, or the
   status of the API call that failed (CPL_E_NOMEM; CPL_E_ARGUMENT for a message that is not UTF-8), which the
   operation function returns in turn. */
static inline cpl_status cpl_adapter_fail_code(cpl_call *call, cpl_foreign_kind kind, int64_t code,
                                               const char *message)
{
    static const char digit[] = "9876543210123456789";     /* digit[9 + r] is the digit of a remainder r in -9..9 */
    char digits[21];
    size_t at = sizeof digits;
    int64_t rest = code;
    cpl_status st;
    do {
        digits[--at] = digit[9 + rest % 10];
        rest /= 10;
    } while (rest != 0);
    if (code < 0)
        digits[--at] = '-';
    st = cpl_call_foreign_detail(call, "foreign_code", sizeof "foreign_code" - 1, digits + at, sizeof digits - at);
    if (st != CPL_OK)
        return st;
    return cpl_call_foreign_error(call, kind, message, message != NULL ? strlen(message) : 0);
}

/* Ok(v) into *result, releasing v (the caller's handle) in every case; returns the status of cpl_result_ok. */
static inline cpl_status cpl_adapter_ok(cpl_runtime *rt, cpl_value v, cpl_value *result)
{
    cpl_status st = cpl_result_ok(rt, v, result, NULL);
    (void)cpl_value_release(rt, v);
    return st;
}

#ifdef __cplusplus
}
#endif
#endif
