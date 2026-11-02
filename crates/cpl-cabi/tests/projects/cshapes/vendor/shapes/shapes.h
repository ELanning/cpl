/* shapes.h: a test library for the declarative binding vocabulary (spec/cabi.md 6.3). */
#ifndef SHAPES_H
#define SHAPES_H 1
#include <stddef.h>
#include <stdint.h>
struct shp_size {
    int32_t w;
    int16_t h;
    int flag;
};
typedef struct shp_ctx shp_ctx;
shp_ctx *shp_new(void);
void shp_free(shp_ctx *c);
/* 0, or 1 for the label "clamp" (both succeed); 7 for the label "bad"; the label "flag2" yields flag 2 */
int shp_scale(shp_ctx *c, const struct shp_size *in, const char *label, int twice, struct shp_size *out);
/* an upper-cased copy, released with shp_text_free; -5 for an empty text; "badutf8" yields a byte 0xff */
int shp_upper(const char *s, size_t len, char **out, size_t *out_len);
void shp_text_free(void *p);
/* 1 when n is even, else 0; 2 for 42 */
int shp_is_even(int64_t n);
void shp_touch(shp_ctx *c);
uint64_t shp_count(shp_ctx *c);
/* 0 for v below 1000; 3 for v up to 2000; 4 above */
uint8_t shp_check(uint16_t v);
/* a message that is not UTF-8 for 4, NULL for others */
const char *shp_message(uint8_t code);
size_t shp_live(void);
#endif
