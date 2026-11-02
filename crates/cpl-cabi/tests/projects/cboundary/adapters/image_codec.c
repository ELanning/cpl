/* adapters/image_codec.c of the cboundary fixture: a reviewed wrapper over the test library imgtest. */
#include "cboundary_adapter_image_codec.h"
#include "imgtest.h"

cpl_status cboundary_adapter_image_codec_init(cpl_runtime *rt, void **state, cpl_error **error)
{
    (void)rt;
    (void)error;
    *state = imgtest_decoder_new();                    /* an opaque foreign object owned by the adapter */
    return *state != NULL ? CPL_OK : CPL_E_NOMEM;
}

void cboundary_adapter_image_codec_fini(void *state)
{
    imgtest_decoder_free(state);
}

cpl_status cboundary_adapter_image_codec_dimensions(void *state, cpl_call *call, const uint8_t *image,
                                                   size_t image_len, cpl_value *result)
{
    cpl_runtime *rt = cpl_call_runtime(call);
    int32_t w = 0, h = 0;
    int rc = imgtest_dimensions(state, image, image_len, &w, &h);              /* the span is borrowed */
    if (rc != 0) {                                                             /* foreign_code, then the error */
        cpl_foreign_kind kind = rc == IMGTEST_EUNSUPPORTED ? CPL_FOREIGN_UNAVAILABLE : CPL_FOREIGN_EXCEPTION;
        return cpl_adapter_fail_code(call, kind, rc, imgtest_strerror(rc));
    }
    cpl_value f[2] = { { { 0, 0 } }, { { 0, 0 } } }, dims = { { 0, 0 } };
    cpl_status st = cpl_int_from_i64(rt, w, &f[0], NULL);
    if (st == CPL_OK)
        st = cpl_int_from_i64(rt, h, &f[1], NULL);
    if (st == CPL_OK)
        st = cpl_struct_new(rt, cboundary_type_dimensions(), f, 2, &dims, NULL);   /* Positive is checked in 6.4 */
    if (st == CPL_OK)
        st = cpl_adapter_ok(rt, dims, result);                                     /* Ok(dims); releases dims */
    (void)cpl_value_release(rt, f[0]);                                             /* {{0, 0}}: a no-op */
    (void)cpl_value_release(rt, f[1]);
    return st;
}
