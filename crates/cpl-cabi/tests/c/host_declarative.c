/* fixture: cthumbs */
/* host_declarative.c: the declarative binding `thumbs` (spec/toolchain.md 12.7, spec/cabi.md 6.3): the owned output
   buffer released exactly once on every path, the int32 argument range check (InvalidArgument before the call), the
   error map, the infallible decoder_version, and [state] created once per grant and destroyed once. The package keeps
   imgtest local (cabi.md 1.6), so the releases are checked by AddressSanitizer (a second release is a double free)
   and LeakSanitizer (a missing one is a leak) in the sanitized configuration. */
#include "host_common.h"
#include "cthumbs.h"

/* Whether text value v is exactly s. */
static int text_is(cpl_runtime *rt, cpl_value v, const char *s)
{
    cpl_strview view = { NULL, 0 };
    return cpl_text_view(rt, v, &view, NULL) == CPL_OK && view.len == strlen(s) && memcmp(view.ptr, s, view.len) == 0;
}

/* Checks the ForeignError e: its kind, its message (NULL: not compared), and its detail keys in order with the values
   of foreign_type and foreign_code (NULL: absent). */
static void check_foreign(cpl_runtime *rt, cpl_value e, cpl_foreign_kind kind, const char *message,
                          const char *foreign_code)
{
    cpl_value k = { { 0, 0 } }, m = { { 0, 0 } }, details = { { 0, 0 } };
    uint32_t variant = 99;
    CHECK(cpl_struct_field(rt, e, CPL_FOREIGN_ERROR_F_KIND, &k, NULL) == CPL_OK);
    CHECK(cpl_enum_variant(rt, k, &variant, NULL) == CPL_OK && variant == (uint32_t)kind);
    CHECK(cpl_struct_field(rt, e, CPL_FOREIGN_ERROR_F_MESSAGE, &m, NULL) == CPL_OK);
    if (message != NULL)
        CHECK(text_is(rt, m, message));
    CHECK(cpl_struct_field(rt, e, CPL_FOREIGN_ERROR_F_DETAILS, &details, NULL) == CPL_OK);
    static const char *const keys[5] = { "adapter", "capability", "operation", "foreign_type", "foreign_code" };
    const char *values[5] = { "thumbs", "cthumbs.thumbs.Thumbnails", "cthumbs.thumbs.Thumbnails.thumbnail",
                              "imgtest_thumbnail", foreign_code };
    size_t want = foreign_code != NULL ? 5u : 4u, n = 0;
    CHECK(cpl_list_len(rt, details, &n, NULL) == CPL_OK && n == want);
    for (size_t i = 0; i < n && i < want; i++) {
        cpl_value d = { { 0, 0 } }, key = { { 0, 0 } }, value = { { 0, 0 } };
        CHECK(cpl_list_get(rt, details, i, &d, NULL) == CPL_OK);
        CHECK(cpl_struct_field(rt, d, CPL_FOREIGN_DETAIL_F_KEY, &key, NULL) == CPL_OK && text_is(rt, key, keys[i]));
        CHECK(cpl_struct_field(rt, d, CPL_FOREIGN_DETAIL_F_VALUE, &value, NULL) == CPL_OK &&
              text_is(rt, value, values[i]));
        cpl_value_release(rt, value);
        cpl_value_release(rt, key);
        cpl_value_release(rt, d);
    }
    cpl_value_release(rt, details);
    cpl_value_release(rt, m);
    cpl_value_release(rt, k);
}

/* thumbnail_of(image, max_side) with max_side given in decimal, with its line: CPL_OK or CPL_ERR, with *ok or *bad
   set. */
static cpl_status thumb(cpl_runtime *rt, cpl_cap thumbs, const uint8_t *image, size_t len, const char *max_side,
                        cpl_value *ok, cpl_value *bad)
{
    cpl_value side = { { 0, 0 } }, bytes = { { 0, 0 } };
    cpl_error *err = NULL;
    CHECK(cpl_int_from_decimal(rt, max_side, strlen(max_side), &side, NULL) == CPL_OK);
    CHECK(cpl_bytes_new(rt, image, len, &bytes, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, "cthumbs.thumbs.thumbnail_of");
    line_arg(&l, bytes);
    line_arg(&l, side);
    line_using(&l, "{\"grant\":\"thumbs\"}");
    cpl_status st = cthumbs_thumbnail_of(rt, image, len, side, thumbs, ok, bad, NULL, &err);
    if (st == CPL_OK)
        line_value(&l, "ok", *ok);
    else if (st == CPL_ERR)
        line_value(&l, "err", *bad);
    else
        line_error(&l, st, err, NULL);
    cpl_value_release(rt, bytes);
    cpl_value_release(rt, side);
    return st;
}

int main(void)
{
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    CHECK(cthumbs_runtime_open(NULL, &rt, &err) == CPL_OK);
    /* two grants: each creates its own decoder, destroyed once when its capability is released */
    cpl_cap thumbs = { { 0, 0 } }, second = { { 0, 0 } };
    CHECK(cthumbs_bind_thumbs(rt, &thumbs, &err) == CPL_OK);
    CHECK(cthumbs_bind_thumbs(rt, &second, &err) == CPL_OK);

    /* a 4 by 3 image: its 2 by 2 thumbnail, copied out of the library's buffer, which is released once */
    static const uint8_t image[24] = { 'I', 'M', 'G', 'T', 4, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12 };
    static const uint8_t expected[16] = { 'I', 'M', 'G', 'T', 2, 0, 0, 0, 2, 0, 0, 0, 1, 2, 5, 6 };
    cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } };
    for (int round = 0; round < 3; round++) {
        CHECK(thumb(rt, round == 1 ? second : thumbs, image, sizeof image, "2", &ok, &bad) == CPL_OK);
        cpl_bytesview view = { NULL, 0 };
        CHECK(cpl_bytes_view(rt, ok, &view, NULL) == CPL_OK && view.len == sizeof expected &&
              memcmp(view.ptr, expected, sizeof expected) == 0);
        cpl_value_release(rt, ok);
    }

    /* the int32 range check: InvalidArgument before the call, with foreign_type and no foreign_code */
    CHECK(thumb(rt, thumbs, image, sizeof image, "2147483648", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_INVALID_ARGUMENT,
                  "argument max_side of cthumbs.thumbs.Thumbnails.thumbnail does not fit int32", NULL);
    cpl_value_release(rt, bad);
    CHECK(thumb(rt, thumbs, image, sizeof image, "-2147483649", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_INVALID_ARGUMENT, NULL, NULL);
    cpl_value_release(rt, bad);
    CHECK(thumb(rt, thumbs, image, sizeof image, "100000000000000000000000", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_INVALID_ARGUMENT, NULL, NULL);
    cpl_value_release(rt, bad);
    /* the bounds themselves fit: INT32_MAX keeps the whole image */
    CHECK(thumb(rt, thumbs, image, sizeof image, "2147483647", &ok, &bad) == CPL_OK);
    cpl_value_release(rt, ok);

    /* the error map: -1 InvalidArgument, -2 Unavailable, -3 the default Exception; foreign_code the status */
    CHECK(thumb(rt, thumbs, (const uint8_t *)"xx", 2, "2", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_INVALID_ARGUMENT, NULL, "-1");
    cpl_value_release(rt, bad);
    static const uint8_t imgx[12] = { 'I', 'M', 'G', 'X', 1, 0, 0, 0, 1, 0, 0, 0 };
    CHECK(thumb(rt, thumbs, imgx, sizeof imgx, "2", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_UNAVAILABLE, NULL, "-2");
    cpl_value_release(rt, bad);
    static const uint8_t huge[12] = { 'I', 'M', 'G', 'T', 0x21, 0x4e, 0, 0, 1, 0, 0, 0 };   /* 20001 wide */
    CHECK(thumb(rt, thumbs, huge, sizeof huge, "2", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_EXCEPTION, NULL, "-3");
    cpl_value_release(rt, bad);
    /* a library failure after a successful status check of the input: max_side 0 is malformed (-1) */
    CHECK(thumb(rt, thumbs, image, sizeof image, "0", &ok, &bad) == CPL_ERR);
    check_foreign(rt, bad, CPL_FOREIGN_INVALID_ARGUMENT, NULL, "-1");
    cpl_value_release(rt, bad);

    /* the infallible scalar */
    cpl_value version = { { 0, 0 } };
    int64_t v = 0;
    line l;
    line_call(&l, rt, "cthumbs.thumbs.decoder_version_of");
    line_using(&l, "{\"grant\":\"thumbs\"}");
    CHECK(cthumbs_decoder_version_of(rt, thumbs, &version, NULL, &err) == CPL_OK);
    line_value(&l, "value", version);
    CHECK(cpl_int_to_i64(rt, version, &v, NULL) == CPL_OK && v == 3);
    cpl_value_release(rt, version);

    CHECK(cpl_cap_release(rt, second) == CPL_OK);
    CHECK(cpl_cap_release(rt, thumbs) == CPL_OK);
    return host_finish(rt);
}
