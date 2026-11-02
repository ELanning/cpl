/* fixture: cboundary */
/* host_basic.c: a C host calls CPL through cboundary_* (spec/toolchain.md 12.7): paginate_ints in its handle and
   _i64 forms, divide_exactly (a value, and the faults of its requires), positive_size (a Text span in, a Result out,
   a span that is not UTF-8 refused), clean (a struct in and out), label (an enum argument), load_page and load_text
   over a host PageRead, image_width over a host ImageCodec (a Bytes parameter); the value API on the results. Every
   call prints its line, which c_hosts.rs compares with the evaluator. */
#include "host_common.h"
#include "cboundary.h"

/* The outcome of a split export into its line. */
static void outcome(line *l, cpl_status st, cpl_value ok, cpl_value bad, cpl_error *err)
{
    if (st == CPL_OK)
        line_value(l, "ok", ok);
    else if (st == CPL_ERR)
        line_value(l, "err", bad);
    else
        line_error(l, st, err, NULL);
}

static cpl_value int_of(cpl_runtime *rt, int64_t n)
{
    cpl_value v = { { 0, 0 } };
    CHECK(cpl_int_from_i64(rt, n, &v, NULL) == CPL_OK);
    return v;
}

static cpl_value text_of(cpl_runtime *rt, const char *s)
{
    cpl_value v = { { 0, 0 } };
    CHECK(cpl_text_new(rt, s, strlen(s), &v, NULL) == CPL_OK);
    return v;
}

static cpl_value page_of(cpl_runtime *rt, const char *id, const char *text)
{
    cpl_value t = text_of(rt, id), pid = { { 0, 0 } }, page = { { 0, 0 } };
    CHECK(cpl_wrapper_new(rt, cboundary_type_page_id(), t, &pid, NULL) == CPL_OK);
    cpl_value fields[2] = { pid, text_of(rt, text) };
    CHECK(cpl_struct_new(rt, cboundary_type_page(), fields, 2, &page, NULL) == CPL_OK);
    cpl_value_release(rt, fields[1]);
    cpl_value_release(rt, pid);
    cpl_value_release(rt, t);
    return page;
}

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

static cpl_status paginate(cpl_runtime *rt, const int64_t *xs, size_t n, int64_t size, int i64)
{
    cpl_value items = { { 0, 0 } }, s = int_of(rt, size), ok = { { 0, 0 } }, bad = { { 0, 0 } };
    cpl_error *err = NULL;
    CHECK(cpl_list_from_i64(rt, xs, n, &items, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, "reader.pages.paginate_ints");
    line_arg(&l, items);
    line_arg(&l, s);
    cpl_status st = i64 ? cboundary_paginate_ints_i64(rt, items, size, &ok, &bad, NULL, &err)
                        : cboundary_paginate_ints(rt, items, s, &ok, &bad, NULL, &err);
    outcome(&l, st, ok, bad, err);
    if (st == CPL_OK) {
        /* the value API: a list of lists of Int */
        size_t pages = 0, first = 0;
        cpl_value p0 = { { 0, 0 } };
        cpl_kind kind = 0;
        CHECK(cpl_value_kind(rt, ok, &kind, NULL) == CPL_OK && kind == CPL_KIND_LIST);
        CHECK(cpl_list_len(rt, ok, &pages, NULL) == CPL_OK);
        if (pages > 0) {
            CHECK(cpl_list_get(rt, ok, 0, &p0, NULL) == CPL_OK);
            int64_t buf[8];
            CHECK(cpl_list_to_i64(rt, p0, buf, 8, &first, NULL) == CPL_OK && first > 0 && buf[0] == xs[0]);
            cpl_value_release(rt, p0);
        }
    }
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    cpl_value_release(rt, items);
    cpl_value_release(rt, s);
    return st;
}

static cpl_status divide(cpl_runtime *rt, int64_t a, int64_t b, int64_t *q)
{
    cpl_value va = int_of(rt, a), vb = int_of(rt, b), out = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, "reader.pages.divide_exactly");
    line_arg(&l, va);
    line_arg(&l, vb);
    cpl_status st = cboundary_divide_exactly(rt, va, vb, &out, NULL, &err);
    if (st == CPL_OK) {
        line_value(&l, "value", out);
        CHECK(cpl_int_to_i64(rt, out, q, NULL) == CPL_OK);
    } else {
        if (st == CPL_FAULT)
            CHECK(strcmp(cpl_error_code(err), "contract_violation") == 0);
        line_error(&l, st, err, NULL);
    }
    cpl_value_release(rt, out);
    cpl_value_release(rt, va);
    cpl_value_release(rt, vb);
    return st;
}

static cpl_status positive(cpl_runtime *rt, const char *text, size_t len, const char *expect)
{
    cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } }, t = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, "reader.pages.positive_size");
    if (cpl_text_new(rt, text, len, &t, NULL) == CPL_OK)
        line_arg(&l, t);
    else
        line_arg_json(&l, "null");
    cpl_status st = cboundary_positive_size(rt, text, len, &ok, &bad, NULL, &err);
    if (st == CPL_OK || st == CPL_ERR)
        outcome(&l, st, ok, bad, err);
    else
        line_error(&l, st, err, expect);
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    cpl_value_release(rt, t);
    return st;
}

/* label(result) for the Result value r */
static void label(cpl_runtime *rt, cpl_value r, const char *expected)
{
    cpl_value out = { { 0, 0 } };
    cpl_error *err = NULL;
    line l;
    line_call(&l, rt, "reader.pages.label");
    line_arg(&l, r);
    cpl_status st = cboundary_label(rt, r, &out, NULL, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK) {
        line_value(&l, "value", out);
        cpl_strview view = { NULL, 0 };
        CHECK(cpl_text_view(rt, out, &view, NULL) == CPL_OK && strcmp(view.ptr, expected) == 0);
    } else {
        line_error(&l, st, err, NULL);
    }
    cpl_value_release(rt, out);
}

/* PageRead.get: Ok(Page { id, text: "hello" }) for "p1", Err(Missing(id)) otherwise */
static cpl_status get(void *user, cpl_call *call, cpl_value id, cpl_value *result)
{
    cpl_runtime *rt = cpl_call_runtime(call);
    cpl_value inner = { { 0, 0 } }, v = { { 0, 0 } };
    cpl_strview s = { NULL, 0 };
    cpl_status st = cpl_wrapper_inner(rt, id, &inner, NULL);
    if (st == CPL_OK)
        st = cpl_text_view(rt, inner, &s, NULL);
    if (st == CPL_OK && s.len == 2 && memcmp(s.ptr, "p1", 2) == 0) {
        cpl_value text = { { 0, 0 } };
        st = cpl_text_new(rt, "hello", 5, &text, NULL);
        cpl_value fields[2] = { id, text };
        if (st == CPL_OK)
            st = cpl_struct_new(rt, cboundary_type_page(), fields, 2, &v, NULL);
        if (st == CPL_OK)
            st = cpl_result_ok(rt, v, result, NULL);
        cpl_value_release(rt, text);
    } else if (st == CPL_OK) {
        cpl_value missing[1] = { id };
        st = cpl_enum_new(rt, cboundary_type_store_error(), CBOUNDARY_STORE_ERROR_V_MISSING, missing, 1, &v, NULL);
        if (st == CPL_OK)
            st = cpl_result_err(rt, v, result, NULL);
    }
    cpl_value_release(rt, v);
    cpl_value_release(rt, inner);
    (*(int *)user)++;
    return st;
}

static int drops;
static void drop(void *user)
{
    (void)user;
    drops++;
}

/* The answers of the host's PageRead for the line (value JSON). */
static const char page_read_answers[] =
    "{\"label\":\"host\",\"answers\":["
    "{\"op\":\"get\",\"args\":[{\"$type\":\"reader.pages.PageId\",\"$payload\":[\"p1\"]}],\"result\":{\"$type\":"
    "\"Result\",\"$variant\":\"Ok\",\"$payload\":[{\"$type\":\"reader.pages.Page\",\"id\":{\"$type\":"
    "\"reader.pages.PageId\",\"$payload\":[\"p1\"]},\"text\":\"hello\"}]}},"
    "{\"op\":\"get\",\"args\":[{\"$type\":\"reader.pages.PageId\",\"$payload\":[\"zz\"]}],\"result\":{\"$type\":"
    "\"Result\",\"$variant\":\"Err\",\"$payload\":[{\"$type\":\"reader.pages.StoreError\",\"$variant\":\"Missing\","
    "\"$payload\":[{\"$type\":\"reader.pages.PageId\",\"$payload\":[\"zz\"]}]}]}}]}";

static void load(cpl_runtime *rt, cpl_cap db, const char *function, const char *id, cpl_status want)
{
    cpl_value t = text_of(rt, id), pid = { { 0, 0 } }, ok = { { 0, 0 } }, bad = { { 0, 0 } };
    cpl_error *err = NULL;
    CHECK(cpl_wrapper_new(rt, cboundary_type_page_id(), t, &pid, NULL) == CPL_OK);
    line l;
    line_call(&l, rt, function);
    line_arg(&l, pid);
    line_using(&l, page_read_answers);
    cpl_status st = strcmp(function, "reader.pages.load_page") == 0
                        ? cboundary_load_page(rt, pid, db, &ok, &bad, NULL, &err)
                        : cboundary_load_text(rt, pid, db, &ok, &bad, NULL, &err);
    CHECK(st == want);
    outcome(&l, st, ok, bad, err);
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    cpl_value_release(rt, pid);
    cpl_value_release(rt, t);
}

/* ImageCodec.dimensions: Ok(Dimensions { 3, 2 }) */
static cpl_status dimensions(void *user, cpl_call *call, const uint8_t *image, size_t image_len, cpl_value *result)
{
    (void)user;
    cpl_runtime *rt = cpl_call_runtime(call);
    CHECK(image_len == 3 && image[0] == 0 && image[1] == 0xff && image[2] == 7);
    cpl_value f[2] = { int_of(rt, 3), int_of(rt, 2) }, dims = { { 0, 0 } };
    cpl_status st = cpl_struct_new(rt, cboundary_type_dimensions(), f, 2, &dims, NULL);
    if (st == CPL_OK)
        st = cpl_result_ok(rt, dims, result, NULL);
    cpl_value_release(rt, dims);
    cpl_value_release(rt, f[0]);
    cpl_value_release(rt, f[1]);
    return st;
}

int main(void)
{
    cpl_runtime *rt = NULL;
    cpl_error *err = NULL;
    CHECK(cboundary_runtime_open(NULL, &rt, &err) == CPL_OK);

    static const int64_t three[3] = { 1, 2, 3 }, five[5] = { 1, 2, 3, 4, 5 }, one[1] = { 1 };
    CHECK(paginate(rt, three, 3, 2, 0) == CPL_OK);
    CHECK(paginate(rt, NULL, 0, 2, 0) == CPL_OK);
    CHECK(paginate(rt, one, 1, 0, 0) == CPL_ERR);
    CHECK(paginate(rt, five, 5, 2, 1) == CPL_OK);
    CHECK(paginate(rt, five, 5, -3, 1) == CPL_ERR);

    int64_t q = 0;
    CHECK(divide(rt, 6, 3, &q) == CPL_OK && q == 2);
    CHECK(divide(rt, -9, 3, &q) == CPL_OK && q == -3);
    CHECK(divide(rt, 7, 2, &q) == CPL_FAULT);
    CHECK(divide(rt, 7, 0, &q) == CPL_FAULT);

    CHECK(positive(rt, "  42 ", 5, NULL) == CPL_OK);
    CHECK(positive(rt, "0", 1, NULL) == CPL_ERR);
    CHECK(positive(rt, "abc", 3, NULL) == CPL_ERR);
    CHECK(positive(rt, "4\xff", 2, "interop_utf8_invalid") == CPL_E_ARGUMENT);

    /* clean: a struct in and out; the value API on the result */
    cpl_value page = page_of(rt, "p", "  hi  "), out = { { 0, 0 } };
    line l;
    line_call(&l, rt, "reader.pages.clean");
    line_arg(&l, page);
    cpl_status st = cboundary_clean(rt, page, &out, NULL, &err);
    CHECK(st == CPL_OK);
    if (st == CPL_OK) {
        line_value(&l, "value", out);
        CHECK(text_is(rt, out, "Page { id: PageId(\"p\"), text: \"hi\" }"));
        cpl_value field = { { 0, 0 } }, inner = { { 0, 0 } };
        cpl_type type = 0;
        int32_t equal = 0;
        CHECK(cpl_value_type(rt, out, &type, NULL) == CPL_OK && type == cboundary_type_page());
        CHECK(cpl_struct_field(rt, out, CBOUNDARY_PAGE_F_ID, &field, NULL) == CPL_OK);
        CHECK(cpl_wrapper_inner(rt, field, &inner, NULL) == CPL_OK && text_is(rt, inner, "\"p\""));
        CHECK(cpl_value_equal(rt, out, page, &equal, NULL) == CPL_OK && equal == 0);
        cpl_value_release(rt, inner);
        cpl_value_release(rt, field);
    } else {
        line_error(&l, st, err, NULL);
        err = NULL;
    }
    cpl_value_release(rt, out);

    /* label: an enum argument in each of its shapes */
    cpl_value r = { { 0, 0 } }, se = { { 0, 0 } }, pid = { { 0, 0 } }, t = text_of(rt, "x");
    CHECK(cpl_result_ok(rt, page, &r, NULL) == CPL_OK);
    label(rt, r, "  hi  ");
    cpl_value_release(rt, r);
    CHECK(cpl_wrapper_new(rt, cboundary_type_page_id(), t, &pid, NULL) == CPL_OK);
    cpl_value missing[1] = { pid };
    CHECK(cpl_enum_new(rt, cboundary_type_store_error(), CBOUNDARY_STORE_ERROR_V_MISSING, missing, 1, &se, NULL) == CPL_OK);
    CHECK(cpl_result_err(rt, se, &r, NULL) == CPL_OK);
    label(rt, r, "Missing page");
    cpl_value_release(rt, r);
    cpl_value_release(rt, se);
    CHECK(cpl_enum_new(rt, cboundary_type_store_error(), CBOUNDARY_STORE_ERROR_V_UNAVAILABLE, NULL, 0, &se, NULL) == CPL_OK);
    CHECK(cpl_result_err(rt, se, &r, NULL) == CPL_OK);
    label(rt, r, "Storage unavailable");
    cpl_value_release(rt, r);
    cpl_value_release(rt, se);
    cpl_value_release(rt, pid);
    cpl_value_release(rt, t);
    cpl_value_release(rt, page);

    /* load_page and load_text over a host PageRead */
    int calls = 0;
    cboundary_page_read_ops ops = { sizeof(cboundary_page_read_ops), 0, get, drop };
    cpl_cap db = { { 0, 0 } };
    CHECK(cboundary_page_read_new(rt, &ops, &calls, &db, &err) == CPL_OK);
    load(rt, db, "reader.pages.load_page", "p1", CPL_OK);
    load(rt, db, "reader.pages.load_page", "zz", CPL_ERR);
    load(rt, db, "reader.pages.load_text", "p1", CPL_OK);
    CHECK(calls == 3);
    CHECK(cpl_cap_release(rt, db) == CPL_OK);
    CHECK(drops == 1);

    /* image_width over a host ImageCodec: a Bytes parameter with a zero and a high byte */
    cboundary_image_codec_ops codec_ops = { sizeof(cboundary_image_codec_ops), 0, dimensions, NULL };
    cpl_cap codec = { { 0, 0 } };
    CHECK(cboundary_image_codec_new(rt, &codec_ops, NULL, &codec, &err) == CPL_OK);
    static const uint8_t image[3] = { 0, 0xff, 7 };
    cpl_value bytes = { { 0, 0 } }, ok = { { 0, 0 } }, bad = { { 0, 0 } };
    CHECK(cpl_bytes_new(rt, image, sizeof image, &bytes, NULL) == CPL_OK);
    line_call(&l, rt, "reader.images.image_width");
    line_arg(&l, bytes);
    line_using(&l, "{\"label\":\"host\",\"answers\":[{\"op\":\"dimensions\",\"args\":[{\"$type\":\"Bytes\",\"hex\":"
                   "\"00ff07\"}],\"result\":{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":[{\"$type\":"
                   "\"reader.images.Dimensions\",\"width\":3,\"height\":2}]}}]}");
    st = cboundary_image_width(rt, image, sizeof image, codec, &ok, &bad, NULL, &err);
    CHECK(st == CPL_OK);
    outcome(&l, st, ok, bad, err);
    err = NULL;
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    cpl_value_release(rt, bytes);
    CHECK(cpl_cap_release(rt, codec) == CPL_OK);
    return host_finish(rt);
}
