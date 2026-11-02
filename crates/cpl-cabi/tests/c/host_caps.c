/* fixture: cboundary */
/* host_caps.c: capabilities a C host implements (spec/toolchain.md 12.7, spec/cabi.md 5.7, semantics S77-S80): a
   host PageRead giving Ok(page), Err(Missing(id)), and Err(Unavailable); a result of the wrong type
   (binding_result_ill_typed); CPL_FOREIGN_ERROR from an operation without a ForeignError arm
   (binding_error_unrepresentable); bad callback statuses (binding_protocol); a host ImageCodec answering width 0
   (Err(ForeignError { kind: InvalidResult }) with the details of stdlib S45); re-entry: a callback that calls
   cboundary_load_text and a second export, each a fresh evaluation with no caller (semantics S79); drop called exactly
   once per capability, after its last release, and never for a user whose constructor failed. */
#include "host_common.h"
#include "cboundary.h"

/* What the host PageRead answers. */
enum mode { OK_PAGE, MISSING, UNAVAILABLE, WRONG_TYPE, FOREIGN, NO_RESULT, NO_FAILURE, BAD_STATUS, REENTER };

struct host {
    enum mode mode;
    int calls, drops;
    cpl_cap self;          /* for REENTER: the capability the re-entrant calls use */
};

static cpl_value page_id(cpl_runtime *rt, const char *s)
{
    cpl_value t = { { 0, 0 } }, id = { { 0, 0 } };
    CHECK(cpl_text_new(rt, s, strlen(s), &t, NULL) == CPL_OK);
    CHECK(cpl_wrapper_new(rt, cboundary_type_page_id(), t, &id, NULL) == CPL_OK);
    cpl_value_release(rt, t);
    return id;
}

/* reader.pages.load_text(id) through cap, with its line; the status. */
static cpl_status load_text(cpl_runtime *rt, cpl_cap cap, cpl_value id, const char *answers)
{
    cpl_value ok = { { 0, 0 } }, bad = { { 0, 0 } };
    cpl_error *err = NULL;
    cpl_status st = cboundary_load_text(rt, id, cap, &ok, &bad, NULL, &err);
    line l;
    line_call(&l, rt, "reader.pages.load_text");
    line_arg(&l, id);
    line_using(&l, answers);
    if (st == CPL_OK)
        line_value(&l, "ok", ok);
    else if (st == CPL_ERR)
        line_value(&l, "err", bad);
    else
        line_error(&l, st, err, NULL);
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    return st;
}

static const char answers_inner[] =
    "{\"label\":\"host\",\"answers\":[{\"op\":\"get\",\"args\":[{\"$type\":\"reader.pages.PageId\",\"$payload\":"
    "[\"in\"]}],\"result\":{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":[{\"$type\":\"reader.pages.Page\","
    "\"id\":{\"$type\":\"reader.pages.PageId\",\"$payload\":[\"in\"]},\"text\":\"hello\"}]}}]}";

static cpl_status get(void *user, cpl_call *call, cpl_value id, cpl_value *result)
{
    struct host *h = (struct host *)user;
    cpl_runtime *rt = cpl_call_runtime(call);
    h->calls++;
    cpl_value v = { { 0, 0 } }, text = { { 0, 0 } };
    cpl_status st = CPL_OK;
    switch (h->mode) {
    case OK_PAGE:
    case REENTER: {
        if (h->mode == REENTER) {
            /* re-entry (S79): an inner load_text over an inner host, and a second export, each a fresh evaluation */
            struct host inner = { OK_PAGE, 0, 0, { { 0, 0 } } };
            cboundary_page_read_ops ops = { sizeof(cboundary_page_read_ops), 0, get, NULL };
            cpl_cap icap = { { 0, 0 } };
            CHECK(cboundary_page_read_new(rt, &ops, &inner, &icap, NULL) == CPL_OK);
            cpl_value iid = page_id(rt, "in");
            CHECK(load_text(rt, icap, iid, answers_inner) == CPL_OK);
            cpl_value_release(rt, iid);
            CHECK(cpl_cap_release(rt, icap) == CPL_OK);
            cpl_value a = { { 0, 0 } }, b = { { 0, 0 } }, q = { { 0, 0 } };
            CHECK(cpl_int_from_i64(rt, 7, &a, NULL) == CPL_OK && cpl_int_from_i64(rt, 0, &b, NULL) == CPL_OK);
            cpl_error *e = NULL;
            line l;
            cpl_status s2 = cboundary_divide_exactly(rt, a, b, &q, NULL, &e);
            CHECK(s2 == CPL_FAULT);
            line_call(&l, rt, "reader.pages.divide_exactly");
            line_arg(&l, a);
            line_arg(&l, b);
            line_error(&l, s2, e, NULL);                /* a fault with no caller: the inner call's own */
            cpl_value_release(rt, a);
            cpl_value_release(rt, b);
        }
        st = cpl_text_new(rt, "hello", 5, &text, NULL);
        cpl_value fields[2] = { id, text };
        if (st == CPL_OK)
            st = cpl_struct_new(rt, cboundary_type_page(), fields, 2, &v, NULL);
        if (st == CPL_OK)
            st = cpl_result_ok(rt, v, result, NULL);
        break;
    }
    case MISSING: {
        cpl_value p[1] = { id };
        st = cpl_enum_new(rt, cboundary_type_store_error(), CBOUNDARY_STORE_ERROR_V_MISSING, p, 1, &v, NULL);
        if (st == CPL_OK)
            st = cpl_result_err(rt, v, result, NULL);
        break;
    }
    case UNAVAILABLE:
        st = cpl_enum_new(rt, cboundary_type_store_error(), CBOUNDARY_STORE_ERROR_V_UNAVAILABLE, NULL, 0, &v, NULL);
        if (st == CPL_OK)
            st = cpl_result_err(rt, v, result, NULL);
        break;
    case WRONG_TYPE:                                        /* a Text where a Result<Page, StoreError> is due */
        st = cpl_text_new(rt, "oops", 4, result, NULL);
        break;
    case FOREIGN:                                           /* the operation has no ForeignError arm */
        st = cpl_call_foreign_detail(call, "attempt", 7, "1", 1);
        if (st == CPL_OK)
            st = cpl_call_foreign_error(call, CPL_FOREIGN_UNAVAILABLE, "store down", 10);
        break;
    case NO_RESULT:                                         /* CPL_OK without a result */
        st = CPL_OK;
        break;
    case NO_FAILURE:                                        /* CPL_FOREIGN_ERROR without a reported failure */
        st = CPL_FOREIGN_ERROR;
        break;
    case BAD_STATUS:                                        /* no status a callback may return */
        st = (cpl_status)42;
        break;
    }
    cpl_value_release(rt, v);
    cpl_value_release(rt, text);
    return st;
}

static void drop(void *user)
{
    ((struct host *)user)->drops++;
}

static const char answer_head[] = "{\"label\":\"host\",\"answers\":[{\"op\":\"get\",\"args\":[{\"$type\":"
                                  "\"reader.pages.PageId\",\"$payload\":[\"p\"]}],";

/* load_page(PageId("p")) over a host PageRead answering in mode m; `answer` the answer of the line (NULL: a
   binding_protocol, checked by its fields). */
static void load_page(cpl_runtime *rt, enum mode m, const char *answer, cpl_status want)
{
    struct host h = { m, 0, 0, { { 0, 0 } } };
    cboundary_page_read_ops ops = { sizeof(cboundary_page_read_ops), 0, get, drop };
    cpl_cap db = { { 0, 0 } };
    CHECK(cboundary_page_read_new(rt, &ops, &h, &db, NULL) == CPL_OK);
    cpl_value id = page_id(rt, "p"), ok = { { 0, 0 } }, bad = { { 0, 0 } };
    cpl_error *err = NULL;
    cpl_status st = cboundary_load_page(rt, id, db, &ok, &bad, NULL, &err);
    CHECK(st == want);
    line l;
    line_call(&l, rt, "reader.pages.load_page");
    line_arg(&l, id);
    if (answer != NULL) {
        char using[1024];
        snprintf(using, sizeof using, "%s%s}]}", answer_head, answer);
        line_using(&l, using);
        if (st == CPL_OK)
            line_value(&l, "ok", ok);
        else if (st == CPL_ERR)
            line_value(&l, "err", bad);
        else
            line_error(&l, st, err, NULL);
    } else {
        line_protocol(&l, err);
    }
    CHECK(h.calls == 1 && h.drops == 0);
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    cpl_value_release(rt, id);
    CHECK(cpl_cap_release(rt, db) == CPL_OK);
    CHECK(h.drops == 1);                                    /* once, after the last release */
}

/* ImageCodec.dimensions: Ok(Dimensions { width: 0, height: 480 }), which Positive refuses */
static cpl_status zero_width(void *user, cpl_call *call, const uint8_t *image, size_t image_len, cpl_value *result)
{
    (void)user;
    (void)image;
    (void)image_len;
    cpl_runtime *rt = cpl_call_runtime(call);
    cpl_value f[2] = { { { 0, 0 } }, { { 0, 0 } } }, dims = { { 0, 0 } };
    cpl_status st = cpl_int_from_i64(rt, 0, &f[0], NULL);
    if (st == CPL_OK)
        st = cpl_int_from_i64(rt, 480, &f[1], NULL);
    if (st == CPL_OK)
        st = cpl_struct_new(rt, cboundary_type_dimensions(), f, 2, &dims, NULL);
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
    static const char page_p[] = "{\"$type\":\"reader.pages.Page\",\"id\":{\"$type\":\"reader.pages.PageId\","
                                 "\"$payload\":[\"p\"]},\"text\":\"hello\"}";
    char answer[512];
    snprintf(answer, sizeof answer, "\"result\":{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":[%s]}", page_p);
    load_page(rt, OK_PAGE, answer, CPL_OK);
    load_page(rt, MISSING,
              "\"result\":{\"$type\":\"Result\",\"$variant\":\"Err\",\"$payload\":[{\"$type\":"
              "\"reader.pages.StoreError\",\"$variant\":\"Missing\",\"$payload\":[{\"$type\":\"reader.pages.PageId\","
              "\"$payload\":[\"p\"]}]}]}",
              CPL_ERR);
    load_page(rt, UNAVAILABLE,
              "\"result\":{\"$type\":\"Result\",\"$variant\":\"Err\",\"$payload\":[{\"$type\":"
              "\"reader.pages.StoreError\",\"$variant\":\"Unavailable\"}]}",
              CPL_ERR);
    load_page(rt, WRONG_TYPE, "\"result\":\"oops\",\"type\":\"Text\"", CPL_FAULT);
    load_page(rt, FOREIGN,
              "\"foreign_error\":{\"kind\":\"Unavailable\",\"message\":\"store down\",\"details\":[[\"attempt\",\"1\"]]}",
              CPL_FAULT);
    load_page(rt, NO_RESULT, NULL, CPL_FAULT);
    load_page(rt, NO_FAILURE, NULL, CPL_FAULT);
    load_page(rt, BAD_STATUS, NULL, CPL_FAULT);
    /* re-entry: the inner calls print their own lines first */
    snprintf(answer, sizeof answer, "\"result\":{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":[%s]}", page_p);
    load_page(rt, REENTER, answer, CPL_OK);

    /* a host ImageCodec answering width 0: InvalidResult with the details of stdlib S45 */
    cboundary_image_codec_ops iops = { sizeof(cboundary_image_codec_ops), 0, zero_width, NULL };
    cpl_cap codec = { { 0, 0 } };
    CHECK(cboundary_image_codec_new(rt, &iops, NULL, &codec, &err) == CPL_OK);
    static const uint8_t image[1] = { 9 };
    cpl_value bytes = { { 0, 0 } }, ok = { { 0, 0 } }, bad = { { 0, 0 } };
    CHECK(cpl_bytes_new(rt, image, 1, &bytes, NULL) == CPL_OK);
    cpl_status st = cboundary_image_width(rt, image, 1, codec, &ok, &bad, NULL, &err);
    CHECK(st == CPL_ERR);
    line l;
    line_call(&l, rt, "reader.images.image_width");
    line_arg(&l, bytes);
    line_using(&l, "{\"label\":\"host\",\"answers\":[{\"op\":\"dimensions\",\"args\":[{\"$type\":\"Bytes\",\"hex\":"
                   "\"09\"}],\"result\":{\"$type\":\"Result\",\"$variant\":\"Ok\",\"$payload\":[{\"$type\":"
                   "\"reader.images.Dimensions\",\"width\":0,\"height\":480}]}}]}");
    if (st == CPL_ERR) {
        line_value(&l, "err", bad);
        cpl_value kind = { { 0, 0 } };
        uint32_t variant = 99;
        CHECK(cpl_struct_field(rt, bad, CPL_FOREIGN_ERROR_F_KIND, &kind, NULL) == CPL_OK);
        CHECK(cpl_enum_variant(rt, kind, &variant, NULL) == CPL_OK && variant == (uint32_t)CPL_FOREIGN_INVALID_RESULT);
        cpl_value_release(rt, kind);
    } else {
        line_error(&l, st, err, NULL);
        err = NULL;
    }
    cpl_value_release(rt, ok);
    cpl_value_release(rt, bad);
    cpl_value_release(rt, bytes);
    CHECK(cpl_cap_release(rt, codec) == CPL_OK);

    /* a constructor that fails never calls drop: a NULL operation, and a struct_size too small */
    struct host h = { OK_PAGE, 0, 0, { { 0, 0 } } };
    cboundary_page_read_ops nulls = { sizeof(cboundary_page_read_ops), 0, NULL, drop };
    cpl_cap c = { { 0, 0 } };
    CHECK(cboundary_page_read_new(rt, &nulls, &h, &c, &err) == CPL_E_ARGUMENT);
    CHECK(err != NULL && strcmp(cpl_error_code(err), "interop_null_pointer") == 0);
    cpl_error_release(err);
    err = NULL;
    cboundary_page_read_ops small = { 4u, 0, get, drop };
    CHECK(cboundary_page_read_new(rt, &small, &h, &c, &err) == CPL_E_VERSION);
    CHECK(err != NULL && strcmp(cpl_error_code(err), "interop_struct_size_unsupported") == 0);
    cpl_error_release(err);
    CHECK(h.drops == 0 && c.bits[0] == 0 && c.bits[1] == 0);
    return host_finish(rt);
}
