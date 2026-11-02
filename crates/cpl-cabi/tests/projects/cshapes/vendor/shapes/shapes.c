/* shapes.c: see shapes.h. */
#include "shapes.h"
#include <stdlib.h>
#include <string.h>

struct shp_ctx {
    uint64_t touches;
};
static size_t live;

shp_ctx *shp_new(void)
{
    shp_ctx *c = (shp_ctx *)calloc(1, sizeof *c);
    if (c != NULL)
        live++;
    return c;
}

void shp_free(shp_ctx *c)
{
    if (c != NULL) {
        live--;
        free(c);
    }
}

int shp_scale(shp_ctx *c, const struct shp_size *in, const char *label, int twice, struct shp_size *out)
{
    (void)c;
    if (strcmp(label, "bad") == 0)
        return 7;
    out->w = twice ? in->w * 2 : in->w;
    out->h = in->h;
    out->flag = strcmp(label, "flag2") == 0 ? 2 : in->flag;
    return strcmp(label, "clamp") == 0 ? 1 : 0;
}

int shp_upper(const char *s, size_t len, char **out, size_t *out_len)
{
    if (len == 0)
        return -5;
    char *p = (char *)malloc(len);
    if (p == NULL)
        return -6;
    live++;
    for (size_t i = 0; i < len; i++)
        p[i] = (s[i] >= 'a' && s[i] <= 'z') ? (char)(s[i] - 'a' + 'A') : s[i];
    if (len == 7 && memcmp(s, "badutf8", 7) == 0)
        p[0] = (char)0xff;
    *out = p;
    *out_len = len;
    return 0;
}

void shp_text_free(void *p)
{
    if (p != NULL) {
        live--;
        free(p);
    }
}

int shp_is_even(int64_t n)
{
    if (n == 42)
        return 2;
    return n % 2 == 0;
}

void shp_touch(shp_ctx *c)
{
    c->touches++;
}

uint64_t shp_count(shp_ctx *c)
{
    return c->touches == 0 ? UINT64_MAX : c->touches;
}

uint8_t shp_check(uint16_t v)
{
    return v < 1000 ? 0 : v <= 2000 ? 3 : 4;
}

const char *shp_message(uint8_t code)
{
    return code == 4 ? "bad \xff message" : NULL;
}

size_t shp_live(void)
{
    return live;
}
