/* cpl_rt_worker.c: the worker transport (spec/python.md Section 6). The runtime's worker client, through which the
   grant of a binding that runs in a worker (a backend-python binding outside a Python host, a backend-c binding with
   isolation "worker") starts one child process speaking cpl-worker protocol 1 and forwards each operation to it; and
   the worker main of the C adapter host, which grants a backend-c binding in process and answers that protocol.
   File-scope names begin with worker_ (2.2). */
#include "cpl_rt_internal.h"
#include "cpl_rt_gen.h"
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

/* Set by the driver's and the native executable's main: a C adapter host is this executable re-executed. */
int CPLRT(worker_self) = 0;

/* The bootstrap of a Python worker (spec/python.md 6.1): reads "<length>\n<source>" from standard input byte-exactly
   and executes the source as the module __cpl_worker__ (cpl_emit_c::python::BOOTSTRAP, the same text). */
static const char worker_boot[] =
    "import os,sys,types\n"
    "def _r(n):\n b=bytearray()\n while len(b)<n:\n  c=os.read(0,n-len(b))\n  if not c:sys.exit(70)\n  b+=c\n return bytes(b)\n"
    "h=bytearray()\n"
    "while not h.endswith(b'\\n'):\n c=os.read(0,1)\n if not c:sys.exit(70)\n h+=c\n"
    "s=_r(int(h))\n"
    "m=types.ModuleType('__cpl_worker__')\n"
    "sys.modules['__cpl_worker__']=m\n"
    "exec(compile(s,'<cpl-worker>','exec'),m.__dict__)\n";

#define WORKER_BANNER_MS 60000u
#define WORKER_GRACE_MS 5000u
#define WORKER_PYTHON 1u
#define WORKER_C 2u

typedef struct worker {
    pthread_mutex_t lock;          /* one request at a time (6.3) */
    pid_t pid;                     /* 0 once reaped */
    int fd;                        /* the parent's end of the channel; -1 once closed */
    int dead;                      /* the worker can no longer answer; `message` is the Transport message of later calls */
    char *message;
    char *binding;
    uint64_t timeout_ms;           /* 0: none */
    char *buf;                     /* bytes read and not yet consumed */
    size_t len, cap, consumed;
} worker;

/* ---- configuration (6.5) -------------------------------------------------------------------------------------- */

static void worker_config_free(cplrt_runtime *rt)
{
    free(rt->worker_python);
    rt->worker_python = NULL;
    for (uint32_t i = 0; i < rt->nworker_env; i++)
        free(rt->worker_env[i]);
    free(rt->worker_env);
    rt->worker_env = NULL;
    rt->nworker_env = 0;
}

static char *worker_strdup(const char *s)
{
    size_t n = strlen(s);
    char *r = (char *)malloc(n + 1u);
    if (r != NULL)
        memcpy(r, s, n + 1u);
    return r;
}

/* The interpreter and the "KEY=VALUE" environment entries of later Python workers of rt (the driver's `workers`
   request, the Python extension); python NULL keeps the default (CPL_PYTHON, else python3). */
int CPLRT(workers_configure)(cplrt_runtime *rt, const char *python, const char *const *env, uint32_t n)
{
    char *py = python != NULL ? worker_strdup(python) : NULL;
    char **e = n > 0 ? (char **)calloc(n, sizeof *e) : NULL;
    if ((python != NULL && py == NULL) || (n > 0 && e == NULL)) {
        free(py);
        free(e);
        return CPLRT_NOMEM;
    }
    for (uint32_t i = 0; i < n; i++) {
        e[i] = worker_strdup(env[i]);
        if (e[i] == NULL) {
            for (uint32_t j = 0; j < i; j++)
                free(e[j]);
            free(e);
            free(py);
            return CPLRT_NOMEM;
        }
    }
    pthread_mutex_lock(&rt->lock);
    worker_config_free(rt);
    rt->worker_python = py;
    rt->worker_env = e;
    rt->nworker_env = n;
    pthread_mutex_unlock(&rt->lock);
    return CPLRT_OK;
}

/* Frees the configuration of a runtime that is being closed. */
void CPLRT(workers_fini)(cplrt_runtime *rt)
{
    worker_config_free(rt);
}

/* ---- process plumbing ----------------------------------------------------------------------------------------- */

static uint64_t worker_now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000u + (uint64_t)ts.tv_nsec / 1000000u;
}

static int worker_key_eq(const char *entry, const char *other)
{
    const char *a = strchr(entry, '='), *b = strchr(other, '=');
    size_t na = a != NULL ? (size_t)(a - entry) : strlen(entry), nb = b != NULL ? (size_t)(b - other) : strlen(other);
    return na == nb && memcmp(entry, other, na) == 0;
}

/* The child's environment: this process's, with the overrides replacing entries of the same key (malloc; the strings
   are borrowed). */
static char **worker_envp(const char *const *over, uint32_t nover)
{
    size_t n = 0;
    while (environ != NULL && environ[n] != NULL)
        n++;
    char **e = (char **)calloc(n + nover + 1u, sizeof *e);
    if (e == NULL)
        return NULL;
    size_t k = 0;
    for (size_t i = 0; i < n; i++) {
        int replaced = 0;
        for (uint32_t j = 0; j < nover && !replaced; j++)
            replaced = worker_key_eq(environ[i], over[j]);
        if (!replaced)
            e[k++] = environ[i];
    }
    for (uint32_t j = 0; j < nover; j++)
        e[k++] = (char *)(uintptr_t)over[j];
    e[k] = NULL;
    return e;
}

/* Starts argv with one end of a fresh socket pair as its standard input and output, its signal mask empty and SIGPIPE
   at its default: 0 and *w->pid, *w->fd, else an errno. */
static int worker_spawn(worker *w, char *const *argv, char *const *envp)
{
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) != 0)
        return errno;
    posix_spawn_file_actions_t fa;
    posix_spawnattr_t at;
    posix_spawn_file_actions_init(&fa);
    posix_spawnattr_init(&at);
    posix_spawn_file_actions_adddup2(&fa, sv[1], 0);
    posix_spawn_file_actions_adddup2(&fa, sv[1], 1);
    sigset_t empty, defaults;
    sigemptyset(&empty);
    sigemptyset(&defaults);
    sigaddset(&defaults, SIGPIPE);
    sigaddset(&defaults, SIGINT);
    sigaddset(&defaults, SIGTERM);
    posix_spawnattr_setsigmask(&at, &empty);
    posix_spawnattr_setsigdefault(&at, &defaults);
    posix_spawnattr_setflags(&at, (short)(POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF));
    pid_t pid = 0;
    int r = posix_spawnp(&pid, argv[0], &fa, &at, argv, envp);
    posix_spawn_file_actions_destroy(&fa);
    posix_spawnattr_destroy(&at);
    close(sv[1]);
    if (r != 0) {
        close(sv[0]);
        return r;
    }
    w->pid = pid;
    w->fd = sv[0];
    return 0;
}

/* Waits up to grace_ms for the process to end, then kills it; renders how it ended ("status <n>", "signal <n>"). */
static void worker_reap(worker *w, uint32_t grace_ms, char *status, size_t n)
{
    if (status != NULL)
        snprintf(status, n, "status -1");
    if (w->pid <= 0)
        return;
    int st = 0;
    pid_t r = 0;
    uint64_t deadline = worker_now_ms() + grace_ms;
    for (;;) {
        r = waitpid(w->pid, &st, WNOHANG);
        if (r < 0 && errno == EINTR)
            continue;
        if (r != 0 || worker_now_ms() >= deadline)
            break;
        struct timespec nap = { 0, 2000000 };
        nanosleep(&nap, NULL);
    }
    if (r == 0) {
        kill(w->pid, SIGKILL);
        do
            r = waitpid(w->pid, &st, 0);
        while (r < 0 && errno == EINTR);
    }
    if (r == w->pid && status != NULL) {
        if (WIFEXITED(st))
            snprintf(status, n, "status %d", WEXITSTATUS(st));
        else if (WIFSIGNALED(st))
            snprintf(status, n, "signal %d", WTERMSIG(st));
    }
    w->pid = 0;
}

static void worker_close(worker *w)
{
    if (w->fd >= 0) {
        close(w->fd);
        w->fd = -1;
    }
}

/* An absolute deadline shared by all partial writes and reads of one exchange; 0 means unlimited. */
static uint64_t worker_deadline(uint64_t timeout_ms)
{
    uint64_t now = worker_now_ms();
    return timeout_ms == 0 ? 0 : timeout_ms > UINT64_MAX - now ? UINT64_MAX : now + timeout_ms;
}

/* 1 when ready, 0 on a channel error, -1 at the deadline, -2 on cancellation. */
static int worker_ready(worker *w, short events, uint64_t deadline, cpl_call *call)
{
    for (;;) {
        if (call != NULL && CPLRT(call_is_cancelled)(call))
            return -2;
        int slice = 50;
        if (deadline > 0) {
            uint64_t now = worker_now_ms();
            if (now >= deadline)
                return -1;
            if (deadline - now < (uint64_t)slice)
                slice = (int)(deadline - now);
        } else if (call == NULL) {
            slice = -1;
        }
        struct pollfd pfd = { w->fd, events, 0 };
        int r = poll(&pfd, 1, slice);
        if (r < 0 && errno == EINTR)
            continue;
        if (r < 0)
            return 0;
        if (r > 0)
            return (pfd.revents & POLLNVAL) == 0;
    }
}

static int worker_write(worker *w, const char *s, size_t n, uint64_t deadline, cpl_call *call)
{
    while (n > 0) {
        int ready = worker_ready(w, POLLOUT, deadline, call);
        if (ready != 1)
            return ready;
        /* Readiness does not mean the entire request fits in the socket. A blocking send could still bypass the
           deadline and cancellation when the worker stops consuming a large argument. */
        ssize_t k = send(w->fd, s, n, MSG_NOSIGNAL | MSG_DONTWAIT);
        if (k < 0) {
            if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)
                continue;
            return 0;
        }
        if (k == 0)
            return 0;
        s += k;
        n -= (size_t)k;
    }
    return 1;
}

/* One line of the channel: 1 with *line (NUL-terminated, valid until the next read); 0 at the end of the channel or
   on an error; -1 at the absolute deadline (0: none); -2 when `call` is not NULL and its token was requested; -3
   when a line grows past 1 GiB. */
static int worker_read_line(worker *w, uint64_t deadline, cpl_call *call, char **line)
{
    if (w->consumed > 0) {
        memmove(w->buf, w->buf + w->consumed, w->len - w->consumed);
        w->len -= w->consumed;
        w->consumed = 0;
    }
    size_t scanned = 0;
    for (;;) {
        char *nl = w->len > scanned ? (char *)memchr(w->buf + scanned, '\n', w->len - scanned) : NULL;
        if (nl != NULL) {
            *nl = '\0';
            w->consumed = (size_t)(nl - w->buf) + 1u;
            *line = w->buf;
            return 1;
        }
        scanned = w->len;
        int ready = worker_ready(w, POLLIN, deadline, call);
        if (ready != 1)
            return ready;
        if (w->len + 65536u + 1u > w->cap) {
            size_t c2 = w->cap == 0 ? 131072u : w->cap * 2u;
            if (c2 > ((size_t)1 << 30))
                return -3;
            char *g = (char *)realloc(w->buf, c2);
            if (g == NULL)
                return 0;
            w->buf = g;
            w->cap = c2;
        }
        ssize_t k = recv(w->fd, w->buf + w->len, w->cap - w->len - 1u, MSG_DONTWAIT);
        if (k < 0 && (errno == EINTR || errno == EAGAIN))
            continue;
        if (k <= 0)
            return 0;
        w->len += (size_t)k;
    }
}

static void worker_free(worker *w)
{
    if (w == NULL)
        return;
    worker_close(w);
    if (w->pid > 0)
        worker_reap(w, 0, NULL, 0);
    free(w->message);
    free(w->binding);
    free(w->buf);
    pthread_mutex_destroy(&w->lock);
    free(w);
}

/* ---- the grant (6.1, 6.2, 6.4) -------------------------------------------------------------------------------- */

static cpl_status worker_bind_failed(cplrt_runtime *rt, const cplrt_grant *g, cpl_error **error, const char *note)
{
    cplrt_datum d;
    memset(&d, 0, sizeof d);
    d.key = "binding";
    d.form = CPLRT_DATUM_STRING;
    d.str = g->binding;
    return CPLRT(error_interop_note)(rt, error, CPLRT_INTEROP_BIND_FAILED, &d, 1, note, strlen(note));
}

/* The C adapter host of a backend-c worker binding (6.6): CPL_C_WORKER; this executable when it is a driver or a
   native executable; else <dir>/<prefix>-worker or <dir>/../bin/<prefix>-worker beside the object that holds the
   runtime. NULL when none is found. */
static char *worker_c_host(const cplrt_program *p)
{
    const char *env = getenv("CPL_C_WORKER");
    if (env != NULL && env[0] != '\0')
        return worker_strdup(env);
    if (CPLRT(worker_self))
        return worker_strdup("/proc/self/exe");
    Dl_info info;
    if (dladdr((void *)(uintptr_t)&CPLRT(worker_grant), &info) == 0 || info.dli_fname == NULL)
        return NULL;
    const char *slash = strrchr(info.dli_fname, '/');
    size_t dir = slash != NULL ? (size_t)(slash - info.dli_fname) : 0u;
    const char *prefix = p->info.prefix;
    static const char *const shapes[2] = { "%.*s/%s-worker", "%.*s/../bin/%s-worker" };
    for (int i = 0; i < 2; i++) {
        size_t n = dir + strlen(prefix) + 32u;
        char *path = (char *)malloc(n);
        if (path == NULL)
            return NULL;
        snprintf(path, n, shapes[i], (int)dir, dir > 0 ? info.dli_fname : ".", prefix);
        if (access(path, X_OK) == 0)
            return path;
        free(path);
    }
    return NULL;
}

/* The grant of a worker binding (spec/python.md 6.1-6.4): starts the worker, checks its banner, binds; CPL_OK with
   c's user the worker, else interop_bind_failed with the cause as a note (or CPL_E_NOMEM). */
cpl_status CPLRT(worker_grant)(cplrt_runtime *rt, const cplrt_grant *g, cplrt_capobj *c, cpl_error **error)
{
    const cplrt_program *p = rt->program;
    worker *w = (worker *)calloc(1, sizeof *w);
    if (w == NULL)
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    w->fd = -1;
    w->timeout_ms = g->timeout_ms;
    w->binding = worker_strdup(g->binding);
    pthread_mutex_init(&w->lock, NULL);
    char note[512];
    const char *kind = g->worker == WORKER_PYTHON ? "python" : "c";
    const char *digest = g->worker == WORKER_PYTHON ? p->python_digest : p->info.program_digest;
    /* argv and the environment overrides */
    char *exe = NULL;
    const char *over[64];
    uint32_t nover = 0;
    pthread_mutex_lock(&rt->lock);
    if (g->worker == WORKER_PYTHON) {
        const char *py = rt->worker_python;
        if (py == NULL) {
            py = getenv("CPL_PYTHON");
            if (py == NULL || py[0] == '\0')
                py = "python3";
        }
        exe = worker_strdup(py);
        for (uint32_t i = 0; i < rt->nworker_env && nover < 63u; i++)
            over[nover++] = rt->worker_env[i];
    } else {
        exe = worker_c_host(p);
        over[nover++] = "CPL_WORKER_MODE=1";
    }
    char **envp = worker_envp(over, nover);
    pthread_mutex_unlock(&rt->lock);
    if (w->binding == NULL || envp == NULL || (g->worker == WORKER_PYTHON && exe == NULL) ||
        (g->worker == WORKER_PYTHON && p->python_worker == NULL)) {
        free(exe);
        free(envp);
        worker_free(w);
        if (g->worker == WORKER_PYTHON && p->python_worker == NULL)
            return worker_bind_failed(rt, g, error, "the program carries no Python worker source");
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    }
    if (exe == NULL) {
        free(envp);
        worker_free(w);
        snprintf(note, sizeof note,
                 "no C adapter host was found for the binding `%s`: set CPL_C_WORKER to the path of %s-worker",
                 g->binding, p->info.prefix);
        return worker_bind_failed(rt, g, error, note);
    }
    char *argv_py[4] = { exe, (char *)(uintptr_t) "-c", (char *)(uintptr_t)worker_boot, NULL };
    char *argv_c[2] = { exe, NULL };
    int r = worker_spawn(w, g->worker == WORKER_PYTHON ? argv_py : argv_c, envp);
    free(envp);
    if (r != 0) {
        snprintf(note, sizeof note, "the %s worker `%s` could not be started: %s", kind, exe, strerror(r));
        free(exe);
        worker_free(w);
        return worker_bind_failed(rt, g, error, note);
    }
    free(exe);
    char status[48];
    uint64_t limit = g->timeout_ms > 0 ? g->timeout_ms : WORKER_BANNER_MS;
    uint64_t deadline = worker_deadline(limit);
    if (g->worker == WORKER_PYTHON) {
        char head[32];
        int k = snprintf(head, sizeof head, "%zu\n", p->python_worker_len);
        r = worker_write(w, head, (size_t)k, deadline, NULL);
        if (r == 1)
            r = worker_write(w, p->python_worker, p->python_worker_len, deadline, NULL);
        if (r != 1) {
            worker_close(w);
            worker_reap(w, r == 0 ? 1000u : 0u, status, sizeof status);
            if (r == -1)
                snprintf(note, sizeof note, "the worker sent no banner within %llu ms", (unsigned long long)limit);
            else
                snprintf(note, sizeof note, "the worker did not take its source (it exited with %s)", status);
            worker_free(w);
            return worker_bind_failed(rt, g, error, note);
        }
    }
    /* the banner */
    char *line = NULL;
    r = worker_read_line(w, deadline, NULL, &line);
    if (r != 1) {
        worker_close(w);
        worker_reap(w, r == 0 ? 1000u : 0u, status, sizeof status);
        if (r == -1)
            snprintf(note, sizeof note, "the worker sent no banner within %llu ms", (unsigned long long)limit);
        else
            snprintf(note, sizeof note, "the worker exited with %s before its banner", status);
        worker_free(w);
        return worker_bind_failed(rt, g, error, note);
    }
    cplrt_jv *banner = NULL;
    size_t at = 0;
    int ok = CPLRT(json_parse)(rt, line, strlen(line), &banner, &at) == CPLRT_OK;
    uint64_t protocol = 0;
    const cplrt_jv *wk = ok ? CPLRT(json_member)(banner, "worker") : NULL;
    const cplrt_jv *pg = ok ? CPLRT(json_member)(banner, "program") : NULL;
    ok = ok && CPLRT(jv_u64)(CPLRT(json_member)(banner, "protocol"), &protocol) && protocol == 1u && wk != NULL &&
         wk->type == CPLRT_JSON_STRING && strcmp(wk->str, kind) == 0 && pg != NULL && pg->type == CPLRT_JSON_STRING &&
         digest != NULL && strcmp(pg->str, digest) == 0;
    CPLRT(json_free)(banner);
    if (!ok) {
        worker_close(w);
        worker_reap(w, 0, status, sizeof status);
        snprintf(note, sizeof note, "the worker's banner is not cpl-worker protocol 1 for this program");
        worker_free(w);
        return worker_bind_failed(rt, g, error, note);
    }
    /* bind */
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_puts)(&b, "{\"op\":\"bind\",\"binding\":");
    CPLRT(write_json_string)(&b, g->binding, strlen(g->binding));
    CPLRT(sbuf_puts)(&b, "}\n");
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        worker_free(w);
        return CPLRT(error_interop)(rt, error, CPLRT_INTEROP_OUT_OF_MEMORY, NULL, 0);
    }
    deadline = worker_deadline(limit);
    r = worker_write(w, b.ptr, b.len, deadline, NULL);
    CPLRT(sbuf_free)(&b);
    if (r == 1)
        r = worker_read_line(w, deadline, NULL, &line);
    if (r != 1) {
        worker_close(w);
        worker_reap(w, r == 0 ? 1000u : 0u, status, sizeof status);
        if (r == -1)
            snprintf(note, sizeof note, "the worker did not bind within %llu ms", (unsigned long long)limit);
        else
            snprintf(note, sizeof note, "the worker exited with %s while binding", status);
        worker_free(w);
        return worker_bind_failed(rt, g, error, note);
    }
    cplrt_jv *answer = NULL;
    ok = CPLRT(json_parse)(rt, line, strlen(line), &answer, &at) == CPLRT_OK;
    uint64_t handle = 1;
    if (ok && CPLRT(jv_u64)(CPLRT(json_member)(answer, "handle"), &handle) && handle == 0u) {
        CPLRT(json_free)(answer);
        c->ref.user = w;
        return CPL_OK;
    }
    /* the adapter's create() failed: its note, else the error's message */
    snprintf(note, sizeof note, "the worker could not grant the binding");
    const cplrt_jv *e = ok ? CPLRT(json_member)(answer, "error") : NULL;
    const cplrt_jv *notes = e != NULL ? CPLRT(json_member)(e, "notes") : NULL;
    if (notes != NULL && notes->type == CPLRT_JSON_ARRAY && notes->n > 0) {
        const cplrt_jv *m = CPLRT(json_member)(&notes->items[0], "message");
        if (m != NULL && m->type == CPLRT_JSON_STRING)
            snprintf(note, sizeof note, "%s", m->str);
    } else if (e != NULL) {
        const cplrt_jv *m = CPLRT(json_member)(e, "message");
        if (m != NULL && m->type == CPLRT_JSON_STRING)
            snprintf(note, sizeof note, "%s", m->str);
    }
    CPLRT(json_free)(answer);
    worker_close(w);
    worker_reap(w, WORKER_GRACE_MS, status, sizeof status);
    worker_free(w);
    return worker_bind_failed(rt, g, error, note);
}

/* ---- operations (6.2, 6.4) ------------------------------------------------------------------------------------ */

/* Marks w dead with `message`, the Transport message of every later call. */
static void worker_die(worker *w, const char *message)
{
    if (!w->dead) {
        w->dead = 1;
        free(w->message);
        w->message = worker_strdup(message != NULL ? message : "the adapter host is gone");
    }
}

static int32_t worker_kind_index(const cplrt_jv *v)
{
    if (v == NULL || v->type != CPLRT_JSON_STRING)
        return -1;
    for (uint32_t i = 0; i < CPLRT_FOREIGN_KINDS_N; i++)
        if (strcmp(v->str, CPLRT_FOREIGN_KINDS[i]) == 0)
            return (int32_t)i;
    return -1;
}

static cpl_status worker_invoke(const cplrt_capref *cap, cpl_call *call, uint32_t op, const cplrt_w *args,
                                cplrt_w *result)
{
    worker *w = (worker *)cap->user;
    cplrt_runtime *rt = CPLRT(call_rt)(call);
    const cplrt_program *p = rt->program;
    const cplrt_op *opd = &p->defs[cap->capability].ops[op];
    char message[512];
    char status[48];
    pthread_mutex_lock(&w->lock);
    if (w->dead) {
        snprintf(message, sizeof message, "%s", w->message != NULL ? w->message : "the adapter host is gone");
        pthread_mutex_unlock(&w->lock);
        return CPLRT(call_host_ended)(call, message, strlen(message));
    }
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_puts)(&b, "{\"op\":\"cap_call\",\"handle\":0,\"operation\":");
    CPLRT(sbuf_u64)(&b, op);
    CPLRT(sbuf_puts)(&b, ",\"args\":[");
    for (uint32_t i = 0; i < opd->nparams; i++) {
        if (i > 0)
            CPLRT(sbuf_putc)(&b, ',');
        CPLRT(nodes_write)(&b, rt, p, args[i], 0);
    }
    CPLRT(sbuf_puts)(&b, "]}\n");
    if (b.status != CPLRT_OK) {
        CPLRT(sbuf_free)(&b);
        pthread_mutex_unlock(&w->lock);
        return CPL_E_NOMEM;
    }
    uint64_t deadline = worker_deadline(w->timeout_ms);
    int r = worker_write(w, b.ptr, b.len, deadline, call);
    CPLRT(sbuf_free)(&b);
    char *line = NULL;
    if (r == 1)
        r = worker_read_line(w, deadline, call, &line);
    if (r != 1) {
        cpl_status st;
        worker_close(w);
        if (r == 0) {
            worker_reap(w, 1000u, status, sizeof status);
            snprintf(message, sizeof message, "the adapter host for %s exited with %s", w->binding, status);
            worker_die(w, message);
            st = CPLRT(call_host_ended)(call, message, strlen(message));
        } else if (r == -1) {
            worker_reap(w, 0, status, sizeof status);
            snprintf(message, sizeof message, "the adapter host for %s did not answer within %llu ms", w->binding,
                     (unsigned long long)w->timeout_ms);
            worker_die(w, message);
            st = CPLRT(call_fail)(call, CPL_FOREIGN_TIMEOUT, message, strlen(message));
            st = st == CPL_FOREIGN_ERROR || st == CPL_OK ? CPL_FOREIGN_ERROR : st;
        } else if (r == -2) {
            worker_reap(w, 0, status, sizeof status);
            snprintf(message, sizeof message, "the call of %s was cancelled", opd->identity);
            worker_die(w, message);
            st = CPLRT(call_fail)(call, CPL_FOREIGN_CANCELLED, message, strlen(message));
            st = st == CPL_FOREIGN_ERROR || st == CPL_OK ? CPL_FOREIGN_ERROR : st;
        } else {
            worker_reap(w, 0, status, sizeof status);
            snprintf(message, sizeof message, "%s sent an answer that is not cpl-worker protocol 1 for %s: %s",
                     w->binding, opd->identity, "not a response to cap_call");
            worker_die(w, message);
            st = CPLRT(call_worker_protocol)(call, "not a response to cap_call");
        }
        pthread_mutex_unlock(&w->lock);
        return st;
    }
    cplrt_jv *answer = NULL;
    size_t at = 0;
    int pr = CPLRT(json_parse)(rt, line, strlen(line), &answer, &at);
    if (pr == CPLRT_NOMEM) {
        pthread_mutex_unlock(&w->lock);
        return CPL_E_NOMEM;
    }
    const char *reason = NULL;
    cpl_status st = CPL_E_INTERNAL;
    if (pr != CPLRT_OK) {
        reason = "not JSON";
    } else {
        const cplrt_jv *ok = CPLRT(json_member)(answer, "ok");
        const cplrt_jv *f = CPLRT(json_member)(answer, "foreign_error");
        const cplrt_jv *e = CPLRT(json_member)(answer, "error");
        if ((ok != NULL) + (f != NULL) + (e != NULL) != 1) {
            reason = "not a response to cap_call";
        } else if (ok != NULL) {
            const char *detail = NULL;
            int k = CPLRT(nodes_read)(rt, ok, result, &detail);
            if (k == CPLRT_OK)
                st = CPL_OK;
            else if (k == CPLRT_NOMEM)
                st = CPL_E_NOMEM;
            else
                reason = "a value that is not a node list";
        } else if (f != NULL) {
            int32_t kind = worker_kind_index(CPLRT(json_member)(f, "kind"));
            const cplrt_jv *m = CPLRT(json_member)(f, "message");
            const cplrt_jv *details = CPLRT(json_member)(f, "details");
            if (kind < 0 || m == NULL || m->type != CPLRT_JSON_STRING ||
                (details != NULL && details->type != CPLRT_JSON_ARRAY)) {
                reason = "not a response to cap_call";
            } else {
                st = CPL_FOREIGN_ERROR;
                for (uint32_t i = 0; details != NULL && i < details->n && st == CPL_FOREIGN_ERROR; i++) {
                    const cplrt_jv *pair = &details->items[i];
                    if (pair->type != CPLRT_JSON_ARRAY || pair->n != 2u || pair->items[0].type != CPLRT_JSON_STRING ||
                        pair->items[1].type != CPLRT_JSON_STRING) {
                        reason = "not a response to cap_call";
                        st = CPL_E_INTERNAL;
                        break;
                    }
                    cpl_status ds = CPLRT(call_detail)(call, pair->items[0].str, pair->items[0].len,
                                                       pair->items[1].str, pair->items[1].len);
                    if (ds == CPL_E_NOMEM)
                        st = CPL_E_NOMEM;
                    else if (ds != CPL_OK) {
                        reason = "not a response to cap_call";
                        st = CPL_E_INTERNAL;
                    }
                }
                if (st == CPL_FOREIGN_ERROR) {
                    cpl_status fs = CPLRT(call_fail)(call, kind, m->str, m->len);
                    if (fs == CPL_E_NOMEM)
                        st = CPL_E_NOMEM;
                    else if (fs != CPL_FOREIGN_ERROR) {
                        reason = "not a response to cap_call";
                        st = CPL_E_INTERNAL;
                    }
                }
            }
        } else {
            reason = "not a response to cap_call";
        }
    }
    CPLRT(json_free)(answer);
    if (reason != NULL) {
        worker_close(w);
        worker_reap(w, 0, status, sizeof status);
        snprintf(message, sizeof message, "%s sent an answer that is not cpl-worker protocol 1 for %s: %s", w->binding,
                 opd->identity, reason);
        worker_die(w, message);
        st = CPLRT(call_worker_protocol)(call, reason);
    }
    pthread_mutex_unlock(&w->lock);
    return st;
}

/* The grant's last reference (6.3): release, shutdown, five seconds for the exit, then SIGKILL; always reaped. */
static void worker_drop(const cplrt_capref *cap)
{
    worker *w = (worker *)cap->user;
    if (w == NULL)
        return;
    uint64_t deadline = worker_deadline(WORKER_GRACE_MS);
    int graceful = !w->dead && w->fd >= 0;
    if (graceful) {
        char *line = NULL;
        static const char release[] = "{\"op\":\"release\",\"handle\":0}\n";
        static const char shutdown[] = "{\"op\":\"shutdown\"}\n";
        graceful = worker_write(w, release, sizeof release - 1u, deadline, NULL) == 1 &&
                   worker_read_line(w, deadline, NULL, &line) == 1;
        if (graceful)
            graceful = worker_write(w, shutdown, sizeof shutdown - 1u, deadline, NULL) == 1 &&
                       worker_read_line(w, deadline, NULL, &line) == 1;
    }
    worker_close(w);
    uint64_t now = worker_now_ms();
    worker_reap(w, graceful && now < deadline ? (uint32_t)(deadline - now) : 0u, NULL, 0);
    worker_free(w);
}

const cplrt_capimpl CPLRT(worker_impl) = { 0u, worker_invoke, worker_drop };

/* ---- the C adapter host (6.6): the worker main ------------------------------------------------------------------ */

static void worker_main_send(int fd, cplrt_sbuf *b)
{
    if (b->status != CPLRT_OK) {
        static const char oom[] = "{\"error\":{\"code\":\"interop_out_of_memory\",\"category\":\"interop\","
                                  "\"severity\":\"error\",\"message\":\"the C adapter host is out of memory\"}}\n";
        (void)!write(fd, oom, sizeof oom - 1u);
        return;
    }
    CPLRT(sbuf_putc)(b, '\n');
    const char *s = b->ptr;
    size_t n = b->len;
    while (n > 0) {
        ssize_t k = write(fd, s, n);
        if (k < 0 && errno == EINTR)
            continue;
        if (k <= 0)
            return;
        s += k;
        n -= (size_t)k;
    }
}

static void worker_main_invalid(int fd, cplrt_runtime *rt, const char *pointer, const char *detail)
{
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_puts)(&b, "{\"error\":{\"code\":\"interop_json_invalid\",\"category\":\"interop\",\"severity\":\"error\","
                         "\"message\":\"the request is not cpl-worker protocol 1\",\"pointer\":");
    CPLRT(write_json_string)(&b, pointer, strlen(pointer));
    CPLRT(sbuf_puts)(&b, ",\"detail\":");
    CPLRT(write_json_string)(&b, detail, strlen(detail));
    CPLRT(sbuf_puts)(&b, "}}");
    worker_main_send(fd, &b);
    CPLRT(sbuf_free)(&b);
}

/* The main of a C adapter host (`CPL_WORKER_MODE=1`): moves the channel off standard input and output, sends the
   banner, and answers bind, cap_call, release, and shutdown, granting the binding in process; returns the exit
   status. */
int CPLRT(worker_main)(void)
{
    int in = dup(0), out = dup(1);
    int null = open("/dev/null", O_RDONLY);
    if (in < 0 || out < 0 || null < 0)
        return 70;
    dup2(null, 0);
    close(null);
    dup2(2, 1);
    const cplrt_program *p = &CPLPKG(program);
    cpl_runtime_options options = CPL_RUNTIME_OPTIONS_INIT;
    cpl_runtime *prt = NULL;
    if (cplrt_runtime_open(p, &options, &prt, NULL) != CPL_OK)
        return 70;
    cplrt_runtime *rt = (cplrt_runtime *)(void *)prt;
    cplrt_sbuf b;
    cplrt_sbuf_init(&b, rt);
    CPLRT(sbuf_puts)(&b, "{\"protocol\":1,\"worker\":\"c\",\"program\":");
    CPLRT(write_json_string)(&b, p->info.program_digest, strlen(p->info.program_digest));
    CPLRT(sbuf_puts)(&b, ",\"runtime\":{\"cpl\":");
    CPLRT(write_json_string)(&b, p->info.runtime_version, strlen(p->info.runtime_version));
    CPLRT(sbuf_puts)(&b, "}}");
    worker_main_send(out, &b);
    CPLRT(sbuf_free)(&b);
    worker reader;
    memset(&reader, 0, sizeof reader);
    reader.fd = in;
    cplrt_capobj *cap = NULL;
    int code = 0;
    for (;;) {
        char *line = NULL;
        if (worker_read_line(&reader, 0, NULL, &line) != 1)
            break;
        cplrt_jv *req = NULL;
        size_t at = 0;
        if (CPLRT(json_parse)(rt, line, strlen(line), &req, &at) != CPLRT_OK) {
            worker_main_invalid(out, rt, "", "not JSON");
            continue;
        }
        const cplrt_jv *op = CPLRT(json_member)(req, "op");
        const char *name = op != NULL && op->type == CPLRT_JSON_STRING ? op->str : "";
        if (strcmp(name, "bind") == 0) {
            const cplrt_jv *bn = CPLRT(json_member)(req, "binding");
            const cplrt_grant *g = NULL;
            for (uint32_t i = 0; bn != NULL && bn->type == CPLRT_JSON_STRING && i < p->ngrants; i++)
                if (p->grants[i].worker == WORKER_C && strcmp(p->grants[i].binding, bn->str) == 0)
                    g = &p->grants[i];
            if (g == NULL || cap != NULL) {
                worker_main_invalid(out, rt, "/binding", "no backend-c worker binding of this name, or one is bound");
            } else {
                cplrt_capobj *c = CPLRT(cap_new_obj)(rt, g->capability, g->impl, NULL, g->binding, NULL);
                cpl_error *e = NULL;
                if (c == NULL) {
                    worker_main_invalid(out, rt, "", "out of memory");
                } else if (CPLRT(grant_init_local)(rt, g, c, &e) != CPL_OK) {
                    CPLRT(cap_unref)(c);
                    cplrt_sbuf_init(&b, rt);
                    CPLRT(sbuf_puts)(&b, "{\"error\":");
                    if (e != NULL) {
                        cpl_strview j = e->api->cpl_error_json(e);
                        CPLRT(sbuf_put)(&b, j.ptr, j.len);
                        e->api->cpl_error_release(e);
                    } else {
                        CPLRT(sbuf_puts)(&b, "null");
                    }
                    CPLRT(sbuf_putc)(&b, '}');
                    worker_main_send(out, &b);
                    CPLRT(sbuf_free)(&b);
                } else {
                    cap = c;
                    static const char handle[] = "{\"handle\":0}\n";
                    (void)!write(out, handle, sizeof handle - 1u);
                }
            }
        } else if (strcmp(name, "cap_call") == 0) {
            uint32_t opi = 0;
            const cplrt_capref *ref = cap != NULL ? CPLRT(cap_ref)(cap) : NULL;
            if (ref == NULL) {
                worker_main_invalid(out, rt, "/handle", "no grant has this handle");
            } else if (!CPLRT(jv_u32)(CPLRT(json_member)(req, "operation"), &opi) ||
                       opi >= p->defs[ref->capability].nmembers) {
                worker_main_invalid(out, rt, "/operation", "the capability has no such operation");
            } else {
                cplrt_w *args = NULL;
                uint32_t n = 0;
                const char *detail = "";
                int r = CPLRT(nodes_read_all)(rt, CPLRT(json_member)(req, "args"), &args, &n, &detail);
                if (r != CPLRT_OK) {
                    worker_main_invalid(out, rt, "/args", r == CPLRT_NOMEM ? "out of memory" : detail);
                } else {
                    cplrt_cx cx;
                    cplrt_memory_record mem;
                    if (CPLRT(cx_begin)(&cx, rt, &mem, 10000u, 1, 0u, 0u, NULL) != CPLRT_OK) {
                        worker_main_invalid(out, rt, "", "out of memory");
                    } else {
                        cplrt_w res = 0;
                        cplrt_sbuf foreign;
                        cplrt_sbuf_init(&foreign, rt);
                        CPLRT(sbuf_put)(&foreign, "", 0);
                        r = CPLRT(cap_call_raw)(&cx, CPLRT_NO_SITE, ref, opi, args, n, &res, &foreign);
                        cplrt_sbuf_init(&b, rt);
                        if (r == CPLRT_OK) {
                            CPLRT(sbuf_puts)(&b, "{\"ok\":");
                            CPLRT(nodes_write)(&b, rt, p, res, 0);
                            CPLRT(sbuf_putc)(&b, '}');
                        } else if (r == CPLRT_INVALID) {
                            CPLRT(sbuf_puts)(&b, "{\"foreign_error\":");
                            CPLRT(sbuf_put)(&b, foreign.ptr, foreign.len);
                            CPLRT(sbuf_putc)(&b, '}');
                        } else {
                            /* a fault of the binding layer (binding_protocol, memory): the C adapter broke its
                               contract in the host, which the client sees as a garbled answer */
                            CPLRT(sbuf_puts)(&b, "{\"fault\":true}");
                        }
                        worker_main_send(out, &b);
                        CPLRT(sbuf_free)(&b);
                        CPLRT(sbuf_free)(&foreign);
                        cplrt_release(res);
                        CPLRT(cx_end)(&cx);
                    }
                    CPLRT(words_release)(args, n);
                }
            }
        } else if (strcmp(name, "release") == 0) {
            if (cap != NULL) {
                CPLRT(cap_unref)(cap);
                cap = NULL;
            }
            static const char okn[] = "{\"ok\":null}\n";
            (void)!write(out, okn, sizeof okn - 1u);
        } else if (strcmp(name, "shutdown") == 0) {
            static const char okn[] = "{\"ok\":null}\n";
            (void)!write(out, okn, sizeof okn - 1u);
            CPLRT(json_free)(req);
            break;
        } else {
            worker_main_invalid(out, rt, "/op", "an unknown request");
        }
        CPLRT(json_free)(req);
    }
    if (cap != NULL)
        CPLRT(cap_unref)(cap);
    free(reader.buf);
    cpl_error *e = NULL;
    (void)prt->api->cpl_runtime_close(prt, CPL_CLOSE_FORCE, &e);
    if (e != NULL)
        e->api->cpl_error_release(e);
    return code;
}
