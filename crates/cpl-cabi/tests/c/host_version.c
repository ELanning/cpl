/* fixture: cboundary */
/* host_version.c: versioning of the C boundary (spec/toolchain.md 12.7, spec/cabi.md 4.2): CPL_E_VERSION
   (interop_version_mismatch) for options.api_version with major 1 and with minor 2; interop_struct_size_unsupported
   for a struct_size too small (and too large); a smaller but sufficient struct_size accepted with defaults; the fields
   of cboundary_library_info; CPL_CALL_UNCHECKED_CONTRACTS refused (interop_option_unsupported). The compile-time
   refusal of a mismatched header (#error) is c_hosts.rs's compile-must-fail check. */
#include <stddef.h>
#include "host_common.h"
#include "cboundary.h"

/* Whether s is 64 lowercase hex digits. */
static int is_digest(const char *s)
{
    if (s == NULL || strlen(s) != 64)
        return 0;
    for (const char *p = s; *p; p++)
        if (!((*p >= '0' && *p <= '9') || (*p >= 'a' && *p <= 'f')))
            return 0;
    return 1;
}

/* Opens a runtime with options o: the status, the runtime closed again when it opened. */
static cpl_status open_with(const cpl_runtime_options *o, const char *expect)
{
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    cpl_status st = cboundary_runtime_open(o, &rt, &err);
    if (st == CPL_OK) {
        CHECK(rt != NULL && err == NULL);
        CHECK(cpl_runtime_close(rt, 0, NULL) == CPL_OK);
        return st;
    }
    CHECK(rt == NULL && err != NULL);
    if (err != NULL) {
        CHECK(cpl_error_status(err) == st);
        CHECK(strcmp(cpl_error_code(err), expect) == 0);
        CHECK(strcmp(cpl_error_category(err), "interop") == 0);
        cpl_strview j = cpl_error_json(err);
        printf("{\"open\":%.*s}\n", (int)j.len, j.ptr);
        cpl_error_release(err);
    }
    return st;
}

int main(void)
{
    /* the library information */
    const cpl_library_info *info = cboundary_library_info();
    CHECK(info->struct_size == sizeof(cpl_library_info));
    CHECK(info->api_version == CPL_API_VERSION);
    CHECK(info->abi == CBOUNDARY_ABI && info->abi == 1u);
    CHECK(info->release == 0u && info->holes == 0u);
    CHECK(strcmp(info->package, "cboundary") == 0 && strcmp(info->prefix, "cboundary") == 0);
    CHECK(strcmp(info->edition, "0.1") == 0 && strcmp(info->toolchain, "0.1.0") == 0);
    CHECK(strcmp(info->runtime_version, "0.1.0") == 0);
    CHECK(is_digest(info->runtime_digest) && is_digest(info->program_digest) && is_digest(info->exports_digest));
    CHECK(info->target != NULL && strlen(info->target) > 0 && info->compiler != NULL && strlen(info->compiler) > 0);
    CHECK(strcmp(info->profile, "optimized") == 0 || strcmp(info->profile, "debug") == 0);
    printf("{\"library\":{\"package\":\"%s\",\"abi\":%u,\"target\":\"%s\",\"compiler\":\"%s\",\"profile\":\"%s\"}}\n",
           info->package, info->abi, info->target, info->compiler, info->profile);

    /* the API version: another major, a newer minor */
    cpl_runtime_options o = CPL_RUNTIME_OPTIONS_INIT;
    o.api_version = (1u << 16) | 0u;
    CHECK(open_with(&o, "interop_version_mismatch") == CPL_E_VERSION);
    o.api_version = (0u << 16) | 2u;
    CHECK(open_with(&o, "interop_version_mismatch") == CPL_E_VERSION);
    o.api_version = CPL_API_VERSION;
    CHECK(open_with(&o, "") == CPL_OK);
    /* the struct size: too small, too large, smaller but sufficient (the rest defaulted) */
    o.struct_size = 4u;
    CHECK(open_with(&o, "interop_struct_size_unsupported") == CPL_E_VERSION);
    o.struct_size = (uint32_t)sizeof(cpl_runtime_options) + 8u;
    CHECK(open_with(&o, "interop_struct_size_unsupported") == CPL_E_VERSION);
    o.struct_size = (uint32_t)offsetof(cpl_runtime_options, allocator);
    o.max_call_depth = 0u;                   /* past struct_size: not read, so the default 10000 applies */
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    CHECK(cboundary_runtime_open(&o, &rt, &err) == CPL_OK);
    cpl_value a = { { 0, 0 } }, b = { { 0, 0 } }, out = { { 0, 0 } };
    CHECK(cpl_int_from_i64(rt, 6, &a, NULL) == CPL_OK && cpl_int_from_i64(rt, 3, &b, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, "reader.pages.divide_exactly");
    line_arg(&l, a);
    line_arg(&l, b);
    cpl_status st = cboundary_divide_exactly(rt, a, b, &out, NULL, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK)
        line_value(&l, "value", out);
    else
        line_error(&l, st, err, NULL);
    cpl_value_release(rt, out);
    /* flags of the runtime options: none in 0.1 */
    cpl_runtime_options f = CPL_RUNTIME_OPTIONS_INIT;
    f.flags = 1u;
    CHECK(open_with(&f, "interop_option_unsupported") == CPL_E_UNSUPPORTED);

    /* call options: CPL_CALL_UNCHECKED_CONTRACTS is reserved for Milestone 5; a struct_size out of range */
    cpl_call_options co = CPL_CALL_OPTIONS_INIT;
    co.flags = CPL_CALL_UNCHECKED_CONTRACTS;
    err = NULL;
    line_call(&l, rt, "reader.pages.divide_exactly");
    line_arg(&l, a);
    line_arg(&l, b);
    st = cboundary_divide_exactly(rt, a, b, &out, &co, &err);
    CHECK(st == CPL_E_UNSUPPORTED);
    line_error(&l, st, err, "interop_option_unsupported");
    co = (cpl_call_options)CPL_CALL_OPTIONS_INIT;
    co.struct_size = 4u;
    err = NULL;
    line_call(&l, rt, "reader.pages.divide_exactly");
    line_arg(&l, a);
    line_arg(&l, b);
    st = cboundary_divide_exactly(rt, a, b, &out, &co, &err);
    CHECK(st == CPL_E_VERSION);
    line_error(&l, st, err, "interop_struct_size_unsupported");
    /* a sufficient call-options struct: the time budget alone */
    co = (cpl_call_options)CPL_CALL_OPTIONS_INIT;
    co.struct_size = (uint32_t)(offsetof(cpl_call_options, time_budget_ms) + sizeof co.time_budget_ms);
    co.time_budget_ms = 60000u;
    err = NULL;
    st = cboundary_divide_exactly(rt, a, b, &out, &co, &err);
    CHECK(st == CPL_OK);
    cpl_value_release(rt, out);
    cpl_error_release(err);
    cpl_value_release(rt, a);
    cpl_value_release(rt, b);
    return host_finish(rt);
}
