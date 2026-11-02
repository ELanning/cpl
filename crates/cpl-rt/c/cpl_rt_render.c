/* cpl_rt_render.c: the growable text buffer, value text and quote_text byte for byte as the evaluator renders them
   (semantics S40; spec/cabi.md 3.3.2), the {name} renderer of the generated templates (spec/cabi.md 2.9), and the small
   writers every renderer shares (decimal, hex, RFC 6901 segments). Value text runs on an explicit work stack, so a
   value nested deeper than any thread's stack renders on any thread (2.4). Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <stdlib.h>
#include <string.h>

/* ---- the text buffer --------------------------------------------------------------------------------------------- */

/* A buffer without a runtime (rt NULL: before a runtime exists, as R3's error objects of runtime_open) uses the C
   library's allocator; otherwise every byte is the runtime's (CPLRT(alloc), charged to the heap budget). */
static void *render_alloc(cplrt_runtime *rt, size_t size)
{
    return rt != NULL ? CPLRT(alloc)(rt, size) : malloc(size);
}

static void render_free(cplrt_runtime *rt, void *p)
{
    if (rt != NULL)
        CPLRT(free)(p);
    else
        free(p);
}

static int render_reserve(cplrt_sbuf *b, size_t more)
{
    if (b->status != CPLRT_OK)
        return 0;
    if (more > SIZE_MAX - b->len - 1u) {
        b->status = CPLRT_NOMEM;
        return 0;
    }
    size_t need = b->len + more + 1u;
    if (b->ptr != NULL && need <= b->cap)
        return 1;
    size_t cap = b->cap < 64u ? 64u : b->cap;
    while (cap < need)
        cap = cap > SIZE_MAX / 2u ? need : cap * 2u;
    char *grown = (char *)render_alloc(b->rt, cap);
    if (grown == NULL) {
        b->status = CPLRT_NOMEM;
        return 0;
    }
    if (b->ptr != NULL) {
        memcpy(grown, b->ptr, b->len + 1u);
        render_free(b->rt, b->ptr);
    } else {
        grown[0] = '\0';
    }
    b->ptr = grown;
    b->cap = cap;
    return 1;
}

void CPLRT(sbuf_put)(cplrt_sbuf *b, const char *s, size_t n)
{
    if (!render_reserve(b, n))
        return;
    if (n != 0)
        memcpy(b->ptr + b->len, s, n);
    b->len += n;
    b->ptr[b->len] = '\0';
}

void CPLRT(sbuf_puts)(cplrt_sbuf *b, const char *s)
{
    CPLRT(sbuf_put)(b, s, strlen(s));
}

void CPLRT(sbuf_putc)(cplrt_sbuf *b, char c)
{
    CPLRT(sbuf_put)(b, &c, 1u);
}

void CPLRT(sbuf_u64)(cplrt_sbuf *b, uint64_t v)
{
    char tmp[24];
    size_t k = sizeof tmp;
    do {
        tmp[--k] = (char)('0' + (int)(v % 10u));
        v /= 10u;
    } while (v != 0);
    CPLRT(sbuf_put)(b, tmp + k, sizeof tmp - k);
}

void CPLRT(sbuf_free)(cplrt_sbuf *b)
{
    if (b->ptr != NULL)
        render_free(b->rt, b->ptr);
    b->ptr = NULL;
    b->len = 0;
    b->cap = 0;
}

void CPLRT(sbuf_hex)(cplrt_sbuf *b, const uint8_t *data, size_t len)
{
    static const char digits[] = "0123456789abcdef";
    if (len > SIZE_MAX / 2u - 1u) {
        if (b->status == CPLRT_OK)
            b->status = CPLRT_NOMEM;
        return;
    }
    if (!render_reserve(b, 2u * len))
        return;
    for (size_t i = 0; i < len; i++) {
        b->ptr[b->len++] = digits[data[i] >> 4];
        b->ptr[b->len++] = digits[data[i] & 15u];
    }
    b->ptr[b->len] = '\0';
}

/* mirrors cpl_eval::json::escape_segment, behind the "/" that Decoder::error writes */
void CPLRT(sbuf_pointer_segment)(cplrt_sbuf *b, const char *s, size_t len)
{
    CPLRT(sbuf_putc)(b, '/');
    size_t run = 0;
    for (size_t i = 0; i < len; i++) {
        if (s[i] != '~' && s[i] != '/')
            continue;
        CPLRT(sbuf_put)(b, s + run, i - run);
        CPLRT(sbuf_puts)(b, s[i] == '~' ? "~0" : "~1");
        run = i + 1u;
    }
    CPLRT(sbuf_put)(b, s + run, len - run);
}

/* ---- quote_text ------------------------------------------------------------------------------------------------ */

/* mirrors cpl_diag::id::quote_text: "..." with \\ \" \n \r \t, every other control below U+0020 and U+007F as \u{h}
   with minimal lowercase hex, and every other scalar as its UTF-8 */
void CPLRT(write_quote_text)(cplrt_sbuf *b, const char *s, size_t len)
{
    static const char digits[] = "0123456789abcdef";
    CPLRT(sbuf_putc)(b, '"');
    size_t run = 0;
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)s[i];
        const char *esc = NULL;
        char hex[8];
        switch (c) {
        case '\\': esc = "\\\\"; break;
        case '"': esc = "\\\""; break;
        case '\n': esc = "\\n"; break;
        case '\r': esc = "\\r"; break;
        case '\t': esc = "\\t"; break;
        default:
            if (c < 0x20u || c == 0x7Fu) {
                size_t k = 0;
                hex[k++] = '\\';
                hex[k++] = 'u';
                hex[k++] = '{';
                if (c >= 0x10u)
                    hex[k++] = digits[c >> 4];
                hex[k++] = digits[c & 15u];
                hex[k++] = '}';
                hex[k] = '\0';
                esc = hex;
            }
            break;
        }
        if (esc == NULL)
            continue;
        CPLRT(sbuf_put)(b, s + run, i - run);
        CPLRT(sbuf_puts)(b, esc);
        run = i + 1u;
    }
    CPLRT(sbuf_put)(b, s + run, len - run);
    CPLRT(sbuf_putc)(b, '"');
}

/* ---- the {name} renderer (2.9) --------------------------------------------------------------------------------- */

/* mirrors cpl_rtspec::messages::render: one pass, left to right; a placeholder is "{", a lowercase ASCII letter, then
   lowercase letters, digits, and "_", and "}"; the first argument of a name wins; values are never rescanned; every
   other byte, a brace included, is copied. A placeholder without an argument sets b->status to CPLRT_INVALID. */
void CPLRT(write_template)(cplrt_sbuf *b, const char *tmpl, const char *const *names, const char *const *values,
                           uint32_t n)
{
    size_t copied = 0, i = 0;
    while (tmpl[i] != '\0') {
        if (tmpl[i] == '{' && tmpl[i + 1] >= 'a' && tmpl[i + 1] <= 'z') {
            size_t len = 1;
            for (;;) {
                char c = tmpl[i + 1 + len];
                if ((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_')
                    len++;
                else
                    break;
            }
            if (tmpl[i + 1 + len] == '}') {
                const char *value = NULL;
                for (uint32_t k = 0; k < n && value == NULL; k++)
                    if (strlen(names[k]) == len && memcmp(names[k], tmpl + i + 1, len) == 0)
                        value = values[k];
                if (value == NULL) {
                    if (b->status == CPLRT_OK)
                        b->status = CPLRT_INVALID;
                    return;
                }
                CPLRT(sbuf_put)(b, tmpl + copied, i - copied);
                CPLRT(sbuf_puts)(b, value);
                i += len + 2u;
                copied = i;
                continue;
            }
        }
        i++;
    }
    CPLRT(sbuf_put)(b, tmpl + copied, i - copied);
}

/* ---- value text (semantics S40) -------------------------------------------------------------------------------- */

static const cplrt_def *render_def(const cplrt_program *p, uint32_t def)
{
    return p != NULL && def < p->ndefs ? &p->defs[def] : NULL;
}

/* mirrors cpl_eval::types::bare_name for a value's declaration */
static void render_bare_name(cplrt_sbuf *b, const cplrt_program *p, uint32_t def)
{
    const cplrt_def *d = render_def(p, def);
    if (d != NULL && d->bare_name != NULL) {
        CPLRT(sbuf_puts)(b, d->bare_name);
        return;
    }
    CPLRT(sbuf_puts)(b, "{def ");
    CPLRT(sbuf_u64)(b, def);
    CPLRT(sbuf_putc)(b, '}');
}

/* mirrors cpl_eval::json::field_name and variant_name: the member's name, or `<field N>` / `<variant N>` for a value
   that does not match its declaration (no such declaration, one that is not a struct, respectively an enum, or no
   such member) */
static void render_member(cplrt_sbuf *b, const cplrt_program *p, uint32_t def, uint32_t index, const char *what)
{
    const cplrt_def *d = render_def(p, def);
    uint32_t kind = what[0] == 'f' ? CPLRT_DEF_STRUCT : CPLRT_DEF_ENUM;
    if (d != NULL && d->kind == kind && index < d->nmembers && d->members != NULL) {
        CPLRT(sbuf_puts)(b, d->members[index]);
        return;
    }
    CPLRT(sbuf_putc)(b, '<');
    CPLRT(sbuf_puts)(b, what);
    CPLRT(sbuf_putc)(b, ' ');
    CPLRT(sbuf_u64)(b, index);
    CPLRT(sbuf_putc)(b, '>');
}

typedef struct render_frame {
    cplrt_w v;
    uint32_t i, n;
} render_frame;

/* Writes the leaf v, or the opening of the composite v and returns its child count (0 for a leaf or none). */
static uint32_t render_open(cplrt_sbuf *b, const cplrt_program *p, cplrt_w v)
{
    if (cplrt_is_small(v)) {
        CPLRT(int_write)(b, v);
        return 0;
    }
    if (v == CPLRT_TRUE || v == CPLRT_FALSE) {
        CPLRT(sbuf_puts)(b, v == CPLRT_TRUE ? "true" : "false");
        return 0;
    }
    const cplrt_obj *o = cplrt_obj_of(v);
    switch (o->kind) {
    case CPLRT_BIGINT: CPLRT(int_write)(b, v); return 0;
    case CPLRT_TEXT: {
        const cplrt_text *t = cplrt_text_of(v);
        CPLRT(write_quote_text)(b, t->utf8, t->bytes);
        return 0;
    }
    case CPLRT_BYTES: {
        const cplrt_bytes *y = cplrt_bytes_of(v);
        CPLRT(sbuf_puts)(b, "Bytes(\"");
        CPLRT(sbuf_hex)(b, y->data, y->len);
        CPLRT(sbuf_puts)(b, "\")");
        return 0;
    }
    case CPLRT_LIST:
        CPLRT(sbuf_putc)(b, '[');
        if (o->len == 0)
            CPLRT(sbuf_putc)(b, ']');
        return o->len;
    case CPLRT_STRUCT:
        render_bare_name(b, p, o->u.live.def);
        CPLRT(sbuf_puts)(b, o->len == 0 ? " {}" : " { ");
        return o->len;
    case CPLRT_ENUM:
        render_member(b, p, o->u.live.def, cplrt_variant(v), "variant");
        if (cplrt_payload_len(v) != 0)
            CPLRT(sbuf_putc)(b, '(');
        return cplrt_payload_len(v);
    case CPLRT_WRAPPER:
        render_bare_name(b, p, o->u.live.def);
        CPLRT(sbuf_putc)(b, '(');
        return 1;
    default: return 0;
    }
}

/* mirrors cpl_eval::json::value_text_untyped (write_text and push_items) */
void CPLRT(write_value_text)(cplrt_sbuf *b, const cplrt_program *p, cplrt_w v)
{
    render_frame local[32];
    render_frame *stack = local;
    size_t cap = sizeof local / sizeof local[0], depth = 0;
    uint32_t n = render_open(b, p, v);
    if (n != 0)
        stack[depth++] = (render_frame){ v, 0, n };
    while (depth > 0 && b->status == CPLRT_OK) {
        render_frame *f = &stack[depth - 1];
        const cplrt_obj *o = cplrt_obj_of(f->v);
        if (f->i == f->n) {
            CPLRT(sbuf_puts)(b, o->kind == CPLRT_LIST ? "]" : (o->kind == CPLRT_STRUCT ? " }" : ")"));
            depth--;
            continue;
        }
        uint32_t i = f->i++;
        if (i > 0)
            CPLRT(sbuf_puts)(b, ", ");
        cplrt_w child;
        if (o->kind == CPLRT_STRUCT) {
            render_member(b, p, o->u.live.def, i, "field");
            CPLRT(sbuf_puts)(b, ": ");
            child = cplrt_item(f->v, i);
        } else if (o->kind == CPLRT_ENUM) {
            child = cplrt_payload(f->v, i);
        } else {
            child = cplrt_item(f->v, i);
        }
        n = render_open(b, p, child);
        if (n == 0)
            continue;
        if (depth == cap) {
            size_t ncap = cap * 2u;
            render_frame *grown = (render_frame *)render_alloc(b->rt, ncap * sizeof *grown);
            if (grown == NULL) {
                b->status = CPLRT_NOMEM;
                break;
            }
            memcpy(grown, stack, depth * sizeof *grown);
            if (stack != local)
                render_free(b->rt, stack);
            stack = grown;
            cap = ncap;
        }
        stack[depth++] = (render_frame){ child, 0, n };
    }
    if (stack != local)
        render_free(b->rt, stack);
}
