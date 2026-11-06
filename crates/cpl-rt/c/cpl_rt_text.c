/* cpl_rt_text.c: Text and Bytes (spec/cabi.md 2.3, 2.6, 2.7): strict UTF-8 validation by Unicode Table 3-7 with
   Utf8Error::valid_up_to offsets, cached scalar counts, the immortal "" and empty Bytes, `++` of both, and the builtins
   of Text, Bytes, std.text, and std.bytes, each transcribing the evaluator's function of its registry id (trim from the
   generated White_Space ranges, ASCII-only case mapping: stdlib.md S22, S23, S47). Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <string.h>

/* ---- the immortal statics (2.3) -------------------------------------------------------------------------------- */

static struct {
    cplrt_obj hdr;
    size_t bytes;
    size_t scalars;
    char utf8[1];
} text_empty = { { CPLRT_TEXT, 0u, 0u, 0u, { .live = { CPLRT_RC_IMMORTAL, 0u } } }, 0u, 0u, { 0 } };

static struct {
    cplrt_obj hdr;
    size_t len;
    uint8_t data[1];
} text_empty_bytes = { { CPLRT_BYTES, 0u, 0u, 0u, { .live = { CPLRT_RC_IMMORTAL, 0u } } }, 0u, { 0 } };

cplrt_w CPLRT(empty_text)(void)
{
    return cplrt_word_of(&text_empty.hdr);
}

cplrt_w CPLRT(empty_bytes)(void)
{
    return cplrt_word_of(&text_empty_bytes.hdr);
}

/* ---- construction ---------------------------------------------------------------------------------------------- */

cplrt_text *CPLRT(text_alloc)(cplrt_runtime *rt, size_t bytes, size_t scalars)
{
    if (bytes > SIZE_MAX - sizeof(cplrt_text) - 1u)
        return NULL;
    cplrt_text *t = (cplrt_text *)CPLRT(obj_new)(rt, CPLRT_TEXT, 0u, sizeof(cplrt_text) + bytes + 1u);
    if (t == NULL)
        return NULL;
    t->bytes = bytes;
    t->scalars = scalars;
    t->utf8[bytes] = '\0';
    return t;
}

cplrt_bytes *CPLRT(bytes_alloc)(cplrt_runtime *rt, size_t len)
{
    if (len > SIZE_MAX - sizeof(cplrt_bytes))
        return NULL;
    cplrt_bytes *b = (cplrt_bytes *)CPLRT(obj_new)(rt, CPLRT_BYTES, 0u, sizeof(cplrt_bytes) + len);
    if (b == NULL)
        return NULL;
    b->len = len;
    return b;
}

static size_t text_count_scalars(const char *s, size_t len)
{
    size_t n = 0;
    for (size_t i = 0; i < len; i++)
        n += ((unsigned char)s[i] & 0xC0u) != 0x80u;
    return n;
}

int CPLRT(text_make)(cplrt_runtime *rt, const char *utf8, size_t len, cplrt_w *out)
{
    if (len == 0) {
        CPLRT_SET(*out, CPLRT(empty_text)());
        return CPLRT_OK;
    }
    cplrt_text *t = CPLRT(text_alloc)(rt, len, text_count_scalars(utf8, len));
    if (t == NULL)
        return CPLRT_NOMEM;
    memcpy(t->utf8, utf8, len);
    CPLRT_SET(*out, cplrt_word_of(&t->hdr));
    return CPLRT_OK;
}

int CPLRT(bytes_make)(cplrt_runtime *rt, const uint8_t *data, size_t len, cplrt_w *out)
{
    if (len == 0) {
        CPLRT_SET(*out, CPLRT(empty_bytes)());
        return CPLRT_OK;
    }
    cplrt_bytes *b = CPLRT(bytes_alloc)(rt, len);
    if (b == NULL)
        return CPLRT_NOMEM;
    memcpy(b->data, data, len);
    CPLRT_SET(*out, cplrt_word_of(&b->hdr));
    return CPLRT_OK;
}

/* mirrors core::str::from_utf8 (the strict decoding of cpl_eval::builtins::bytes::to_text): the well-formed byte
   sequences of Unicode Table 3-7, so no surrogate, no overlong form, nothing above U+10FFFF; on failure the offset of
   the first byte of the first invalid or truncated sequence, Utf8Error::valid_up_to. */
size_t CPLRT(utf8_clip)(char *s, size_t len)
{
    size_t start = len;
    /* back over the continuation bytes of the last sequence, to its lead byte */
    while (start > 0 && len - start < 4u && ((unsigned char)s[start - 1] & 0xC0u) == 0x80u)
        start--;
    if (start > 0) {
        unsigned char lead = (unsigned char)s[start - 1];
        size_t need = lead < 0x80u ? 1u : lead >= 0xF0u ? 4u : lead >= 0xE0u ? 3u : lead >= 0xC0u ? 2u : 0u;
        if (need == 0 || len - (start - 1u) < need)
            len = start - 1u; /* the last sequence is cut (or a stray continuation): drop it */
    } else {
        len = 0; /* nothing but continuation bytes */
    }
    s[len] = '\0';
    return len;
}

int CPLRT(utf8_valid)(const char *s, size_t len, size_t *valid_up_to, size_t *scalars)
{
    const unsigned char *p = (const unsigned char *)s;
    size_t i = 0, count = 0;
    while (i < len) {
        unsigned c = p[i];
        if (c < 0x80u) {
            i++;
            count++;
            continue;
        }
        size_t need;
        unsigned lo = 0x80u, hi = 0xBFu;
        if (c >= 0xC2u && c <= 0xDFu)
            need = 1;
        else if (c == 0xE0u) {
            need = 2;
            lo = 0xA0u;
        } else if ((c >= 0xE1u && c <= 0xECu) || c == 0xEEu || c == 0xEFu)
            need = 2;
        else if (c == 0xEDu) {
            need = 2;
            hi = 0x9Fu;
        } else if (c == 0xF0u) {
            need = 3;
            lo = 0x90u;
        } else if (c >= 0xF1u && c <= 0xF3u)
            need = 3;
        else if (c == 0xF4u) {
            need = 3;
            hi = 0x8Fu;
        } else
            break;
        if (len - i - 1u < need || p[i + 1] < lo || p[i + 1] > hi)
            break;
        size_t k = 2;
        while (k <= need && (p[i + k] & 0xC0u) == 0x80u)
            k++;
        if (k <= need)
            break;
        i += need + 1u;
        count++;
    }
    if (i < len) {
        if (valid_up_to != NULL)
            *valid_up_to = i;
        return 0;
    }
    if (scalars != NULL)
        *scalars = count;
    return 1;
}

/* The scalar that starts at p (valid UTF-8) and its length in bytes. */
static uint32_t text_decode(const unsigned char *p, size_t *len)
{
    unsigned c = p[0];
    if (c < 0x80u) {
        *len = 1;
        return c;
    }
    if (c < 0xE0u) {
        *len = 2;
        return ((uint32_t)(c & 0x1Fu) << 6) | (p[1] & 0x3Fu);
    }
    if (c < 0xF0u) {
        *len = 3;
        return ((uint32_t)(c & 0x0Fu) << 12) | ((uint32_t)(p[1] & 0x3Fu) << 6) | (p[2] & 0x3Fu);
    }
    *len = 4;
    return ((uint32_t)(c & 0x07u) << 18) | ((uint32_t)(p[1] & 0x3Fu) << 12) | ((uint32_t)(p[2] & 0x3Fu) << 6) |
           (p[3] & 0x3Fu);
}

/* ---- `++` (2.6) ------------------------------------------------------------------------------------------------- */

/* Append assignments retain spare allocation space without changing the Text/Bytes layout. The allocator's hidden
   size header is only read for unique dynamic objects, never immortal statics. Copy both inputs before releasing
   the old slot: other may be that slot itself. A failed allocation leaves it and every snapshot unchanged. */
static int text_bytes_append_owned(cplrt_cx *cx, cplrt_w *slot, cplrt_w other, int is_text)
{
    cplrt_obj *x = cplrt_obj_of(*slot);
    const cplrt_obj *y = cplrt_obj_of(other);
    size_t nx = is_text ? ((const cplrt_text *)x)->bytes : ((const cplrt_bytes *)x)->len;
    size_t ny = is_text ? ((const cplrt_text *)y)->bytes : ((const cplrt_bytes *)y)->len;
    if (ny == 0)
        return CPLRT_OK;
    if (nx == 0) {
        CPLRT_COPY(*slot, other);
        return CPLRT_OK;
    }
    size_t prefix = is_text ? sizeof(cplrt_text) : sizeof(cplrt_bytes);
    size_t overhead = prefix + (is_text ? 1u : 0u);
    if (ny > SIZE_MAX - overhead || nx > SIZE_MAX - overhead - ny)
        return CPLRT(oom)(cx);
    size_t needed = nx + ny;
    size_t scalars = is_text ? ((const cplrt_text *)x)->scalars + ((const cplrt_text *)y)->scalars : 0;
    int unique = atomic_load_explicit(&x->u.live.rc, memory_order_acquire) == 1u;
    cplrt_obj *out = x;
    if (!unique || CPLRT(alloc_size)(x) - overhead < needed) {
        size_t capacity = nx <= (SIZE_MAX - overhead) / 2u ? nx * 2u : needed;
        if (capacity < 64u)
            capacity = 64u;
        if (capacity < needed)
            capacity = needed;
        out = CPLRT(obj_new)(cx->rt, x->kind, 0u, overhead + capacity);
        if (out == NULL)
            return CPLRT(oom)(cx);
        memcpy((char *)out + prefix, (const char *)x + prefix, nx);
    }
    memcpy((char *)out + prefix + nx, (const char *)y + prefix, ny);
    if (is_text) {
        cplrt_text *t = (cplrt_text *)out;
        t->bytes = needed;
        t->scalars = scalars;
        t->utf8[needed] = '\0';
    } else {
        ((cplrt_bytes *)out)->len = needed;
    }
    if (out != x)
        CPLRT_SET(*slot, cplrt_word_of(out));
    return CPLRT_OK;
}

/* mirrors cpl_eval::value::Value::append (Text) */
int CPLRT(text_concat_owned)(cplrt_cx *cx, cplrt_w *slot, cplrt_w other)
{
    return text_bytes_append_owned(cx, slot, other, 1);
}

/* mirrors cpl_eval::value::Value::append (Bytes) */
int CPLRT(bytes_concat_owned)(cplrt_cx *cx, cplrt_w *slot, cplrt_w other)
{
    return text_bytes_append_owned(cx, slot, other, 0);
}

/* mirrors cpl_eval::value::Value::concat (Text) */
int CPLRT(text_concat)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    const cplrt_text *x = cplrt_text_of(a), *y = cplrt_text_of(b);
    if (y->bytes == 0) {
        CPLRT_COPY(*out, a);
        return CPLRT_OK;
    }
    if (x->bytes == 0) {
        CPLRT_COPY(*out, b);
        return CPLRT_OK;
    }
    if (x->bytes > SIZE_MAX / 2u || y->bytes > SIZE_MAX / 2u)
        return CPLRT_NOMEM;
    cplrt_text *t = CPLRT(text_alloc)(rt, x->bytes + y->bytes, x->scalars + y->scalars);
    if (t == NULL)
        return CPLRT_NOMEM;
    memcpy(t->utf8, x->utf8, x->bytes);
    memcpy(t->utf8 + x->bytes, y->utf8, y->bytes);
    CPLRT_SET(*out, cplrt_word_of(&t->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::value::Value::concat (Bytes) */
int CPLRT(bytes_concat)(cplrt_runtime *rt, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    const cplrt_bytes *x = cplrt_bytes_of(a), *y = cplrt_bytes_of(b);
    if (y->len == 0) {
        CPLRT_COPY(*out, a);
        return CPLRT_OK;
    }
    if (x->len == 0) {
        CPLRT_COPY(*out, b);
        return CPLRT_OK;
    }
    if (x->len > SIZE_MAX / 2u || y->len > SIZE_MAX / 2u)
        return CPLRT_NOMEM;
    cplrt_bytes *r = CPLRT(bytes_alloc)(rt, x->len + y->len);
    if (r == NULL)
        return CPLRT_NOMEM;
    memcpy(r->data, x->data, x->len);
    memcpy(r->data + x->len, y->data, y->len);
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* ---- the builtins (2.7) ---------------------------------------------------------------------------------------- */

/* A fault leaving a builtin: the memory fault here, a precondition fault from CPLRT(raise_builtin_requires). */
static int text_oom(cplrt_cx *cx, uint32_t site, uint32_t stub)
{
    return CPLRT(builtin_leave)(cx, site, stub, CPLRT(oom)(cx));
}

/* mirrors cpl_eval::builtins::prelude::text_len */
int CPLRT(bi_text_len)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    size_t n = cplrt_text_of(a0)->scalars;
    if (CPLRT(int_from_u64)(cx->rt, (uint64_t)n, out) != CPLRT_OK)
        return text_oom(cx, site, stub);
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::text_is_empty */
int CPLRT(bi_text_is_empty)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    CPLRT_SET(*out, cplrt_bool(cplrt_text_of(a0)->bytes == 0));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::bytes_len */
int CPLRT(bi_bytes_len)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    if (CPLRT(int_from_u64)(cx->rt, (uint64_t)cplrt_bytes_of(a0)->len, out) != CPLRT_OK)
        return text_oom(cx, site, stub);
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::bytes_is_empty */
int CPLRT(bi_bytes_is_empty)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    CPLRT_SET(*out, cplrt_bool(cplrt_bytes_of(a0)->len == 0));
    return CPLRT_OK;
}

static int text_is_white_space(uint32_t c)
{
    for (uint32_t i = 0; i < CPLRT_WHITE_SPACE_N; i++)
        if (c >= CPLRT_WHITE_SPACE[i].lo && c <= CPLRT_WHITE_SPACE[i].hi)
            return 1;
    return 0;
}

/* The text t without leading (when start) and trailing (when end) White_Space scalars; the operand itself when nothing
   is stripped, as same_or_new does. */
static int text_trim_sides(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, int start, int end, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0);
    const unsigned char *p = (const unsigned char *)t->utf8;
    size_t lo = 0, hi = t->bytes, removed = 0, k = 0;
    if (start) {
        while (lo < hi && text_is_white_space(text_decode(p + lo, &k))) {
            lo += k;
            removed++;
        }
    }
    if (end) {
        while (hi > lo) {
            size_t s = hi - 1;
            while (s > lo && (p[s] & 0xC0u) == 0x80u)
                s--;
            if (!text_is_white_space(text_decode(p + s, &k)))
                break;
            hi = s;
            removed++;
        }
    }
    if (lo == 0 && hi == t->bytes) {
        CPLRT_COPY(*out, a0);
        return CPLRT_OK;
    }
    if (lo == hi) {
        CPLRT_SET(*out, CPLRT(empty_text)());
        return CPLRT_OK;
    }
    cplrt_text *r = CPLRT(text_alloc)(cx->rt, hi - lo, t->scalars - removed);
    if (r == NULL)
        return text_oom(cx, site, stub);
    memcpy(r->utf8, t->utf8 + lo, hi - lo);
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::text::trim */
int CPLRT(bi_text_trim)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return text_trim_sides(cx, site, stub, a0, 1, 1, out);
}

/* mirrors cpl_eval::builtins::text::trim_start */
int CPLRT(bi_text_trim_start)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return text_trim_sides(cx, site, stub, a0, 1, 0, out);
}

/* mirrors cpl_eval::builtins::text::trim_end */
int CPLRT(bi_text_trim_end)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return text_trim_sides(cx, site, stub, a0, 0, 1, out);
}

/* mirrors cpl_eval::builtins::text::starts_with: a byte prefix, exact on scalars because UTF-8 is self-synchronizing */
int CPLRT(bi_text_starts_with)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0), *x = cplrt_text_of(a1);
    CPLRT_SET(*out, cplrt_bool(x->bytes <= t->bytes && memcmp(t->utf8, x->utf8, x->bytes) == 0));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::text::ends_with */
int CPLRT(bi_text_ends_with)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0), *x = cplrt_text_of(a1);
    CPLRT_SET(*out,
              cplrt_bool(x->bytes <= t->bytes && memcmp(t->utf8 + (t->bytes - x->bytes), x->utf8, x->bytes) == 0));
    return CPLRT_OK;
}

/* The first byte offset at or after `from` where needle (nlen > 0) occurs in hay, or SIZE_MAX. */
static size_t text_find(const char *hay, size_t hlen, size_t from, const char *needle, size_t nlen)
{
    if (from > hlen || nlen > hlen - from)
        return SIZE_MAX;
    const char *hit = CPLRT(find_bytes)(hay + from, hlen - from, needle, nlen);
    return hit == NULL ? SIZE_MAX : (size_t)(hit - hay);
}

/* mirrors cpl_eval::builtins::text::contains */
int CPLRT(bi_text_contains)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0), *x = cplrt_text_of(a1);
    int found = x->bytes == 0 || text_find(t->utf8, t->bytes, 0, x->utf8, x->bytes) != SIZE_MAX;
    CPLRT_SET(*out, cplrt_bool(found));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::text::split: the precondition first, then the pieces between leftmost-first,
   non-overlapping occurrences of sep, empty pieces kept (stdlib.md S24) */
int CPLRT(bi_text_split)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0), *sep = cplrt_text_of(a1);
    if (sep->bytes == 0) {
        cplrt_w args[2] = { a0, a1 };
        return CPLRT(builtin_leave)(cx, site, stub, CPLRT(raise_builtin_requires)(cx, stub, "text_split", 0, args, 2));
    }
    size_t first = text_find(t->utf8, t->bytes, 0, sep->utf8, sep->bytes);
    if (first == SIZE_MAX) {
        if (CPLRT(node_make)(cx->rt, CPLRT_LIST, 0u, 0u, &a0, 1u, out) != CPLRT_OK)
            return text_oom(cx, site, stub);
        return CPLRT_OK;
    }
    size_t pieces = 1;
    for (size_t at = first; at != SIZE_MAX;
         at = text_find(t->utf8, t->bytes, at + sep->bytes, sep->utf8, sep->bytes))
        pieces++;
    if (pieces > UINT32_MAX)
        return text_oom(cx, site, stub);
    cplrt_node *list = CPLRT(list_alloc)(cx->rt, (uint32_t)pieces);
    if (list == NULL)
        return text_oom(cx, site, stub);
    size_t start = 0;
    uint32_t i = 0;
    for (;;) {
        size_t at = start == 0 ? first : text_find(t->utf8, t->bytes, start, sep->utf8, sep->bytes);
        size_t stop = at == SIZE_MAX ? t->bytes : at;
        if (CPLRT(text_make)(cx->rt, t->utf8 + start, stop - start, &list->item[i]) != CPLRT_OK) {
            cplrt_release(cplrt_word_of(&list->hdr));
            return text_oom(cx, site, stub);
        }
        i++;
        if (at == SIZE_MAX)
            break;
        start = at + sep->bytes;
    }
    CPLRT_SET(*out, cplrt_word_of(&list->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::text::join */
int CPLRT(bi_text_join)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    uint32_t n = cplrt_len(a0);
    const cplrt_text *sep = cplrt_text_of(a1);
    if (n == 1u) {
        CPLRT_COPY(*out, cplrt_item(a0, 0));
        return CPLRT_OK;
    }
    size_t bytes = 0, scalars = 0;
    for (uint32_t i = 0; i < n; i++) {
        const cplrt_text *part = cplrt_text_of(cplrt_item(a0, i));
        size_t add = part->bytes + (i > 0 ? sep->bytes : 0u);
        if (add < part->bytes || bytes > SIZE_MAX / 2u - add)
            return text_oom(cx, site, stub);
        bytes += add;
        scalars += part->scalars + (i > 0 ? sep->scalars : 0u);
    }
    if (bytes == 0) {
        CPLRT_SET(*out, CPLRT(empty_text)());
        return CPLRT_OK;
    }
    cplrt_text *r = CPLRT(text_alloc)(cx->rt, bytes, scalars);
    if (r == NULL)
        return text_oom(cx, site, stub);
    size_t at = 0;
    for (uint32_t i = 0; i < n; i++) {
        const cplrt_text *part = cplrt_text_of(cplrt_item(a0, i));
        if (i > 0) {
            memcpy(r->utf8 + at, sep->utf8, sep->bytes);
            at += sep->bytes;
        }
        memcpy(r->utf8 + at, part->utf8, part->bytes);
        at += part->bytes;
    }
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* ASCII case mapping only (stdlib.md S23): from..to shifted by delta, every other byte copied; the operand when no
   byte changes, as to_upper and to_lower share the buffer. */
static int text_ascii_case(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, char from, char to, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0);
    size_t first = t->bytes;
    for (size_t i = 0; i < t->bytes; i++)
        if (t->utf8[i] >= from && t->utf8[i] <= to) {
            first = i;
            break;
        }
    if (first == t->bytes) {
        CPLRT_COPY(*out, a0);
        return CPLRT_OK;
    }
    cplrt_text *r = CPLRT(text_alloc)(cx->rt, t->bytes, t->scalars);
    if (r == NULL)
        return text_oom(cx, site, stub);
    for (size_t i = 0; i < t->bytes; i++) {
        char c = t->utf8[i];
        r->utf8[i] = c >= from && c <= to ? (char)(c ^ 0x20) : c;
    }
    CPLRT_SET(*out, cplrt_word_of(&r->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::text::to_upper */
int CPLRT(bi_text_to_upper)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return text_ascii_case(cx, site, stub, a0, 'a', 'z', out);
}

/* mirrors cpl_eval::builtins::text::to_lower */
int CPLRT(bi_text_to_lower)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return text_ascii_case(cx, site, stub, a0, 'A', 'Z', out);
}

/* mirrors cpl_eval::builtins::text::parse_int */
int CPLRT(bi_text_parse_int)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    cplrt_runtime *rt = cx->rt;
    const cplrt_text *t = cplrt_text_of(a0);
    cplrt_w n = CPLRT_EMPTY;
    int st = CPLRT(int_parse)(rt, t->utf8, t->bytes, &n);
    if (st == CPLRT_OK) {
        st = CPLRT(node_make)(rt, CPLRT_ENUM, rt->program->std.result, CPL_RESULT_OK, &n, 1, out);
        cplrt_release(n);
    } else if (st == CPLRT_INVALID) {
        cplrt_sbuf quoted, message;
        cplrt_sbuf_init(&quoted, rt);
        cplrt_sbuf_init(&message, rt);
        CPLRT(write_quote_text)(&quoted, t->utf8, t->bytes);
        const char *names[1] = { "quoted" };
        const char *values[1] = { quoted.ptr };
        if (quoted.status == CPLRT_OK)
            CPLRT(write_template)(&message, CPLRT_MSG_PARSE_INT_FAILED, names, values, 1);
        st = quoted.status == CPLRT_OK && message.status == CPLRT_OK
                 ? CPLRT(constraint_err)(rt, "Int", 3u, message.ptr, message.len, out)
                 : CPLRT_NOMEM;
        CPLRT(sbuf_free)(&quoted);
        CPLRT(sbuf_free)(&message);
    }
    return st == CPLRT_OK ? CPLRT_OK : text_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::text::from_int */
int CPLRT(bi_text_from_int)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    return CPLRT(int_text)(cx->rt, a0, out) == CPLRT_OK ? CPLRT_OK : text_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::text::chars */
int CPLRT(bi_text_chars)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0);
    if (t->scalars == 0) {
        CPLRT_SET(*out, CPLRT(empty_list)());
        return CPLRT_OK;
    }
    if (t->scalars > UINT32_MAX)
        return text_oom(cx, site, stub);
    cplrt_node *list = CPLRT(list_alloc)(cx->rt, (uint32_t)t->scalars);
    if (list == NULL)
        return text_oom(cx, site, stub);
    size_t at = 0, k = 0;
    for (uint32_t i = 0; i < (uint32_t)t->scalars; i++) {
        (void)text_decode((const unsigned char *)t->utf8 + at, &k);
        cplrt_text *one = CPLRT(text_alloc)(cx->rt, k, 1u);
        if (one == NULL) {
            cplrt_release(cplrt_word_of(&list->hdr));
            return text_oom(cx, site, stub);
        }
        memcpy(one->utf8, t->utf8 + at, k);
        list->item[i] = cplrt_word_of(&one->hdr);
        at += k;
    }
    CPLRT_SET(*out, cplrt_word_of(&list->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::bytes::from_list: the values checked left to right, the precondition fault at the first
   one outside 0..255 with `input` the whole list */
int CPLRT(bi_bytes_from_list)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    uint32_t n = cplrt_len(a0);
    for (uint32_t i = 0; i < n; i++) {
        cplrt_w v = cplrt_item(a0, i);
        if (!cplrt_is_small(v) || cplrt_small_value(v) < 0 || cplrt_small_value(v) > 255)
            return CPLRT(builtin_leave)(cx, site, stub,
                                        CPLRT(raise_builtin_requires)(cx, stub, "bytes_from_list", 0, &a0, 1));
    }
    if (n == 0) {
        CPLRT_SET(*out, CPLRT(empty_bytes)());
        return CPLRT_OK;
    }
    cplrt_bytes *b = CPLRT(bytes_alloc)(cx->rt, n);
    if (b == NULL)
        return text_oom(cx, site, stub);
    for (uint32_t i = 0; i < n; i++)
        b->data[i] = (uint8_t)cplrt_small_value(cplrt_item(a0, i));
    CPLRT_SET(*out, cplrt_word_of(&b->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::bytes::to_list */
int CPLRT(bi_bytes_to_list)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    const cplrt_bytes *b = cplrt_bytes_of(a0);
    if (b->len == 0) {
        CPLRT_SET(*out, CPLRT(empty_list)());
        return CPLRT_OK;
    }
    if (b->len > UINT32_MAX)
        return text_oom(cx, site, stub);
    cplrt_node *list = CPLRT(list_alloc)(cx->rt, (uint32_t)b->len);
    if (list == NULL)
        return text_oom(cx, site, stub);
    for (size_t i = 0; i < b->len; i++)
        list->item[i] = cplrt_small((int64_t)b->data[i]);
    CPLRT_SET(*out, cplrt_word_of(&list->hdr));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::bytes::from_text */
int CPLRT(bi_bytes_from_text)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    const cplrt_text *t = cplrt_text_of(a0);
    return CPLRT(bytes_make)(cx->rt, (const uint8_t *)t->utf8, t->bytes, out) == CPLRT_OK ? CPLRT_OK
                                                                                          : text_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::bytes::to_text */
int CPLRT(bi_bytes_to_text)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    cplrt_runtime *rt = cx->rt;
    const cplrt_bytes *b = cplrt_bytes_of(a0);
    size_t offset = 0;
    int st;
    if (CPLRT(utf8_valid)((const char *)b->data, b->len, &offset, NULL)) {
        cplrt_w t = CPLRT_EMPTY;
        st = CPLRT(text_make)(rt, (const char *)b->data, b->len, &t);
        if (st == CPLRT_OK)
            st = CPLRT(node_make)(rt, CPLRT_ENUM, rt->program->std.result, CPL_RESULT_OK, &t, 1, out);
        cplrt_release(t);
    } else {
        cplrt_sbuf number, message;
        cplrt_sbuf_init(&number, rt);
        cplrt_sbuf_init(&message, rt);
        CPLRT(sbuf_u64)(&number, (uint64_t)offset);
        const char *names[1] = { "offset" };
        const char *values[1] = { number.ptr };
        if (number.status == CPLRT_OK)
            CPLRT(write_template)(&message, CPLRT_MSG_BYTES_TO_TEXT_FAILED, names, values, 1);
        st = number.status == CPLRT_OK && message.status == CPLRT_OK
                 ? CPLRT(constraint_err)(rt, "Text", 4u, message.ptr, message.len, out)
                 : CPLRT_NOMEM;
        CPLRT(sbuf_free)(&number);
        CPLRT(sbuf_free)(&message);
    }
    return st == CPLRT_OK ? CPLRT_OK : text_oom(cx, site, stub);
}
