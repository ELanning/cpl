/* cpl_rt_int.c: arbitrary-precision Int (spec/cabi.md 2.5): small immediates, then normalized sign-and-magnitude
   bigints of uint32_t limbs; schoolbook addition, subtraction, comparison, and multiplication; Knuth's algorithm D for
   division in cplrt_int_divrem, the only division; the decimal reader of stdlib.md S25 and the canonical decimal
   writer; checked conversions with machine integers and 64-bit word arrays. Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <string.h>

#define INT_SMALL_MAG_MAX (UINT64_C(1) << 62) /* the magnitude of CPLRT_SMALL_MIN; SMALL_MAX is one less */

/* An Int's magnitude as limbs, least significant first, with its sign; a small Int is copied into buf, so a view is
   used in place and never copied. */
typedef struct int_view {
    const uint32_t *limb;
    uint32_t n;
    int neg;
    uint32_t buf[2];
} int_view;

static uint64_t int_abs64(int64_t v)
{
    return v < 0 ? (uint64_t)0 - (uint64_t)v : (uint64_t)v;
}

static void int_view_of(int_view *x, cplrt_w v)
{
    if (cplrt_is_small(v)) {
        int64_t s = cplrt_small_value(v);
        uint64_t m = int_abs64(s);
        x->neg = s < 0;
        x->buf[0] = (uint32_t)m;
        x->buf[1] = (uint32_t)(m >> 32);
        x->n = m == 0 ? 0u : (x->buf[1] != 0 ? 2u : 1u);
        x->limb = x->buf;
    } else {
        const cplrt_bigint *b = (const cplrt_bigint *)cplrt_obj_of(v);
        x->limb = b->limb;
        x->n = b->hdr.len;
        x->neg = (b->hdr.flags & CPLRT_F_NEGATIVE) != 0;
    }
}

/* A fresh bigint body of cap limbs (at least one), for a result computed in place. */
static cplrt_bigint *int_alloc(cplrt_runtime *rt, uint32_t cap)
{
    if (cap == 0)
        cap = 1;
    return (cplrt_bigint *)CPLRT(obj_new)(rt, CPLRT_BIGINT, cap, sizeof(cplrt_bigint) + (size_t)cap * sizeof(uint32_t));
}

/* The one representation of the magnitude in b->limb[0..n) with the sign neg (2.5): the high zero limbs dropped, a
   value in the small range as an immediate (b is then freed), -0 as 0. Stores into *out with CPLRT_SET semantics. */
static void int_finish(cplrt_bigint *b, uint32_t n, int neg, cplrt_w *out)
{
    while (n > 0 && b->limb[n - 1] == 0)
        n--;
    if (n <= 2) {
        uint64_t m = n == 0 ? 0u : (n == 1 ? b->limb[0] : ((uint64_t)b->limb[1] << 32) | b->limb[0]);
        if (neg ? m <= INT_SMALL_MAG_MAX : m < INT_SMALL_MAG_MAX) {
            int64_t v = neg ? (m == INT_SMALL_MAG_MAX ? CPLRT_SMALL_MIN : -(int64_t)m) : (int64_t)m;
            cplrt_release(cplrt_word_of(&b->hdr));
            CPLRT_SET(*out, cplrt_small(v));
            return;
        }
    }
    b->hdr.len = n;
    b->hdr.flags = neg ? CPLRT_F_NEGATIVE : 0u;
    CPLRT_SET(*out, cplrt_word_of(&b->hdr));
}

/* ---- magnitudes ------------------------------------------------------------------------------------------------ */

static int int_mag_cmp(const uint32_t *a, uint32_t na, const uint32_t *b, uint32_t nb)
{
    if (na != nb)
        return na < nb ? -1 : 1;
    for (uint32_t i = na; i > 0; i--)
        if (a[i - 1] != b[i - 1])
            return a[i - 1] < b[i - 1] ? -1 : 1;
    return 0;
}

/* r = a + b; r has max(na, nb) + 1 limbs; returns that count. */
static uint32_t int_mag_add(uint32_t *r, const uint32_t *a, uint32_t na, const uint32_t *b, uint32_t nb)
{
    if (na < nb) {
        const uint32_t *t = a;
        a = b;
        b = t;
        uint32_t tn = na;
        na = nb;
        nb = tn;
    }
    uint64_t carry = 0;
    for (uint32_t i = 0; i < na; i++) {
        uint64_t s = (uint64_t)a[i] + (i < nb ? b[i] : 0u) + carry;
        r[i] = (uint32_t)s;
        carry = s >> 32;
    }
    r[na] = (uint32_t)carry;
    return na + 1;
}

/* r = a - b for a >= b; r has na limbs; returns na. */
static uint32_t int_mag_sub(uint32_t *r, const uint32_t *a, uint32_t na, const uint32_t *b, uint32_t nb)
{
    uint64_t borrow = 0;
    for (uint32_t i = 0; i < na; i++) {
        uint64_t d = (uint64_t)a[i] - (i < nb ? b[i] : 0u) - borrow;
        r[i] = (uint32_t)d;
        borrow = (d >> 63) & 1u;
    }
    return na;
}

/* r = a * b (schoolbook); r has na + nb limbs, zeroed here. */
static void int_mag_mul(uint32_t *r, const uint32_t *a, uint32_t na, const uint32_t *b, uint32_t nb)
{
    memset(r, 0, ((size_t)na + nb) * sizeof(uint32_t));
    for (uint32_t i = 0; i < na; i++) {
        uint64_t carry = 0, ai = a[i];
        if (ai == 0)
            continue;
        for (uint32_t j = 0; j < nb; j++) {
            uint64_t t = ai * b[j] + r[i + j] + carry;
            r[i + j] = (uint32_t)t;
            carry = t >> 32;
        }
        r[i + nb] = (uint32_t)carry;
    }
}

/* q = u / d and returns u % d for one limb d != 0; q has n limbs (q may be u). */
static uint32_t int_mag_divmod1(uint32_t *q, const uint32_t *u, uint32_t n, uint32_t d)
{
    uint64_t rem = 0;
    for (uint32_t i = n; i > 0; i--) {
        uint64_t cur = (rem << 32) | u[i - 1];
        q[i - 1] = (uint32_t)(cur / d);
        rem = cur % d;
    }
    return (uint32_t)rem;
}

static unsigned int_clz32(uint32_t x)
{
#if defined(__GNUC__) || defined(__clang__)
    return x == 0 ? 32u : (unsigned)__builtin_clz(x);
#else
    unsigned n = 0;
    if (x == 0)
        return 32u;
    while (!(x & 0x80000000u)) {
        x <<= 1;
        n++;
    }
    return n;
#endif
}

/* Knuth's algorithm D (TAOCP 4.3.1), as in Hacker's Delight's divmnu with unsigned borrows: q[0..m-n] = u / v and
   r[0..n) = u % v for m >= n >= 2 and v[n-1] != 0; un (m + 1 limbs) and vn (n limbs) are scratch. */
static void int_mag_knuth(uint32_t *q, uint32_t *r, const uint32_t *u, uint32_t m, const uint32_t *v, uint32_t n,
                          uint32_t *un, uint32_t *vn)
{
    const uint64_t base = UINT64_C(1) << 32;
    unsigned s = int_clz32(v[n - 1]);
    for (uint32_t i = n - 1; i > 0; i--)
        vn[i] = s ? (v[i] << s) | (v[i - 1] >> (32u - s)) : v[i];
    vn[0] = v[0] << s;
    un[m] = s ? u[m - 1] >> (32u - s) : 0u;
    for (uint32_t i = m - 1; i > 0; i--)
        un[i] = s ? (u[i] << s) | (u[i - 1] >> (32u - s)) : u[i];
    un[0] = u[0] << s;
    for (uint32_t jj = m - n + 1; jj > 0; jj--) {
        uint32_t j = jj - 1;
        uint64_t num = ((uint64_t)un[j + n] << 32) | un[j + n - 1];
        uint64_t qhat = num / vn[n - 1];
        uint64_t rhat = num - qhat * vn[n - 1];
        while (qhat >= base || qhat * vn[n - 2] > ((rhat << 32) | un[j + n - 2])) {
            qhat--;
            rhat += vn[n - 1];
            if (rhat >= base)
                break;
        }
        uint64_t borrow = 0, carry = 0;
        for (uint32_t i = 0; i < n; i++) {
            uint64_t p = qhat * vn[i] + carry;
            carry = p >> 32;
            uint64_t d = (uint64_t)un[i + j] - (p & 0xFFFFFFFFu) - borrow;
            un[i + j] = (uint32_t)d;
            borrow = (d >> 63) & 1u;
        }
        uint64_t d = (uint64_t)un[j + n] - carry - borrow;
        un[j + n] = (uint32_t)d;
        if ((d >> 63) & 1u) { /* subtracted too much: add v back once */
            qhat--;
            uint64_t c = 0;
            for (uint32_t i = 0; i < n; i++) {
                uint64_t t = (uint64_t)un[i + j] + vn[i] + c;
                un[i + j] = (uint32_t)t;
                c = t >> 32;
            }
            un[j + n] = (uint32_t)((uint64_t)un[j + n] + c);
        }
        q[j] = (uint32_t)qhat;
    }
    for (uint32_t i = 0; i < n - 1; i++)
        r[i] = s ? (un[i] >> s) | (un[i + 1] << (32u - s)) : un[i];
    r[n - 1] = un[n - 1] >> s;
}

/* ---- conversions ----------------------------------------------------------------------------------------------- */

int CPLRT(int_from_u64)(cplrt_runtime *rt, uint64_t v, cplrt_w *out)
{
    if (v < INT_SMALL_MAG_MAX) {
        CPLRT_SET(*out, cplrt_small((int64_t)v));
        return CPLRT_OK;
    }
    cplrt_bigint *b = int_alloc(rt, 2);
    if (b == NULL)
        return CPLRT_NOMEM;
    b->limb[0] = (uint32_t)v;
    b->limb[1] = (uint32_t)(v >> 32);
    int_finish(b, 2, 0, out);
    return CPLRT_OK;
}

int CPLRT(int_from_i64)(cplrt_runtime *rt, int64_t v, cplrt_w *out)
{
    if (v >= CPLRT_SMALL_MIN && v <= CPLRT_SMALL_MAX) {
        CPLRT_SET(*out, cplrt_small(v));
        return CPLRT_OK;
    }
    uint64_t m = int_abs64(v);
    cplrt_bigint *b = int_alloc(rt, 2);
    if (b == NULL)
        return CPLRT_NOMEM;
    b->limb[0] = (uint32_t)m;
    b->limb[1] = (uint32_t)(m >> 32);
    int_finish(b, 2, v < 0, out);
    return CPLRT_OK;
}

int CPLRT(int_from_words)(cplrt_runtime *rt, int negative, const uint64_t *words, size_t n, cplrt_w *out)
{
    while (n > 0 && words[n - 1] == 0)
        n--;
    if (n > (size_t)(UINT32_MAX / 2u))
        return CPLRT_NOMEM;
    cplrt_bigint *b = int_alloc(rt, (uint32_t)(2u * n));
    if (b == NULL)
        return CPLRT_NOMEM;
    for (size_t i = 0; i < n; i++) {
        b->limb[2 * i] = (uint32_t)words[i];
        b->limb[2 * i + 1] = (uint32_t)(words[i] >> 32);
    }
    int_finish(b, (uint32_t)(2u * n), negative != 0, out);
    return CPLRT_OK;
}

int CPLRT(int_to_i64)(cplrt_w v, int64_t *out)
{
    if (cplrt_is_small(v)) {
        *out = cplrt_small_value(v);
        return 1;
    }
    int_view x;
    int_view_of(&x, v);
    if (x.n > 2)
        return 0;
    uint64_t m = x.n == 2 ? ((uint64_t)x.limb[1] << 32) | x.limb[0] : (x.n == 1 ? x.limb[0] : 0u);
    if (x.neg) {
        if (m > (UINT64_C(1) << 63))
            return 0;
        *out = m == (UINT64_C(1) << 63) ? INT64_MIN : -(int64_t)m;
        return 1;
    }
    if (m > (uint64_t)INT64_MAX)
        return 0;
    *out = (int64_t)m;
    return 1;
}

int CPLRT(int_to_u64)(cplrt_w v, uint64_t *out)
{
    if (cplrt_is_small(v)) {
        int64_t s = cplrt_small_value(v);
        if (s < 0)
            return 0;
        *out = (uint64_t)s;
        return 1;
    }
    int_view x;
    int_view_of(&x, v);
    if (x.neg || x.n > 2)
        return 0;
    *out = x.n == 2 ? ((uint64_t)x.limb[1] << 32) | x.limb[0] : (x.n == 1 ? x.limb[0] : 0u);
    return 1;
}

size_t CPLRT(int_to_words)(cplrt_w v, int *negative, uint64_t *words, size_t cap)
{
    int_view x;
    int_view_of(&x, v);
    size_t count = ((size_t)x.n + 1u) / 2u;
    *negative = x.n != 0 && x.neg;
    if (words != NULL && cap >= count) {
        for (size_t i = 0; i < count; i++) {
            uint64_t lo = x.limb[2 * i];
            uint64_t hi = 2 * i + 1 < x.n ? x.limb[2 * i + 1] : 0u;
            words[i] = (hi << 32) | lo;
        }
    }
    return count;
}

/* mirrors cpl_eval::builtins::text::parse_decimal: ("+" | "-")? ASCII_DIGIT+, leading zeros accepted, -0 is 0, no
   length limit; nothing else (stdlib.md S25). Digits are taken nine at a time (base 10^9), O(n^2) as the reference's
   parser is (2.5). */
int CPLRT(int_parse)(cplrt_runtime *rt, const char *s, size_t len, cplrt_w *out)
{
    size_t i = 0;
    int neg = 0;
    if (len > 0 && (s[0] == '+' || s[0] == '-')) {
        neg = s[0] == '-';
        i = 1;
    }
    if (i == len)
        return CPLRT_INVALID;
    for (size_t k = i; k < len; k++)
        if (s[k] < '0' || s[k] > '9')
            return CPLRT_INVALID;
    while (i < len - 1 && s[i] == '0')
        i++;
    size_t digits = len - i;
    if (digits <= 18) {
        uint64_t m = 0;
        for (size_t k = i; k < len; k++)
            m = m * 10u + (uint64_t)(s[k] - '0');
        if (neg ? m <= INT_SMALL_MAG_MAX : m < INT_SMALL_MAG_MAX) {
            int64_t v = neg ? (m == INT_SMALL_MAG_MAX ? CPLRT_SMALL_MIN : -(int64_t)m) : (int64_t)m;
            CPLRT_SET(*out, cplrt_small(v));
            return CPLRT_OK;
        }
    }
    if (digits / 9u + 2u > UINT32_MAX)
        return CPLRT_NOMEM;
    uint32_t cap = (uint32_t)(digits / 9u + 2u);
    cplrt_bigint *b = int_alloc(rt, cap);
    if (b == NULL)
        return CPLRT_NOMEM;
    uint32_t n = 0;
    size_t first = digits % 9u == 0 ? 9u : digits % 9u;
    for (size_t k = i; k < len;) {
        size_t take = k == i ? first : 9u;
        uint32_t chunk = 0, scale = 1;
        for (size_t t = 0; t < take; t++) {
            chunk = chunk * 10u + (uint32_t)(s[k + t] - '0');
            scale *= 10u;
        }
        k += take;
        /* b = b * scale + chunk, in place */
        uint64_t carry = chunk;
        for (uint32_t j = 0; j < n; j++) {
            uint64_t t = (uint64_t)b->limb[j] * scale + carry;
            b->limb[j] = (uint32_t)t;
            carry = t >> 32;
        }
        if (carry != 0)
            b->limb[n++] = (uint32_t)carry;
    }
    int_finish(b, n, neg, out);
    return CPLRT_OK;
}

/* The canonical decimal: an optional -, then the digits without leading zeros, 0 for zero (stdlib.md 2.3), written
   in chunks of base 10^9 (2.5). A failed allocation sets b->status. */
void CPLRT(int_write)(cplrt_sbuf *b, cplrt_w v)
{
    char tmp[24];
    if (b->status != CPLRT_OK)
        return;
    if (cplrt_is_small(v)) {
        int64_t s = cplrt_small_value(v);
        uint64_t m = int_abs64(s);
        size_t k = sizeof tmp;
        do {
            tmp[--k] = (char)('0' + (int)(m % 10u));
            m /= 10u;
        } while (m != 0);
        if (s < 0)
            tmp[--k] = '-';
        CPLRT(sbuf_put)(b, tmp + k, sizeof tmp - k);
        return;
    }
    int_view x;
    int_view_of(&x, v);
    size_t nchunks_cap = (size_t)x.n * 2u + 2u; /* 32 bits are less than 2 decimal chunks of 9 digits */
    uint32_t *work = (uint32_t *)CPLRT(alloc)(b->rt, (size_t)x.n * sizeof(uint32_t));
    uint32_t *chunks = (uint32_t *)CPLRT(alloc)(b->rt, nchunks_cap * sizeof(uint32_t));
    if (work == NULL || chunks == NULL) {
        CPLRT(free)(work);
        CPLRT(free)(chunks);
        b->status = CPLRT_NOMEM;
        return;
    }
    memcpy(work, x.limb, (size_t)x.n * sizeof(uint32_t));
    uint32_t n = x.n;
    size_t nchunks = 0;
    while (n > 0) {
        chunks[nchunks++] = int_mag_divmod1(work, work, n, 1000000000u);
        while (n > 0 && work[n - 1] == 0)
            n--;
    }
    if (x.neg)
        CPLRT(sbuf_putc)(b, '-');
    for (size_t c = nchunks; c > 0; c--) {
        uint32_t chunk = chunks[c - 1];
        size_t k = sizeof tmp;
        size_t width = 0;
        do {
            tmp[--k] = (char)('0' + (int)(chunk % 10u));
            chunk /= 10u;
            width++;
        } while (chunk != 0);
        while (c != nchunks && width < 9u) {
            tmp[--k] = '0';
            width++;
        }
        CPLRT(sbuf_put)(b, tmp + k, sizeof tmp - k);
    }
    CPLRT(free)(work);
    CPLRT(free)(chunks);
}

int CPLRT(int_text)(cplrt_runtime *rt, cplrt_w v, cplrt_w *out)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(int_write)(&b, v);
    int st = b.status == CPLRT_OK ? CPLRT(text_make)(rt, b.ptr, b.len, out) : CPLRT_NOMEM;
    CPLRT(sbuf_free)(&b);
    return st;
}

/* ---- arithmetic (2.5) ------------------------------------------------------------------------------------------ */

/* x + y with the signs of the views (y's possibly flipped by the caller for a subtraction). */
static int int_add_views(cplrt_runtime *rt, const int_view *x, int yneg, const int_view *y, cplrt_w *out)
{
    uint32_t cap = (x->n > y->n ? x->n : y->n) + 1u;
    if (cap == 0)
        return CPLRT_NOMEM;
    cplrt_bigint *b = int_alloc(rt, cap);
    if (b == NULL)
        return CPLRT_NOMEM;
    if (x->neg == yneg) {
        uint32_t n = int_mag_add(b->limb, x->limb, x->n, y->limb, y->n);
        int_finish(b, n, x->neg, out);
        return CPLRT_OK;
    }
    int c = int_mag_cmp(x->limb, x->n, y->limb, y->n);
    if (c == 0) {
        int_finish(b, 0, 0, out);
    } else if (c > 0) {
        uint32_t n = int_mag_sub(b->limb, x->limb, x->n, y->limb, y->n);
        int_finish(b, n, x->neg, out);
    } else {
        uint32_t n = int_mag_sub(b->limb, y->limb, y->n, x->limb, x->n);
        int_finish(b, n, yneg, out);
    }
    return CPLRT_OK;
}

static int int_small_result(cplrt_runtime *rt, int64_t r, cplrt_w *out)
{
    return CPLRT(int_from_i64)(rt, r, out);
}

/* mirrors cpl_eval::arith::binary (BinOp::Add) */
int cplrt_int_add(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        /* both lie in [-2^62, 2^62 - 1], so the sum fits int64_t */
        int64_t r = cplrt_small_value(a) + cplrt_small_value(b);
        if (r >= CPLRT_SMALL_MIN && r <= CPLRT_SMALL_MAX) {
            CPLRT_SET(*out, cplrt_small(r));
            return CPLRT_OK;
        }
        return int_small_result(cx->rt, r, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
    }
    int_view x, y;
    int_view_of(&x, a);
    int_view_of(&y, b);
    return int_add_views(cx->rt, &x, y.neg, &y, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

/* mirrors cpl_eval::arith::binary (BinOp::Sub) */
int cplrt_int_sub(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t r = cplrt_small_value(a) - cplrt_small_value(b);
        if (r >= CPLRT_SMALL_MIN && r <= CPLRT_SMALL_MAX) {
            CPLRT_SET(*out, cplrt_small(r));
            return CPLRT_OK;
        }
        return int_small_result(cx->rt, r, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
    }
    int_view x, y;
    int_view_of(&x, a);
    int_view_of(&y, b);
    return int_add_views(cx->rt, &x, !y.neg, &y, out) == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}

/* mirrors cpl_eval::arith::binary (BinOp::Mul) */
int cplrt_int_mul(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *out)
{
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t x = cplrt_small_value(a), y = cplrt_small_value(b), r;
#if defined(__GNUC__) || defined(__clang__)
        if (!__builtin_mul_overflow(x, y, &r) && r >= CPLRT_SMALL_MIN && r <= CPLRT_SMALL_MAX) {
            CPLRT_SET(*out, cplrt_small(r));
            return CPLRT_OK;
        }
#else
        /* portable fallback: |x|, |y| < 2^31 cannot overflow the small range */
        if (int_abs64(x) < (UINT64_C(1) << 31) && int_abs64(y) < (UINT64_C(1) << 31)) {
            r = x * y;
            CPLRT_SET(*out, cplrt_small(r));
            return CPLRT_OK;
        }
#endif
    }
    int_view x, y;
    int_view_of(&x, a);
    int_view_of(&y, b);
    if (x.n == 0 || y.n == 0) {
        CPLRT_SET(*out, cplrt_small(0));
        return CPLRT_OK;
    }
    if ((uint64_t)x.n + y.n > UINT32_MAX)
        return CPLRT(oom)(cx);
    cplrt_bigint *r = int_alloc(cx->rt, x.n + y.n);
    if (r == NULL)
        return CPLRT(oom)(cx);
    int_mag_mul(r->limb, x.limb, x.n, y.limb, y.n);
    int_finish(r, x.n + y.n, x.neg != y.neg, out);
    return CPLRT_OK;
}

/* mirrors cpl_eval::arith::negate (UnaryOp::Neg) */
int cplrt_int_neg(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w *out)
{
    if (cplrt_is_small(a)) {
        int64_t v = cplrt_small_value(a);
        if (v != CPLRT_SMALL_MIN) {
            CPLRT_SET(*out, cplrt_small(-v));
            return CPLRT_OK;
        }
    }
    int_view x;
    int_view_of(&x, a);
    cplrt_bigint *r = int_alloc(cx->rt, x.n);
    if (r == NULL)
        return CPLRT(oom)(cx);
    memcpy(r->limb, x.limb, (size_t)x.n * sizeof(uint32_t));
    int_finish(r, x.n, !x.neg, out);
    return CPLRT_OK;
}

/* mirrors cpl_rtspec::arith::div_rem (cpl_eval::arith::div_rem, T9 as amended by T70): the quotient truncated toward
   zero and the remainder a - q * b, which has the sign of a; here the remainder is the magnitude remainder of
   algorithm D with a's sign, the same value. A zero divisor is division_by_zero at site with input {"a", "b"}. */
int cplrt_int_divrem(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b, cplrt_w *q, cplrt_w *r)
{
    if (b == cplrt_small(0))
        return CPLRT(raise_division_by_zero)(cx, site, a, b);
    cplrt_runtime *rt = cx->rt;
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t x = cplrt_small_value(a), y = cplrt_small_value(b);
        int64_t qq = x / y; /* C truncates toward zero; |x| <= 2^62, so x / y fits (-2^62 / -1 is 2^62) */
        int64_t rr = x - qq * y;
        if (q != NULL && CPLRT(int_from_i64)(rt, qq, q) != CPLRT_OK)
            return CPLRT(oom)(cx);
        if (r != NULL)
            CPLRT_SET(*r, cplrt_small(rr));
        return CPLRT_OK;
    }
    int_view x, y;
    int_view_of(&x, a);
    int_view_of(&y, b);
    if (int_mag_cmp(x.limb, x.n, y.limb, y.n) < 0) {
        if (q != NULL)
            CPLRT_SET(*q, cplrt_small(0));
        if (r != NULL)
            CPLRT_COPY(*r, a);
        return CPLRT_OK;
    }
    uint32_t m = x.n, n = y.n;
    cplrt_bigint *qb = int_alloc(rt, m - n + 1u);
    cplrt_bigint *rb = int_alloc(rt, n);
    uint32_t *scratch = NULL;
    if (qb != NULL && rb != NULL && n >= 2)
        scratch = (uint32_t *)CPLRT(alloc)(rt, ((size_t)m + 1u + n) * sizeof(uint32_t));
    if (qb == NULL || rb == NULL || (n >= 2 && scratch == NULL)) {
        if (qb != NULL)
            cplrt_release(cplrt_word_of(&qb->hdr));
        if (rb != NULL)
            cplrt_release(cplrt_word_of(&rb->hdr));
        CPLRT(free)(scratch);
        return CPLRT(oom)(cx);
    }
    if (n == 1) {
        rb->limb[0] = int_mag_divmod1(qb->limb, x.limb, m, y.limb[0]);
    } else {
        int_mag_knuth(qb->limb, rb->limb, x.limb, m, y.limb, n, scratch, scratch + m + 1u);
        CPLRT(free)(scratch);
    }
    cplrt_w qw = CPLRT_EMPTY, rw = CPLRT_EMPTY;
    int_finish(qb, m - n + 1u, x.neg != y.neg, &qw);
    int_finish(rb, n, x.neg, &rw);
    if (q != NULL)
        CPLRT_SET(*q, qw);
    else
        cplrt_release(qw);
    if (r != NULL)
        CPLRT_SET(*r, rw);
    else
        cplrt_release(rw);
    return CPLRT_OK;
}

/* Int ordering: a sign-and-magnitude comparison (2.8), -1, 0, or 1; it cannot fail. */
int cplrt_int_cmp(cplrt_w a, cplrt_w b)
{
    if (cplrt_is_small(a) && cplrt_is_small(b)) {
        int64_t x = cplrt_small_value(a), y = cplrt_small_value(b);
        return x < y ? -1 : (x > y ? 1 : 0);
    }
    int_view x, y;
    int_view_of(&x, a);
    int_view_of(&y, b);
    int sx = x.n == 0 ? 0 : (x.neg ? -1 : 1), sy = y.n == 0 ? 0 : (y.neg ? -1 : 1);
    if (sx != sy)
        return sx < sy ? -1 : 1;
    int c = int_mag_cmp(x.limb, x.n, y.limb, y.n);
    return sx < 0 ? -c : c;
}
