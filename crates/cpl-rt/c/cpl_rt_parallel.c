/* Structured, bounded parallel maps. Workers borrow immutable inputs and own disjoint output slots.
   All spawned tasks finish before the caller releases their captures. */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <pthread.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>

static pthread_once_t parallel_once = PTHREAD_ONCE_INIT;
static uint32_t parallel_limit;
static _Atomic uint32_t parallel_active;

static void parallel_init(void)
{
    long cpus = sysconf(_SC_NPROCESSORS_ONLN);
    parallel_limit = cpus > 0 ? (cpus > 8 ? 8u : (uint32_t)cpus) : 1u;
    const char *text = getenv("CPL_PARALLELISM");
    if (text != NULL && *text >= '0' && *text <= '9') {
        char *end = NULL;
        unsigned long n = strtoul(text, &end, 10);
        if (*end == '\0' && n >= 1u && n <= 64u)
            parallel_limit = (uint32_t)n;
    }
}

static uint32_t parallel_acquire(uint32_t wanted)
{
    pthread_once(&parallel_once, parallel_init);
    uint32_t current = atomic_load_explicit(&parallel_active, memory_order_relaxed);
    for (;;) {
        uint32_t available = current < parallel_limit - 1u ? parallel_limit - 1u - current : 0u;
        uint32_t n = wanted < available ? wanted : available;
        if (atomic_compare_exchange_weak_explicit(&parallel_active, &current, current + n,
                                                  memory_order_acq_rel, memory_order_relaxed))
            return n;
    }
}

typedef struct parallel_job {
    const cplrt_cx *parent;
    cplrt_w input;
    cplrt_node *result;
    cplrt_parallel_task task;
    const cplrt_w *captures;
    uint32_t start, end, site;
    _Atomic uint32_t *failure;
    pthread_t thread;
    int started, failed, oom, attributed, cancelled;
    cplrt_fault *fault, *violation;
} parallel_job;

static void *parallel_run(void *opaque)
{
    parallel_job *job = (parallel_job *)opaque;
    const cplrt_cx *parent = job->parent;
    cplrt_cx cx;
    cplrt_memory_record memory;
    if (CPLRT(cx_begin)(&cx, parent->rt, &memory, parent->max_call_depth,
                        parent->check_contracts, 0, 0, parent->cancel) != CPLRT_OK) {
        uint32_t before = atomic_load_explicit(job->failure, memory_order_relaxed);
        while (job->start < before && !atomic_compare_exchange_weak_explicit(job->failure, &before, job->start, memory_order_acq_rel, memory_order_relaxed)) { }
        job->failed = job->oom = 1;
        return NULL;
    }
    cx.deadline_ns = parent->deadline_ns;
    cx.budget_ms = parent->budget_ms;
    cx.calls = parent->calls;
    cx.parallel_depth = parent->parallel_depth + 1u;
    /* Parent frames stay alive and unchanged until all workers join. Header decreases may inspect them. */
    cx.top = parent->top;
    cx.parallel_failure = job->failure;
    for (uint32_t i = job->start; i < job->end; i++) {
        cx.parallel_index = i;
        if (CPLRT(task_tick)(&cx, job->site) != CPLRT_OK ||
            job->task(&cx, cplrt_item(job->input, i), job->captures, &job->result->item[i]) != CPLRT_OK) {
            uint32_t before = atomic_load_explicit(job->failure, memory_order_relaxed);
            while (i < before && !atomic_compare_exchange_weak_explicit(job->failure, &before, i, memory_order_acq_rel, memory_order_relaxed)) { }
            job->failed = 1;
            job->attributed = cx.attributed;
            job->cancelled = cx.cancelled;
            if (cx.fault == cx.memory)
                job->oom = 1;
            else {
                job->fault = cx.fault;
                cx.fault = NULL;
            }
            job->violation = cx.violation;
            cx.violation = NULL;
            break;
        }
    }
    /* cx_end must never release a measure owned by the waiting parent. */
    cx.top = NULL;
    CPLRT(cx_end)(&cx);
    return NULL;
}

/* mirrors cpl_eval::interp::Interpreter::eval_parallel */
int cplrt_parallel_map(cplrt_cx *cx, uint32_t site, cplrt_w input, cplrt_parallel_task task,
                       const cplrt_w *captures, uint32_t ncaptures, cplrt_w *out)
{
    uint32_t n = cplrt_len(input);
    if (n == 0) {
        CPLRT_SET(*out, CPLRT(empty_list)());
        return CPLRT_OK;
    }
    cplrt_node *result = CPLRT(list_alloc)(cx->rt, n);
    if (result == NULL)
        return CPLRT(oom)(cx);
    uint32_t permits = parallel_acquire(cx->parallel_depth == 0 ? n - 1u : 0u);
    if (permits == 0) {
        int st = CPLRT_OK;
        cx->parallel_depth++;
        for (uint32_t i = 0; i < n; i++) {
            if (CPLRT(task_tick)(cx, site) != CPLRT_OK || task(cx, cplrt_item(input, i), captures, &result->item[i]) != CPLRT_OK) {
                st = CPLRT_FAULT;
                break;
            }
        }
        cx->parallel_depth--;
        if (st == CPLRT_OK)
            CPLRT_SET(*out, cplrt_word_of(&result->hdr));
        else
            cplrt_release(cplrt_word_of(&result->hdr));
        return st;
    }
    /* every task reads the input and the captures, on threads of their own: their counts become atomic */
    int shared = CPLRT(value_share)(cx->rt, input);
    for (uint32_t c = 0; c < ncaptures && shared == CPLRT_OK; c++)
        shared = CPLRT(value_share)(cx->rt, captures[c]);
    if (shared != CPLRT_OK) {
        atomic_fetch_sub_explicit(&parallel_active, permits, memory_order_release);
        cplrt_release(cplrt_word_of(&result->hdr));
        return CPLRT(oom)(cx);
    }
    _Atomic uint32_t failure = UINT32_MAX;
    uint32_t workers = permits + 1u;
    parallel_job *jobs = (parallel_job *)CPLRT(alloc)(cx->rt, (size_t)workers * sizeof *jobs);
    if (jobs == NULL) {
        atomic_fetch_sub_explicit(&parallel_active, permits, memory_order_release);
        cplrt_release(cplrt_word_of(&result->hdr));
        return CPLRT(oom)(cx);
    }
    memset(jobs, 0, (size_t)workers * sizeof *jobs);
    uint32_t chunk = n / workers + (n % workers != 0u);
    pthread_attr_t attr;
    int attr_ok = pthread_attr_init(&attr) == 0;
    if (attr_ok && pthread_attr_setstacksize(&attr, CPLRT_LIMIT_EVAL_STACK_SIZE) != 0) {
        pthread_attr_destroy(&attr);
        attr_ok = 0;
    }
    for (uint32_t j = 0; j < workers; j++) {
        parallel_job *job = &jobs[j];
        job->parent = cx;
        job->failure = &failure;
        job->input = input;
        job->result = result;
        job->task = task;
        job->captures = captures;
        job->site = site;
        uint64_t start = (uint64_t)j * chunk;
        job->start = start < n ? (uint32_t)start : n;
        job->end = n - job->start < chunk ? n : job->start + chunk;
        if (j + 1u < workers && attr_ok && job->start != job->end &&
            pthread_create(&job->thread, &attr, parallel_run, job) == 0)
            job->started = 1;
        else
            parallel_run(job);
    }
    if (attr_ok)
        pthread_attr_destroy(&attr);
    for (uint32_t j = 0; j < workers; j++)
        if (jobs[j].started)
            pthread_join(jobs[j].thread, NULL);
    atomic_fetch_sub_explicit(&parallel_active, permits, memory_order_release);
    int st = CPLRT_OK;
    for (uint32_t j = 0; j < workers; j++) {
        parallel_job *job = &jobs[j];
        if (st == CPLRT_OK && job->failed) {
            st = CPLRT_FAULT;
            if (job->oom)
                CPLRT(oom)(cx);
            else {
                CPLRT(fault_free)(cx->fault);
                cx->fault = job->fault;
                job->fault = NULL;
                cx->attributed = job->attributed;
                cx->cancelled = job->cancelled;
                CPLRT(fault_free)(cx->violation);
                cx->violation = job->violation;
                job->violation = NULL;
            }
        }
        CPLRT(fault_free)(job->fault);
        CPLRT(fault_free)(job->violation);
    }
    CPLRT(free)(jobs);
    if (st == CPLRT_OK)
        CPLRT_SET(*out, cplrt_word_of(&result->hdr));
    else
        cplrt_release(cplrt_word_of(&result->hdr));
    return st;
}
