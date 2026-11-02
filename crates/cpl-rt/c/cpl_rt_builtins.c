/* cpl_rt_builtins.c: the callable builtins that are neither Text, Bytes, nor List kernels (spec/cabi.md 2.7): the Int
   members, the Option and Result members, and the check<R> failure value; each transcribes the function of the same
   registry id in crates/cpl-eval/src/builtins/prelude.rs. File-scope names begin with builtins_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <string.h>

static int builtins_oom(cplrt_cx *cx, uint32_t site, uint32_t stub)
{
    return CPLRT(builtin_leave)(cx, site, stub, CPLRT(oom)(cx));
}

/* internal_error type_confusion of a builtin argument (builtins::confusion), as call_builtin reports it */
static int builtins_confusion(cplrt_cx *cx, uint32_t site, uint32_t stub, const char *id, const char *index,
                              const char *expected, cplrt_w found)
{
    static const char *const names[4] = { "index", "id", "found", "expected" };
    const char *values[4] = { index, id, CPLRT(kind_name)(found), expected };
    return CPLRT(builtin_leave)(cx, site, stub,
                                CPLRT(raise_internal)(cx, CPLRT_NONE, "type_confusion",
                                                      CPLRT_MSG_DETAIL_TYPE_CONFUSION_BUILTIN, names, values, 4));
}

static int builtins_is_int(cplrt_w w)
{
    return cplrt_is_small(w) || (cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_BIGINT);
}

static int builtins_is_enum_of(cplrt_w w, uint32_t def)
{
    return cplrt_is_obj(w) && cplrt_obj_of(w)->kind == CPLRT_ENUM && cplrt_obj_of(w)->u.live.def == def;
}

/* mirrors cpl_eval::builtins::prelude::int_to_text */
int CPLRT(bi_int_to_text)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    if (!builtins_is_int(a0))
        return builtins_confusion(cx, site, stub, "int_to_text", "0", "an Int", a0);
    return CPLRT(int_text)(cx->rt, a0, out) == CPLRT_OK ? CPLRT_OK : builtins_oom(cx, site, stub);
}

/* mirrors cpl_eval::builtins::prelude::int_abs */
int CPLRT(bi_int_abs)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    if (!builtins_is_int(a0))
        return builtins_confusion(cx, site, stub, "int_abs", "0", "an Int", a0);
    if (!cplrt_int_lt_small(a0, 0)) {
        CPLRT_COPY(*out, a0);
        return CPLRT_OK;
    }
    return CPLRT(builtin_leave)(cx, site, stub, cplrt_int_neg(cx, CPLRT_NO_SITE, a0, out));
}

/* mirrors cpl_eval::builtins::prelude::int_min: b < a ? b : a, so a tie returns a */
int CPLRT(bi_int_min)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    if (!builtins_is_int(a0))
        return builtins_confusion(cx, site, stub, "int_min", "0", "an Int", a0);
    if (!builtins_is_int(a1))
        return builtins_confusion(cx, site, stub, "int_min", "1", "an Int", a1);
    CPLRT_COPY(*out, cplrt_int_cmp(a1, a0) < 0 ? a1 : a0);
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::int_max: b > a ? b : a, so a tie returns a */
int CPLRT(bi_int_max)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    if (!builtins_is_int(a0))
        return builtins_confusion(cx, site, stub, "int_max", "0", "an Int", a0);
    if (!builtins_is_int(a1))
        return builtins_confusion(cx, site, stub, "int_max", "1", "an Int", a1);
    CPLRT_COPY(*out, cplrt_int_cmp(a1, a0) > 0 ? a1 : a0);
    return CPLRT_OK;
}

/* ---- Option (prelude::option_*) --------------------------------------------------------------------------------- */

#define BUILTINS_OPTION(id)                                                                                            \
    if (!builtins_is_enum_of(a0, cx->program->std.option))                                                             \
        return builtins_confusion(cx, site, stub, id, "0", "an Option", a0);

/* mirrors cpl_eval::builtins::prelude::option_is_some */
int CPLRT(bi_option_is_some)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    BUILTINS_OPTION("option_is_some")
    CPLRT_SET(*out, cplrt_bool(cplrt_variant(a0) == CPL_OPTION_SOME));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::option_is_none */
int CPLRT(bi_option_is_none)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    BUILTINS_OPTION("option_is_none")
    CPLRT_SET(*out, cplrt_bool(cplrt_variant(a0) != CPL_OPTION_SOME));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::option_unwrap_or */
int CPLRT(bi_option_unwrap_or)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    BUILTINS_OPTION("option_unwrap_or")
    CPLRT_COPY(*out, cplrt_variant(a0) == CPL_OPTION_SOME ? cplrt_payload(a0, 0) : a1);
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::option_ok_or */
int CPLRT(bi_option_ok_or)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    BUILTINS_OPTION("option_ok_or")
    int st = cplrt_variant(a0) == CPL_OPTION_SOME ? cplrt_result_ok(cx, cplrt_payload(a0, 0), out)
                                                  : cplrt_result_err(cx, a1, out);
    return CPLRT(builtin_leave)(cx, site, stub, st);
}

/* ---- Result (prelude::result_*) --------------------------------------------------------------------------------- */

#define BUILTINS_RESULT(id)                                                                                            \
    if (!builtins_is_enum_of(a0, cx->program->std.result))                                                             \
        return builtins_confusion(cx, site, stub, id, "0", "a Result", a0);

/* mirrors cpl_eval::builtins::prelude::result_is_ok */
int CPLRT(bi_result_is_ok)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    BUILTINS_RESULT("result_is_ok")
    CPLRT_SET(*out, cplrt_bool(cplrt_variant(a0) == CPL_RESULT_OK));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::result_is_err */
int CPLRT(bi_result_is_err)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    BUILTINS_RESULT("result_is_err")
    CPLRT_SET(*out, cplrt_bool(cplrt_variant(a0) != CPL_RESULT_OK));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::result_unwrap_or */
int CPLRT(bi_result_unwrap_or)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w a1, cplrt_w *out)
{
    BUILTINS_RESULT("result_unwrap_or")
    CPLRT_COPY(*out, cplrt_variant(a0) == CPL_RESULT_OK ? cplrt_payload(a0, 0) : a1);
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::result_ok */
int CPLRT(bi_result_ok)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    BUILTINS_RESULT("result_ok")
    if (cplrt_variant(a0) == CPL_RESULT_OK)
        return CPLRT(builtin_leave)(cx, site, stub, cplrt_option_some(cx, cplrt_payload(a0, 0), out));
    CPLRT_SET(*out, CPLRT(enum_static)(cx->program, cx->program->std.option, CPL_OPTION_NONE));
    return CPLRT_OK;
}

/* mirrors cpl_eval::builtins::prelude::result_err */
int CPLRT(bi_result_err)(cplrt_cx *cx, uint32_t site, uint32_t stub, cplrt_w a0, cplrt_w *out)
{
    BUILTINS_RESULT("result_err")
    if (cplrt_variant(a0) != CPL_RESULT_OK)
        return CPLRT(builtin_leave)(cx, site, stub, cplrt_option_some(cx, cplrt_payload(a0, 0), out));
    CPLRT_SET(*out, CPLRT(enum_static)(cx->program, cx->program->std.option, CPL_OPTION_NONE));
    return CPLRT_OK;
}

/* ---- check<R> (semantics 6.10) ----------------------------------------------------------------------------------- */

/* mirrors cpl_eval::builtins::check_failure: Err(ConstraintError { type_name, message: "<value text> does not satisfy
   <predicate text>" }) for the refinement whose predicate was false */
int cplrt_check_failure(cplrt_cx *cx, uint32_t refinement, cplrt_w v, cplrt_w *out)
{
    const cplrt_program *p = cx->program;
    const cplrt_refinement *r = refinement < p->nrefs ? &p->refinements[refinement] : NULL;
    const char *predicate = r != NULL && r->predicate != NULL ? r->predicate : "";
    const char *type_name = r != NULL && r->type_name != NULL ? r->type_name : "";
    cplrt_sbuf vt, m;
    cplrt_sbuf_init(&vt, cx->rt);
    CPLRT(sbuf_put)(&vt, "", 0);
    CPLRT(write_value_text)(&vt, p, v);
    if (vt.status != CPLRT_OK || vt.ptr == NULL) {
        CPLRT(sbuf_free)(&vt);
        return CPLRT(oom)(cx);
    }
    static const char *const names[2] = { "value", "predicate" };
    const char *values[2] = { vt.ptr, predicate };
    cplrt_sbuf_init(&m, cx->rt);
    CPLRT(sbuf_put)(&m, "", 0);
    CPLRT(write_template)(&m, CPLRT_MSG_CHECK_FAILED, names, values, 2);
    CPLRT(sbuf_free)(&vt);
    if (m.status != CPLRT_OK || m.ptr == NULL) {
        CPLRT(sbuf_free)(&m);
        return CPLRT(oom)(cx);
    }
    int st = CPLRT(constraint_err)(cx->rt, type_name, strlen(type_name), m.ptr, m.len, out);
    CPLRT(sbuf_free)(&m);
    return st == CPLRT_OK ? CPLRT_OK : CPLRT(oom)(cx);
}
