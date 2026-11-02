/* cpl_rt_platform_linux.c: threads, the monotonic clock, and per-thread cached stack bounds on Linux (spec/cabi.md
   2.1, 3.4, 3.6, 3.8, 8). The only runtime file that uses what the prologue's _GNU_SOURCE enables
   (pthread_getattr_np and memmem, spec/cabi.md 1.5). Track R1. */
#include "cpl_rt_internal.h"
#include "cpl_rt_platform.h"
#include <errno.h>
#include <pthread.h>
#include <string.h>
#include <time.h>

const char *CPLRT(find_bytes)(const char *hay, size_t hlen, const char *needle, size_t nlen)
{
    return (const char *)memmem(hay, hlen, needle, nlen);
}

/* The monotonic clock in nanoseconds (3.6): the time budget's deadline is computed and polled on it. */
uint64_t CPLRT(clock_ns)(void)
{
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
        return 0;
    return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
}

/* The calling thread's stack bounds, queried once per thread (3.4). A failed query is not cached, so a later call on
   the same thread asks again. */
typedef struct platform_linux_bounds {
    int known;
    uintptr_t lo, hi;
} platform_linux_bounds;
static _Thread_local platform_linux_bounds platform_linux_cached;

int CPLRT(stack_bounds)(uintptr_t *lo, uintptr_t *hi)
{
    if (!platform_linux_cached.known) {
        pthread_attr_t attr;
        void *addr = NULL;
        size_t size = 0;
        if (pthread_getattr_np(pthread_self(), &attr) != 0)
            return CPLRT_NOMEM;
        int e = pthread_attr_getstack(&attr, &addr, &size);
        pthread_attr_destroy(&attr);
        if (e != 0 || addr == NULL || size == 0)
            return CPLRT_NOMEM;
        platform_linux_cached.lo = (uintptr_t)addr;
        platform_linux_cached.hi = (uintptr_t)addr + size;
        platform_linux_cached.known = 1;
    }
    *lo = platform_linux_cached.lo;
    *hi = platform_linux_cached.hi;
    return CPLRT_OK;
}

typedef struct platform_linux_job {
    void (*fn)(void *arg);
    void *arg;
} platform_linux_job;

static void *platform_linux_start(void *p)
{
    const platform_linux_job *job = (const platform_linux_job *)p;
    job->fn(job->arg);
    return NULL;
}

/* The evaluation-thread launcher of the driver and the native executable (3.4, 3.8, 8): fn(arg) on a new thread with
   `stack` bytes of stack reserved (0: the default), joined before returning. */
int CPLRT(run_on_thread)(size_t stack, void (*fn)(void *arg), void *arg)
{
    pthread_attr_t attr;
    pthread_t thread;
    platform_linux_job job;
    job.fn = fn;
    job.arg = arg;
    if (pthread_attr_init(&attr) != 0)
        return CPLRT_NOMEM;
    if (stack != 0 && pthread_attr_setstacksize(&attr, stack) != 0) {
        pthread_attr_destroy(&attr);
        return CPLRT_NOMEM;
    }
    int e = pthread_create(&thread, &attr, platform_linux_start, &job);
    pthread_attr_destroy(&attr);
    if (e != 0)
        return CPLRT_NOMEM;
    (void)pthread_join(thread, NULL);
    return CPLRT_OK;
}
