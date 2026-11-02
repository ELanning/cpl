/* cpl_rt_platform.h: the platform layer of the runtime (spec/cabi.md 2.1): threads, the monotonic clock, and the
   per-thread cached stack bounds, whose functions cpl_rt_internal.h declares (R1 section) and cpl_rt_platform_linux.c
   defines; this header adds the compiler-dependent helpers, each behind a portable fallback (spec/cabi.md 1.5).
   Internal: only the runtime's units include it, after the runtime prologue of spec/cabi.md 2.2. Track R1. */
#ifndef CPL_RT_PLATFORM_H
#define CPL_RT_PLATFORM_H 1
#include <stdint.h>

/* The address of the calling function's frame, the position the stack guard compares with the cached bounds
   (spec/cabi.md 3.4): __builtin_frame_address(0) where the compiler has it, else the address of a local of the caller
   (the macro expands in the caller, so the local lives in the caller's frame). */
#if defined(__GNUC__) || defined(__clang__)
#define CPLRT_FRAME_ADDRESS() ((uintptr_t)__builtin_frame_address(0))
#else
#define CPLRT_FRAME_ADDRESS() ((uintptr_t)(void *)&(char){0})
#endif

/* Branch hints (spec/cabi.md 1.5: __builtin_expect only behind a fallback). */
#if defined(__GNUC__) || defined(__clang__)
#define CPLRT_LIKELY(e) __builtin_expect(!!(e), 1)
#define CPLRT_UNLIKELY(e) __builtin_expect(!!(e), 0)
#else
#define CPLRT_LIKELY(e) (e)
#define CPLRT_UNLIKELY(e) (e)
#endif

#endif
