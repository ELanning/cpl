/* cpl_rt_eval.c: evaluation contexts, frames, the depth counter, the stack guard, the time budget, and cancellation
   (spec/cabi.md 1.2, 3.4, 3.6, 4.3), at exactly the points where the evaluator checks them: call_fn (the depth),
   enter_activation (the guard, then the time budget, then the frame push), check_time before every loop condition
   (crates/cpl-eval/src/interp.rs, crates/cpl-eval/src/interp/eval.rs). File-scope names begin with eval_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include "cpl_rt_platform.h"
#include <string.h>

/* The stack floors of the evaluations of this runtime copy active on this thread, outermost first (3.4: a re-entrant
   evaluation never starts below the floor of the one it re-enters). Floors only rise along the stack, so beyond the
   recorded depth the deepest recorded floor stays a valid lower bound. */
#define EVAL_TL_DEPTH 64u
typedef struct eval_thread_state {
    uint32_t n;
    uintptr_t floors[EVAL_TL_DEPTH];
} eval_thread_state;
static _Thread_local eval_thread_state eval_tl;

static uintptr_t eval_margin(uintptr_t size)
{
    uintptr_t eighth = size / 8u;
    return eighth < (uintptr_t)CPLRT_LIMIT_GUARD_MARGIN ? eighth : (uintptr_t)CPLRT_LIMIT_GUARD_MARGIN;
}

/* mirrors cpl_eval::interp::Interpreter::begin_evaluation and cpl_eval::stack::set_guard (spec/cabi.md 3.4, 3.6, 4.3) */
int CPLRT(cx_begin)(cplrt_cx *cx, cplrt_runtime *rt, cplrt_memory_record *memory, uint32_t max_call_depth,
                    int check_contracts, uint64_t budget_ms, uint64_t stack_bytes, const _Atomic uint32_t *cancel)
{
    memset(cx, 0, sizeof *cx);
    cx->rt = rt;
    cx->program = rt->program;
    cx->top = NULL;
    cx->calls = 0;
    cx->max_call_depth = max_call_depth == UINT32_MAX ? rt->max_call_depth : max_call_depth;
    cx->check_contracts = check_contracts != 0;
    cx->attributed = 0;
    cx->cancelled = 0;
    cx->cancel = cancel;
    cx->fault = NULL;
    cx->violation = NULL;
    cx->range_text = NULL;

    /* the stack floor */
    uintptr_t floor = 0;
    if (stack_bytes != 0) {
        uintptr_t here = CPLRT_FRAME_ADDRESS();
        uintptr_t size = stack_bytes > (uint64_t)UINTPTR_MAX ? UINTPTR_MAX : (uintptr_t)stack_bytes;
        uintptr_t end = here > size ? here - size : 0u; /* saturating */
        uintptr_t margin = eval_margin(size);
        floor = end > UINTPTR_MAX - margin ? UINTPTR_MAX : end + margin;
    } else {
        uintptr_t lo = 0, hi = 0;
        if (CPLRT(stack_bounds)(&lo, &hi) != CPLRT_OK)
            return CPLRT_NOMEM;
        floor = lo + eval_margin(hi - lo);
    }
    uint32_t depth = eval_tl.n;
    if (depth > 0) {
        uintptr_t outer = eval_tl.floors[(depth < EVAL_TL_DEPTH ? depth : EVAL_TL_DEPTH) - 1u];
        if (outer > floor)
            floor = outer;
    }
    if (depth < EVAL_TL_DEPTH)
        eval_tl.floors[depth] = floor;
    eval_tl.n = depth + 1u;
    cx->stack_floor = floor;

    /* the deadline: a budget whose deadline would overflow is no budget (3.4) */
    cx->budget_ms = budget_ms;
    cx->deadline_ns = 0;
    if (budget_ms != 0 && budget_ms <= UINT64_MAX / UINT64_C(1000000)) {
        uint64_t now = CPLRT(clock_ns)();
        uint64_t span = budget_ms * UINT64_C(1000000);
        if (now <= UINT64_MAX - span)
            cx->deadline_ns = now + span;
    }

    /* the preallocated memory record (3.3.1) */
    memset(memory, 0, sizeof *memory);
    memory->fault.kind = CPLRT_FK_RESOURCE_EXHAUSTION;
    memory->fault.resource = CPLRT_RES_MEMORY;
    memory->fault.span = CPLRT_NONE;
    memory->fault.owner = CPLRT_NONE;
    memory->fault.flags = CPLRT_FAULT_STATIC;
    memory->fault.message = CPLRT_MSG_RESOURCE_MEMORY;
    memory->fault.message_len = sizeof(CPLRT_MSG_RESOURCE_MEMORY) - 1u;
    memory->fault.data = memory->data;
    memory->fault.data_cap = 2;
    memory->fault.frames = memory->frames;
    memory->fault.frames_cap = CPLRT_MEMORY_FRAMES;
    memory->data[0].key = "resource";
    memory->data[0].form = CPLRT_DATUM_STRING;
    memory->data[0].str = "memory";
    memory->data[1].key = "limit";
    memory->data[1].form = CPLRT_DATUM_NUMBER;
    memory->data[1].num = rt->max_heap_bytes;
    memory->fault.ndata = 2;
    cx->memory = &memory->fault;
    return CPLRT_OK;
}

void CPLRT(cx_end)(cplrt_cx *cx)
{
    if (cx->fault != NULL && cx->fault != cx->memory)
        CPLRT(fault_free)(cx->fault);
    cx->fault = NULL;
    CPLRT(fault_free)(cx->violation);
    cx->violation = NULL;
    /* frames left by an aborted evaluation hold no references but their measures */
    for (cplrt_frame *fr = cx->top; fr != NULL; fr = fr->prev) {
        cplrt_release(fr->measure);
        fr->measure = 0;
    }
    cx->top = NULL;
    cx->calls = 0;
    if (eval_tl.n > 0)
        eval_tl.n--;
}

static const cplrt_def *eval_current(const cplrt_cx *cx)
{
    if (cx->top == NULL || cx->top->def >= cx->program->ndefs)
        return NULL;
    return &cx->program->defs[cx->top->def];
}

/* A resource_exhaustion raised from the call or the loop condition at `site` (Interpreter::raise): blamed on the
   innermost active declaration (none at the top level, semantics S72). */
static int eval_exhausted(cplrt_cx *cx, uint32_t site, uint32_t resource, const char *tmpl, uint64_t limit,
                          int with_depth)
{
    char limit_text[24], depth_text[24];
    uint64_t v = limit;
    int k = 0;
    char digits[24];
    do {
        digits[k++] = (char)('0' + (int)(v % 10u));
        v /= 10u;
    } while (v != 0);
    int j = 0;
    while (k > 0)
        limit_text[j++] = digits[--k];
    limit_text[j] = '\0';
    v = cx->calls;
    k = 0;
    do {
        digits[k++] = (char)('0' + (int)(v % 10u));
        v /= 10u;
    } while (v != 0);
    j = 0;
    while (k > 0)
        depth_text[j++] = digits[--k];
    depth_text[j] = '\0';
    static const char *const names[2] = { "limit", "depth" };
    const char *values[2] = { limit_text, depth_text };
    cplrt_sbuf m;
    cplrt_sbuf_init(&m, cx->rt);
    CPLRT(sbuf_put)(&m, "", 0);
    CPLRT(write_template)(&m, tmpl, names, values, 2);
    if (m.status != CPLRT_OK || m.ptr == NULL) {
        CPLRT(sbuf_free)(&m);
        return CPLRT(oom)(cx);
    }
    const cplrt_def *d = eval_current(cx);
    cplrt_fault *f = CPLRT(fault_raise)(cx, CPLRT_FK_RESOURCE_EXHAUSTION, 0, resource, m.ptr, m.len, site,
                                        d != NULL ? d->stable_id : NULL, d != NULL ? d->qualified_name : NULL);
    CPLRT(sbuf_free)(&m);
    if (f == NULL)
        return CPLRT_FAULT;
    const char *rname = resource == CPLRT_RES_STACK ? "stack" : resource == CPLRT_RES_TIME ? "time" : "memory";
    if (CPLRT(fault_set_str)(cx, f, "resource", rname) != CPLRT_OK ||
        CPLRT(fault_set_num)(cx, f, "limit", limit) != CPLRT_OK)
        return CPLRT_FAULT;
    if (with_depth)
        CPLRT(fault_set_num)(cx, f, "depth", cx->calls);
    return CPLRT_FAULT;
}

/* The time budget and cancellation (3.6): one predictable branch without either. */
static int eval_time(cplrt_cx *cx, uint32_t site)
{
    if (cx->parallel_failure != NULL && cx->parallel_index > atomic_load_explicit(cx->parallel_failure, memory_order_acquire)) {
        cx->cancelled = 1;
        return CPLRT_FAULT;
    }
    if (CPLRT_UNLIKELY(cx->cancel != NULL) &&
        atomic_load_explicit((_Atomic uint32_t *)(uintptr_t)cx->cancel, memory_order_relaxed) != 0u) {
        cx->cancelled = 1;
        return CPLRT_FAULT;
    }
    if (CPLRT_UNLIKELY(cx->deadline_ns != 0) && CPLRT(clock_ns)() > cx->deadline_ns)
        return eval_exhausted(cx, site, CPLRT_RES_TIME, CPLRT_MSG_RESOURCE_TIME, cx->budget_ms, 0);
    return CPLRT_OK;
}

/* mirrors cpl_eval::interp::Interpreter::enter_activation: the stack guard, then the time budget and cancellation */
static int eval_enter(cplrt_cx *cx, uint32_t site)
{
    if (CPLRT_UNLIKELY(CPLRT_FRAME_ADDRESS() < cx->stack_floor))
        return eval_exhausted(cx, site, CPLRT_RES_STACK, CPLRT_MSG_RESOURCE_STACK_GUARD, cx->max_call_depth, 1);
    return eval_time(cx, site);
}

/* mirrors cpl_eval::interp::Interpreter::call_fn: the depth limit (refused at the call, blamed on the caller), then
   enter_activation, then the frame with the parameters and the call count */
int cplrt_enter_fn(cplrt_cx *cx, cplrt_frame *fr, uint32_t def, uint32_t site, const cplrt_w *params, uint32_t n)
{
    if (CPLRT_UNLIKELY(cx->calls >= cx->max_call_depth))
        return eval_exhausted(cx, site, CPLRT_RES_STACK, CPLRT_MSG_RESOURCE_STACK, cx->max_call_depth, 0);
    if (eval_enter(cx, site) != CPLRT_OK)
        return CPLRT_FAULT;
    fr->prev = cx->top;
    fr->def = def;
    fr->site = site;
    fr->params = params;
    fr->nparams = n;
    fr->call = 1;
    fr->measure = 0;
    cx->top = fr;
    cx->calls++;
    return CPLRT_OK;
}

/* a predicate, a test body, a range bound: an activation, not a call (semantics S72) */
int cplrt_enter_act(cplrt_cx *cx, cplrt_frame *fr, uint32_t def, uint32_t site)
{
    if (eval_enter(cx, site) != CPLRT_OK)
        return CPLRT_FAULT;
    fr->prev = cx->top;
    fr->def = def;
    fr->site = site;
    fr->params = NULL;
    fr->nparams = 0;
    fr->call = 0;
    fr->measure = 0;
    cx->top = fr;
    return CPLRT_OK;
}

/* mirrors cpl_eval::interp::Interpreter::leave_activation and the free function `leaving` */
int cplrt_leave(cplrt_cx *cx, cplrt_frame *fr, int st)
{
    cx->top = fr->prev;
    if (fr->call)
        cx->calls--;
    cplrt_release(fr->measure);
    fr->measure = 0;
    if (st != CPLRT_OK && !cx->cancelled && cx->fault != NULL)
        CPLRT(fault_push_frame)(cx, cx->fault, fr->def, fr->site);
    return st == CPLRT_OK ? CPLRT_OK : CPLRT_FAULT;
}

/* mirrors the check_time before every loop condition in cpl_eval::interp::Interpreter::exec_while */
int cplrt_tick(cplrt_cx *cx, uint32_t site)
{
    return eval_time(cx, site);
}

/* ---- coverage (1.4, 3.8) ---------------------------------------------------------------------------------------- */

const char *const CPLRT(cov_claim_names)[CPLRT_COV_CLAIMS] = {
    "requires", "ensures", "header_decreases", "invariant", "loop_decreases", "where", "op_requires", "op_ensures",
    "range", "already_claimed"
};

#if defined(CPLRT_COVERAGE) && CPLRT_COVERAGE
#define COV_BUILTINS 512u
static struct {
    const char *id;
    uint64_t n;
} cov_bi[COV_BUILTINS];
static uint32_t cov_nbi_;
static uint64_t cov_claims[CPLRT_COV_CLAIMS];
static pthread_mutex_t cov_lock = PTHREAD_MUTEX_INITIALIZER;

void CPLRT(cov_bi)(const char *id)
{
    pthread_mutex_lock(&cov_lock);
    uint32_t i = 0;
    while (i < cov_nbi_ && strcmp(cov_bi[i].id, id) != 0)
        i++;
    if (i == cov_nbi_ && cov_nbi_ < COV_BUILTINS) {
        cov_bi[i].id = id;
        cov_bi[i].n = 0;
        cov_nbi_++;
    }
    if (i < cov_nbi_)
        cov_bi[i].n++;
    pthread_mutex_unlock(&cov_lock);
}

void CPLRT(cov_claim)(uint32_t index)
{
    if (index >= CPLRT_COV_CLAIMS)
        return;
    pthread_mutex_lock(&cov_lock);
    cov_claims[index]++;
    pthread_mutex_unlock(&cov_lock);
}

uint32_t CPLRT(cov_nbi)(void)
{
    pthread_mutex_lock(&cov_lock);
    uint32_t n = cov_nbi_;
    pthread_mutex_unlock(&cov_lock);
    return n;
}

const char *CPLRT(cov_bi_at)(uint32_t i, uint64_t *count)
{
    pthread_mutex_lock(&cov_lock);
    const char *id = i < cov_nbi_ ? cov_bi[i].id : NULL;
    *count = i < cov_nbi_ ? cov_bi[i].n : 0u;
    pthread_mutex_unlock(&cov_lock);
    return id;
}

uint64_t CPLRT(cov_claims_at)(uint32_t i)
{
    pthread_mutex_lock(&cov_lock);
    uint64_t n = i < CPLRT_COV_CLAIMS ? cov_claims[i] : 0u;
    pthread_mutex_unlock(&cov_lock);
    return n;
}
#else
void CPLRT(cov_claim)(uint32_t index)
{
    (void)index;
}

uint32_t CPLRT(cov_nbi)(void)
{
    return 0;
}

const char *CPLRT(cov_bi_at)(uint32_t i, uint64_t *count)
{
    (void)i;
    *count = 0;
    return NULL;
}

uint64_t CPLRT(cov_claims_at)(uint32_t i)
{
    (void)i;
    return 0;
}
#endif

int CPLRT(task_tick)(cplrt_cx *cx, uint32_t site) { return eval_enter(cx, site); }
