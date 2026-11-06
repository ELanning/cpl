/* cpl_rt_wire.c: values as the node lists of cpl-driver protocol 1 and cpl-worker protocol 1 (spec/cabi.md 3.8,
   spec/python.md 6.2): the iterative writer and reader that the driver, the worker client, the C adapter host, and
   the Python extension share, so that no side recurses on a value's depth and a shared child is sent once.
   File-scope names begin with wire_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <stdlib.h>
#include <string.h>

/* ---- values as node lists (3.8) ------------------------------------------------------------------------------- */

/* An open-addressing map from words to node indices. */
typedef struct wire_map {
    uint64_t *keys;
    uint32_t *vals;
    size_t cap, n;
} wire_map;

static size_t wire_hash(uint64_t k, size_t cap)
{
    k ^= k >> 33;
    k *= UINT64_C(0xff51afd7ed558ccd);
    k ^= k >> 33;
    return (size_t)k & (cap - 1u);
}

static int wire_map_grow(wire_map *m)
{
    size_t cap = m->cap == 0 ? 64u : m->cap * 2u;
    uint64_t *keys = (uint64_t *)calloc(cap, sizeof *keys);
    uint32_t *vals = (uint32_t *)calloc(cap, sizeof *vals);
    if (keys == NULL || vals == NULL) {
        free(keys);
        free(vals);
        return 0;
    }
    for (size_t i = 0; i < m->cap; i++) {
        if (m->keys[i] != 0) {
            size_t j = wire_hash(m->keys[i], cap);
            while (keys[j] != 0)
                j = (j + 1u) & (cap - 1u);
            keys[j] = m->keys[i];
            vals[j] = m->vals[i];
        }
    }
    free(m->keys);
    free(m->vals);
    m->keys = keys;
    m->vals = vals;
    m->cap = cap;
    return 1;
}

static int wire_map_get(const wire_map *m, uint64_t k, uint32_t *v)
{
    if (m->cap == 0)
        return 0;
    size_t j = wire_hash(k, m->cap);
    while (m->keys[j] != 0) {
        if (m->keys[j] == k) {
            *v = m->vals[j];
            return 1;
        }
        j = (j + 1u) & (m->cap - 1u);
    }
    return 0;
}

static int wire_map_put(wire_map *m, uint64_t k, uint32_t v)
{
    if ((m->n + 1u) * 2u > m->cap && !wire_map_grow(m))
        return 0;
    size_t j = wire_hash(k, m->cap);
    while (m->keys[j] != 0)
        j = (j + 1u) & (m->cap - 1u);
    m->keys[j] = k;
    m->vals[j] = v;
    m->n++;
    return 1;
}

static uint32_t wire_nchildren(cplrt_w w)
{
    if (!cplrt_is_obj(w))
        return 0;
    switch (cplrt_obj_of(w)->kind) {
    case CPLRT_LIST:
    case CPLRT_STRUCT:
    case CPLRT_WRAPPER: return cplrt_len(w);
    case CPLRT_ENUM: return cplrt_payload_len(w);
    default: return 0;
    }
}

static cplrt_w wire_child(cplrt_w w, uint32_t i)
{
    return cplrt_obj_of(w)->kind == CPLRT_ENUM ? cplrt_payload(w, i) : cplrt_item(w, i);
}

/* {"nodes": [...], "json": "...", "text": "..."}: the post-order node list of w, every distinct word once, with the C
   writer's value JSON and value text (the root last). */
void CPLRT(nodes_write)(cplrt_sbuf *b, cplrt_runtime *scratch, const cplrt_program *p, cplrt_w root, int renderings)
{
    wire_map map = { NULL, NULL, 0, 0 };
    cplrt_w *order = NULL;
    size_t norder = 0, ocap = 0;
    typedef struct { cplrt_w w; uint32_t next; } frame;
    frame *stack = NULL;
    size_t n = 0, scap = 0;
    int failed = 0;
#define WIRE_PUSH(word)                                                                                              \
    do {                                                                                                               \
        if (n == scap) {                                                                                               \
            size_t c2 = scap == 0 ? 64u : scap * 2u;                                                                   \
            frame *g = (frame *)realloc(stack, c2 * sizeof *g);                                                        \
            if (g == NULL) {                                                                                           \
                failed = 1;                                                                                            \
                break;                                                                                                 \
            }                                                                                                          \
            stack = g;                                                                                                 \
            scap = c2;                                                                                                 \
        }                                                                                                              \
        stack[n].w = (word);                                                                                           \
        stack[n].next = 0;                                                                                             \
        n++;                                                                                                           \
    } while (0)
    WIRE_PUSH(root);
    while (n > 0 && !failed) {
        frame *f = &stack[n - 1];
        uint32_t idx;
        if (wire_map_get(&map, f->w, &idx)) {
            n--;
            continue;
        }
        if (f->next < wire_nchildren(f->w)) {
            cplrt_w c = wire_child(f->w, f->next++);
            if (!wire_map_get(&map, c, &idx))
                WIRE_PUSH(c);
            continue;
        }
        if (norder == ocap) {
            size_t c2 = ocap == 0 ? 64u : ocap * 2u;
            cplrt_w *g = (cplrt_w *)realloc(order, c2 * sizeof *g);
            if (g == NULL) {
                failed = 1;
                break;
            }
            order = g;
            ocap = c2;
        }
        if (!wire_map_put(&map, f->w, (uint32_t)norder)) {
            failed = 1;
            break;
        }
        order[norder++] = f->w;
        n--;
    }
#undef WIRE_PUSH
    if (failed) {
        b->status = CPLRT_NOMEM;
    } else {
        CPLRT(sbuf_puts)(b, "{\"nodes\":[");
        for (size_t i = 0; i < norder; i++) {
            cplrt_w w = order[i];
            if (i > 0)
                CPLRT(sbuf_putc)(b, ',');
            if (cplrt_is_small(w) || (cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_BIGINT)) {
                CPLRT(sbuf_puts)(b, "[\"i\",\"");
                CPLRT(int_write)(b, w);
                CPLRT(sbuf_puts)(b, "\"]");
            } else if (w == CPLRT_TRUE) {
                CPLRT(sbuf_puts)(b, "[\"t\"]");
            } else if (w == CPLRT_FALSE) {
                CPLRT(sbuf_puts)(b, "[\"f\"]");
            } else {
                const cplrt_obj *o = cplrt_obj_of(w);
                switch (o->kind) {
                case CPLRT_TEXT:
                    CPLRT(sbuf_puts)(b, "[\"s\",");
                    CPLRT(write_json_string)(b, cplrt_text_of(w)->utf8, cplrt_text_of(w)->bytes);
                    CPLRT(sbuf_putc)(b, ']');
                    break;
                case CPLRT_BYTES:
                    CPLRT(sbuf_puts)(b, "[\"y\",\"");
                    CPLRT(sbuf_hex)(b, cplrt_bytes_of(w)->data, cplrt_bytes_of(w)->len);
                    CPLRT(sbuf_puts)(b, "\"]");
                    break;
                default: {
                    const char *tag = o->kind == CPLRT_LIST     ? "l"
                                      : o->kind == CPLRT_STRUCT ? "r"
                                      : o->kind == CPLRT_ENUM   ? "e"
                                                                : "w";
                    CPLRT(sbuf_puts)(b, "[\"");
                    CPLRT(sbuf_puts)(b, tag);
                    CPLRT(sbuf_putc)(b, '"');
                    if (o->kind != CPLRT_LIST) {
                        CPLRT(sbuf_putc)(b, ',');
                        CPLRT(sbuf_u64)(b, o->u.live.def);
                    }
                    if (o->kind == CPLRT_ENUM) {
                        CPLRT(sbuf_putc)(b, ',');
                        CPLRT(sbuf_u64)(b, cplrt_variant(w));
                    }
                    uint32_t nc = wire_nchildren(w);
                    for (uint32_t k = 0; k < nc; k++) {
                        uint32_t ci = 0;
                        (void)wire_map_get(&map, wire_child(w, k), &ci);
                        CPLRT(sbuf_putc)(b, ',');
                        CPLRT(sbuf_u64)(b, ci);
                    }
                    CPLRT(sbuf_putc)(b, ']');
                    break;
                }
                }
            }
        }
        if (!renderings) {
            CPLRT(sbuf_puts)(b, "]}");
            goto done;
        }
        CPLRT(sbuf_puts)(b, "],\"json\":");
        cplrt_sbuf t;
        cplrt_sbuf_init(&t, scratch);
        CPLRT(sbuf_put)(&t, "", 0);
        CPLRT(write_value_json)(&t, p, root);
        if (t.status == CPLRT_OK)
            CPLRT(write_json_string)(b, t.ptr, t.len);
        else
            b->status = CPLRT_NOMEM;
        CPLRT(sbuf_free)(&t);
        CPLRT(sbuf_puts)(b, ",\"text\":");
        cplrt_sbuf_init(&t, scratch);
        CPLRT(sbuf_put)(&t, "", 0);
        CPLRT(write_value_text)(&t, p, root);
        if (t.status == CPLRT_OK)
            CPLRT(write_json_string)(b, t.ptr, t.len);
        else
            b->status = CPLRT_NOMEM;
        CPLRT(sbuf_free)(&t);
        CPLRT(sbuf_putc)(b, '}');
    }
done:
    free(map.keys);
    free(map.vals);
    free(order);
    free(stack);
}

/* A number of a request as a uint64, 1 when the node is a nonnegative integer that fits. */
int CPLRT(jv_u64)(const cplrt_jv *v, uint64_t *out)
{
    if (v == NULL || v->type != CPLRT_JSON_NUMBER || v->len == 0)
        return 0;
    uint64_t x = 0;
    for (size_t i = 0; i < v->len; i++) {
        char c = v->str[i];
        if (c < '0' || c > '9')
            return 0;
        uint64_t d = (uint64_t)(c - '0');
        if (x > (UINT64_MAX - d) / 10u)
            return 0;
        x = x * 10u + d;
    }
    *out = x;
    return 1;
}

int CPLRT(jv_u32)(const cplrt_jv *v, uint32_t *out)
{
    uint64_t x;
    if (!CPLRT(jv_u64)(v, &x) || x > UINT32_MAX)
        return 0;
    *out = (uint32_t)x;
    return 1;
}

static int wire_hexval(char c)
{
    if (c >= '0' && c <= '9')
        return c - '0';
    if (c >= 'a' && c <= 'f')
        return c - 'a' + 10;
    return -1; /* lowercase only: one spelling of each byte string (cabi.md 3.8) */
}

/* The value of a node list {"nodes": [...]} built in the configured runtime: CPLRT_OK and *out (owned), CPLRT_INVALID
   with a detail, or CPLRT_NOMEM. */
/* The most positions a value from another process may have once the nodes it shares are expanded: every walk of it
   (rendering, equality, its uses in CPL code) visits them all, and a few hundred bytes of wire nodes can otherwise
   describe more of them than any walk finishes. */
#define WIRE_MAX_EXPANDED (UINT64_C(1) << 26)
/* The most digits of an Int from another process: parsing is quadratic in them (about a second at this bound). */
#define WIRE_MAX_INT_DIGITS 1000000u

/* `-?(0|[1-9][0-9]*)` but not "-0": the one decimal spelling of each Int (spec/cabi.md 3.8). */
static int wire_canonical_int(const char *s, size_t len)
{
    size_t i = len > 0 && s[0] == '-' ? 1u : 0u;
    if (i == len)
        return 0;
    if (s[i] == '0')
        return len == i + 1u && i == 0;
    for (; i < len; i++)
        if (s[i] < '0' || s[i] > '9')
            return 0;
    return 1;
}

int CPLRT(nodes_read)(cplrt_runtime *rt, const cplrt_jv *v, cplrt_w *out, const char **detail)
{
    const cplrt_program *p = rt->program;
    const cplrt_jv *nodes = CPLRT(json_member)(v, "nodes");
    if (nodes == NULL || nodes->type != CPLRT_JSON_ARRAY || nodes->n == 0) {
        *detail = "a value is an object with a non-empty \"nodes\" array";
        return CPLRT_INVALID;
    }
    uint32_t n = nodes->n;
    cplrt_w *words = (cplrt_w *)calloc(n, sizeof *words);
    uint64_t *expanded = (uint64_t *)calloc(n, sizeof *expanded); /* each node's positions, saturating */
    if (words == NULL || expanded == NULL) {
        free(words);
        free(expanded);
        return CPLRT_NOMEM;
    }
    int r = CPLRT_OK;
    for (uint32_t i = 0; i < n && r == CPLRT_OK; i++) {
        const cplrt_jv *node = &nodes->items[i];
        if (node->type != CPLRT_JSON_ARRAY || node->n == 0 || node->items[0].type != CPLRT_JSON_STRING ||
            node->items[0].len != 1u) {
            *detail = "a node is an array beginning with its one-letter tag";
            r = CPLRT_INVALID;
            break;
        }
        char tag = node->items[0].str[0];
        const cplrt_jv *a = node->items + 1;
        uint32_t na = node->n - 1u;
        switch (tag) {
        case 'i':
            if (na != 1 || a[0].type != CPLRT_JSON_STRING) {
                *detail = "an Int node is [\"i\", \"<decimal>\"]";
                r = CPLRT_INVALID;
            } else if (!wire_canonical_int(a[0].str, a[0].len)) {
                *detail = "an Int node holds a canonical decimal";
                r = CPLRT_INVALID;
            } else if (a[0].len > WIRE_MAX_INT_DIGITS) {
                *detail = "an Int node holds at most 1000000 digits";
                r = CPLRT_INVALID;
            } else {
                int k = CPLRT(int_parse)(rt, a[0].str, a[0].len, &words[i]);
                if (k == CPLRT_INVALID) {
                    *detail = "an Int node holds a canonical decimal";
                    r = CPLRT_INVALID;
                } else if (k != CPLRT_OK) {
                    r = CPLRT_NOMEM;
                }
            }
            break;
        case 't':
        case 'f': words[i] = tag == 't' ? CPLRT_TRUE : CPLRT_FALSE; break;
        case 's':
            if (na != 1 || a[0].type != CPLRT_JSON_STRING) {
                *detail = "a Text node is [\"s\", \"<text>\"]";
                r = CPLRT_INVALID;
            } else if (CPLRT(text_make)(rt, a[0].str, a[0].len, &words[i]) != CPLRT_OK) {
                r = CPLRT_NOMEM;
            }
            break;
        case 'y': {
            if (na != 1 || a[0].type != CPLRT_JSON_STRING || a[0].len % 2u != 0) {
                *detail = "a Bytes node is [\"y\", \"<lowercase hex>\"]";
                r = CPLRT_INVALID;
                break;
            }
            size_t len = a[0].len / 2u;
            uint8_t *bytes = (uint8_t *)malloc(len + 1u);
            if (bytes == NULL) {
                r = CPLRT_NOMEM;
                break;
            }
            for (size_t k = 0; k < len && r == CPLRT_OK; k++) {
                int hi = wire_hexval(a[0].str[2 * k]), lo = wire_hexval(a[0].str[2 * k + 1]);
                if (hi < 0 || lo < 0) {
                    *detail = "a Bytes node holds lowercase hex digits";
                    r = CPLRT_INVALID;
                }
                bytes[k] = (uint8_t)(hi * 16 + lo);
            }
            if (r == CPLRT_OK && CPLRT(bytes_make)(rt, bytes, len, &words[i]) != CPLRT_OK)
                r = CPLRT_NOMEM;
            free(bytes);
            break;
        }
        case 'l':
        case 'r':
        case 'e':
        case 'w': {
            uint32_t fixed = tag == 'l' ? 0u : tag == 'e' ? 2u : 1u;
            if (na < fixed) {
                *detail = "a composite node lacks its declaration";
                r = CPLRT_INVALID;
                break;
            }
            uint32_t def = 0, variant = 0;
            if (fixed >= 1u && (!CPLRT(jv_u32)(&a[0], &def) || def >= p->ndefs)) {
                *detail = "a node names a declaration the program does not have";
                r = CPLRT_INVALID;
                break;
            }
            if (fixed == 2u && !CPLRT(jv_u32)(&a[1], &variant)) {
                *detail = "an enum node names its variant by index";
                r = CPLRT_INVALID;
                break;
            }
            uint32_t nk = na - fixed;
            cplrt_w *kids = nk > 0 ? (cplrt_w *)malloc(nk * sizeof *kids) : NULL;
            if (nk > 0 && kids == NULL) {
                r = CPLRT_NOMEM;
                break;
            }
            uint64_t size = 1;
            for (uint32_t k = 0; k < nk; k++) {
                uint32_t ci = 0;
                if (!CPLRT(jv_u32)(&a[fixed + k], &ci) || ci >= i) {
                    *detail = "a node refers to a child that is not an earlier node";
                    r = CPLRT_INVALID;
                    break;
                }
                kids[k] = words[ci];
                size = size + expanded[ci] < size ? UINT64_MAX : size + expanded[ci];
            }
            expanded[i] = size;
            if (r == CPLRT_OK) {
                const cplrt_def *d = fixed >= 1u ? &p->defs[def] : NULL;
                if (tag == 'r' && (d->kind != CPLRT_DEF_STRUCT || nk != d->nmembers)) {
                    *detail = "a struct node does not match its declaration";
                    r = CPLRT_INVALID;
                } else if (tag == 'e' && (d->kind != CPLRT_DEF_ENUM || variant >= d->nmembers)) {
                    *detail = "an enum node does not match its declaration";
                    r = CPLRT_INVALID;
                } else if (tag == 'w' && (d->kind != CPLRT_DEF_WRAPPER || nk != 1u)) {
                    *detail = "a wrapper node does not match its declaration";
                    r = CPLRT_INVALID;
                } else {
                    uint8_t kind = tag == 'l' ? CPLRT_LIST : tag == 'r' ? CPLRT_STRUCT : tag == 'e' ? CPLRT_ENUM
                                                                                                     : CPLRT_WRAPPER;
                    if (CPLRT(node_make)(rt, kind, def, variant, kids, nk, &words[i]) != CPLRT_OK)
                        r = CPLRT_NOMEM;
                }
            }
            free(kids);
            break;
        }
        default:
            *detail = "a node has an unknown tag";
            r = CPLRT_INVALID;
            break;
        }
        if (expanded[i] == 0)
            expanded[i] = 1; /* a leaf */
    }
    if (r == CPLRT_OK && expanded[n - 1] > WIRE_MAX_EXPANDED) {
        *detail = "the value has too many positions once its shared nodes are expanded";
        r = CPLRT_INVALID;
    }
    if (r == CPLRT_OK) {
        cplrt_retain(words[n - 1]);
        *out = words[n - 1];
    }
    for (uint32_t i = 0; i < n; i++)
        cplrt_release(words[i]);
    free(words);
    free(expanded);
    return r;
}

/* Decodes an array of values; *out (malloc) holds owned words. */
int CPLRT(nodes_read_all)(cplrt_runtime *rt, const cplrt_jv *arr, cplrt_w **out, uint32_t *n, const char **detail)
{
    *out = NULL;
    *n = 0;
    if (arr == NULL || arr->type != CPLRT_JSON_ARRAY) {
        *detail = "an array of values is expected";
        return CPLRT_INVALID;
    }
    cplrt_w *words = arr->n > 0 ? (cplrt_w *)calloc(arr->n, sizeof *words) : NULL;
    if (arr->n > 0 && words == NULL)
        return CPLRT_NOMEM;
    for (uint32_t i = 0; i < arr->n; i++) {
        int r = CPLRT(nodes_read)(rt, &arr->items[i], &words[i], detail);
        if (r != CPLRT_OK) {
            for (uint32_t j = 0; j < i; j++)
                cplrt_release(words[j]);
            free(words);
            return r;
        }
    }
    *out = words;
    *n = arr->n;
    return CPLRT_OK;
}

void CPLRT(words_release)(cplrt_w *words, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++)
        cplrt_release(words[i]);
    free(words);
}

