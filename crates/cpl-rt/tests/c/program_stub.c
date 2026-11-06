/* program_stub.c: the hand-written minimal program tables of the prefix `rttest` that every C unit test of the runtime
   links (spec/toolchain.md 1.12; crates/cpl-rt/tests/runtime_c.rs): CPLPKG(program) and CPLPKG(code) holding the std
   declarations the runtime constructs (Unit, Option, Result, ConstraintError, ForeignError, ForeignErrorKind,
   ForeignDetail) with their immortal payload-less statics, the four builtin stubs with preconditions and their clauses
   and sites, and a few declarations of the design's reader example and of shapes the tests need.

   It also holds, for the tests only:
   - rttest_runtime_init and rttest_runtime_fini, a runtime object over these tables built without the C API (track R3
     owns <prefix>_runtime_open), with the C library's allocator and an optional heap budget;
   - weak stand-ins for the functions of tracks R2 and R3 that the R1 files and tests call (CPLRT(oom), the raising
     helpers, CPLRT(builtin_leave), CPLRT(cx_begin), CPLRT(cx_end), CPLRT(fault_free), and the capability and token
     counts), so that the runtime links while those files do not exist; a real definition in the runtime overrides
     each at link time. Each stand-in records only what the R1 tests check: the fault kind, clause kind, and resource
     of a static record.

   The DefId.0, CPL_X, CPL_S, CPL_CL, and CPL_R indices of the tables are fixed below (RTTEST_*), for the tests. */
#define _GNU_SOURCE 1
#define CPLRT_NS rttest
#define CPLRT(name) cplrt_rttest__##name
#define CPLPKG(name) cplpkg_rttest__##name
#include "cpl_rt_internal.h"
#include <stdlib.h>
#include <string.h>

/* ---- the immortal statics of payload-less variants ---------------------------------------------------------------- */
#define STATIC_VARIANT(def, variant) { CPLRT_ENUM, 0u, (variant), 0u, { .live = { CPLRT_RC_IMMORTAL, (def) } } }
static cplrt_obj s_unit = STATIC_VARIANT(0u, 0u);
static cplrt_obj s_none = STATIC_VARIANT(1u, 1u);
static cplrt_obj s_fek[7] = { STATIC_VARIANT(5u, 0u), STATIC_VARIANT(5u, 1u), STATIC_VARIANT(5u, 2u),
                              STATIC_VARIANT(5u, 3u), STATIC_VARIANT(5u, 4u), STATIC_VARIANT(5u, 5u),
                              STATIC_VARIANT(5u, 6u) };
static cplrt_obj s_unavailable = STATIC_VARIANT(9u, 1u);
static cplrt_obj *const statics_unit[1] = { &s_unit };
static cplrt_obj *const statics_option[2] = { NULL, &s_none };
static cplrt_obj *const statics_result[2] = { NULL, NULL };
static cplrt_obj *const statics_fek[7] = { &s_fek[0], &s_fek[1], &s_fek[2], &s_fek[3], &s_fek[4], &s_fek[5], &s_fek[6] };
static cplrt_obj *const statics_page_error[2] = { NULL, &s_unavailable };

/* ---- member names and their byte order ------------------------------------------------------------------------------ */
static const char *const m_unit[1] = { "Unit" };
static const uint32_t o_unit[1] = { 0 };
static const char *const m_option[2] = { "Some", "None" };
static const uint32_t o_option[2] = { 1, 0 };
static const char *const m_result[2] = { "Ok", "Err" };
static const uint32_t o_result[2] = { 1, 0 };
static const char *const m_cerr[2] = { "type_name", "message" };
static const uint32_t o_cerr[2] = { 1, 0 };
static const char *const m_ferr[3] = { "kind", "message", "details" };
static const uint32_t o_ferr[3] = { 2, 0, 1 };
static const char *const m_fek[7] = { "Exception", "InvalidArgument", "InvalidResult", "Transport",
                                      "Timeout",   "Cancelled",       "Unavailable" };
static const uint32_t o_fek[7] = { 5, 0, 1, 2, 4, 3, 6 };
static const char *const m_fdetail[2] = { "key", "value" };
static const uint32_t o_fdetail[2] = { 0, 1 };
static const char *const m_page[2] = { "id", "text" };
static const uint32_t o_page[2] = { 0, 1 };
static const char *const m_page_error[2] = { "Missing", "Unavailable" };
static const uint32_t o_page_error[2] = { 0, 1 };
static const char *const m_pair[3] = { "zeta", "alpha", "mid" };
static const uint32_t o_pair[3] = { 1, 2, 0 };
static const char *const m_self_n[2] = { "self", "n" };
static const uint32_t o_self_n[2] = { 1, 0 };
static const char *const m_text_sep[2] = { "text", "sep" };
static const uint32_t o_text_sep[2] = { 1, 0 };
static const char *const m_values[1] = { "values" };
static const uint32_t o_values[1] = { 0 };
static const char *const m_esc[2] = { "a\"b", "c/d~e" };
static const uint32_t o_esc[2] = { 0, 1 };

static const uint32_t r_take[1] = { 0 }, r_drop[1] = { 1 }, r_split[1] = { 2 }, r_from_list[1] = { 3 };

#define DEF(id, q, t, b, kind, n, m, o, st, nr, r) { id, q, t, b, kind, n, m, o, st, NULL, nr, r, NULL, NULL, 0, NULL }
static const cplrt_def defs[19] = {
    /* 0 */ DEF("std.prelude.Unit", "std.prelude.Unit", "Unit", "Unit", CPLRT_DEF_ENUM, 1, m_unit, o_unit, statics_unit,
                0, NULL),
    /* 1 */ DEF("std.prelude.Option", "std.prelude.Option", "Option", "Option", CPLRT_DEF_ENUM, 2, m_option, o_option,
                statics_option, 0, NULL),
    /* 2 */ DEF("std.prelude.Result", "std.prelude.Result", "Result", "Result", CPLRT_DEF_ENUM, 2, m_result, o_result,
                statics_result, 0, NULL),
    /* 3 */ DEF("std.prelude.ConstraintError", "std.prelude.ConstraintError", "ConstraintError", "ConstraintError",
                CPLRT_DEF_STRUCT, 2, m_cerr, o_cerr, NULL, 0, NULL),
    /* 4 */ DEF("std.foreign.ForeignError", "std.foreign.ForeignError", "std.foreign.ForeignError", "ForeignError",
                CPLRT_DEF_STRUCT, 3, m_ferr, o_ferr, NULL, 0, NULL),
    /* 5 */ DEF("std.foreign.ForeignErrorKind", "std.foreign.ForeignErrorKind", "std.foreign.ForeignErrorKind",
                "ForeignErrorKind", CPLRT_DEF_ENUM, 7, m_fek, o_fek, statics_fek, 0, NULL),
    /* 6 */ DEF("std.foreign.ForeignDetail", "std.foreign.ForeignDetail", "std.foreign.ForeignDetail", "ForeignDetail",
                CPLRT_DEF_STRUCT, 2, m_fdetail, o_fdetail, NULL, 0, NULL),
    /* 7 */ DEF("reader.pages.PageId", "reader.pages.PageId", "reader.pages.PageId", "PageId", CPLRT_DEF_WRAPPER, 0,
                NULL, NULL, NULL, 0, NULL),
    /* 8 */ DEF("reader.pages.Page", "reader.pages.Page", "reader.pages.Page", "Page", CPLRT_DEF_STRUCT, 2, m_page,
                o_page, NULL, 0, NULL),
    /* 9 */ DEF("reader.pages.PageError", "reader.pages.PageError", "reader.pages.PageError", "PageError",
                CPLRT_DEF_ENUM, 2, m_page_error, o_page_error, statics_page_error, 0, NULL),
    /* 10 */ DEF("rttest.shapes.Pair", "rttest.shapes.Pair", "rttest.shapes.Pair", "Pair", CPLRT_DEF_STRUCT, 3, m_pair,
                 o_pair, NULL, 0, NULL),
    /* 11 */ DEF("rttest.shapes.Empty", "rttest.shapes.Empty", "rttest.shapes.Empty", "Empty", CPLRT_DEF_STRUCT, 0,
                 NULL, NULL, NULL, 0, NULL),
    /* 12 */ DEF("std.prelude.List.take", "std.prelude.List.take", "std.prelude.List.take", "take", CPLRT_DEF_FN, 2,
                 m_self_n, o_self_n, NULL, 1, r_take),
    /* 13 */ DEF("std.prelude.List.drop", "std.prelude.List.drop", "std.prelude.List.drop", "drop", CPLRT_DEF_FN, 2,
                 m_self_n, o_self_n, NULL, 1, r_drop),
    /* 14 */ DEF("std.text.split", "std.text.split", "std.text.split", "split", CPLRT_DEF_FN, 2, m_text_sep,
                 o_text_sep, NULL, 1, r_split),
    /* 15 */ DEF("std.bytes.from_list", "std.bytes.from_list", "std.bytes.from_list", "from_list", CPLRT_DEF_FN, 1,
                 m_values, o_values, NULL, 1, r_from_list),
    /* 16 */ DEF("rttest.main.f", "rttest.main.f", "rttest.main.f", "f", CPLRT_DEF_FN, 0, NULL, NULL, NULL, 0, NULL),
    /* 17 */ DEF("reader.pages.Positive", "reader.pages.Positive", "reader.pages.Positive", "Positive",
                 CPLRT_DEF_REFINEMENT, 0, NULL, NULL, NULL, 0, NULL),
    /* 18 */ DEF("rttest.shapes.Esc", "rttest.shapes.Esc", "rttest.shapes.E\"sc", "E\"sc", CPLRT_DEF_STRUCT, 2, m_esc,
                 o_esc, NULL, 0, NULL),
};

static const char *const files[4] = { "std/prelude.cpl", "std/text.cpl", "std/bytes.cpl", "src/rttest/main.cpl" };
/* the clause spans of the stubs as the evaluator reports them (`requires ...` from its keyword; std/ of 2026-10-29) */
static const cplrt_site sites[6] = {
    { 0, 4491, 4506, 138, 5, 138, 20 }, /* 0: List.take's requires n >= 0 */
    { 0, 4791, 4806, 146, 5, 146, 20 }, /* 1: List.drop's requires n >= 0 */
    { 1, 1191, 1218, 38, 5, 38, 32 },   /* 2: split's requires not sep.is_empty() */
    { 2, 151, 200, 7, 5, 7, 54 },       /* 3: from_list's requires forall ... */
    { 3, 10, 20, 2, 5, 2, 15 },          /* 4: a call in rttest.main.f */
    { 7, 0, 1, 1, 1, 1, 2 },             /* 5: a site whose file the program does not know */
};
static const cplrt_clause clauses[5] = {
    { "n >= 0", CPLRT_CK_REQUIRES, 0, 12, CPLRT_NONE, "std.prelude.List.take", "std.prelude.List.take", CPLRT_NONE },
    { "n >= 0", CPLRT_CK_REQUIRES, 1, 13, CPLRT_NONE, "std.prelude.List.drop", "std.prelude.List.drop", CPLRT_NONE },
    { "not sep.is_empty()", CPLRT_CK_REQUIRES, 2, 14, CPLRT_NONE, "std.text.split", "std.text.split", CPLRT_NONE },
    { "forall v in values: v >= 0 and v <= 255", CPLRT_CK_REQUIRES, 3, 15, CPLRT_NONE, "std.bytes.from_list",
      "std.bytes.from_list", CPLRT_NONE },
    { "self > 0", CPLRT_CK_WHERE, 4, 17, CPLRT_NONE, "reader.pages.Positive", "reader.pages.Positive", CPLRT_NONE },
};
static const uint32_t chain_positive[1] = { 0 };
static const cplrt_refinement refinements[1] = {
    { 17, "reader.pages.Positive", "Positive", "self > 0", 4, 1, 1, chain_positive },
};

/* ---- the type table: [1..11] the well-known types of cpl.h, then the tests' own -------------------------------- */
static const uint32_t t_payload_one_param[3] = { 0, 1, 1 };   /* Option<T>: Some(T), None */
static const uint32_t t_option_param_types[1] = { 21 };
static const uint32_t t_result_offsets[3] = { 0, 1, 2 };      /* Result<T, E>: Ok(T), Err(E) */
static const uint32_t t_result_param_types[2] = { 21, 21 };
static const uint32_t t_cerr_fields[2] = { 3, 3 };
static const uint32_t t_ferr_fields[3] = { 10, 3, 24 };
static const uint32_t t_fek_offsets[8] = { 0, 0, 0, 0, 0, 0, 0, 0 };
static const uint32_t t_fdetail_fields[2] = { 3, 3 };
static const uint32_t t_page_fields[2] = { 12, 3 };
static const uint32_t t_page_error_offsets[3] = { 0, 1, 1 };
static const uint32_t t_page_error_types[1] = { 12 };
static const uint32_t t_option_int_types[1] = { 1 };
static const uint32_t t_pair_fields[3] = { 1, 3, 2 };
static const uint32_t t_unit_offsets[2] = { 0, 0 };
static const uint32_t t_result_int_text_types[2] = { 1, 3 };
static const uint32_t t_esc_fields[2] = { 1, 1 };
static const uint32_t t_option_positive_types[1] = { 16 };

#define TY(text, display, form, def, elem, n, members, payloads) { text, display, form, def, elem, n, members, payloads }
static const cplrt_type types[30] = {
    /* 0 */ TY("", "", 0, 0, 0, 0, NULL, NULL),
    /* 1 */ TY("Int", "Int", CPLRT_TY_INT, 0, 0, 0, NULL, NULL),
    /* 2 */ TY("Bool", "Bool", CPLRT_TY_BOOL, 0, 0, 0, NULL, NULL),
    /* 3 */ TY("Text", "Text", CPLRT_TY_TEXT, 0, 0, 0, NULL, NULL),
    /* 4 */ TY("Bytes", "Bytes", CPLRT_TY_BYTES, 0, 0, 0, NULL, NULL),
    /* 5 */ TY("Unit", "Unit", CPLRT_TY_ENUM, 0, 0, 1, t_unit_offsets, NULL),
    /* 6 */ TY("Option<T>", "Option<T>", CPLRT_TY_ENUM, 1, 0, 2, t_payload_one_param, t_option_param_types),
    /* 7 */ TY("Result<T, E>", "Result<T, E>", CPLRT_TY_ENUM, 2, 0, 2, t_result_offsets, t_result_param_types),
    /* 8 */ TY("ConstraintError", "ConstraintError", CPLRT_TY_STRUCT, 3, 0, 2, t_cerr_fields, NULL),
    /* 9 */ TY("std.foreign.ForeignError", "ForeignError", CPLRT_TY_STRUCT, 4, 0, 3, t_ferr_fields, NULL),
    /* 10 */ TY("std.foreign.ForeignErrorKind", "ForeignErrorKind", CPLRT_TY_ENUM, 5, 0, 7, t_fek_offsets, NULL),
    /* 11 */ TY("std.foreign.ForeignDetail", "ForeignDetail", CPLRT_TY_STRUCT, 6, 0, 2, t_fdetail_fields, NULL),
    /* 12 */ TY("reader.pages.PageId", "PageId", CPLRT_TY_WRAPPER, 7, 3, 0, NULL, NULL),
    /* 13 */ TY("reader.pages.Page", "Page", CPLRT_TY_STRUCT, 8, 0, 2, t_page_fields, NULL),
    /* 14 */ TY("reader.pages.PageError", "PageError", CPLRT_TY_ENUM, 9, 0, 2, t_page_error_offsets,
                t_page_error_types),
    /* 15 */ TY("List<Int>", "List<Int>", CPLRT_TY_LIST, 0, 1, 0, NULL, NULL),
    /* 16 */ TY("reader.pages.Positive", "Positive", CPLRT_TY_REFINED, 0, 1, 0, NULL, NULL),
    /* 17 */ TY("Option<Int>", "Option<Int>", CPLRT_TY_ENUM, 1, 0, 2, t_payload_one_param, t_option_int_types),
    /* 18 */ TY("List<reader.pages.Page>", "List<Page>", CPLRT_TY_LIST, 0, 13, 0, NULL, NULL),
    /* 19 */ TY("rttest.shapes.Pair", "Pair", CPLRT_TY_STRUCT, 10, 0, 3, t_pair_fields, NULL),
    /* 20 */ TY("rttest.shapes.Empty", "Empty", CPLRT_TY_STRUCT, 11, 0, 0, NULL, NULL),
    /* 21 */ TY("T", "T", CPLRT_TY_PARAM, 0, 0, 0, NULL, NULL),
    /* 22 */ TY("reader.pages.PageRead", "PageRead", CPLRT_TY_CAPABILITY, 0, 0, 0, NULL, NULL),
    /* 23 */ TY("List<List<Int>>", "List<List<Int>>", CPLRT_TY_LIST, 0, 15, 0, NULL, NULL),
    /* 24 */ TY("List<std.foreign.ForeignDetail>", "List<ForeignDetail>", CPLRT_TY_LIST, 0, 11, 0, NULL, NULL),
    /* 25 */ TY("Result<Int, Text>", "Result<Int, Text>", CPLRT_TY_ENUM, 2, 0, 2, t_result_offsets,
                t_result_int_text_types),
    /* 26 */ TY("rttest.shapes.Esc", "Esc", CPLRT_TY_STRUCT, 18, 0, 2, t_esc_fields, NULL),
    /* 27 */ TY("Option<reader.pages.Positive>", "Option<Positive>", CPLRT_TY_ENUM, 1, 0, 2, t_payload_one_param,
                t_option_positive_types),
    /* 28 */ TY("reader.pages.Broken", "Broken", CPLRT_TY_REFINED, 0, 0, 0, NULL, NULL),
    /* 29: a list of itself, no CPL type: it lets the tests decode arbitrarily deep arrays */
    /* 29 */ TY("rttest.shapes.Nest", "Nest", CPLRT_TY_LIST, 0, 29, 0, NULL, NULL),
};

static const cplrt_fn_entry no_fns[19];
static const cplrt_pred_entry no_preds[19];
const cplrt_code CPLPKG(code) = { 19, no_fns, no_preds, NULL, 0, NULL };

const cplrt_program CPLPKG(program) = {
    { sizeof(cpl_library_info), CPL_API_VERSION, 1, 0, 0, 0, "rttest", "rttest", "0.1", "0.1.0", "0.1.0",
      "0000000000000000000000000000000000000000000000000000000000000000",
      "0000000000000000000000000000000000000000000000000000000000000000",
      "0000000000000000000000000000000000000000000000000000000000000000", "x86_64-linux-gnu", "gcc 11.4.0",
      "debug" },
    &CPLPKG(code),
    19, defs,
    4, files,
    6, sites,
    5, clauses,
    0, NULL,
    0, NULL,
    1, refinements,
    30, types,
    0, NULL,
    0, NULL,
    { 0, 1, 2, 3, 4, 5, 6 },
    10000,
    NULL, 0, NULL,
};

/* ---- a runtime object for the tests ---------------------------------------------------------------------------- */
void rttest_runtime_init(cplrt_runtime *rt, uint64_t max_heap_bytes);
uint64_t rttest_runtime_fini(cplrt_runtime *rt);

void rttest_runtime_init(cplrt_runtime *rt, uint64_t max_heap_bytes)
{
    memset(rt, 0, sizeof *rt);
    rt->program = &CPLPKG(program);
    rt->identity = malloc(16);
    rt->max_heap_bytes = max_heap_bytes;
    atomic_init(&rt->live_bytes, 0u);
    atomic_init(&rt->peak_bytes, 0u);
    atomic_init(&rt->live_objects, 0u);
    atomic_init(&rt->active_calls, 0u);
    atomic_init(&rt->poisoned, 0u);
    rt->max_call_depth = 10000u;
    pthread_mutex_init(&rt->lock, NULL);
    rt->free_head = CPLRT_NONE;
    rt->free_tail = CPLRT_NONE;
}

/* Releases what the handle table still holds, frees the table, and returns the live objects left (0 when the test
   released everything it made). */
uint64_t rttest_runtime_fini(cplrt_runtime *rt)
{
    for (uint32_t i = 0; i < rt->nslots; i++)
        if (rt->slots[i].kind == CPLRT_SLOT_VALUE)
            cplrt_release(rt->slots[i].u.value);
    CPLRT(free)(rt->slots);
    rt->slots = NULL;
    pthread_mutex_destroy(&rt->lock);
    free(rt->identity);
    return atomic_load(&rt->live_objects);
}

/* ---- weak stand-ins for tracks R2 and R3 (overridden by the runtime's own definitions) ------------------------- */
#define WEAK __attribute__((weak))

static cplrt_fault standin_record(uint32_t kind, uint32_t clause_kind, uint32_t resource)
{
    cplrt_fault f;
    memset(&f, 0, sizeof f);
    f.kind = kind;
    f.clause_kind = clause_kind;
    f.resource = resource;
    f.span = CPLRT_NONE;
    f.flags = CPLRT_FAULT_STATIC;
    f.owner = CPLRT_NONE;
    f.message = "";
    return f;
}
static _Thread_local cplrt_fault standin_fault;

WEAK int CPLRT(cx_begin)(cplrt_cx *cx, cplrt_runtime *rt, cplrt_memory_record *memory, uint32_t max_call_depth,
                         int check_contracts, uint64_t budget_ms, uint64_t stack_bytes, const _Atomic uint32_t *cancel)
{
    memset(cx, 0, sizeof *cx);
    cx->rt = rt;
    cx->program = rt->program;
    cx->max_call_depth = max_call_depth == UINT32_MAX ? rt->max_call_depth : max_call_depth;
    cx->check_contracts = check_contracts;
    cx->cancel = cancel;
    memset(memory, 0, sizeof *memory);
    memory->fault = standin_record(CPLRT_FK_RESOURCE_EXHAUSTION, 0, CPLRT_RES_MEMORY);
    memory->fault.data = memory->data;
    memory->fault.frames = memory->frames;
    memory->fault.frames_cap = CPLRT_MEMORY_FRAMES;
    memory->fault.data_cap = 2;
    cx->memory = &memory->fault;
    return CPLRT_OK;
}

WEAK void CPLRT(cx_end)(cplrt_cx *cx)
{
    cx->fault = NULL;
    cx->violation = NULL;
}

WEAK void CPLRT(fault_free)(cplrt_fault *f)
{
    (void)f;
}

WEAK int CPLRT(oom)(cplrt_cx *cx)
{
    cx->fault = cx->memory;
    return CPLRT_FAULT;
}

WEAK int CPLRT(raise_division_by_zero)(cplrt_cx *cx, uint32_t site, cplrt_w a, cplrt_w b)
{
    standin_fault = standin_record(CPLRT_FK_DIVISION_BY_ZERO, 0, 0);
    cx->fault = &standin_fault;
    return CPLRT_FAULT;
}

WEAK int CPLRT(raise_builtin_requires)(cplrt_cx *cx, uint32_t stub, const char *id, uint32_t clause,
                                       const cplrt_w *args, uint32_t n)
{
    standin_fault = standin_record(CPLRT_FK_CONTRACT_VIOLATION, CPLRT_CK_REQUIRES, 0);
    cx->fault = &standin_fault;
    return CPLRT_FAULT;
}

WEAK int CPLRT(raise_internal)(cplrt_cx *cx, uint32_t site, const char *kind, const char *tmpl,
                               const char *const *names, const char *const *values, uint32_t n)
{
    standin_fault = standin_record(CPLRT_FK_INTERNAL_ERROR, 0, 0);
    cx->fault = &standin_fault;
    return CPLRT_FAULT;
}

WEAK int CPLRT(builtin_leave)(cplrt_cx *cx, uint32_t site, uint32_t stub, int st)
{
    return st;
}

WEAK void CPLRT(cap_retain)(cplrt_capobj *cap)
{
    (void)cap;
}

WEAK void CPLRT(cap_unref)(cplrt_capobj *cap)
{
    (void)cap;
}

WEAK void CPLRT(token_retain)(cplrt_token *token)
{
    (void)token;
}

WEAK void CPLRT(token_unref)(cplrt_token *token)
{
    (void)token;
}
