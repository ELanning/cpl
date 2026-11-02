/* adapters/checksum.c of the pyshop fixture: a C adapter that runs in the package's C adapter host
   (isolation "worker", spec/python.md 6.6), so that its crash ends the worker and not the host. */
#include "pyshop_adapter_checksum.h"
#include <stdlib.h>

cpl_status pyshop_adapter_checksum_init(cpl_runtime *rt, void **state, cpl_error **error)
{
    (void)rt;
    (void)error;
    *state = NULL;
    return CPL_OK;
}

void pyshop_adapter_checksum_fini(void *state)
{
    (void)state;
}

/* the text's bytes folded base 31, modulo 10 */
cpl_status pyshop_adapter_checksum_digit(void *state, cpl_call *call, const char *text, size_t text_len,
                                         cpl_value *result)
{
    (void)state;
    uint32_t sum = 0;
    for (size_t i = 0; i < text_len; i++)
        sum = sum * 31u + (uint32_t)(unsigned char)text[i];
    return cpl_int_from_i64(cpl_call_runtime(call), (int64_t)(sum % 10u), result, NULL);
}

/* a C crash: the adapter host dies of SIGABRT, which the host sees as the foreign failure Transport */
cpl_status pyshop_adapter_checksum_crash(void *state, cpl_call *call, cpl_value *result)
{
    (void)state;
    (void)call;
    (void)result;
    abort();
}
