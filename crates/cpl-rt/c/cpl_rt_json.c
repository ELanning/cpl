/* cpl_rt_json.c: JSON (spec/cabi.md 3.3.2, 3.8, 4.8): serde_json's compact string escapes, value JSON byte for byte
   as serde_json::to_string(value_to_json_untyped) writes it (object keys in byte order, semantics S40), the S49
   diagnostic of a fault record, the payload object of a driver's fault record, the strict value-JSON reader of
   toolchain 7.3 (duplicate keys rejected, no nesting limit), and the protocol reader of the driver (a cplrt_jv tree,
   at most CPLRT_JSON_MAX_DEPTH deep). Every writer and the reader run on explicit work stacks (2.4). Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <stdlib.h>
#include <string.h>

/* ---- growable arrays ------------------------------------------------------------------------------------------- */

/* Grows *items (of *cap elements of elem bytes) to hold need elements; 0 when the runtime cannot allocate. */
static int json_grow(cplrt_runtime *rt, void **items, size_t *cap, size_t need, size_t elem)
{
    if (need <= *cap)
        return 1;
    size_t ncap = *cap != 0 ? *cap : 16u;
    while (ncap < need) {
        if (ncap > SIZE_MAX / 2u / elem)
            return 0;
        ncap *= 2u;
    }
    void *grown = CPLRT(alloc)(rt, ncap * elem);
    if (grown == NULL)
        return 0;
    if (*items != NULL) {
        memcpy(grown, *items, *cap * elem);
        CPLRT(free)(*items);
    }
    *items = grown;
    *cap = ncap;
    return 1;
}

/* ---- strings ----------------------------------------------------------------------------------------------------- */

/* serde_json's compact formatter: \" \\ \b \t \n \f \r, \u00xx (lowercase) for the other controls below U+0020, and
   every other byte as it is (U+007F and non-ASCII included) */
void CPLRT(write_json_string)(cplrt_sbuf *b, const char *s, size_t len)
{
    static const char digits[] = "0123456789abcdef";
    CPLRT(sbuf_putc)(b, '"');
    size_t run = 0;
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)s[i];
        const char *esc = NULL;
        char u[7];
        switch (c) {
        case '"': esc = "\\\""; break;
        case '\\': esc = "\\\\"; break;
        case '\b': esc = "\\b"; break;
        case '\t': esc = "\\t"; break;
        case '\n': esc = "\\n"; break;
        case '\f': esc = "\\f"; break;
        case '\r': esc = "\\r"; break;
        default:
            if (c < 0x20u) {
                memcpy(u, "\\u00", 4);
                u[4] = digits[c >> 4];
                u[5] = digits[c & 15u];
                u[6] = '\0';
                esc = u;
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

static void json_cstring(cplrt_sbuf *b, const char *s)
{
    CPLRT(write_json_string)(b, s, strlen(s));
}

/* ---- value JSON ---------------------------------------------------------------------------------------------- */

static const cplrt_def *json_def(const cplrt_program *p, uint32_t def)
{
    return p != NULL && def < p->ndefs ? &p->defs[def] : NULL;
}

/* mirrors cpl_eval::types::type_name: the `$type` of a value's declaration */
static void json_type_name(cplrt_sbuf *b, const cplrt_program *p, uint32_t def)
{
    const cplrt_def *d = json_def(p, def);
    if (d != NULL && d->type_name != NULL) {
        json_cstring(b, d->type_name);
        return;
    }
    char tmp[32] = "{def ";
    size_t k = 5;
    char digits[12];
    size_t nd = 0;
    uint32_t v = def;
    do {
        digits[nd++] = (char)('0' + (int)(v % 10u));
        v /= 10u;
    } while (v != 0);
    while (nd > 0)
        tmp[k++] = digits[--nd];
    tmp[k++] = '}';
    CPLRT(write_json_string)(b, tmp, k);
}

/* mirrors cpl_eval::json::field_name and variant_name: the name of member `index` of def, or `<what N>` when the value
   does not match its declaration (no such declaration, one that is not a struct, respectively an enum, or no such
   member), into tmp when it must be formatted. */
static const char *json_member_name(const cplrt_program *p, uint32_t def, uint32_t index, const char *what, char *tmp,
                                    size_t cap)
{
    const cplrt_def *d = json_def(p, def);
    uint32_t kind = what[0] == 'f' ? CPLRT_DEF_STRUCT : CPLRT_DEF_ENUM;
    if (d != NULL && d->kind == kind && index < d->nmembers && d->members != NULL)
        return d->members[index];
    size_t k = 0, wl = strlen(what);
    tmp[k++] = '<';
    memcpy(tmp + k, what, wl);
    k += wl;
    tmp[k++] = ' ';
    char digits[12];
    size_t nd = 0;
    uint32_t v = index;
    do {
        digits[nd++] = (char)('0' + (int)(v % 10u));
        v /= 10u;
    } while (v != 0);
    while (nd > 0 && k + 2u < cap)
        tmp[k++] = digits[--nd];
    tmp[k++] = '>';
    tmp[k] = '\0';
    return tmp;
}

/* The field index whose name is the k-th in byte order among the len fields of a struct of def: cplrt_def.sorted for
   a value that matches its declaration, else a rank computed over the names, the `<field N>` ones included. */
static uint32_t json_field_at(const cplrt_program *p, uint32_t def, uint32_t len, uint32_t k)
{
    const cplrt_def *d = json_def(p, def);
    if (d != NULL && d->kind == CPLRT_DEF_STRUCT && d->sorted != NULL && d->members != NULL && d->nmembers == len)
        return d->sorted[k];
    char ta[40], tb[40];
    for (uint32_t j = 0; j < len; j++) {
        const char *nj = json_member_name(p, def, j, "field", ta, sizeof ta);
        uint32_t rank = 0;
        for (uint32_t i = 0; i < len; i++) {
            const char *ni = json_member_name(p, def, i, "field", tb, sizeof tb);
            int c = strcmp(ni, nj);
            if (c < 0 || (c == 0 && i < j))
                rank++;
        }
        if (rank == k)
            return j;
    }
    return k;
}

typedef struct json_frame {
    cplrt_w v;
    uint32_t i, n;
} json_frame;

/* Writes the leaf v, or the opening of the composite v, and returns its child count (0 for a leaf or none). */
static uint32_t json_open(cplrt_sbuf *b, const cplrt_program *p, cplrt_w v)
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
        CPLRT(write_json_string)(b, t->utf8, t->bytes);
        return 0;
    }
    case CPLRT_BYTES: {
        const cplrt_bytes *y = cplrt_bytes_of(v);
        CPLRT(sbuf_puts)(b, "{\"$type\":\"Bytes\",\"hex\":\"");
        CPLRT(sbuf_hex)(b, y->data, y->len);
        CPLRT(sbuf_puts)(b, "\"}");
        return 0;
    }
    case CPLRT_LIST:
        CPLRT(sbuf_putc)(b, '[');
        if (o->len == 0)
            CPLRT(sbuf_putc)(b, ']');
        return o->len;
    case CPLRT_STRUCT:
        CPLRT(sbuf_puts)(b, "{\"$type\":");
        json_type_name(b, p, o->u.live.def);
        if (o->len == 0)
            CPLRT(sbuf_putc)(b, '}');
        return o->len;
    case CPLRT_ENUM: {
        uint32_t n = cplrt_payload_len(v);
        if (n != 0) {
            CPLRT(sbuf_puts)(b, "{\"$payload\":[");
            return n;
        }
        CPLRT(sbuf_puts)(b, "{\"$type\":");
        json_type_name(b, p, o->u.live.def);
        if (p == NULL || o->u.live.def != p->std.unit) {
            char tmp[40];
            CPLRT(sbuf_puts)(b, ",\"$variant\":");
            json_cstring(b, json_member_name(p, o->u.live.def, cplrt_variant(v), "variant", tmp, sizeof tmp));
        }
        CPLRT(sbuf_putc)(b, '}');
        return 0;
    }
    case CPLRT_WRAPPER: CPLRT(sbuf_puts)(b, "{\"$payload\":["); return 1;
    default: return 0;
    }
}

/* mirrors cpl_eval::json::value_to_json_untyped (assemble_json), serialized as serde_json::to_string does */
void CPLRT(write_value_json)(cplrt_sbuf *b, const cplrt_program *p, cplrt_w v)
{
    json_frame local[32];
    json_frame *stack = local;
    size_t cap = sizeof local / sizeof local[0], depth = 0;
    uint32_t n = json_open(b, p, v);
    if (n != 0)
        stack[depth++] = (json_frame){ v, 0, n };
    while (depth > 0 && b->status == CPLRT_OK) {
        json_frame *f = &stack[depth - 1];
        const cplrt_obj *o = cplrt_obj_of(f->v);
        if (f->i == f->n) {
            if (o->kind == CPLRT_LIST) {
                CPLRT(sbuf_putc)(b, ']');
            } else if (o->kind == CPLRT_STRUCT) {
                CPLRT(sbuf_putc)(b, '}');
            } else {
                char tmp[40];
                CPLRT(sbuf_puts)(b, "],\"$type\":");
                json_type_name(b, p, o->u.live.def);
                if (o->kind == CPLRT_ENUM) {
                    CPLRT(sbuf_puts)(b, ",\"$variant\":");
                    json_cstring(b, json_member_name(p, o->u.live.def, cplrt_variant(f->v), "variant", tmp, sizeof tmp));
                }
                CPLRT(sbuf_putc)(b, '}');
            }
            depth--;
            continue;
        }
        uint32_t i = f->i++;
        cplrt_w child;
        if (o->kind == CPLRT_STRUCT) {
            char tmp[40];
            uint32_t field = json_field_at(p, o->u.live.def, o->len, i);
            CPLRT(sbuf_putc)(b, ',');
            json_cstring(b, json_member_name(p, o->u.live.def, field, "field", tmp, sizeof tmp));
            CPLRT(sbuf_putc)(b, ':');
            child = cplrt_item(f->v, field);
        } else {
            if (i > 0)
                CPLRT(sbuf_putc)(b, ',');
            child = o->kind == CPLRT_ENUM ? cplrt_payload(f->v, i) : cplrt_item(f->v, i);
        }
        n = json_open(b, p, child);
        if (n == 0)
            continue;
        if (depth == cap) {
            size_t ncap = cap * 2u;
            json_frame *grown = b->rt != NULL ? (json_frame *)CPLRT(alloc)(b->rt, ncap * sizeof *grown) : NULL;
            if (grown == NULL) {
                b->status = CPLRT_NOMEM;
                break;
            }
            memcpy(grown, stack, depth * sizeof *grown);
            if (stack != local)
                CPLRT(free)(stack);
            stack = grown;
            cap = ncap;
        }
        stack[depth++] = (json_frame){ child, 0, n };
    }
    if (stack != local)
        CPLRT(free)(stack);
}

/* ---- the S49 diagnostic of a fault record (3.3.2) -------------------------------------------------------------- */

/* cpl_diag::RESERVED_KEYS: the diagnostic object's own fields, which a payload key never replaces */
static const char *const json_reserved[10] = { "code",   "category",       "severity", "message", "span",
                                               "symbol", "qualified_name", "facet",    "notes",   "backtrace" };

static int json_is_reserved(const char *key)
{
    for (size_t i = 0; i < sizeof json_reserved / sizeof json_reserved[0]; i++)
        if (strcmp(json_reserved[i], key) == 0)
            return 1;
    return 0;
}

/* One payload key to write: a datum of the record, or a string the kind implies. */
typedef struct json_entry {
    const char *key;
    const cplrt_datum *datum; /* NULL: str */
    const char *str;
} json_entry;

static void json_datum(cplrt_sbuf *b, const cplrt_datum *d)
{
    switch (d->form) {
    case CPLRT_DATUM_STRING: json_cstring(b, d->str != NULL ? d->str : ""); break;
    case CPLRT_DATUM_NUMBER: CPLRT(sbuf_u64)(b, d->num); break;
    case CPLRT_DATUM_JSON:
        if (d->json != NULL)
            CPLRT(sbuf_put)(b, d->json, d->len);
        else
            CPLRT(sbuf_puts)(b, "null");
        break;
    default: CPLRT(sbuf_puts)(b, "null"); break;
    }
}

/* Writes the entries as `"key":value` pairs, sorted by key bytes (every serde_json map is a sorted map), each preceded
   by a comma when `comma` or after the first. */
static void json_entries(cplrt_sbuf *b, json_entry *e, uint32_t n, int comma)
{
    for (uint32_t i = 1; i < n; i++) { /* insertion sort: payloads hold a handful of keys */
        json_entry t = e[i];
        uint32_t j = i;
        while (j > 0 && strcmp(e[j - 1].key, t.key) > 0) {
            e[j] = e[j - 1];
            j--;
        }
        e[j] = t;
    }
    for (uint32_t i = 0; i < n; i++) {
        if (comma || i > 0)
            CPLRT(sbuf_putc)(b, ',');
        json_cstring(b, e[i].key);
        CPLRT(sbuf_putc)(b, ':');
        if (e[i].datum != NULL)
            json_datum(b, e[i].datum);
        else
            json_cstring(b, e[i].str);
    }
}

/* The Location of a site (cpl_diag::Location: file, line, col, end_line, end_col), when the program knows its file. */
static int json_location(cplrt_sbuf *b, const cplrt_program *p, uint32_t site)
{
    if (p == NULL || site == CPLRT_NONE || site >= p->nsites)
        return 0;
    const cplrt_site *s = &p->sites[site];
    if (s->file >= p->nfiles || p->files == NULL)
        return 0;
    CPLRT(sbuf_puts)(b, "{\"file\":");
    json_cstring(b, p->files[s->file]);
    CPLRT(sbuf_puts)(b, ",\"line\":");
    CPLRT(sbuf_u64)(b, s->line);
    CPLRT(sbuf_puts)(b, ",\"col\":");
    CPLRT(sbuf_u64)(b, s->col);
    CPLRT(sbuf_puts)(b, ",\"end_line\":");
    CPLRT(sbuf_u64)(b, s->end_line);
    CPLRT(sbuf_puts)(b, ",\"end_col\":");
    CPLRT(sbuf_u64)(b, s->end_col);
    CPLRT(sbuf_putc)(b, '}');
    return 1;
}

static int json_has_location(const cplrt_program *p, uint32_t site)
{
    return p != NULL && site != CPLRT_NONE && site < p->nsites && p->files != NULL &&
           p->sites[site].file < p->nfiles;
}

static const char *json_clause_kind(uint32_t k)
{
    switch (k) {
    case CPLRT_CK_REQUIRES: return "requires";
    case CPLRT_CK_ENSURES: return "ensures";
    case CPLRT_CK_INVARIANT: return "invariant";
    case CPLRT_CK_DECREASES: return "decreases";
    default: return NULL;
    }
}

static const char *json_resource(uint32_t r)
{
    switch (r) {
    case CPLRT_RES_STACK: return "stack";
    case CPLRT_RES_MEMORY: return "memory";
    case CPLRT_RES_TIME: return "time";
    case CPLRT_RES_STEPS: return "steps";
    default: return NULL;
    }
}

/* mirrors cpl_eval::fault::Fault::to_diagnostic followed by cpl_diag::Diagnostic::to_json, serialized compactly:
   code, category, severity, message, span, symbol, qualified_name, facet, backtrace (notes are never present), then the
   payload keys in byte order with the kind's own keys (clause_kind, status, attributed_to, resource, kind) */
void CPLRT(write_s49)(cplrt_sbuf *b, const cplrt_program *p, const cplrt_fault *f)
{
    static const char *const codes[8] = { NULL, "contract_violation", "assertion_failed", "unresolved_hole",
                                          "division_by_zero", "refinement_violation", "resource_exhaustion",
                                          "internal_error" };
    static const char *const categories[8] = { NULL, "contract", "test", "hole", "runtime", "runtime", "internal",
                                               "internal" };
    uint32_t kind = f->kind < 8u ? f->kind : CPLRT_FK_INTERNAL_ERROR;
    if (kind == 0)
        kind = CPLRT_FK_INTERNAL_ERROR;
    CPLRT(sbuf_puts)(b, "{\"code\":");
    json_cstring(b, codes[kind]);
    CPLRT(sbuf_puts)(b, ",\"category\":");
    json_cstring(b, categories[kind]);
    CPLRT(sbuf_puts)(b, ",\"severity\":\"error\",\"message\":");
    CPLRT(write_json_string)(b, f->message != NULL ? f->message : "", f->message != NULL ? f->message_len : 0u);
    if (json_has_location(p, f->span)) {
        CPLRT(sbuf_puts)(b, ",\"span\":");
        json_location(b, p, f->span);
    }
    const char *symbol = kind == CPLRT_FK_INTERNAL_ERROR ? NULL : f->symbol;
    if (symbol != NULL) {
        CPLRT(sbuf_puts)(b, ",\"symbol\":");
        json_cstring(b, symbol);
        CPLRT(sbuf_puts)(b, ",\"qualified_name\":");
        json_cstring(b, f->qualified_name != NULL ? f->qualified_name : symbol);
    }
    const char *facet = NULL;
    if (kind == CPLRT_FK_CONTRACT_VIOLATION)
        facet = "contract";
    else if (kind != CPLRT_FK_INTERNAL_ERROR)
        facet = f->facet == CPLRT_FACET_CONTRACT ? "contract" : (symbol != NULL ? "body" : NULL);
    if (facet != NULL) {
        CPLRT(sbuf_puts)(b, ",\"facet\":");
        json_cstring(b, facet);
    }
    if (f->nframes > 0) {
        CPLRT(sbuf_puts)(b, ",\"backtrace\":[");
        for (uint32_t i = 0; i < f->nframes; i++) {
            const cplrt_btframe *fr = &f->frames[i];
            CPLRT(sbuf_puts)(b, i > 0 ? ",{\"qualified_name\":" : "{\"qualified_name\":");
            const cplrt_def *d = json_def(p, fr->def);
            if (d != NULL && d->qualified_name != NULL) {
                json_cstring(b, d->qualified_name);
            } else { /* fault::def_path of an id the program lacks */
                char tmp[32];
                size_t k = 0;
                memcpy(tmp, "{def ", 5);
                k = 5;
                char digits[12];
                size_t nd = 0;
                uint32_t v = fr->def;
                do {
                    digits[nd++] = (char)('0' + (int)(v % 10u));
                    v /= 10u;
                } while (v != 0);
                while (nd > 0)
                    tmp[k++] = digits[--nd];
                tmp[k++] = '}';
                CPLRT(write_json_string)(b, tmp, k);
            }
            if (json_has_location(p, fr->site)) {
                CPLRT(sbuf_puts)(b, ",\"span\":");
                json_location(b, p, fr->site);
            }
            CPLRT(sbuf_putc)(b, '}');
        }
        CPLRT(sbuf_putc)(b, ']');
    }
    /* the payload: the record's keys, the reserved ones dropped and the kind's own keys present and winning */
    json_entry local[24];
    json_entry *e = local;
    uint32_t cap = (uint32_t)(sizeof local / sizeof local[0]);
    if (f->ndata + 3u > cap) {
        e = b->rt != NULL ? (json_entry *)CPLRT(alloc)(b->rt, ((size_t)f->ndata + 3u) * sizeof *e) : NULL;
        if (e == NULL) {
            if (b->status == CPLRT_OK)
                b->status = CPLRT_NOMEM;
            return;
        }
    }
    uint32_t n = 0;
    const char *own_key[2] = { NULL, NULL }, *own_value[2] = { NULL, NULL };
    const char *default_key = NULL, *default_value = NULL;
    if (kind == CPLRT_FK_CONTRACT_VIOLATION) {
        const char *ck = json_clause_kind(f->clause_kind);
        own_key[0] = ck != NULL ? "clause_kind" : NULL;
        own_value[0] = ck;
        own_key[1] = "status";
        own_value[1] = "failed";
        default_key = "attributed_to";
        default_value = f->clause_kind == CPLRT_CK_REQUIRES ? "caller" : "body";
    } else if (kind == CPLRT_FK_RESOURCE_EXHAUSTION) {
        const char *r = json_resource(f->resource);
        own_key[0] = r != NULL ? "resource" : NULL;
        own_value[0] = r;
    } else if (kind == CPLRT_FK_INTERNAL_ERROR) {
        default_key = "kind";
        default_value = "unspecified";
    }
    int has_default = 0;
    for (uint32_t i = 0; i < f->ndata; i++) {
        const cplrt_datum *d = &f->data[i];
        if (d->key == NULL || json_is_reserved(d->key))
            continue;
        if ((own_key[0] != NULL && strcmp(d->key, own_key[0]) == 0) ||
            (own_key[1] != NULL && strcmp(d->key, own_key[1]) == 0))
            continue;
        if (default_key != NULL && strcmp(d->key, default_key) == 0)
            has_default = 1;
        e[n++] = (json_entry){ d->key, d, NULL };
    }
    for (int k = 0; k < 2; k++)
        if (own_key[k] != NULL)
            e[n++] = (json_entry){ own_key[k], NULL, own_value[k] };
    if (default_key != NULL && !has_default)
        e[n++] = (json_entry){ default_key, NULL, default_value };
    json_entries(b, e, n, 1);
    CPLRT(sbuf_putc)(b, '}');
    if (e != local)
        CPLRT(free)(e);
}

/* The record's payload as one object, keys in byte order: the `data` of a driver's fault record (3.8). */
void CPLRT(write_payload)(cplrt_sbuf *b, const cplrt_fault *f)
{
    json_entry local[24];
    json_entry *e = local;
    if (f->ndata > sizeof local / sizeof local[0]) {
        e = b->rt != NULL ? (json_entry *)CPLRT(alloc)(b->rt, (size_t)f->ndata * sizeof *e) : NULL;
        if (e == NULL) {
            if (b->status == CPLRT_OK)
                b->status = CPLRT_NOMEM;
            return;
        }
    }
    uint32_t n = 0;
    for (uint32_t i = 0; i < f->ndata; i++)
        if (f->data[i].key != NULL)
            e[n++] = (json_entry){ f->data[i].key, &f->data[i], NULL };
    CPLRT(sbuf_putc)(b, '{');
    json_entries(b, e, n, 0);
    CPLRT(sbuf_putc)(b, '}');
    if (e != local)
        CPLRT(free)(e);
}

/* ---- the JSON parser: one document as a flat node array, iteratively ---------------------------------------------- */

#define JSON_NONE UINT32_MAX

typedef struct json_node {
    uint32_t type;           /* CPLRT_JSON_* */
    uint32_t n;              /* array elements or object members */
    uint32_t first, last;    /* children, linked through next */
    uint32_t next;
    uint32_t reserved;
    size_t off, len;         /* STRING: the decoded bytes in the arena; NUMBER: the text in the input */
    size_t key_off, key_len; /* a member of an object: its decoded key in the arena */
    size_t key_at;           /* the input offset of that key */
} json_node;

typedef struct json_doc {
    cplrt_runtime *rt;
    const char *text;
    size_t len;
    json_node *nodes;
    size_t nnodes, ncap;
    char *arena; /* decoded strings, each followed by a NUL */
    size_t alen, acap;
    uint32_t *stack; /* the open containers, outermost first */
    size_t depth, scap;
    size_t members;  /* object members in the document */
    size_t numbytes; /* the bytes of every number's text */
    int status;      /* CPLRT_OK, CPLRT_INVALID, CPLRT_NOMEM */
    size_t error_at;
    const char *reason; /* static */
    int duplicate;      /* the failure is a duplicate key, dup_off and dup_len in the arena */
    size_t dup_off, dup_len;
} json_doc;

static void json_doc_init(json_doc *d, cplrt_runtime *rt, const char *text, size_t len)
{
    memset(d, 0, sizeof *d);
    d->rt = rt;
    d->text = text;
    d->len = len;
    d->status = CPLRT_OK;
}

static void json_doc_free(json_doc *d)
{
    CPLRT(free)(d->nodes);
    CPLRT(free)(d->arena);
    CPLRT(free)(d->stack);
    d->nodes = NULL;
    d->arena = NULL;
    d->stack = NULL;
}

static int json_fail(json_doc *d, size_t at, const char *reason)
{
    if (d->status == CPLRT_OK) {
        d->status = CPLRT_INVALID;
        d->error_at = at;
        d->reason = reason;
    }
    return 0;
}

static int json_nomem(json_doc *d)
{
    d->status = CPLRT_NOMEM;
    return 0;
}

static size_t json_ws(const json_doc *d, size_t i)
{
    while (i < d->len && (d->text[i] == ' ' || d->text[i] == '\t' || d->text[i] == '\n' || d->text[i] == '\r'))
        i++;
    return i;
}

static int json_arena_put(json_doc *d, const char *s, size_t n)
{
    if (n == 0u)
        return 1;
    if (!json_grow(d->rt, (void **)&d->arena, &d->acap, d->alen + n, 1u))
        return json_nomem(d);
    memcpy(d->arena + d->alen, s, n);
    d->alen += n;
    return 1;
}

/* A new node of type, linked as the last child of the innermost open container (with the pending key of an object);
   JSON_NONE after an allocation failure. */
static uint32_t json_node_new(json_doc *d, uint32_t type, int has_key, size_t key_off, size_t key_len, size_t key_at)
{
    if (d->nnodes >= JSON_NONE - 1u || !json_grow(d->rt, (void **)&d->nodes, &d->ncap, d->nnodes + 1u, sizeof(json_node))) {
        json_nomem(d);
        return JSON_NONE;
    }
    uint32_t k = (uint32_t)d->nnodes++;
    json_node *node = &d->nodes[k];
    memset(node, 0, sizeof *node);
    node->type = type;
    node->first = node->last = node->next = JSON_NONE;
    if (has_key) {
        node->key_off = key_off;
        node->key_len = key_len;
        node->key_at = key_at;
    }
    if (d->depth > 0) {
        json_node *parent = &d->nodes[d->stack[d->depth - 1]];
        if (parent->last == JSON_NONE)
            parent->first = k;
        else
            d->nodes[parent->last].next = k;
        parent->last = k;
        parent->n++;
        if (parent->type == CPLRT_JSON_OBJECT)
            d->members++;
    }
    return k;
}

static int json_hex4(const json_doc *d, size_t i, uint32_t *out)
{
    if (d->len - i < 4u)
        return 0;
    uint32_t v = 0;
    for (size_t k = 0; k < 4u; k++) {
        char c = d->text[i + k];
        uint32_t digit;
        if (c >= '0' && c <= '9')
            digit = (uint32_t)(c - '0');
        else if (c >= 'a' && c <= 'f')
            digit = (uint32_t)(c - 'a') + 10u;
        else if (c >= 'A' && c <= 'F')
            digit = (uint32_t)(c - 'A') + 10u;
        else
            return 0;
        v = v * 16u + digit;
    }
    *out = v;
    return 1;
}

/* The string whose opening quote is at *pos, decoded into the arena with a NUL after it: *off and *slen; *pos after the
   closing quote. Unpaired surrogates and raw controls are rejected, as serde_json rejects them. */
static int json_string(json_doc *d, size_t *pos, size_t *off, size_t *slen)
{
    size_t i = *pos + 1u, run = i;
    size_t start = d->alen;
    for (;;) {
        if (i >= d->len)
            return json_fail(d, i, "EOF while parsing a string");
        unsigned char c = (unsigned char)d->text[i];
        if (c == '"')
            break;
        if (c < 0x20u)
            return json_fail(d, i, "control character (\\u0000-\\u001F) found while parsing a string");
        if (c != '\\') {
            i++;
            continue;
        }
        if (!json_arena_put(d, d->text + run, i - run))
            return 0;
        if (i + 1u >= d->len)
            return json_fail(d, i + 1u, "EOF while parsing a string");
        char e = d->text[i + 1u];
        char one = 0;
        switch (e) {
        case '"': one = '"'; break;
        case '\\': one = '\\'; break;
        case '/': one = '/'; break;
        case 'b': one = '\b'; break;
        case 'f': one = '\f'; break;
        case 'n': one = '\n'; break;
        case 'r': one = '\r'; break;
        case 't': one = '\t'; break;
        case 'u': break;
        default: return json_fail(d, i + 1u, "invalid escape");
        }
        if (e != 'u') {
            if (!json_arena_put(d, &one, 1u))
                return 0;
            i += 2u;
            run = i;
            continue;
        }
        uint32_t u;
        if (!json_hex4(d, i + 2u, &u))
            return json_fail(d, i + 2u, "invalid escape");
        i += 6u;
        if (u >= 0xDC00u && u <= 0xDFFFu)
            return json_fail(d, i, "lone trailing surrogate in hex escape");
        if (u >= 0xD800u && u <= 0xDBFFu) {
            uint32_t lo;
            if (i + 1u >= d->len || d->text[i] != '\\' || d->text[i + 1u] != 'u' || !json_hex4(d, i + 2u, &lo) ||
                lo < 0xDC00u || lo > 0xDFFFu)
                return json_fail(d, i, "lone leading surrogate in hex escape");
            u = 0x10000u + ((u - 0xD800u) << 10) + (lo - 0xDC00u);
            i += 6u;
        }
        char utf8[4];
        size_t k;
        if (u < 0x80u) {
            utf8[0] = (char)u;
            k = 1;
        } else if (u < 0x800u) {
            utf8[0] = (char)(0xC0u | (u >> 6));
            utf8[1] = (char)(0x80u | (u & 0x3Fu));
            k = 2;
        } else if (u < 0x10000u) {
            utf8[0] = (char)(0xE0u | (u >> 12));
            utf8[1] = (char)(0x80u | ((u >> 6) & 0x3Fu));
            utf8[2] = (char)(0x80u | (u & 0x3Fu));
            k = 3;
        } else {
            utf8[0] = (char)(0xF0u | (u >> 18));
            utf8[1] = (char)(0x80u | ((u >> 12) & 0x3Fu));
            utf8[2] = (char)(0x80u | ((u >> 6) & 0x3Fu));
            utf8[3] = (char)(0x80u | (u & 0x3Fu));
            k = 4;
        }
        if (!json_arena_put(d, utf8, k))
            return 0;
        run = i;
    }
    if (!json_arena_put(d, d->text + run, i - run) || !json_arena_put(d, "", 1u))
        return 0;
    *off = start;
    *slen = d->alen - start - 1u;
    *pos = i + 1u;
    return 1;
}

static int json_digit(const json_doc *d, size_t i)
{
    return i < d->len && d->text[i] >= '0' && d->text[i] <= '9';
}

/* A number of the JSON grammar at *pos: -?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)? */
static int json_number(json_doc *d, size_t *pos)
{
    size_t i = *pos;
    if (i < d->len && d->text[i] == '-')
        i++;
    if (!json_digit(d, i))
        return json_fail(d, i, "invalid number");
    if (d->text[i] == '0')
        i++;
    else
        while (json_digit(d, i))
            i++;
    if (i < d->len && d->text[i] == '.') {
        i++;
        if (!json_digit(d, i))
            return json_fail(d, i, "invalid number");
        while (json_digit(d, i))
            i++;
    }
    if (i < d->len && (d->text[i] == 'e' || d->text[i] == 'E')) {
        i++;
        if (i < d->len && (d->text[i] == '+' || d->text[i] == '-'))
            i++;
        if (!json_digit(d, i))
            return json_fail(d, i, "invalid number");
        while (json_digit(d, i))
            i++;
    }
    *pos = i;
    return 1;
}

/* A key and its colon at *pos (after any whitespace): the decoded key's place in the arena. */
static int json_key(json_doc *d, size_t *pos, size_t *off, size_t *len, size_t *at)
{
    size_t i = *pos;
    if (i >= d->len || d->text[i] != '"')
        return json_fail(d, i, "key must be a string");
    *at = i;
    if (!json_string(d, &i, off, len))
        return 0;
    i = json_ws(d, i);
    if (i >= d->len || d->text[i] != ':')
        return json_fail(d, i, "expected `:`");
    *pos = json_ws(d, i + 1u);
    return 1;
}

typedef struct json_keyref {
    const char *s;
    size_t len, at;
} json_keyref;

static int json_keyref_cmp(const void *a, const void *b)
{
    const json_keyref *x = (const json_keyref *)a, *y = (const json_keyref *)b;
    size_t n = x->len < y->len ? x->len : y->len;
    int c = n != 0 ? memcmp(x->s, y->s, n) : 0;
    if (c != 0)
        return c;
    if (x->len != y->len)
        return x->len < y->len ? -1 : 1;
    return x->at < y->at ? -1 : (x->at > y->at ? 1 : 0);
}

/* An object being closed: a key that occurs twice is a failure (spec/cabi.md 4.8: duplicate keys included). */
static int json_check_duplicates(json_doc *d, uint32_t object)
{
    const json_node *o = &d->nodes[object];
    if (o->n < 2u)
        return 1;
    json_keyref *keys = (json_keyref *)CPLRT(alloc)(d->rt, (size_t)o->n * sizeof *keys);
    if (keys == NULL)
        return json_nomem(d);
    size_t k = 0;
    for (uint32_t c = o->first; c != JSON_NONE; c = d->nodes[c].next)
        keys[k++] = (json_keyref){ d->arena + d->nodes[c].key_off, d->nodes[c].key_len, d->nodes[c].key_at };
    qsort(keys, k, sizeof *keys, json_keyref_cmp);
    int ok = 1;
    for (size_t i = 1; i < k && ok; i++) {
        if (keys[i].len == keys[i - 1].len && (keys[i].len == 0 || memcmp(keys[i].s, keys[i - 1].s, keys[i].len) == 0)) {
            d->duplicate = 1;
            d->dup_off = (size_t)(keys[i].s - d->arena);
            d->dup_len = keys[i].len;
            ok = json_fail(d, keys[i].at, "duplicate key");
        }
    }
    CPLRT(free)(keys);
    return ok;
}

/* Parses the whole text into d: the root is node 0. max_depth 0 is no nesting limit. 1 on success. */
static int json_parse_doc(json_doc *d, size_t max_depth)
{
    size_t ok_upto = 0;
    if (!CPLRT(utf8_valid)(d->text, d->len, &ok_upto, NULL))
        return json_fail(d, ok_upto, "invalid UTF-8");
    size_t i = json_ws(d, 0);
    int value = 1, has_key = 0;
    size_t key_off = 0, key_len = 0, key_at = 0;
    for (;;) {
        if (value) {
            if (i >= d->len)
                return json_fail(d, i, "EOF while parsing a value");
            char c = d->text[i];
            uint32_t k;
            if (c == '{' || c == '[') {
                if (max_depth != 0 && d->depth >= max_depth)
                    return json_fail(d, i, "recursion limit exceeded");
                k = json_node_new(d, c == '{' ? CPLRT_JSON_OBJECT : CPLRT_JSON_ARRAY, has_key, key_off, key_len, key_at);
                if (k == JSON_NONE || !json_grow(d->rt, (void **)&d->stack, &d->scap, d->depth + 1u, sizeof(uint32_t)))
                    return json_nomem(d);
                d->stack[d->depth++] = k;
                has_key = 0;
                i = json_ws(d, i + 1u);
                char close = c == '{' ? '}' : ']';
                if (i < d->len && d->text[i] == close) {
                    d->depth--;
                    i++;
                    value = 0;
                    continue;
                }
                if (c == '{') {
                    if (!json_key(d, &i, &key_off, &key_len, &key_at))
                        return 0;
                    has_key = 1;
                }
                continue;
            }
            if (c == '"') {
                size_t off = 0, slen = 0;
                k = json_node_new(d, CPLRT_JSON_STRING, has_key, key_off, key_len, key_at);
                if (k == JSON_NONE || !json_string(d, &i, &off, &slen))
                    return 0;
                d->nodes[k].off = off;
                d->nodes[k].len = slen;
            } else if (c == '-' || (c >= '0' && c <= '9')) {
                size_t start = i;
                if (!json_number(d, &i))
                    return 0;
                k = json_node_new(d, CPLRT_JSON_NUMBER, has_key, key_off, key_len, key_at);
                if (k == JSON_NONE)
                    return 0;
                d->nodes[k].off = start;
                d->nodes[k].len = i - start;
                d->numbytes += i - start + 1u;
            } else if (c == 't' && d->len - i >= 4u && memcmp(d->text + i, "true", 4) == 0) {
                if (json_node_new(d, CPLRT_JSON_TRUE, has_key, key_off, key_len, key_at) == JSON_NONE)
                    return 0;
                i += 4u;
            } else if (c == 'f' && d->len - i >= 5u && memcmp(d->text + i, "false", 5) == 0) {
                if (json_node_new(d, CPLRT_JSON_FALSE, has_key, key_off, key_len, key_at) == JSON_NONE)
                    return 0;
                i += 5u;
            } else if (c == 'n' && d->len - i >= 4u && memcmp(d->text + i, "null", 4) == 0) {
                if (json_node_new(d, CPLRT_JSON_NULL, has_key, key_off, key_len, key_at) == JSON_NONE)
                    return 0;
                i += 4u;
            } else {
                return json_fail(d, i, "expected value");
            }
            has_key = 0;
            value = 0;
            continue;
        }
        i = json_ws(d, i);
        if (d->depth == 0) {
            if (i != d->len)
                return json_fail(d, i, "trailing characters");
            return 1;
        }
        const json_node *top = &d->nodes[d->stack[d->depth - 1]];
        int object = top->type == CPLRT_JSON_OBJECT;
        if (i >= d->len)
            return json_fail(d, i, object ? "EOF while parsing an object" : "EOF while parsing a list");
        char c = d->text[i];
        if (c == ',') {
            i = json_ws(d, i + 1u);
            if (object) {
                if (!json_key(d, &i, &key_off, &key_len, &key_at))
                    return 0;
                has_key = 1;
            }
            value = 1;
            continue;
        }
        if (c == (object ? '}' : ']')) {
            if (object && !json_check_duplicates(d, d->stack[d->depth - 1]))
                return 0;
            d->depth--;
            i++;
            continue;
        }
        return json_fail(d, i, object ? "expected `,` or `}`" : "expected `,` or `]`");
    }
}

/* ---- the protocol reader (3.8) --------------------------------------------------------------------------------- */

/* The tree is one block of the runtime: the cplrt_jv nodes (the root first, every container's items contiguous), then
   the decoded strings and the numbers' texts, so CPLRT(json_free) of the root frees it all. */
int CPLRT(json_parse)(cplrt_runtime *rt, const char *text, size_t len, cplrt_jv **out, size_t *error_offset)
{
    json_doc d;
    json_doc_init(&d, rt, text, len);
    if (!json_parse_doc(&d, CPLRT_JSON_MAX_DEPTH)) {
        int st = d.status;
        if (st == CPLRT_INVALID && error_offset != NULL)
            *error_offset = d.error_at;
        json_doc_free(&d);
        return st;
    }
    size_t count = d.nnodes + d.members;
    size_t bytes = count * sizeof(cplrt_jv) + d.alen + d.numbytes + 1u;
    cplrt_jv *jv = (cplrt_jv *)CPLRT(alloc)(rt, bytes);
    size_t *queue = (size_t *)CPLRT(alloc)(rt, 2u * d.nnodes * sizeof(size_t));
    if (jv == NULL || queue == NULL) {
        CPLRT(free)(jv);
        CPLRT(free)(queue);
        json_doc_free(&d);
        return CPLRT_NOMEM;
    }
    char *strings = (char *)(void *)(jv + count);
    if (d.alen != 0)
        memcpy(strings, d.arena, d.alen);
    char *numbers = strings + d.alen;
    size_t head = 0, tail = 0, next = 1;
    queue[tail++] = 0;
    queue[tail++] = 0;
    while (head < tail) {
        const json_node *node = &d.nodes[queue[head++]];
        cplrt_jv *v = &jv[queue[head++]];
        v->type = node->type;
        v->n = node->n;
        v->str = NULL;
        v->len = 0;
        v->items = NULL;
        if (node->type == CPLRT_JSON_STRING) {
            v->str = strings + node->off;
            v->len = node->len;
        } else if (node->type == CPLRT_JSON_NUMBER) {
            memcpy(numbers, text + node->off, node->len);
            numbers[node->len] = '\0';
            v->str = numbers;
            v->len = node->len;
            numbers += node->len + 1u;
        } else if (node->type == CPLRT_JSON_ARRAY || node->type == CPLRT_JSON_OBJECT) {
            int object = node->type == CPLRT_JSON_OBJECT;
            v->items = node->n != 0 ? &jv[next] : NULL;
            size_t at = next;
            next += object ? 2u * (size_t)node->n : node->n;
            for (uint32_t c = node->first; c != JSON_NONE; c = d.nodes[c].next) {
                if (object) {
                    cplrt_jv *key = &jv[at++];
                    key->type = CPLRT_JSON_STRING;
                    key->n = 0;
                    key->str = strings + d.nodes[c].key_off;
                    key->len = d.nodes[c].key_len;
                    key->items = NULL;
                }
                queue[tail++] = c;
                queue[tail++] = at++;
            }
        }
    }
    CPLRT(free)(queue);
    json_doc_free(&d);
    *out = jv;
    return CPLRT_OK;
}

void CPLRT(json_free)(cplrt_jv *v)
{
    CPLRT(free)(v);
}

const cplrt_jv *CPLRT(json_member)(const cplrt_jv *v, const char *key)
{
    if (v == NULL || v->type != CPLRT_JSON_OBJECT)
        return NULL;
    size_t len = strlen(key);
    for (uint32_t i = 0; i < v->n; i++) {
        const cplrt_jv *k = &v->items[2u * i];
        if (k->len == len && memcmp(k->str, key, len) == 0)
            return &v->items[2u * i + 1u];
    }
    return NULL;
}

/* ---- the strict value-JSON reader (4.8; toolchain 7.3) --------------------------------------------------------- */

typedef struct json_seg {
    const char *s; /* NULL: an index */
    size_t len;
    uint64_t index;
} json_seg;

#define JSON_DEC_LIST 1u
#define JSON_DEC_STRUCT 2u
#define JSON_DEC_ENUM 3u
#define JSON_DEC_WRAPPER 4u

typedef struct json_dframe {
    uint32_t what;   /* JSON_DEC_* */
    uint32_t type;   /* CPL_X of the container's type */
    uint32_t def, variant;
    uint32_t node;   /* the object of a struct, else the array whose elements are decoded */
    uint32_t child;  /* the next element (not for a struct) */
    uint32_t i, n;
    uint32_t ptypes; /* an enum: the offset of the variant's payload types in cplrt_type.payloads */
    uint32_t reserved;
    size_t base;     /* the height of the value stack when the frame began */
    size_t entry;    /* the path length at the frame's position */
    size_t mark;     /* the path length its children's segments follow */
} json_dframe;

typedef struct json_dec {
    cplrt_runtime *rt;
    const cplrt_program *p;
    json_doc *doc;
    cplrt_w *values;
    size_t nvalues, vcap;
    json_dframe *frames;
    size_t nframes, fcap;
    json_seg *path;
    size_t npath, pcap;
    cplrt_sbuf *pointer, *detail;
    int status;
} json_dec;

static int json_dec_nomem(json_dec *d)
{
    d->status = CPLRT_NOMEM;
    return -1;
}

static int json_path_push(json_dec *d, const char *s, size_t len, uint64_t index)
{
    if (!json_grow(d->rt, (void **)&d->path, &d->pcap, d->npath + 1u, sizeof(json_seg)))
        return 0;
    d->path[d->npath++] = (json_seg){ s, len, index };
    return 1;
}

/* The decoding failure at the current path (and the key `extra`, when not NULL): the pointer is written, and the detail
   buffer returned for the reason. */
static cplrt_sbuf *json_err(json_dec *d, const char *extra, size_t extra_len)
{
    d->status = CPLRT_INVALID;
    CPLRT(sbuf_put)(d->pointer, "", 0);
    for (size_t i = 0; i < d->npath; i++) {
        const json_seg *s = &d->path[i];
        if (s->s != NULL) {
            CPLRT(sbuf_pointer_segment)(d->pointer, s->s, s->len);
        } else {
            char tmp[24];
            size_t k = sizeof tmp;
            uint64_t v = s->index;
            do {
                tmp[--k] = (char)('0' + (int)(v % 10u));
                v /= 10u;
            } while (v != 0);
            CPLRT(sbuf_pointer_segment)(d->pointer, tmp + k, sizeof tmp - k);
        }
    }
    if (extra != NULL)
        CPLRT(sbuf_pointer_segment)(d->pointer, extra, extra_len);
    return d->detail;
}

static const json_node *json_n(const json_dec *d, uint32_t node)
{
    return &d->doc->nodes[node];
}

static const char *json_str(const json_dec *d, uint32_t node)
{
    return d->doc->arena + json_n(d, node)->off;
}

/* The member of the object node named (s, len), or JSON_NONE. */
static uint32_t json_get(const json_dec *d, uint32_t object, const char *s, size_t len)
{
    for (uint32_t c = json_n(d, object)->first; c != JSON_NONE; c = json_n(d, c)->next) {
        const json_node *m = json_n(d, c);
        if (m->key_len == len && (len == 0 || memcmp(d->doc->arena + m->key_off, s, len) == 0))
            return c;
    }
    return JSON_NONE;
}

/* A number's text as serde_json's Number displays it (`{n}`): with arbitrary_precision the text as written, except
   that Deserializer::scan_exponent stores the exponent marker as "e" followed by its sign, "+" when none was written
   ("1E5" and "1e5" are both "1e+5"). */
static void json_number_text(cplrt_sbuf *b, const char *s, size_t len)
{
    for (size_t i = 0; i < len; i++) {
        if (s[i] != 'e' && s[i] != 'E')
            continue;
        CPLRT(sbuf_put)(b, s, i);
        CPLRT(sbuf_putc)(b, 'e');
        if (i + 1u < len && s[i + 1u] != '+' && s[i + 1u] != '-')
            CPLRT(sbuf_putc)(b, '+');
        CPLRT(sbuf_put)(b, s + i + 1u, len - i - 1u);
        return;
    }
    CPLRT(sbuf_put)(b, s, len);
}

/* mirrors cpl_eval::json::describe */
static void json_describe(const json_dec *d, cplrt_sbuf *b, uint32_t node)
{
    const json_node *j = json_n(d, node);
    switch (j->type) {
    case CPLRT_JSON_NULL: CPLRT(sbuf_puts)(b, "null"); break;
    case CPLRT_JSON_TRUE: CPLRT(sbuf_puts)(b, "the boolean true"); break;
    case CPLRT_JSON_FALSE: CPLRT(sbuf_puts)(b, "the boolean false"); break;
    case CPLRT_JSON_NUMBER:
        CPLRT(sbuf_puts)(b, "the number ");
        json_number_text(b, d->doc->text + j->off, j->len);
        break;
    case CPLRT_JSON_STRING: CPLRT(sbuf_puts)(b, "a string"); break;
    case CPLRT_JSON_ARRAY: CPLRT(sbuf_puts)(b, "an array"); break;
    default: {
        uint32_t t = json_get(d, node, "$type", 5u);
        if (t != JSON_NONE && json_n(d, t)->type == CPLRT_JSON_STRING) {
            CPLRT(sbuf_puts)(b, "an object with \"$type\" ");
            CPLRT(write_json_string)(b, json_str(d, t), json_n(d, t)->len);
        } else {
            CPLRT(sbuf_puts)(b, "an object");
        }
        break;
    }
    }
}

/* mirrors Decoder::expected: "expected `<type>` (<form>), found <description>" */
static int json_expected(json_dec *d, const cplrt_type *ty, const char *form, size_t form_len, uint32_t node)
{
    cplrt_sbuf *b = json_err(d, NULL, 0);
    CPLRT(sbuf_puts)(b, "expected `");
    CPLRT(sbuf_puts)(b, ty->display != NULL ? ty->display : ty->text);
    CPLRT(sbuf_puts)(b, "` (");
    CPLRT(sbuf_put)(b, form, form_len);
    CPLRT(sbuf_puts)(b, "), found ");
    json_describe(d, b, node);
    return -1;
}

/* The form of a tagged object: an object with "$type" "<tag>" (JSON-quoted). */
static void json_tag_form(cplrt_sbuf *b, const char *tag)
{
    CPLRT(sbuf_puts)(b, "an object with \"$type\" ");
    json_cstring(b, tag);
}

/* mirrors Decoder::tagged_object: 1 when node is an object whose "$type" is tag */
static int json_tagged(json_dec *d, uint32_t node, const char *tag, const cplrt_type *ty, const char *form)
{
    if (json_n(d, node)->type != CPLRT_JSON_OBJECT) {
        if (form != NULL) {
            json_expected(d, ty, form, strlen(form), node);
        } else {
            cplrt_sbuf f;
            cplrt_sbuf_init(&f, d->rt);
            json_tag_form(&f, tag);
            if (f.status != CPLRT_OK)
                d->status = CPLRT_NOMEM;
            else
                json_expected(d, ty, f.ptr, f.len, node);
            CPLRT(sbuf_free)(&f);
        }
        return 0;
    }
    uint32_t t = json_get(d, node, "$type", 5u);
    if (t == JSON_NONE) {
        cplrt_sbuf *b = json_err(d, NULL, 0);
        CPLRT(sbuf_puts)(b, "missing \"$type\": expected ");
        json_cstring(b, tag);
        return 0;
    }
    const json_node *tn = json_n(d, t);
    if (tn->type == CPLRT_JSON_STRING && tn->len == strlen(tag) && memcmp(json_str(d, t), tag, tn->len) == 0)
        return 1;
    cplrt_sbuf *b = json_err(d, "$type", 5u);
    CPLRT(sbuf_puts)(b, "expected \"$type\" ");
    json_cstring(b, tag);
    CPLRT(sbuf_puts)(b, ", found ");
    if (tn->type == CPLRT_JSON_STRING)
        CPLRT(write_json_string)(b, json_str(d, t), tn->len);
    else
        json_describe(d, b, t);
    return 0;
}

/* Which keys an object form allows: the fixed keys, and for a struct the field names of def. */
typedef struct json_allow {
    const char *const *keys;
    uint32_t nkeys;
    const cplrt_def *fields; /* a struct's declaration, or NULL */
} json_allow;

static int json_allowed(const json_allow *a, const char *s, size_t len)
{
    for (uint32_t i = 0; i < a->nkeys; i++)
        if (strlen(a->keys[i]) == len && memcmp(a->keys[i], s, len) == 0)
            return 1;
    if (a->fields != NULL && a->fields->members != NULL)
        for (uint32_t i = 0; i < a->fields->nmembers; i++)
            if (strlen(a->fields->members[i]) == len && memcmp(a->fields->members[i], s, len) == 0)
                return 1;
    return 0;
}

/* mirrors Decoder::only_keys: the first key in byte order (the order of serde_json's map) that the form refuses */
static int json_only_keys(json_dec *d, uint32_t object, const json_allow *allow, const char *what)
{
    const json_node *worst = NULL;
    for (uint32_t c = json_n(d, object)->first; c != JSON_NONE; c = json_n(d, c)->next) {
        const json_node *m = json_n(d, c);
        if (json_allowed(allow, d->doc->arena + m->key_off, m->key_len))
            continue;
        if (worst != NULL) {
            json_keyref x = { d->doc->arena + m->key_off, m->key_len, 0 }, y = { d->doc->arena + worst->key_off,
                                                                                  worst->key_len, 0 };
            if (json_keyref_cmp(&x, &y) >= 0)
                continue;
        }
        worst = m;
    }
    if (worst == NULL)
        return 1;
    const char *key = d->doc->arena + worst->key_off;
    cplrt_sbuf *b = json_err(d, key, worst->key_len);
    CPLRT(sbuf_puts)(b, "unexpected key ");
    CPLRT(write_json_string)(b, key, worst->key_len);
    CPLRT(sbuf_puts)(b, " in ");
    CPLRT(sbuf_puts)(b, what);
    return 0;
}

static int json_push_value(json_dec *d, cplrt_w w)
{
    if (!json_grow(d->rt, (void **)&d->values, &d->vcap, d->nvalues + 1u, sizeof(cplrt_w))) {
        cplrt_release(w);
        return json_dec_nomem(d);
    }
    d->values[d->nvalues++] = w;
    return 0;
}

static int json_push_frame(json_dec *d, const json_dframe *f)
{
    if (!json_grow(d->rt, (void **)&d->frames, &d->fcap, d->nframes + 1u, sizeof(json_dframe)))
        return json_dec_nomem(d);
    d->frames[d->nframes] = *f;
    d->frames[d->nframes].base = d->nvalues;
    d->frames[d->nframes].entry = d->npath;
    d->frames[d->nframes].mark = d->npath;
    d->nframes++;
    return 1;
}

/* mirrors Decoder::payload's checks: an array of exactly n values at "$payload" (the caller pushed that segment) */
static int json_payload_array(json_dec *d, uint32_t pj, uint32_t n)
{
    const json_node *a = json_n(d, pj);
    if (a->type != CPLRT_JSON_ARRAY) {
        cplrt_sbuf *b = json_err(d, NULL, 0);
        CPLRT(sbuf_puts)(b, "expected an array of ");
        CPLRT(sbuf_u64)(b, n);
        CPLRT(sbuf_puts)(b, " values, found ");
        json_describe(d, b, pj);
        return 0;
    }
    if (a->n != n) {
        cplrt_sbuf *b = json_err(d, NULL, 0);
        CPLRT(sbuf_puts)(b, "expected ");
        CPLRT(sbuf_u64)(b, n);
        CPLRT(sbuf_puts)(b, " payload values, found ");
        CPLRT(sbuf_u64)(b, a->n);
        return 0;
    }
    return 1;
}

/* mirrors Decoder::bytes */
static int json_bytes(json_dec *d, uint32_t node, const cplrt_type *ty)
{
    static const char *const keys[2] = { "$type", "hex" };
    json_allow allow = { keys, 2u, NULL };
    if (!json_tagged(d, node, "Bytes", ty, "{\"$type\": \"Bytes\", \"hex\": ...}") ||
        !json_only_keys(d, node, &allow, "a Bytes value"))
        return -1;
    uint32_t hex = json_get(d, node, "hex", 3u);
    if (hex == JSON_NONE) {
        CPLRT(sbuf_puts)(json_err(d, NULL, 0), "missing \"hex\" of a Bytes value");
        return -1;
    }
    const json_node *h = json_n(d, hex);
    if (h->type != CPLRT_JSON_STRING) {
        cplrt_sbuf *b = json_err(d, "hex", 3u);
        CPLRT(sbuf_puts)(b, "expected a string of lowercase hex digits, found ");
        json_describe(d, b, hex);
        return -1;
    }
    if (h->len % 2u != 0) {
        CPLRT(sbuf_puts)(json_err(d, "hex", 3u), "expected an even number of hex digits");
        return -1;
    }
    const char *s = json_str(d, hex);
    for (size_t i = 0; i < h->len; i += 2u) {
        int ok = 1;
        for (size_t k = 0; k < 2u; k++)
            ok &= (s[i + k] >= '0' && s[i + k] <= '9') || (s[i + k] >= 'a' && s[i + k] <= 'f');
        if (!ok) {
            cplrt_sbuf *b = json_err(d, "hex", 3u);
            CPLRT(sbuf_puts)(b, "expected lowercase hex digits (0-9, a-f), found ");
            /* String::from_utf8_lossy of the pair: each maximal invalid prefix becomes one U+FFFD */
            size_t at = i;
            while (at < i + 2u) {
                unsigned c = (unsigned char)s[at];
                if (c < 0x80u) {
                    CPLRT(sbuf_putc)(b, (char)c);
                    at++;
                    continue;
                }
                size_t need = 0, ok_len = 1;
                unsigned lo = 0x80u, hi = 0xBFu;
                if (c >= 0xC2u && c <= 0xDFu)
                    need = 1;
                else if (c >= 0xE0u && c <= 0xEFu) {
                    need = 2;
                    lo = c == 0xE0u ? 0xA0u : 0x80u;
                    hi = c == 0xEDu ? 0x9Fu : 0xBFu;
                } else if (c >= 0xF0u && c <= 0xF4u) {
                    need = 3;
                    lo = c == 0xF0u ? 0x90u : 0x80u;
                    hi = c == 0xF4u ? 0x8Fu : 0xBFu;
                }
                if (need != 0 && at + 1u < i + 2u && (unsigned char)s[at + 1u] >= lo &&
                    (unsigned char)s[at + 1u] <= hi)
                    ok_len = 2;
                if (need == 1 && ok_len == 2) {
                    CPLRT(sbuf_put)(b, s + at, 2u);
                } else {
                    CPLRT(sbuf_puts)(b, "\xEF\xBF\xBD");
                }
                at += ok_len;
            }
            return -1;
        }
    }
    uint8_t local[64];
    uint8_t *out = h->len / 2u <= sizeof local ? local : (uint8_t *)CPLRT(alloc)(d->rt, h->len / 2u);
    if (out == NULL)
        return json_dec_nomem(d);
    for (size_t i = 0; i < h->len; i += 2u) {
        unsigned hi = (unsigned)(s[i] <= '9' ? s[i] - '0' : s[i] - 'a' + 10);
        unsigned lo = (unsigned)(s[i + 1u] <= '9' ? s[i + 1u] - '0' : s[i + 1u] - 'a' + 10);
        out[i / 2u] = (uint8_t)(hi * 16u + lo);
    }
    cplrt_w w = CPLRT_EMPTY;
    int st = CPLRT(bytes_make)(d->rt, out, h->len / 2u, &w);
    if (out != local)
        CPLRT(free)(out);
    if (st != CPLRT_OK)
        return json_dec_nomem(d);
    return json_push_value(d, w);
}

/* A detail naming a declaration as Decoder::adt and Decoder::wrapper do. */
static void json_quoted_name(cplrt_sbuf *b, const char *prefix, const char *name, const char *suffix)
{
    CPLRT(sbuf_puts)(b, prefix);
    CPLRT(sbuf_puts)(b, name);
    CPLRT(sbuf_puts)(b, suffix);
}

/* mirrors Decoder::adt (struct and enum) and Decoder::wrapper: 0 a value pushed, 1 a frame pushed, -1 failed */
static int json_adt(json_dec *d, uint32_t node, uint32_t type, const cplrt_type *ty)
{
    const cplrt_program *p = d->p;
    if (ty->def >= p->ndefs) {
        cplrt_sbuf *b = json_err(d, NULL, 0);
        if (ty->form == CPLRT_TY_WRAPPER) { /* Decoder::wrapper: no declaration is no wrapper */
            json_quoted_name(b, "`", ty->display != NULL ? ty->display : ty->text, "` is not a wrapper type");
            return -1;
        }
        CPLRT(sbuf_puts)(b, "the program has no declaration ");
        CPLRT(sbuf_u64)(b, ty->def);
        return -1;
    }
    const cplrt_def *def = &p->defs[ty->def];
    const char *tag = def->type_name != NULL ? def->type_name : "";
    const char *display = ty->display != NULL ? ty->display : ty->text;
    int is_struct = ty->form == CPLRT_TY_STRUCT && def->kind == CPLRT_DEF_STRUCT;
    int is_enum = ty->form == CPLRT_TY_ENUM && def->kind == CPLRT_DEF_ENUM;
    int is_wrapper = ty->form == CPLRT_TY_WRAPPER && def->kind == CPLRT_DEF_WRAPPER;
    if (!is_struct && !is_enum && !is_wrapper) {
        json_quoted_name(json_err(d, NULL, 0), "`", display,
                         ty->form == CPLRT_TY_WRAPPER ? "` is not a wrapper type" : "` is not a struct or enum type");
        return -1;
    }
    if (!json_tagged(d, node, tag, ty, NULL))
        return -1;
    cplrt_sbuf what;
    cplrt_sbuf_init(&what, d->rt);
    json_quoted_name(&what, "a `", tag, is_struct ? "` struct" : "` value");
    if (what.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&what);
        return json_dec_nomem(d);
    }
    int r = -1;
    if (is_struct) {
        static const char *const keys[1] = { "$type" };
        json_allow allow = { keys, 1u, def };
        if (json_only_keys(d, node, &allow, what.ptr)) {
            json_dframe f;
            memset(&f, 0, sizeof f);
            f.what = JSON_DEC_STRUCT;
            f.type = type;
            f.def = ty->def;
            f.node = node;
            f.n = ty->nmembers;
            r = json_push_frame(d, &f);
        }
    } else if (is_enum && ty->def == p->std.unit) {
        static const char *const keys[1] = { "$type" };
        json_allow allow = { keys, 1u, NULL };
        if (json_only_keys(d, node, &allow, "the Unit value {\"$type\": \"Unit\"}")) {
            cplrt_w w = CPLRT_EMPTY;
            r = CPLRT(node_make)(d->rt, CPLRT_ENUM, ty->def, 0, NULL, 0, &w) == CPLRT_OK ? json_push_value(d, w)
                                                                                         : json_dec_nomem(d);
        }
    } else if (is_enum) {
        static const char *const keys[3] = { "$type", "$variant", "$payload" };
        json_allow allow = { keys, 3u, NULL };
        if (json_only_keys(d, node, &allow, what.ptr))
            r = 2;
    } else {
        static const char *const keys[2] = { "$type", "$payload" };
        json_allow allow = { keys, 2u, NULL };
        if (json_only_keys(d, node, &allow, what.ptr))
            r = 3;
    }
    if (r == 2) { /* an enum: its variant, then its payload */
        r = -1;
        uint32_t vj = json_get(d, node, "$variant", 8u);
        if (vj == JSON_NONE) {
            json_quoted_name(json_err(d, NULL, 0), "missing \"$variant\" of `", tag, "`");
        } else if (json_n(d, vj)->type != CPLRT_JSON_STRING) {
            cplrt_sbuf *b = json_err(d, "$variant", 8u);
            CPLRT(sbuf_puts)(b, "expected a variant name, found ");
            json_describe(d, b, vj);
        } else {
            const char *name = json_str(d, vj);
            size_t nlen = json_n(d, vj)->len;
            uint32_t index = def->nmembers;
            for (uint32_t i = 0; i < def->nmembers && index == def->nmembers; i++)
                if (strlen(def->members[i]) == nlen && memcmp(def->members[i], name, nlen) == 0)
                    index = i;
            if (index == def->nmembers || index >= ty->nmembers) {
                cplrt_sbuf *b = json_err(d, "$variant", 8u);
                CPLRT(sbuf_putc)(b, '`');
                CPLRT(sbuf_puts)(b, tag);
                CPLRT(sbuf_puts)(b, "` has no variant ");
                CPLRT(write_json_string)(b, name, nlen);
                CPLRT(sbuf_puts)(b, "; its variants are ");
                for (uint32_t i = 0; i < def->nmembers; i++) {
                    if (i > 0)
                        CPLRT(sbuf_puts)(b, ", ");
                    CPLRT(sbuf_puts)(b, def->members[i]);
                }
            } else {
                uint32_t lo = ty->members[index], count = ty->members[index + 1u] - lo;
                uint32_t pj = json_get(d, node, "$payload", 8u);
                if (pj == JSON_NONE && count == 0) {
                    cplrt_w w = CPLRT_EMPTY;
                    r = CPLRT(node_make)(d->rt, CPLRT_ENUM, ty->def, index, NULL, 0, &w) == CPLRT_OK
                            ? json_push_value(d, w)
                            : json_dec_nomem(d);
                } else if (pj == JSON_NONE) {
                    cplrt_sbuf *b = json_err(d, NULL, 0);
                    CPLRT(sbuf_puts)(b, "missing \"$payload\" of the variant ");
                    CPLRT(sbuf_puts)(b, def->members[index]);
                    CPLRT(sbuf_puts)(b, " (");
                    CPLRT(sbuf_u64)(b, count);
                    CPLRT(sbuf_puts)(b, " values)");
                } else if (count == 0) {
                    cplrt_sbuf *b = json_err(d, "$payload", 8u);
                    CPLRT(sbuf_puts)(b, "the variant ");
                    CPLRT(sbuf_puts)(b, def->members[index]);
                    CPLRT(sbuf_puts)(b, " has no payload; omit \"$payload\"");
                } else if (!json_path_push(d, "$payload", 8u, 0)) {
                    r = json_dec_nomem(d);
                } else if (json_payload_array(d, pj, count)) {
                    d->npath--;
                    json_dframe f;
                    memset(&f, 0, sizeof f);
                    f.what = JSON_DEC_ENUM;
                    f.type = type;
                    f.def = ty->def;
                    f.variant = index;
                    f.node = pj;
                    f.child = json_n(d, pj)->first;
                    f.n = count;
                    f.ptypes = lo;
                    r = json_push_frame(d, &f);
                    if (r == 1 && !json_path_push(d, "$payload", 8u, 0))
                        r = json_dec_nomem(d);
                    else if (r == 1)
                        d->frames[d->nframes - 1u].mark = d->npath;
                }
            }
        }
    } else if (r == 3) { /* a wrapper: its one payload value */
        r = -1;
        uint32_t pj = json_get(d, node, "$payload", 8u);
        if (pj == JSON_NONE) {
            json_quoted_name(json_err(d, NULL, 0), "missing \"$payload\" of `", tag, "`");
        } else if (!json_path_push(d, "$payload", 8u, 0)) {
            r = json_dec_nomem(d);
        } else if (json_payload_array(d, pj, 1u)) {
            d->npath--;
            json_dframe f;
            memset(&f, 0, sizeof f);
            f.what = JSON_DEC_WRAPPER;
            f.type = type;
            f.def = ty->def;
            f.node = pj;
            f.child = json_n(d, pj)->first;
            f.n = 1;
            r = json_push_frame(d, &f);
            if (r == 1 && !json_path_push(d, "$payload", 8u, 0))
                r = json_dec_nomem(d);
            else if (r == 1)
                d->frames[d->nframes - 1u].mark = d->npath;
        }
    }
    CPLRT(sbuf_free)(&what);
    return r;
}

/* mirrors Decoder::decode for one position: 0 a value pushed, 1 a frame pushed, -1 failed */
static int json_begin(json_dec *d, uint32_t node, uint32_t type)
{
    const cplrt_program *p = d->p;
    while (type != 0 && type < p->ntypes && p->types[type].form == CPLRT_TY_REFINED) {
        uint32_t base = p->types[type].elem;
        if (base == 0 || base >= p->ntypes || p->types[base].form == CPLRT_TY_REFINED) {
            const cplrt_type *ty = &p->types[type];
            json_quoted_name(json_err(d, NULL, 0), "the refinement `", ty->display != NULL ? ty->display : ty->text,
                             "` has no base type");
            return -1;
        }
        type = base;
    }
    if (type == 0 || type >= p->ntypes) {
        CPLRT(sbuf_puts)(json_err(d, NULL, 0), "the expected type has errors");
        return -1;
    }
    const cplrt_type *ty = &p->types[type];
    const json_node *j = json_n(d, node);
    switch (ty->form) {
    case CPLRT_TY_INT: {
        if (j->type != CPLRT_JSON_NUMBER)
            return json_expected(d, ty, "a JSON number", 13u, node);
        const char *s = d->doc->text + j->off;
        for (size_t i = 0; i < j->len; i++)
            if (s[i] == '.' || s[i] == 'e' || s[i] == 'E') {
                cplrt_sbuf *b = json_err(d, NULL, 0);
                CPLRT(sbuf_puts)(b, "expected `Int`, found the number ");
                json_number_text(b, s, j->len);
                CPLRT(sbuf_puts)(b, ": an Int has no fraction or exponent");
                return -1;
            }
        cplrt_w w = CPLRT_EMPTY;
        if (CPLRT(int_parse)(d->rt, s, j->len, &w) != CPLRT_OK)
            return json_dec_nomem(d);
        return json_push_value(d, w);
    }
    case CPLRT_TY_BOOL:
        if (j->type != CPLRT_JSON_TRUE && j->type != CPLRT_JSON_FALSE)
            return json_expected(d, ty, "true or false", 13u, node);
        return json_push_value(d, cplrt_bool(j->type == CPLRT_JSON_TRUE));
    case CPLRT_TY_TEXT: {
        if (j->type != CPLRT_JSON_STRING)
            return json_expected(d, ty, "a JSON string", 13u, node);
        cplrt_w w = CPLRT_EMPTY;
        if (CPLRT(text_make)(d->rt, json_str(d, node), j->len, &w) != CPLRT_OK)
            return json_dec_nomem(d);
        return json_push_value(d, w);
    }
    case CPLRT_TY_BYTES: return json_bytes(d, node, ty);
    case CPLRT_TY_LIST: {
        if (j->type != CPLRT_JSON_ARRAY)
            return json_expected(d, ty, "a JSON array", 12u, node);
        json_dframe f;
        memset(&f, 0, sizeof f);
        f.what = JSON_DEC_LIST;
        f.type = type;
        f.node = node;
        f.child = j->first;
        f.n = j->n;
        return json_push_frame(d, &f);
    }
    case CPLRT_TY_STRUCT:
    case CPLRT_TY_ENUM:
    case CPLRT_TY_WRAPPER: return json_adt(d, node, type, ty);
    case CPLRT_TY_PARAM:
        CPLRT(sbuf_puts)(json_err(d, NULL, 0),
                         "a value of a generic type parameter cannot be decoded; run a monomorphic wrapper");
        return -1;
    case CPLRT_TY_CAPABILITY:
        json_quoted_name(json_err(d, NULL, 0), "a capability handle (`", ty->display != NULL ? ty->display : ty->text,
                         "`) cannot be supplied as JSON");
        return -1;
    default: CPLRT(sbuf_puts)(json_err(d, NULL, 0), "the expected type has errors"); return -1;
    }
}

/* The frame f is done: its value from the values above its base. */
static int json_finish(json_dec *d, const json_dframe *f)
{
    uint32_t n = (uint32_t)(d->nvalues - f->base);
    const cplrt_w *items = d->values + f->base;
    cplrt_w w = CPLRT_EMPTY;
    int st;
    switch (f->what) {
    case JSON_DEC_LIST: st = CPLRT(node_make)(d->rt, CPLRT_LIST, 0, 0, items, n, &w); break;
    case JSON_DEC_STRUCT: st = CPLRT(node_make)(d->rt, CPLRT_STRUCT, f->def, 0, items, n, &w); break;
    case JSON_DEC_ENUM: st = CPLRT(node_make)(d->rt, CPLRT_ENUM, f->def, f->variant, items, n, &w); break;
    default: st = CPLRT(node_make)(d->rt, CPLRT_WRAPPER, f->def, 0, items, n, &w); break;
    }
    for (size_t i = f->base; i < d->nvalues; i++)
        cplrt_release(d->values[i]);
    d->nvalues = f->base;
    if (st != CPLRT_OK)
        return json_dec_nomem(d);
    return json_push_value(d, w);
}

/* mirrors cpl_eval::json::value_from_json (Decoder), iteratively and on a parsed document whose duplicate keys were
   refused */
static int json_decode(json_dec *d, uint32_t type)
{
    int r = json_begin(d, 0, type);
    while (r >= 0 && d->nframes > 0) {
        json_dframe *f = &d->frames[d->nframes - 1u];
        if (f->i == f->n) {
            json_dframe done = *f;
            d->nframes--;
            d->npath = done.entry;
            r = json_finish(d, &done);
            continue;
        }
        d->npath = f->mark;
        uint32_t i = f->i++;
        uint32_t child, ctype;
        const cplrt_type *ty = &d->p->types[f->type];
        if (f->what == JSON_DEC_STRUCT) {
            const cplrt_def *def = &d->p->defs[f->def];
            const char *name = def->members != NULL && i < def->nmembers ? def->members[i] : "";
            size_t nlen = strlen(name);
            child = json_get(d, f->node, name, nlen);
            if (child == JSON_NONE) {
                cplrt_sbuf *b = json_err(d, NULL, 0);
                CPLRT(sbuf_puts)(b, "missing field ");
                json_cstring(b, name);
                CPLRT(sbuf_puts)(b, " of `");
                CPLRT(sbuf_puts)(b, def->type_name != NULL ? def->type_name : "");
                CPLRT(sbuf_putc)(b, '`');
                r = -1;
                break;
            }
            ctype = ty->members[i];
            if (!json_path_push(d, name, nlen, 0)) {
                r = json_dec_nomem(d);
                break;
            }
        } else {
            child = f->child;
            f->child = json_n(d, child)->next;
            ctype = f->what == JSON_DEC_LIST ? ty->elem
                                              : (f->what == JSON_DEC_ENUM ? ty->payloads[f->ptypes + i] : ty->elem);
            if (!json_path_push(d, NULL, 0, i)) {
                r = json_dec_nomem(d);
                break;
            }
        }
        r = json_begin(d, child, ctype);
    }
    return r;
}

int CPLRT(read_value_json)(cplrt_runtime *rt, uint32_t type, const char *json, size_t len, cplrt_w *out,
                           cplrt_sbuf *pointer, cplrt_sbuf *detail)
{
    json_doc doc;
    json_doc_init(&doc, rt, json, len);
    if (!json_parse_doc(&doc, 0)) {
        int st = doc.status;
        if (st == CPLRT_INVALID) {
            /* the position of the innermost open container, and the parser's reason */
            CPLRT(sbuf_put)(pointer, "", 0);
            for (size_t k = 0; k + 1u < doc.depth; k++) {
                const json_node *c = &doc.nodes[doc.stack[k]];
                const json_node *child = &doc.nodes[doc.stack[k + 1u]];
                if (c->type == CPLRT_JSON_ARRAY) {
                    char tmp[24];
                    size_t at = sizeof tmp;
                    uint64_t v = c->n - 1u;
                    do {
                        tmp[--at] = (char)('0' + (int)(v % 10u));
                        v /= 10u;
                    } while (v != 0);
                    CPLRT(sbuf_pointer_segment)(pointer, tmp + at, sizeof tmp - at);
                } else {
                    CPLRT(sbuf_pointer_segment)(pointer, doc.arena + child->key_off, child->key_len);
                }
            }
            if (doc.duplicate) {
                CPLRT(sbuf_puts)(detail, "duplicate key ");
                CPLRT(write_json_string)(detail, doc.arena + doc.dup_off, doc.dup_len);
            } else {
                CPLRT(sbuf_puts)(detail, doc.reason);
                CPLRT(sbuf_puts)(detail, " at byte ");
                CPLRT(sbuf_u64)(detail, doc.error_at);
            }
            if (pointer->status != CPLRT_OK || detail->status != CPLRT_OK)
                st = CPLRT_NOMEM;
        }
        json_doc_free(&doc);
        return st;
    }
    json_dec d;
    memset(&d, 0, sizeof d);
    d.rt = rt;
    d.p = rt->program;
    d.doc = &doc;
    d.pointer = pointer;
    d.detail = detail;
    d.status = CPLRT_OK;
    int r = json_decode(&d, type);
    int st = d.status;
    if (r >= 0 && st == CPLRT_OK && d.nvalues == 1u) {
        CPLRT_SET(*out, d.values[0]);
        d.nvalues = 0;
    } else if (st == CPLRT_OK) {
        st = CPLRT_NOMEM;
    }
    if (st == CPLRT_INVALID && (pointer->status != CPLRT_OK || detail->status != CPLRT_OK))
        st = CPLRT_NOMEM;
    for (size_t i = 0; i < d.nvalues; i++)
        cplrt_release(d.values[i]);
    CPLRT(free)(d.values);
    CPLRT(free)(d.frames);
    CPLRT(free)(d.path);
    json_doc_free(&doc);
    return st;
}
