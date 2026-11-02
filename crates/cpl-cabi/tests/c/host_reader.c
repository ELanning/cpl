/* fixture: reader */
/* host_reader.c: the design's Export example from C on the verbatim corpus (spec/toolchain.md 12.7):
   reader_paginate_ints([1, 2, 3], 2) is CPL_OK with [[1, 2], [3]]; ([], 2) gives []; ([1], 0) is CPL_ERR with the
   ConstraintError of reader.pages.Positive; reader_image_width with a host ImageCodec answering
   Ok(Dimensions { 640, 480 }) gives Ok(640); a released handle is refused. */
#include "host_common.h"
#include "reader.h"

/* Whether v renders as `expected`. */
static int text_is(cpl_runtime *rt, cpl_value v, const char *expected)
{
    cpl_buf b = { NULL, 0 };
    int same = cpl_value_to_text(rt, v, &b, NULL) == CPL_OK && strcmp(b.ptr, expected) == 0;
    if (!same && b.ptr != NULL)
        printf("  rendered: %s\n", b.ptr);
    cpl_buf_release(rt, &b);
    return same;
}

/* reader_paginate_ints(items, size) with its line; the outcome kept in *ok or *bad. */
static cpl_status paginate(cpl_runtime *rt, const int64_t *xs, size_t n, int64_t size, cpl_value *ok, cpl_value *bad)
{
    cpl_value items = { { 0, 0 } }, s = { { 0, 0 } };
    CHECK(cpl_list_from_i64(rt, xs, n, &items, NULL) == CPL_OK);
    CHECK(cpl_int_from_i64(rt, size, &s, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, "reader.pages.paginate_ints");
    line_arg(&l, items);
    line_arg(&l, s);
    cpl_error *err = NULL;
    cpl_status st = reader_paginate_ints(rt, items, s, ok, bad, NULL, &err);
    if (st == CPL_OK)
        line_value(&l, "ok", *ok);
    else if (st == CPL_ERR)
        line_value(&l, "err", *bad);
    else
        line_error(&l, st, err, NULL);
    cpl_value_release(rt, items);
    cpl_value_release(rt, s);
    return st;
}

/* ImageCodec.dimensions: Ok(Dimensions { width: 640, height: 480 }) */
static cpl_status dimensions(void *user, cpl_call *call, const uint8_t *image, size_t image_len, cpl_value *result)
{
    (void)image;
    (void)image_len;
    cpl_runtime *rt = cpl_call_runtime(call);
    cpl_value f[2] = { { { 0, 0 } }, { { 0, 0 } } }, dims = { { 0, 0 } };
    cpl_status st = cpl_int_from_i64(rt, 640, &f[0], NULL);
    if (st == CPL_OK)
        st = cpl_int_from_i64(rt, 480, &f[1], NULL);
    if (st == CPL_OK)
        st = cpl_struct_new(rt, reader_type_dimensions(), f, 2, &dims, NULL);
    if (st == CPL_OK)
        st = cpl_result_ok(rt, dims, result, NULL);
    cpl_value_release(rt, dims);
    cpl_value_release(rt, f[0]);
    cpl_value_release(rt, f[1]);
    (*(int *)user)++;
    return st;
}

static int dropped;
static void drop(void *user)
{
    (void)user;
    dropped++;
}

int main(void)
{
    const cpl_library_info *info = reader_library_info();
    CHECK(strcmp(info->package, "reader") == 0 && info->abi == 0);
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    CHECK(reader_runtime_open(NULL, &rt, &err) == CPL_OK);
    cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } };

    static const int64_t three[3] = { 1, 2, 3 }, one[1] = { 1 };
    CHECK(paginate(rt, three, 3, 2, &ok, &bad) == CPL_OK);
    CHECK(text_is(rt, ok, "[[1, 2], [3]]"));
    cpl_value_release(rt, ok);
    CHECK(paginate(rt, NULL, 0, 2, &ok, &bad) == CPL_OK);
    CHECK(text_is(rt, ok, "[]"));
    cpl_value_release(rt, ok);
    CHECK(paginate(rt, one, 1, 0, &ok, &bad) == CPL_ERR);
    CHECK(text_is(rt, bad, "ConstraintError { type_name: \"reader.pages.Positive\", message: \"0 does not satisfy self > 0\" }"));
    cpl_value_release(rt, bad);

    /* a released handle is refused, and nothing runs */
    cpl_value items = { { 0, 0 } }, size = { { 0, 0 } };
    CHECK(cpl_list_from_i64(rt, three, 3, &items, NULL) == CPL_OK);
    CHECK(cpl_int_from_i64(rt, 2, &size, NULL) == CPL_OK);
    cpl_value stale = size;
    cpl_value_release(rt, size);
    line l;
    line_call(&l, rt, "reader.pages.paginate_ints");
    line_arg(&l, items);
    line_arg_json(&l, "null");
    cpl_status st = reader_paginate_ints(rt, items, stale, &ok, &bad, NULL, &err);
    CHECK(st == CPL_E_ARGUMENT);
    line_error(&l, st, err, "interop_handle_released");
    err = NULL;
    cpl_value_release(rt, items);

    /* image_width with a host ImageCodec */
    int calls = 0;
    reader_image_codec_ops ops = { sizeof(reader_image_codec_ops), 0, dimensions, drop };
    cpl_cap codec = { { 0, 0 } };
    CHECK(reader_image_codec_new(rt, &ops, &calls, &codec, &err) == CPL_OK);
    static const uint8_t image[12] = { 'I', 'M', 'G', 'T', 0x80, 0x02, 0, 0, 0xe0, 0x01, 0, 0 };
    cpl_value bytes = { { 0, 0 } };
    CHECK(cpl_bytes_new(rt, image, sizeof image, &bytes, NULL) == CPL_OK);
    line_call(&l, rt, "reader.images.image_width");
    line_arg(&l, bytes);
    line_using(&l, "{\"label\":\"host\",\"answers\":[{\"op\":\"dimensions\",\"args\":[{\"$type\":\"Bytes\",\"hex\":"
                   "\"494d475480020000e0010000\"}],\"result\":{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":"
                   "[{\"$type\":\"reader.images.Dimensions\",\"width\":640,\"height\":480}]}}]}");
    st = reader_image_width(rt, image, sizeof image, codec, &ok, &bad, NULL, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK) {
        line_value(&l, "ok", ok);
        int64_t width = 0;
        CHECK(cpl_int_to_i64(rt, ok, &width, NULL) == CPL_OK && width == 640);
        cpl_value_release(rt, ok);
    } else {
        line_error(&l, st, err, NULL);
        err = NULL;
    }
    CHECK(calls == 1);
    cpl_value_release(rt, bytes);
    CHECK(cpl_cap_release(rt, codec) == CPL_OK);
    CHECK(dropped == 1);
    return host_finish(rt);
}
