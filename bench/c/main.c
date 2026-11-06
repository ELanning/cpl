/* The kernels of bench/cpl through the native engine's C API, as a C host calls them: `cpl build
   --target c`, then link this file with the library. Prints "name n result ms", the best of three. */
#define _POSIX_C_SOURCE 199309L
#include "bench.h"
#include <stdio.h>
#include <string.h>
#include <time.h>

typedef cpl_status (*kernel)(cpl_runtime *, cpl_value, cpl_value *, const cpl_call_options *, cpl_error **);

static double now_ms(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec * 1e3 + (double)t.tv_nsec / 1e6;
}

int main(int argc, char **argv)
{
    struct { const char *name; kernel f; int64_t n; } ks[] = {
        { "arith", bench_arith, 50000000 },        { "fib", bench_fib, 32 },
        { "collatz", bench_collatz, 300000 },      { "list_index", bench_list_index, 5000000 },
        { "list_walk", bench_list_walk, 5000000 }, { "sorting", bench_sorting, 500000 },
        { "structs", bench_structs, 20000000 },    { "text", bench_text, 1000000 },
        { "bigint", bench_bigint, 5000 },          { "trees", bench_trees, 20 },
        { "records", bench_records, 2000000 },     { "matrix", bench_matrix, 150 },
        { "business", bench_business, 2000 },
    };
    cpl_runtime *rt = NULL;
    if (bench_runtime_open(NULL, &rt, NULL) != CPL_OK) {
        fprintf(stderr, "runtime_open failed\n");
        return 1;
    }
    for (size_t k = 0; k < sizeof ks / sizeof ks[0]; k++) {
        int run = argc < 2;
        for (int a = 1; a < argc; a++)
            run |= strcmp(argv[a], ks[k].name) == 0;
        if (!run)
            continue;
        double best = 1e18;
        int64_t result = 0;
        for (int r = 0; r < 3; r++) {
            cpl_value n = { { 0, 0 } }, out = { { 0, 0 } };
            cpl_int_from_i64(rt, ks[k].n, &n, NULL);
            double t = now_ms();
            cpl_status st = ks[k].f(rt, n, &out, NULL, NULL);
            double ms = now_ms() - t;
            if (st != CPL_OK) {
                printf("%s fault %d\n", ks[k].name, (int)st);
                break;
            }
            cpl_int_to_i64(rt, out, &result, NULL);
            cpl_value_release(rt, out);
            cpl_value_release(rt, n);
            if (ms < best)
                best = ms;
        }
        printf("%s %lld %lld %.2f\n", ks[k].name, (long long)ks[k].n, (long long)result, best);
    }
    cpl_runtime_close(rt, 0, NULL);
    return 0;
}
