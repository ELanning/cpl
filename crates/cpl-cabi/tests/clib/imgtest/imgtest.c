/* imgtest.c: the C test library of the C boundary suites (spec/toolchain.md 12.7). An image is the four bytes IMGT,
   its width and height as uint32_t little-endian, then width * height pixel bytes. */
#include "imgtest.h"
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define IMGTEST_MAGIC 0x494d4754u
struct imgtest_decoder {
    uint32_t magic;
    _Atomic uint32_t used;     /* atomic: the operations of one grant may run concurrently (spec/cabi.md 6.2) */
};
static _Atomic size_t imgtest_live;
static _Atomic size_t imgtest_doubles;
static atomic_flag imgtest_registered = ATOMIC_FLAG_INIT;

/* The counters at exit, appended as "live <n> double_frees <n>" to the file that IMGTEST_REPORT names: the package
   keeps imgtest local (spec/cabi.md 1.6), so a host cannot call imgtest_live_allocations, and the C host suites
   read this report instead (spec/toolchain.md 12.7). */
static void imgtest_report(void)
{
    const char *path = getenv("IMGTEST_REPORT");
    if (path == NULL || path[0] == '\0')
        return;
    FILE *f = fopen(path, "a");
    if (f == NULL)
        return;
    fprintf(f, "live %zu double_frees %zu\n", imgtest_live_allocations(), imgtest_double_frees());
    fclose(f);
}

static void *imgtest_alloc(size_t n)
{
    if (!atomic_flag_test_and_set(&imgtest_registered))
        (void)atexit(imgtest_report);
    void *p = malloc(n == 0 ? 1 : n);
    if (p != NULL)
        atomic_fetch_add(&imgtest_live, 1);
    return p;
}

void imgtest_free(void *p)
{
    if (p != NULL) {
        atomic_fetch_sub(&imgtest_live, 1);
        free(p);
    }
}

imgtest_decoder *imgtest_decoder_new(void)
{
    imgtest_decoder *d = (imgtest_decoder *)imgtest_alloc(sizeof *d);
    if (d != NULL) {
        d->magic = IMGTEST_MAGIC;
        atomic_init(&d->used, 0u);
    }
    return d;
}

void imgtest_decoder_free(imgtest_decoder *d)
{
    if (d == NULL)
        return;
    if (d->magic != IMGTEST_MAGIC) {
        atomic_fetch_add(&imgtest_doubles, 1);
        return;
    }
    d->magic = 0;
    imgtest_free(d);
}

static uint32_t imgtest_u32(const uint8_t *p)
{
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

int imgtest_dimensions(imgtest_decoder *d, const uint8_t *data, size_t len, int32_t *w, int32_t *h)
{
    if (d != NULL)
        atomic_fetch_add(&d->used, 1u);
    if (len == 4 && memcmp(data, "NEGW", 4) == 0) {
        *w = -3;
        *h = 1;
        return 0;
    }
    if (len < 12)
        return IMGTEST_EMALFORMED;
    if (memcmp(data, "IMGX", 4) == 0)
        return IMGTEST_EUNSUPPORTED;
    if (memcmp(data, "IMGT", 4) != 0)
        return IMGTEST_EMALFORMED;
    uint32_t width = imgtest_u32(data + 4), height = imgtest_u32(data + 8);
    if (width > 16384 || height > 16384)
        return IMGTEST_ETOOLARGE;
    *w = (int32_t)width;
    *h = (int32_t)height;
    return 0;
}

int imgtest_thumbnail(imgtest_decoder *d, const uint8_t *data, size_t len, int32_t max_side, uint8_t **out,
                      size_t *out_len)
{
    int32_t w = 0, h = 0;
    int rc = imgtest_dimensions(d, data, len, &w, &h);
    if (rc != 0)
        return rc;
    if (w < 0 || h < 0 || max_side <= 0)
        return IMGTEST_EMALFORMED;
    if ((size_t)w * (size_t)h > len - 12)
        return IMGTEST_EMALFORMED;
    int32_t tw = w < max_side ? w : max_side, th = h < max_side ? h : max_side;
    size_t n = 12 + (size_t)tw * (size_t)th;
    uint8_t *buf = (uint8_t *)imgtest_alloc(n);
    if (buf == NULL)
        return IMGTEST_ETOOLARGE;
    memcpy(buf, "IMGT", 4);
    for (int i = 0; i < 4; i++) {
        buf[4 + i] = (uint8_t)((uint32_t)tw >> (8 * i));
        buf[8 + i] = (uint8_t)((uint32_t)th >> (8 * i));
    }
    for (int32_t y = 0; y < th; y++)
        for (int32_t x = 0; x < tw; x++)
            buf[12 + (size_t)y * (size_t)tw + (size_t)x] = data[12 + (size_t)y * (size_t)w + (size_t)x];
    *out = buf;
    *out_len = n;
    return 0;
}

const char *imgtest_strerror(int code)
{
    switch (code) {
    case 0: return "ok";
    case IMGTEST_EMALFORMED: return "malformed image";
    case IMGTEST_EUNSUPPORTED: return "unsupported image format";
    case IMGTEST_ETOOLARGE: return "image too large";
    default: return "unknown error";
    }
}

int32_t imgtest_version(void)
{
    return 3;
}

size_t imgtest_live_allocations(void)
{
    return atomic_load(&imgtest_live);
}

size_t imgtest_double_frees(void)
{
    return atomic_load(&imgtest_doubles);
}
