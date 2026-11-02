//! Worker failure and lifecycle regressions (spec/python.md 6.1-6.4).
//! The fake interpreter has its own alarm so a broken client fails instead of hanging the test runner.

use cpl_cabi::worker::{Worker, WorkerAnswer, WorkerConfig, WorkerDeath};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// Leave enough room to start CPython while the workspace's compiler/sanitizer suites saturate the machine.
const LIMIT: Duration = Duration::from_millis(1500);
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const FAKE: &str = r#"
import ast, json, os, re, signal, socket, sys, time
signal.alarm(12)
mode = os.environ['CPL_TEST_WORKER_MODE']
with open(os.environ['CPL_TEST_WORKER_PID'], 'w') as f:
    f.write(str(os.getpid()))
if mode == 'start_exit':
    os._exit(3)
if mode == 'source_stall':
    time.sleep(30)
reader = sys.stdin.buffer
size = int(reader.readline())
source = reader.read(size).decode('utf-8')
match = re.search(r'^PROGRAM = (.+)$', source, re.M)
digest = ast.literal_eval(match[1]) if match else 'a' * 64
def send(value):
    sys.stdout.buffer.write(json.dumps(value, separators=(',', ':')).encode() + b'\n')
    sys.stdout.buffer.flush()
if mode == 'wrong_banner':
    digest = '0' * 64
send({'protocol': 1, 'worker': 'python', 'program': digest, 'runtime': {'python': 'test'}})
json.loads(reader.readline())
send({'handle': 0})
if mode == 'write_stall':
    time.sleep(30)
for line in reader:
    req = json.loads(line)
    if req['op'] == 'release':
        if mode == 'shutdown_stall':
            time.sleep(30)
        send({'ok': None})
    elif req['op'] == 'shutdown':
        send({'ok': None})
        break
    elif mode == 'timeout':
        time.sleep(30)
    elif mode == 'closed_channel':
        socket.socket(fileno=os.dup(0)).shutdown(socket.SHUT_RDWR)
        time.sleep(30)
    elif mode == 'trickle':
        for byte in b'{"ok":{"nodes":[["i","7"]]}}\n':
            os.write(1, bytes([byte]))
            time.sleep(0.08)
    elif mode == 'invalid_utf8':
        os.write(1, b'\xff\n')
    elif mode == 'invalid_kind':
        send({'foreign_error': {'kind': 'Bogus', 'message': 'bad kind', 'details': []}})
    elif mode == 'invalid_nodes':
        send({'ok': {'nodes': [['l', 0]]}})
    elif mode == 'garbled':
        os.write(1, b'not json\n')
    elif mode == 'ambiguous':
        send({'ok': {'nodes': [['i', '7']]}, 'foreign_error': {'kind': 'Exception', 'message': 'ambiguous'}})
    else:
        send({'ok': {'nodes': [['i', '7']]}})
"#;

struct Fake {
    root: PathBuf,
    config: WorkerConfig,
}

impl Fake {
    fn new(mode: &str) -> Option<Self> {
        let python = std::env::var_os("CPL_PYTHON").unwrap_or_else(|| "python3".into());
        let probe = Command::new(python)
            .args([
                "-c",
                "import sys; assert sys.version_info >= (3, 10); print(sys.executable)",
            ])
            .output();
        let Some(probe) = probe.ok().filter(|o| o.status.success()) else {
            assert_ne!(
                std::env::var("CPL_REQUIRE_PYTHON").as_deref(),
                Ok("1"),
                "CPython 3.10+ required"
            );
            println!("skipped: no CPython 3.10+ (set CPL_REQUIRE_PYTHON=1 to fail)");
            return None;
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "worker-{mode}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("fake.py");
        std::fs::write(&script, FAKE).unwrap();
        let executable = root.join("python");
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nexec {} {}\n",
                quote(String::from_utf8(probe.stdout).unwrap().trim()),
                quote(script.to_str().unwrap())
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = WorkerConfig {
            python: executable,
            env: vec![
                ("CPL_TEST_WORKER_MODE".into(), mode.into()),
                (
                    "CPL_TEST_WORKER_PID".into(),
                    root.join("pid").display().to_string(),
                ),
            ],
        };
        Some(Self { root, config })
    }

    fn start(&self) -> Worker {
        Worker::start_python("# test", DIGEST, "service", &self.config, Some(LIMIT)).unwrap()
    }

    fn assert_reaped(&self) {
        let pid = std::fs::read_to_string(self.root.join("pid")).unwrap();
        assert!(
            !Path::new("/proc").join(pid.trim()).exists(),
            "worker {pid} is still alive or a zombie"
        );
    }
}

#[test]
fn rust_worker_bounds_source_upload_and_reaps_start_failures() {
    for mode in ["source_stall", "wrong_banner"] {
        let Some(fake) = Fake::new(mode) else { return };
        let started = Instant::now();
        let result = Worker::start_python(
            &"#".repeat(2 * 1024 * 1024),
            DIGEST,
            "service",
            &fake.config,
            Some(LIMIT),
        );
        assert!(result.is_err(), "{mode}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{mode}: {:?}",
            started.elapsed()
        );
        fake.assert_reaped();
    }
}

#[test]
fn rust_worker_observes_exit_before_the_banner_without_waiting_for_timeout() {
    let Some(fake) = Fake::new("start_exit") else {
        return;
    };
    let started = Instant::now();
    let error = Worker::start_python(
        "# test",
        DIGEST,
        "service",
        &fake.config,
        Some(Duration::from_secs(8)),
    )
    .err()
    .expect("startup must fail");
    assert!(
        error.notes.iter().any(|n| n.message.contains("status 3")),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "startup took {:?}",
        started.elapsed()
    );
    fake.assert_reaped();
}

#[test]
fn rust_worker_bounds_the_whole_request_including_partial_io() {
    for mode in ["write_stall", "trickle", "timeout", "closed_channel"] {
        let Some(fake) = Fake::new(mode) else { return };
        let mut worker = fake.start();
        let arg = cpl_cabi::wire::to_nodes(&cpl_eval::Value::text(&"x".repeat(2 * 1024 * 1024)));
        let started = Instant::now();
        let death = worker.call(0, &[arg]).unwrap_err();
        if mode == "closed_channel" {
            assert!(matches!(death, WorkerDeath::Ended(_)), "{death:?}");
        } else {
            assert_eq!(
                death,
                WorkerDeath::Timeout(LIMIT.as_millis() as u64),
                "{mode}"
            );
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{mode}: {:?}",
            started.elapsed()
        );
        assert_eq!(worker.call(0, &[]).unwrap_err(), death);
        fake.assert_reaped();
    }
}

#[test]
fn rust_worker_rejects_malformed_responses_without_panicking() {
    for mode in ["invalid_utf8", "invalid_kind", "garbled", "ambiguous"] {
        let Some(fake) = Fake::new(mode) else { return };
        let mut worker = fake.start();
        assert!(
            matches!(worker.call(0, &[]), Err(WorkerDeath::Protocol(_))),
            "{mode}"
        );
        fake.assert_reaped();
    }
}

#[test]
fn rust_worker_shutdown_has_one_grace_period_and_is_idempotent() {
    let Some(fake) = Fake::new("shutdown_stall") else {
        return;
    };
    let mut worker = fake.start();
    assert!(matches!(worker.call(0, &[]), Ok(WorkerAnswer::Ok(_))));
    let started = Instant::now();
    worker.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "shutdown took {:?}",
        started.elapsed()
    );
    fake.assert_reaped();
    let started = Instant::now();
    worker.shutdown();
    drop(worker);
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn rust_worker_without_a_timeout_can_call_and_close_cleanly() {
    let Some(fake) = Fake::new("ok") else { return };
    let mut worker = Worker::start_python("# test", DIGEST, "service", &fake.config, None).unwrap();
    assert!(matches!(worker.call(0, &[]), Ok(WorkerAnswer::Ok(_))));
    worker.shutdown();
    fake.assert_reaped();
}

#[test]
fn both_worker_clients_report_and_remember_failures() {
    use cpl_cabi::worker::WorkerAdapter;
    use cpl_eval::{CapabilityImpl, EvalConfig, HostContext, Value};

    let Some(setup) = Fake::new("ok") else { return };
    if cpl_rt::testing::find_cc().is_none() {
        assert_ne!(
            std::env::var("CPL_REQUIRE_CC").as_deref(),
            Ok("1"),
            "C compiler required"
        );
        println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
        return;
    }
    std::fs::create_dir_all(setup.root.join("src")).unwrap();
    std::fs::write(
        setup.root.join("cpl.toml"),
        r#"
[project]
name = "worker"
edition = "0.1"
[interop]
targets = ["c", "python"]
[bindings.service]
capability = "worker.Service"
backend = "python"
adapter = "unused"
timeout_ms = 1500
"#,
    )
    .unwrap();
    std::fs::write(
        setup.root.join("src/worker.cpl"),
        r#"
module worker
use std.foreign.{ForeignError}

pub capability Service {
    fn exchange(s: Text) -> Result<Int, ForeignError>
    fn plain() -> Int
}

pub fn exchange(s: Text, using service: Service) -> Result<Int, ForeignError>
    effects { service.exchange }
{
    service.exchange(s)
}

pub fn plain(using service: Service) -> Int
    effects { service.plain }
{
    service.plain()
}
"#,
    )
    .unwrap();
    let project =
        cpl_project::load_project(&setup.root, &cpl_project::LoadOptions::default()).unwrap();
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let p = &checked.program;
    let plan = checked.interop.as_ref().unwrap();
    let find = |name| p.defs().find(|d| p.display_path(d.id) == name).unwrap().id;
    let capability = find("worker.Service");
    let function = find("worker.exchange");
    let plain = find("worker.plain");
    let ty = &p.def(function).kind.as_fn().unwrap().ret;
    let model = cpl_emit_c::python::PyModel::new(p, &project.manifest, &[function, plain]);
    let sanitizers = [
        cpl_emit_c::cc::Sanitizer::Address,
        cpl_emit_c::cc::Sanitizer::Undefined,
    ];
    let sanitized =
        cpl_emit_c::cc::Compiler::discover(None, None).is_ok_and(|cc| cc.supports(&sanitizers));
    if !sanitized {
        assert_ne!(
            std::env::var("CPL_REQUIRE_SANITIZERS").as_deref(),
            Ok("1"),
            "ASan and UBSan required"
        );
        println!("skipped: worker sanitizers unavailable (set CPL_REQUIRE_SANITIZERS=1 to fail); running unsanitized");
    }
    let config = cpl_emit_c::package::DriverConfig {
        cache_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("workers-cache"),
        profile: cpl_emit_c::cc::Profile::Debug,
        sanitize: if sanitized {
            sanitizers.to_vec()
        } else {
            vec![]
        },
        coverage: false,
        cc: None,
        runtime: cpl_rt::RuntimeSource::Embedded,
        werror: true,
    };
    let mut native =
        cpl_cabi::NativeSession::start(p, &project, plan, &EvalConfig::default(), &config).unwrap();
    native.set_request_timeout(Some(Duration::from_secs(20)));
    for mode in [
        "timeout",
        "write_stall",
        "trickle",
        "closed_channel",
        "invalid_utf8",
        "invalid_kind",
        "invalid_nodes",
        "garbled",
        "ambiguous",
    ] {
        let fake = Fake::new(mode).unwrap();
        let worker = Worker::start_python(
            &model.worker_source(),
            &model.digest,
            "service",
            &fake.config,
            Some(LIMIT),
        )
        .unwrap();
        let mut adapter = WorkerAdapter::new(worker, capability);
        let args = vec![Value::text(&"x".repeat(if mode == "write_stall" {
            2 * 1024 * 1024
        } else {
            1
        }))];
        let mut ctx = HostContext { program: p };
        let started = Instant::now();
        let first = adapter.call("exchange", args.clone(), &mut ctx);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "eval {mode}: {:?}",
            started.elapsed()
        );
        fake.assert_reaped();
        let second = adapter.call("exchange", args.clone(), &mut ctx).unwrap();
        let second_text = cpl_eval::value_text(p, &second, ty);
        assert!(
            second_text.contains("kind: Transport"),
            "{mode}: {second_text}"
        );
        let plain_fault = adapter
            .call("plain", vec![], &mut ctx)
            .unwrap_err()
            .to_diagnostic(p);
        assert!(
            plain_fault.message.contains("binding_host_failed"),
            "{mode}: {plain_fault:?}"
        );

        native.configure_workers(&fake.config).unwrap();
        if mode == "timeout" {
            let invalid = WorkerConfig {
                python: fake.root.join("missing-python"),
                env: vec![("BAD=KEY".into(), "value".into())],
            };
            assert!(native.configure_workers(&invalid).is_err());
            // The rejected update must not replace the valid interpreter or environment used by this grant.
        }
        let handle = native.bind("service").unwrap();
        let started = Instant::now();
        let actual = native.call(function, &args, &[handle], true).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "native {mode}: {:?}",
            started.elapsed()
        );
        fake.assert_reaped();
        match (first, actual) {
            (Ok(a), Ok(b)) => assert_eq!(
                cpl_eval::value_text(p, &a, ty),
                cpl_eval::value_text(p, &b, ty),
                "{mode}"
            ),
            (Err(a), Err(b)) => assert_eq!(
                a.to_diagnostic(p).message,
                b.to_diagnostic(p).message,
                "{mode}"
            ),
            (a, b) => panic!("{mode}: eval {a:?}, native {b:?}"),
        }
        let later = native
            .call(function, &args, &[handle], true)
            .unwrap()
            .unwrap();
        assert_eq!(
            second_text,
            cpl_eval::value_text(p, &later, ty),
            "{mode}: subsequent call"
        );
        let later_plain = native
            .call(plain, &[], &[handle], true)
            .unwrap()
            .unwrap_err();
        assert_eq!(
            plain_fault.message,
            later_plain.to_diagnostic(p).message,
            "{mode}: subsequent plain call"
        );
    }
    for mode in ["source_stall", "wrong_banner", "start_exit"] {
        let fake = Fake::new(mode).unwrap();
        native.configure_workers(&fake.config).unwrap();
        let started = Instant::now();
        assert!(native.bind("service").is_err(), "native {mode}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "native {mode}: {:?}",
            started.elapsed()
        );
        fake.assert_reaped();
    }
    let fake = Fake::new("shutdown_stall").unwrap();
    native.configure_workers(&fake.config).unwrap();
    native.bind("service").unwrap();
    let started = Instant::now();
    let report = native.shutdown().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "native shutdown: {:?}",
        started.elapsed()
    );
    fake.assert_reaped();
    assert_eq!(
        (
            report.live_handles,
            report.live_caps,
            report.live_objects,
            report.live_buffers
        ),
        (0, 0, 0, 0)
    );
    assert!(
        report.exit_status.success(),
        "driver shutdown: {:?}",
        report.exit_status
    );
}
