/* fixture: cboundary */
/* host_imports.c: CPL calls a C library (spec/toolchain.md 12.7): the grant cboundary_bind_image_codec, then
   cboundary_image_width over imgtest; the opaque decoder is created once per grant and destroyed once (LeakSanitizer
   checks it in the sanitized configuration). */
#include "host_common.h"
#include "cboundary.h"

/* cboundary_image_width(image) through the grant `codec`, with its line; the outcome kept in *ok or *bad. */
static cpl_status width_of(cpl_runtime *rt, cpl_cap codec, const uint8_t *image, size_t len, cpl_value *ok,
                           cpl_value *bad)
{
    cpl_value bytes = { { 0, 0 } };
    CHECK(cpl_bytes_new(rt, image, len, &bytes, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, "reader.images.image_width");
    line_arg(&l, bytes);
    line_using(&l, "{\"grant\":\"image_codec\"}");
    cpl_error *err = NULL;
    cpl_status st = cboundary_image_width(rt, image, len, codec, ok, bad, NULL, &err);
    if (st == CPL_OK)
        line_value(&l, "ok", *ok);
    else if (st == CPL_ERR)
        line_value(&l, "err", *bad);
    else
        line_error(&l, st, err, NULL);
    cpl_value_release(rt, bytes);
    return st;
}

int main(void)
{
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    CHECK(cboundary_runtime_open(NULL, &rt, &err) == CPL_OK);
    cpl_cap codec = { { 0, 0 } };
    CHECK(cboundary_bind_image_codec(rt, &codec, &err) == CPL_OK);
    static const uint8_t image[12] = { 'I', 'M', 'G', 'T', 0x80, 0x02, 0, 0, 0xe0, 0x01, 0, 0 };
    cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } };
    CHECK(width_of(rt, codec, image, sizeof image, &ok, &bad) == CPL_OK);
    int64_t width = 0;
    CHECK(cpl_int_to_i64(rt, ok, &width, NULL) == CPL_OK && width == 640);
    cpl_value_release(rt, ok);
    /* NEGW: the library reports a negative width, which the runtime replaces by InvalidResult (6.4) */
    CHECK(width_of(rt, codec, (const uint8_t *)"NEGW", 4, &ok, &bad) == CPL_ERR);
    cpl_value kind = { { 0, 0 } };
    uint32_t variant = 99;
    CHECK(cpl_struct_field(rt, bad, CPL_FOREIGN_ERROR_F_KIND, &kind, NULL) == CPL_OK);
    CHECK(cpl_enum_variant(rt, kind, &variant, NULL) == CPL_OK && variant == (uint32_t)CPL_FOREIGN_INVALID_RESULT);
    cpl_value_release(rt, kind);
    cpl_value_release(rt, bad);
    /* a malformed image: Exception with foreign_code -1 */
    CHECK(width_of(rt, codec, (const uint8_t *)"xx", 2, &ok, &bad) == CPL_ERR);
    cpl_value_release(rt, bad);
    /* IMGX: unsupported is Unavailable */
    static const uint8_t imgx[12] = { 'I', 'M', 'G', 'X', 1, 0, 0, 0, 1, 0, 0, 0 };
    CHECK(width_of(rt, codec, imgx, sizeof imgx, &ok, &bad) == CPL_ERR);
    CHECK(cpl_struct_field(rt, bad, CPL_FOREIGN_ERROR_F_KIND, &kind, NULL) == CPL_OK);
    CHECK(cpl_enum_variant(rt, kind, &variant, NULL) == CPL_OK && variant == (uint32_t)CPL_FOREIGN_UNAVAILABLE);
    cpl_value_release(rt, kind);
    cpl_value_release(rt, bad);
    CHECK(cpl_cap_release(rt, codec) == CPL_OK);
    /* imgtest is local to the package (cabi.md 1.6): its counters are checked from IMGTEST_REPORT at exit */
    return host_finish(rt);
}
