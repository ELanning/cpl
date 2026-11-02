/* fixture: cboundary second */
/* host_two_packages.c: two packages in one host (spec/toolchain.md 12.7, spec/cabi.md 1.6, 4.5): each with its own
   runtime copy and runtimes, linked dynamically and, separately, statically; a handle of one package refused by the
   other (interop_handle_foreign), in both directions. */
#include "host_common.h"
#include "cboundary.h"
#include "second.h"

int main(void)
{
    const cpl_library_info *a = cboundary_library_info(), *b = second_library_info();
    CHECK(strcmp(a->package, "cboundary") == 0 && strcmp(b->package, "second") == 0);
    CHECK(strcmp(a->program_digest, b->program_digest) != 0);
    cpl_runtime *ra = NULL, *rb = NULL;
    CHECK(cboundary_runtime_open(NULL, &ra, NULL) == CPL_OK);
    CHECK(second_runtime_open(NULL, &rb, NULL) == CPL_OK);

    /* each package calls its own program */
    cpl_value x = { { 0, 0 } }, y = { { 0, 0 } }, q = { { 0, 0 } }, d = { { 0, 0 } };
    cpl_error *err = NULL;
    CHECK(cpl_int_from_i64(ra, 8, &x, NULL) == CPL_OK && cpl_int_from_i64(ra, 2, &y, NULL) == CPL_OK);
    line l;
    line_call(&l, ra, "reader.pages.divide_exactly");
    line_arg(&l, x);
    line_arg(&l, y);
    cpl_status st = cboundary_divide_exactly(ra, x, y, &q, NULL, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK)
        line_value(&l, "value", q);
    else
        line_error(&l, st, err, NULL);
    err = NULL;
    cpl_value n = { { 0, 0 } };
    CHECK(cpl_int_from_i64(rb, 21, &n, NULL) == CPL_OK);
    line_call_in(&l, rb, "second", "second.two.double");
    line_arg(&l, n);
    st = second_double(rb, n, &d, NULL, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK)
        line_value(&l, "value", d);
    else
        line_error(&l, st, err, NULL);
    err = NULL;
    int64_t v = 0;
    CHECK(cpl_int_to_i64(rb, d, &v, NULL) == CPL_OK && v == 42);

    /* a handle of one package is foreign to the other, both ways, and changes nothing */
    cpl_value out = { { 0, 0 } };
    line_call_in(&l, rb, "second", "second.two.double");
    line_arg_json(&l, "null");
    st = second_double(rb, x, &out, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT);
    line_error(&l, st, err, "interop_handle_foreign");
    err = NULL;
    line_call(&l, ra, "reader.pages.divide_exactly");
    line_arg_json(&l, "null");
    line_arg_json(&l, "null");
    st = cboundary_divide_exactly(ra, n, y, &out, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT);
    line_error(&l, st, err, "interop_handle_foreign");
    err = NULL;
    CHECK(cpl_value_release(ra, n) == CPL_E_ARGUMENT);              /* refused by the other package's runtime */
    CHECK(cpl_value_release(rb, x) == CPL_E_ARGUMENT);

    cpl_value_release(ra, x);
    cpl_value_release(ra, y);
    cpl_value_release(ra, q);
    cpl_value_release(rb, n);
    cpl_value_release(rb, d);
    CHECK(host_close(rb));
    return host_finish(ra);
}
