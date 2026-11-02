/* imgtest.h: the C test library of the C boundary suites (spec/toolchain.md 12.7). */
#ifndef IMGTEST_H
#define IMGTEST_H 1
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define IMGTEST_EMALFORMED (-1)
#define IMGTEST_EUNSUPPORTED (-2)
#define IMGTEST_ETOOLARGE (-3)
typedef struct imgtest_decoder imgtest_decoder;
imgtest_decoder *imgtest_decoder_new(void);
void imgtest_decoder_free(imgtest_decoder *d);
int imgtest_dimensions(imgtest_decoder *d, const uint8_t *data, size_t len, int32_t *w, int32_t *h);
int imgtest_thumbnail(imgtest_decoder *d, const uint8_t *data, size_t len, int32_t max_side, uint8_t **out,
                      size_t *out_len);
void imgtest_free(void *p);
const char *imgtest_strerror(int code);
int32_t imgtest_version(void);
size_t imgtest_live_allocations(void);
size_t imgtest_double_frees(void);
#ifdef __cplusplus
}
#endif
#endif
