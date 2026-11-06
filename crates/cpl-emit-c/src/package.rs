//! Building packages and drivers (`spec/toolchain.md` 1.13, 7.2, 11.2; `spec/cabi.md` 1.5, 1.6, 3.8, 9). Owner: the
//! build track (D).

use crate::cc;
use crate::InteropPlan;
pub use cpl_diag::json::BuildOutput;
use cpl_diag::json::Digest;
use cpl_diag::Diagnostic;
use cpl_hir::Program;
pub use cpl_project::ArtifactKind;
use cpl_project::Project;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The configuration of one `cpl build --target c` (toolchain 7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildConfig {
    /// `--out`, default `<project>/.cpl/build/c`.
    pub out_dir: PathBuf,
    /// The per-user cache of `cpl build` (toolchain 7.2): `<user cache>/cpl/<project key>/c`, never in the project.
    pub cache_dir: PathBuf,
    pub profile: cc::Profile,
    pub sanitize: Vec<cc::Sanitizer>,
    /// cpl_project's (toolchain 11.1): `--artifact`, else `[interop.c] artifacts`.
    pub artifacts: Vec<ArtifactKind>,
    /// The lock comparisons of toolchain 11.2 (the release check ran first, toolchain 7.2).
    pub release: bool,
    /// Write the lock after a successful build (toolchain 11.2).
    pub lock: bool,
    /// `--cc`.
    pub cc: Option<PathBuf>,
    /// At least 1.
    pub jobs: usize,
    pub runtime: cpl_rt::RuntimeSource,
    /// Adds `-Werror` (`cpl_rt::flags::WERROR`) to the compiles of the runtime and the emitted code; never set by the
    /// CLI, always by the library tests (toolchain 12.8).
    pub werror: bool,
}

/// The Python target's configuration (`spec/python.md` 1): the interpreter whose `sysconfig` gives the headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonConfig {
    /// `--python`, else `CPL_PYTHON`, else `python3` (searched on `PATH` when it has no `/`).
    pub interpreter: PathBuf,
}

/// What the probe of an interpreter found (`spec/python.md` 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonProbe {
    /// `sys.executable`.
    pub executable: String,
    /// `platform.python_version()`.
    pub version: String,
    /// `sysconfig.get_paths()["include"]`, which holds `Python.h`.
    pub include: PathBuf,
}

/// Probes an interpreter for `--target python`: CPython 3.10 or later whose include directory has `Python.h`, else
/// `build_python_unavailable` (`interpreter`, `reason`).
pub fn probe_python(interpreter: &std::path::Path) -> Result<PythonProbe, Diagnostic> {
    let shown = interpreter.display().to_string();
    let fail = |reason: String| {
        Diagnostic::error(
            crate::codes::BUILD_PYTHON_UNAVAILABLE,
            cpl_diag::Category::Build,
            format!("no usable Python for --target python: {shown}: {reason}"),
        )
        .with_data("interpreter", serde_json::json!(shown))
        .with_data("reason", serde_json::json!(reason))
    };
    const PROBE: &str = "import json,os,platform,sys,sysconfig\n\
inc=sysconfig.get_paths()['include']\n\
print(json.dumps({'impl':sys.implementation.name,'major':sys.version_info[0],'minor':sys.version_info[1],\
'version':platform.python_version(),'executable':sys.executable,'include':inc,\
'header':os.path.isfile(os.path.join(inc,'Python.h'))}))\n";
    let output = std::process::Command::new(interpreter)
        .args(["-I", "-c", PROBE])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| fail(format!("it cannot be run: {e}")))?;
    if !output.status.success() {
        return Err(fail(format!(
            "the probe failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let v: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| fail("it did not answer the probe".to_string()))?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    if text("impl") != "cpython" {
        return Err(fail(format!("it is {}, not CPython", text("impl"))));
    }
    if (num("major"), num("minor")) < (3, 10) {
        return Err(fail(format!(
            "it is Python {}, older than 3.10",
            text("version")
        )));
    }
    if v.get("header").and_then(|x| x.as_bool()) != Some(true) {
        return Err(fail(format!(
            "{} has no Python.h (install the development headers)",
            text("include")
        )));
    }
    Ok(PythonProbe {
        executable: text("executable"),
        version: text("version"),
        include: PathBuf::from(text("include")),
    })
}

/// The compile flags the extension unit adds (`spec/python.md` 2), the include directory passed as `-isystem` so
/// that no warning of the interpreter's headers counts as the toolchain's.
fn python_unit_flags(probe: &PythonProbe) -> Vec<String> {
    vec![
        "-DPy_LIMITED_API=0x030A0000".to_string(),
        "-isystem".to_string(),
        probe.include.display().to_string(),
    ]
}

/// The extension's link flags (`spec/python.md` 2): the shared link's without `-Wl,-z,defs`, since the interpreter
/// provides libpython's symbols at load time.
const PYTHON_LINK: &[&str] = &[
    "-shared",
    "-Wl,-z,now",
    "-Wl,--gc-sections",
    "-Wl,-Bsymbolic",
    "-Wl,--trace",
];

/// A successful build: its `BuildOutput`, also written as `<out>/build.json`, and its warnings and infos.
pub struct Built {
    pub output: BuildOutput,
    pub diagnostics: Vec<Diagnostic>,
    /// The C entries of `cpl.lock` this build determines, which `cpl build --lock` writes (toolchain 11.2).
    pub lock: LockEntries,
}

/// Compiler discovery and the target check, emission, compilation (in parallel, with `-MD` depfiles, each object
/// cached and reused by the rule of `spec/cabi.md` 1.6), linking, and `build.json`; then, with `release`, the lock
/// comparisons and, with `lock`, the lock write of toolchain 11.2. Builds that share a `cache_dir` may run at once,
/// the parallel tests of one test binary among them, because every cache entry is written atomically (cabi.md 1.6).
/// Err holds the failure's diagnostics (the `build_*` codes and `internal_error` kinds of toolchain Appendix B) after
/// the warnings found before it.
pub fn build(
    project: &Project,
    p: &Program,
    plan: &InteropPlan,
    cfg: &BuildConfig,
) -> Result<Built, Vec<Diagnostic>> {
    build_target(project, p, plan, cfg, None)
}

/// `cpl build --target python` (`spec/python.md` 1): the steps of [`build`] up to the link, then the extension
/// `<package>/_native.abi3.so` (Section 2), the package's modules and stubs (Section 3), the C adapter host when a
/// backend-c binding runs in a worker (Section 6.6), and the wheel (Section 1.1); `build.json` has `target` `"python"`
/// and the `python` object, and [`Built::lock`]'s `python` holds `targets.python` and the Python adapters (Section 9).
/// `cfg.artifacts` does not apply. A missing or unusable interpreter is `build_python_unavailable`.
pub fn build_python(
    project: &Project,
    p: &Program,
    plan: &InteropPlan,
    cfg: &BuildConfig,
    py: &PythonConfig,
) -> Result<Built, Vec<Diagnostic>> {
    build_target(project, p, plan, cfg, Some(py))
}

fn build_target(
    project: &Project,
    p: &Program,
    plan: &InteropPlan,
    cfg: &BuildConfig,
    py: Option<&PythonConfig>,
) -> Result<Built, Vec<Diagnostic>> {
    let started = std::time::Instant::now();
    let mut diagnostics = Vec::new();
    let manifest_cc = project
        .manifest
        .interop_c
        .as_ref()
        .and_then(|c| c.compiler.as_deref());
    let compiler = cc::Compiler::discover(cfg.cc.as_deref(), manifest_cc).map_err(|d| vec![d])?;
    // the platform matrix (cabi.md 1.7)
    if compiler.target.starts_with("aarch64-") && compiler.target.contains("linux") {
        diagnostics.push(
            Diagnostic::warning(
                crate::codes::BUILD_TARGET_UNTESTED,
                cpl_diag::Category::Build,
                format!(
                    "the target {} is built but untested in this edition",
                    compiler.target
                ),
            )
            .with_data("target", serde_json::json!(compiler.target)),
        );
    } else if !(compiler.target.starts_with("x86_64-") && compiler.target.contains("linux")) {
        diagnostics.push(
            Diagnostic::error(
                crate::codes::BUILD_TARGET_UNSUPPORTED,
                cpl_diag::Category::Build,
                format!(
                    "the target {} is outside the platform matrix of edition 0.1",
                    compiler.target
                ),
            )
            .with_data("target", serde_json::json!(compiler.target)),
        );
        return Err(diagnostics);
    }
    if !cfg.sanitize.is_empty() && !compiler.supports(&cfg.sanitize) {
        let names: Vec<&str> = cfg.sanitize.iter().map(|s| s.as_str()).collect();
        diagnostics.push(
            Diagnostic::error(
                crate::codes::BUILD_SANITIZER_UNAVAILABLE,
                cpl_diag::Category::Build,
                format!(
                    "the compiler cannot link the sanitizers {}",
                    names.join(",")
                ),
            )
            .with_data("sanitizer", serde_json::json!(names.join(","))),
        );
        return Err(diagnostics);
    }
    let probe = match py {
        Some(py) => match probe_python(&py.interpreter) {
            Ok(probe) => Some(probe),
            Err(d) => {
                diagnostics.push(d);
                return Err(diagnostics);
            }
        },
        None => None,
    };
    let package = project.manifest.python_package();
    let ecfg = crate::EmitConfig {
        optimize: cfg.profile == cc::Profile::Optimized,
        with_tests: false,
        coverage: false,
        release: cfg.release,
        runtime: cfg.runtime.clone(),
        target: compiler.target.clone(),
        compiler: compiler.name(),
        profile: cfg.profile.as_str().to_string(),
    };
    let pkg = crate::emit(p, project, plan, &ecfg)?;
    let prefix = crate::unit_prefix(plan);
    let runtime_digest = cpl_rt::runtime_digest(&cfg.runtime)
        .map_err(|e| vec![internal(&format!("cannot read the runtime: {e}"))])?;
    let out = &cfg.out_dir;
    let io = |e: std::io::Error| {
        vec![io_error(&format!(
            "cannot write the package to {}: {e}",
            out.display()
        ))]
    };
    for sub in ["src", "include", "obj", "lib"] {
        let _ = std::fs::remove_dir_all(out.join(sub));
    }
    let _ = std::fs::remove_dir_all(out.join("bin"));
    if probe.is_some() {
        let _ = std::fs::remove_dir_all(out.join(&package));
        let _ = std::fs::remove_dir_all(out.join("dist"));
    }
    std::fs::create_dir_all(out).map_err(io)?;
    // the Python target links no library and no executable of its own (spec/python.md 1)
    let want_exe = probe.is_none() && cfg.artifacts.contains(&ArtifactKind::Executable);
    // the C adapter host of the backend-c bindings that run in a worker (spec/python.md 6.6)
    let want_worker = plan.bindings.iter().any(|b| b.worker);
    let mut files: Vec<(String, String)> =
        cpl_rt::amalgamation(&prefix, &cfg.runtime).map_err(io)?;
    files.retain(|(rel, _)| {
        !rel.ends_with("cpl_rt_driver.c")
            && (want_exe || want_worker || !rel.ends_with("cpl_rt_exe.c"))
    });
    files.extend(pkg.files.iter().map(|f| (f.path.clone(), f.text.clone())));
    let python_unit = format!("src/{prefix}_python.c");
    if probe.is_some() {
        files.push((python_unit.clone(), crate::python::extension_c(&prefix)));
        files.push((
            format!("src/{prefix}_python.map"),
            "{\n  global:\n    PyInit__native;\n  local:\n    *;\n};\n".to_string(),
        ));
    }
    // the version script: the exact exported names (cabi.md 1.6, 9.1)
    let mut symbols: Vec<String> = vec![
        format!("{prefix}_runtime_open"),
        format!("{prefix}_library_info"),
    ];
    symbols.extend(plan.exports.iter().map(|e| e.symbol.clone()));
    symbols.extend(
        plan.types
            .iter()
            .map(|t| format!("{prefix}_type_{}", t.c_name)),
    );
    for cap in crate::api::listed_capabilities(p, plan) {
        symbols.push(format!(
            "{prefix}_{}_new",
            cpl_sema::interop::snake_case(&p.def(cap).name.display())
        ));
    }
    symbols.extend(
        plan.bindings
            .iter()
            .map(|b| format!("{prefix}_bind_{}", b.name))
            .chain(
                plan.python_bindings
                    .iter()
                    .map(|b| format!("{prefix}_bind_{}", b.name)),
            ),
    );
    symbols.sort();
    symbols.dedup();
    let node = format!("{}_{}", prefix.to_ascii_uppercase(), plan.abi);
    let mut map = format!("{node} {{\n  global:\n");
    for s in &symbols {
        map.push_str(&format!("    {s};\n"));
    }
    map.push_str("  local:\n    *;\n};\n");
    files.push((format!("src/{prefix}.map"), map));
    for (rel, text) in &files {
        let path = out.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).map_err(io)?;
        std::fs::write(&path, text).map_err(io)?;
    }
    // compile every unit (cabi.md 1.5): the base set, the profile or the sanitizers
    let mut flags: Vec<String> = cpl_rt::flags::BASE.iter().map(|s| s.to_string()).collect();
    if cfg.sanitize.is_empty() {
        flags.extend(cfg.profile.flags().iter().map(|s| s.to_string()));
    } else {
        flags.extend(
            cc::sanitizer_flags(&cfg.sanitize)
                .iter()
                .map(|s| s.to_string()),
        );
    }
    if cfg.werror {
        flags.push(cpl_rt::flags::WERROR.to_string());
    }
    flags.push(format!("-ffile-prefix-map={}=.", out.display()));
    flags.push(format!("-I{}", out.join("src").display()));
    flags.push(format!("-I{}", out.join("include").display()));
    let units: Vec<String> = files
        .iter()
        .map(|(r, _)| r.clone())
        .filter(|r| r.ends_with(".c") && !is_wrapper(plan, r))
        .collect();
    let jobs = cfg.jobs.max(1);
    let python_flags: Vec<String> = match &probe {
        Some(probe) => flags
            .iter()
            .cloned()
            .chain(python_unit_flags(probe))
            .collect(),
        None => Vec::new(),
    };
    // each object through the cache of cabi.md 1.6 (crate::cache), with its depfile
    let obj_cache = cfg.cache_dir.join("obj");
    let objects: Vec<Result<(String, PathBuf, bool), Diagnostic>> = std::thread::scope(|scope| {
        let mut results = Vec::new();
        for chunk in units.chunks(units.len().div_ceil(jobs).max(1)) {
            let compiler = &compiler;
            let base_flags = &flags;
            let python_flags = &python_flags;
            let python_unit = &python_unit;
            let out = out.clone();
            let obj_cache = &obj_cache;
            results.push(scope.spawn(move || {
                chunk
                    .iter()
                    .map(|rel| {
                        let stem = rel.trim_start_matches("src/");
                        let obj = out.join("obj").join(format!("{stem}.o"));
                        let dep = out.join("obj").join(format!("{stem}.d"));
                        let _ = std::fs::create_dir_all(obj.parent().expect("a parent"));
                        let flags = if rel == python_unit {
                            python_flags
                        } else {
                            base_flags
                        };
                        match crate::cache::compile(
                            Some(obj_cache),
                            compiler,
                            flags,
                            &out.join(rel),
                            &obj,
                            &dep,
                        ) {
                            Ok(reused) => Ok((rel.clone(), obj, reused)),
                            Err(stderr) => Err(rejected(rel, &stderr)),
                        }
                    })
                    .collect::<Vec<_>>()
            }));
        }
        results
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut objs = Vec::new();
    let mut exe_obj = None;
    let mut python_obj = None;
    let mut every_object_reused = true;
    for o in objects {
        match o {
            Ok((rel, path, reused)) => {
                every_object_reused &= reused;
                if rel.ends_with("cpl_rt_exe.c") {
                    exe_obj = Some(path);
                } else if rel == python_unit {
                    python_obj = Some(path);
                } else {
                    objs.push(path);
                }
            }
            Err(d) => diagnostics.push(d),
        }
    }
    if diagnostics
        .iter()
        .any(|d| d.severity == cpl_diag::Severity::Error)
    {
        return Err(diagnostics);
    }
    let profile_flags: Vec<String> = if cfg.sanitize.is_empty() {
        cfg.profile.flags().iter().map(|s| s.to_string()).collect()
    } else {
        cc::sanitizer_flags(&cfg.sanitize)
            .iter()
            .map(|s| s.to_string())
            .collect()
    };
    let adapter_headers = match compile_adapters(
        &compiler,
        plan,
        project,
        &profile_flags,
        out,
        &out.join("obj"),
        Some(&obj_cache),
    ) {
        Ok(more) => {
            objs.extend(more.objs);
            every_object_reused &= more.reused;
            more.headers
        }
        Err(ds) => {
            diagnostics.extend(ds);
            return Err(diagnostics);
        }
    };
    let (_, binding_link) = adapter_units(project, plan, out);
    if probe.is_none() {
        std::fs::create_dir_all(out.join("lib")).map_err(io)?;
    }
    let san: Vec<&str> = cc::sanitizer_flags(&cfg.sanitize);
    let mut artifacts: Vec<cpl_diag::json::BuildArtifact> = Vec::new();
    // the trace of the final link (the shared library's, else the executable's): what it opened (cabi.md 6.6)
    let mut trace: Option<String> = None;
    let link = |args: &mut std::process::Command, what: &str| -> Result<String, Diagnostic> {
        match args.output() {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).to_string()),
            Ok(o) => Err(link_error(
                project,
                what,
                &String::from_utf8_lossy(&o.stderr),
            )),
            Err(e) => Err(link_failed(what, &e.to_string())),
        }
    };
    let mut ldflags: Vec<String> = binding_link.clone();
    ldflags.extend(
        project
            .manifest
            .interop_c
            .as_ref()
            .map(|c| c.ldflags.clone())
            .unwrap_or_default(),
    );
    if let (Some(_), Some(ext_obj)) = (&probe, &python_obj) {
        // the extension (spec/python.md 2): the program's objects and the extension unit, PyInit__native alone exported
        let dir = out.join(&package);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let so = dir.join("_native.abi3.so");
        let mut c = cc::command(&compiler.path);
        c.args(&san)
            .args(PYTHON_LINK)
            .arg(format!(
                "-Wl,--version-script={}",
                out.join("src")
                    .join(format!("{prefix}_python.map"))
                    .display()
            ))
            .args(&objs)
            .arg(ext_obj)
            .arg("-o")
            .arg(&so)
            .args(&ldflags)
            .arg("-lpthread");
        match link(&mut c, "the Python extension") {
            Ok(stdout) => trace = Some(stdout),
            Err(d) => {
                diagnostics.push(d);
                return Err(diagnostics);
            }
        }
    }
    if probe.is_none() && cfg.artifacts.contains(&ArtifactKind::Shared) {
        let so = out.join("lib").join(format!("lib{prefix}.so.{}", plan.abi));
        let mut c = cc::command(&compiler.path);
        c.args(&san)
            .args(cpl_rt::flags::SHARED_LINK)
            .arg(format!("-Wl,-soname,lib{prefix}.so.{}", plan.abi))
            .arg(format!(
                "-Wl,--version-script={}",
                out.join("src").join(format!("{prefix}.map")).display()
            ))
            .args(&objs)
            .arg("-o")
            .arg(&so)
            .args(&ldflags)
            .arg("-lpthread");
        match link(&mut c, "the shared library") {
            Ok(stdout) => trace = Some(stdout),
            Err(d) => {
                diagnostics.push(d);
                return Err(diagnostics);
            }
        }
        let link_path = out.join("lib").join(format!("lib{prefix}.so"));
        let _ = std::fs::remove_file(&link_path);
        let _ = std::os::unix::fs::symlink(format!("lib{prefix}.so.{}", plan.abi), &link_path);
        artifacts.push(artifact("shared_library", &so, project));
    }
    if probe.is_none() && cfg.artifacts.contains(&ArtifactKind::Static) {
        // one relocatable object keeping exactly the exported names global (cabi.md 1.6)
        let rel_obj = out.join("obj").join(format!("{prefix}.o"));
        let mut c = cc::command(&compiler.path);
        c.arg("-r")
            .arg("-nostdlib")
            .args(&objs)
            .arg("-o")
            .arg(&rel_obj);
        if let Err(d) = link(&mut c, "the static library's object") {
            diagnostics.push(d);
            return Err(diagnostics);
        }
        let tool = |name: &str| -> PathBuf {
            cc::command(&compiler.path)
                .arg(format!("-print-prog-name={name}"))
                .output()
                .ok()
                .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| PathBuf::from(name))
        };
        let keep = out.join("obj").join("keep.txt");
        std::fs::write(&keep, symbols.join("\n") + "\n").map_err(io)?;
        let mut oc = cc::command(&tool("objcopy"));
        oc.arg(format!("--keep-global-symbols={}", keep.display()))
            .arg(&rel_obj);
        if let Err(d) = link(&mut oc, "objcopy") {
            diagnostics.push(d);
            return Err(diagnostics);
        }
        let a = out.join("lib").join(format!("lib{prefix}.a"));
        let _ = std::fs::remove_file(&a);
        let mut ar = cc::command(&tool("ar"));
        ar.arg("rcsD").arg(&a).arg(&rel_obj);
        if let Err(d) = link(&mut ar, "ar") {
            diagnostics.push(d);
            return Err(diagnostics);
        }
        artifacts.push(artifact("static_library", &a, project));
    }
    if want_exe {
        let bin = out.join("bin").join(&prefix);
        std::fs::create_dir_all(out.join("bin")).map_err(io)?;
        let mut c = cc::command(&compiler.path);
        c.args(&san).args(&objs);
        if let Some(e) = &exe_obj {
            c.arg(e);
        }
        c.arg("-o").arg(&bin).args(&ldflags).arg("-lpthread");
        if trace.is_none() {
            c.arg("-Wl,--trace");
        }
        match link(&mut c, "the executable") {
            Ok(stdout) => {
                if trace.is_none() {
                    trace = Some(stdout);
                }
            }
            Err(d) => {
                diagnostics.push(d);
                return Err(diagnostics);
            }
        }
        artifacts.push(artifact("executable", &bin, project));
    }
    if want_worker {
        // the C adapter host (spec/python.md 6.6): the executable's objects, whose main serves CPL_WORKER_MODE=1
        let bin = match &probe {
            Some(_) => out.join(&package).join(format!("{prefix}-worker")),
            None => out.join("bin").join(format!("{prefix}-worker")),
        };
        std::fs::create_dir_all(bin.parent().expect("a parent")).map_err(io)?;
        let mut c = cc::command(&compiler.path);
        c.args(&san).args(&objs);
        if let Some(e) = &exe_obj {
            c.arg(e);
        }
        c.arg("-o").arg(&bin).args(&ldflags).arg("-lpthread");
        if let Err(d) = link(&mut c, "the C adapter host") {
            diagnostics.push(d);
            return Err(diagnostics);
        }
        if probe.is_none() {
            artifacts.push(artifact("worker", &bin, project));
        }
    }
    let mut python_info = None;
    if let Some(probe) = &probe {
        match write_python_package(project, p, plan, &compiler, out, &package) {
            Ok(written) => artifacts.extend(written),
            Err(e) => return Err(io(e)),
        }
        python_info = Some(cpl_diag::json::PythonBuildInfo {
            interpreter: probe.executable.clone(),
            version: probe.version.clone(),
            package: package.clone(),
            distribution_version: project.manifest.python_version(),
            tag: python_tag(&compiler),
        });
    } else {
        let mut headers: Vec<PathBuf> = std::fs::read_dir(out.join("include"))
            .map_err(io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        headers.sort();
        for h in headers {
            artifacts.push(artifact("header", &h, project));
        }
    }
    artifacts.sort_by(|a, b| a.path.cmp(&b.path));
    let exports = plan
        .exports
        .iter()
        .map(|e| {
            let d = p.def(e.function);
            let sig = crate::lockgen::signature_digest(p, &e.symbol, e.int, e.function).0;
            let mut effects: Vec<String> =
                match &d.kind.as_fn().and_then(|f| f.effects.declared.clone()) {
                    Some(set) => set
                        .iter()
                        .map(|r| {
                            let name = d
                                .kind
                                .locals()
                                .get(r.param.0 as usize)
                                .map(|l| l.name.clone())
                                .unwrap_or_default();
                            format!("{name}.{}", r.op)
                        })
                        .collect(),
                    None => Vec::new(),
                };
            effects.sort();
            cpl_diag::json::BuildExport {
                symbol: e.symbol.clone(),
                function: p.display_path(e.function),
                id: d.stable_id.clone(),
                signature: Digest(sig),
                int: format!("{:?}", e.int).to_ascii_lowercase(),
                effects,
            }
        })
        .collect();
    // every binding with a grant (spec/python.md 5.3, 6.6): backend c as the manifest isolates it; backend python in a
    // worker, except in process from the Python package when the manifest says so
    let mut bindings: Vec<cpl_diag::json::BuildBinding> = plan
        .bindings
        .iter()
        .map(|b| cpl_diag::json::BuildBinding {
            binding: b.name.clone(),
            capability: p.display_path(b.capability),
            form: b.form.as_str().to_string(),
            isolation: if b.worker { "worker" } else { "in_process" }.to_string(),
            grant: format!("{prefix}_bind_{}", b.name),
        })
        .chain(plan.python_bindings.iter().map(|b| {
            cpl_diag::json::BuildBinding {
                binding: b.name.clone(),
                capability: p.display_path(b.capability),
                form: "python".to_string(),
                isolation: if b.worker || probe.is_none() {
                    "worker"
                } else {
                    "in_process"
                }
                .to_string(),
                grant: format!("{prefix}_bind_{}", b.name),
            }
        }))
        .collect();
    bindings.sort_by(|a, b| a.binding.cmp(&b.binding));
    // the C entries of cpl.lock for this build (toolchain 11.2; cabi.md 1.5, 6.6, 9.3)
    let mut lock_entries = lock_entries(
        project,
        p,
        plan,
        &compiler,
        cfg,
        &runtime_digest,
        &pkg.exports_digest,
        &symbols,
        &adapter_headers,
        trace.as_deref().unwrap_or(""),
        out,
    );
    if let Some(probe) = &probe {
        lock_entries.python = Some(python_lock_entries(
            project,
            plan,
            &compiler,
            cfg,
            &runtime_digest,
            probe,
            &out.join("obj").join(format!("{prefix}_python.c.d")),
        ));
    }
    let mut lock_state = cpl_diag::json::BuildLock::default();
    if cfg.release && probe.is_some() {
        if let (Some(lock), Some(entries)) = (&project.lock, &lock_entries.python) {
            let found = crate::lockgen::compare_release_python(lock, entries);
            lock_state.checked = true;
            if !found.is_empty() {
                diagnostics.extend(found);
                return Err(diagnostics);
            }
        }
    } else if cfg.release {
        // after the build: build_abi_changed and build_lock_mismatch (a missing lock is the CLI's build_lock_missing)
        if let Some(lock) = &project.lock {
            let found =
                crate::lockgen::compare_release(lock, &lock_entries.target, &lock_entries.foreign);
            lock_state.checked = true;
            if !found.is_empty() {
                diagnostics.extend(found);
                return Err(diagnostics);
            }
        }
    }
    if cfg.lock && probe.is_none() {
        // T81: --lock never records an ABI break against the lock it replaces
        if let Some(lock) = &project.lock {
            let breaks = crate::lockgen::abi_changes(lock, &lock_entries.target);
            if !breaks.is_empty() {
                diagnostics.extend(breaks);
                return Err(diagnostics);
            }
        }
    }
    let inputs = build_inputs(project);
    let output = BuildOutput {
        target: if probe.is_some() { "python" } else { "c" }.to_string(),
        release: cfg.release,
        profile: cfg.profile.as_str().to_string(),
        sanitize: cfg
            .sanitize
            .iter()
            .map(|s| s.as_str().to_string())
            .collect(),
        out_dir: rel_path(out, project),
        compiler: cpl_diag::json::CompilerInfo {
            path: compiler.path.display().to_string(),
            family: match compiler.family {
                cc::Family::Gcc => "gcc".to_string(),
                cc::Family::Clang => "clang".to_string(),
            },
            version: compiler.version.clone(),
            target: compiler.target.clone(),
            digest: compiler.digest.clone(),
        },
        runtime: cpl_diag::json::RuntimeInfo {
            version: cpl_rt::RUNTIME_VERSION.to_string(),
            api: format!(
                "{}.{}",
                cpl_rtspec::limits::C_API_VERSION.0,
                cpl_rtspec::limits::C_API_VERSION.1
            ),
            source: match cfg.runtime {
                cpl_rt::RuntimeSource::Embedded => "embedded".to_string(),
                cpl_rt::RuntimeSource::Dir(_) => "dir".to_string(),
            },
            digest: runtime_digest,
        },
        program_digest: pkg.program_digest.clone(),
        exports_digest: pkg.exports_digest.clone(),
        exports,
        bindings,
        artifacts,
        inputs,
        holes: pkg.holes,
        lock: lock_state,
        cached: every_object_reused,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        python: python_info,
    };
    let json = serde_json::to_string_pretty(&output).expect("BuildOutput serializes");
    // replaced, never written through a link the project planted there
    cpl_diag::fsutil::write_atomic(&out.join("build.json"), (json + "\n").as_bytes()).map_err(io)?;
    Ok(Built {
        output,
        diagnostics,
        lock: lock_entries,
    })
}

/// The build inputs of the plan's backend-c bindings (toolchain Section 0, 7.2 `inputs`): each adapter file and
/// vendored source with its digest, in path order.
fn build_inputs(project: &Project) -> Vec<cpl_diag::json::BuildInput> {
    let Some(root) = &project.root else {
        return Vec::new();
    };
    let digests = cpl_project::c_input_digests(&project.manifest, root);
    let mut out = Vec::new();
    for (kind, by_binding) in [
        ("adapter", &digests.adapters),
        ("vendored_source", &digests.sources),
    ] {
        for (binding, files) in by_binding {
            for (path, digest) in files {
                out.push(cpl_diag::json::BuildInput {
                    kind: kind.to_string(),
                    binding: binding.clone(),
                    path: path.clone(),
                    digest: digest.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.binding.cmp(&b.binding)));
    out
}

/// The C entries of `cpl.lock` that a build determines (toolchain 11.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockEntries {
    /// `[targets] c`.
    pub target: cpl_project::TargetC,
    /// The backend-c entries of `[adapters]`, by binding.
    pub adapters: BTreeMap<String, cpl_project::AdapterC>,
    /// `[foreign] c`, by binding.
    pub foreign: BTreeMap<String, cpl_project::ForeignC>,
    /// Milestone 3, a Python build's (`spec/python.md` 9): `[targets] python` and the backend-python `[adapters]`.
    pub python: Option<PythonLockEntries>,
}

/// The Python entries of `cpl.lock` that `cpl build --target python` determines (`spec/python.md` 9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonLockEntries {
    pub target: cpl_project::TargetPython,
    /// By binding name.
    pub adapters: BTreeMap<String, cpl_project::AdapterPython>,
}

#[allow(clippy::too_many_arguments)]
fn lock_entries(
    project: &Project,
    p: &Program,
    plan: &InteropPlan,
    compiler: &cc::Compiler,
    cfg: &BuildConfig,
    runtime_digest: &Digest,
    exports_digest: &Digest,
    symbols: &[String],
    headers: &BTreeMap<String, BTreeSet<PathBuf>>,
    trace: &str,
    out: &std::path::Path,
) -> LockEntries {
    let opened = crate::lockgen::trace_files(trace);
    let root = project.root.clone().unwrap_or_default();
    let inputs = cpl_project::c_input_digests(&project.manifest, &root);
    // each binding's -l names: the first file of the trace named lib<name>.so or lib<name>.a
    let mut foreign = BTreeMap::new();
    let mut resolved: BTreeSet<PathBuf> = BTreeSet::new();
    for b in &plan.bindings {
        let c = project
            .manifest
            .binding_c
            .get(&b.name)
            .cloned()
            .unwrap_or_default();
        let mut libraries = BTreeMap::new();
        for l in &c.libraries {
            let names = [format!("lib{l}.so"), format!("lib{l}.a")];
            if let Some(f) = opened.iter().find(|f| {
                f.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| names.iter().any(|x| x == n))
            }) {
                resolved.insert(f.clone());
                libraries.insert(
                    l.clone(),
                    cpl_project::LockedLibrary {
                        path: crate::lockgen::lock_path(f, project),
                        digest: crate::lockgen::file_digest(f)
                            .unwrap_or_else(|| Digest(String::new())),
                    },
                );
            }
        }
        foreign.insert(
            b.name.clone(),
            cpl_project::ForeignC {
                sources: inputs.sources.get(&b.name).cloned().unwrap_or_default(),
                headers: crate::lockgen::files_digest(
                    &headers.get(&b.name).cloned().unwrap_or_default(),
                    project,
                ),
                libraries,
            },
        );
    }
    let adapters = plan
        .bindings
        .iter()
        .map(|b| {
            let capability = project
                .manifest
                .bindings
                .get(&b.name)
                .map(|x| x.capability.clone())
                .unwrap_or_else(|| p.display_path(b.capability));
            (
                b.name.clone(),
                cpl_project::AdapterC {
                    form: b.form,
                    isolation: if b.worker { "worker" } else { "in_process" }.to_string(),
                    capability,
                    files: inputs.adapters.get(&b.name).cloned().unwrap_or_default(),
                },
            )
        })
        .collect();
    // the platform files: what the final link opened that is neither an object of this build nor a binding's library
    let platform: BTreeSet<PathBuf> = opened
        .into_iter()
        .filter(|f| !f.starts_with(out) && !resolved.contains(f))
        .collect();
    let exports = plan
        .exports
        .iter()
        .map(|e| {
            (
                e.symbol.clone(),
                cpl_project::LockedExport {
                    function: p.display_path(e.function),
                    id: p.def(e.function).stable_id.clone(),
                    signature: crate::lockgen::signature_digest(p, &e.symbol, e.int, e.function),
                    int: e.int,
                },
            )
        })
        .collect();
    let profile: Vec<&str> = cfg.profile.flags().to_vec();
    let sanitize: Vec<&str> = cc::sanitizer_flags(&cfg.sanitize);
    let mut sorted = symbols.to_vec();
    sorted.sort();
    LockEntries {
        target: cpl_project::TargetC {
            api: format!(
                "{}.{}",
                cpl_rtspec::limits::C_API_VERSION.0,
                cpl_rtspec::limits::C_API_VERSION.1
            ),
            runtime: cpl_rt::RUNTIME_VERSION.to_string(),
            runtime_digest: runtime_digest.clone(),
            target: compiler.target.clone(),
            compiler: format!(
                "{} {}",
                match compiler.family {
                    cc::Family::Gcc => "gcc",
                    cc::Family::Clang => "clang",
                },
                compiler.version
            ),
            compiler_path: compiler.path.display().to_string(),
            compiler_digest: compiler.digest.clone(),
            flags_digest: crate::lockgen::flags_digest(project, &profile, &sanitize),
            system_digest: crate::lockgen::files_digest(&platform, project),
            prefix: crate::unit_prefix(plan),
            abi: plan.abi,
            exports_digest: exports_digest.clone(),
            exports,
            symbols: sorted,
        },
        adapters,
        foreign,
        python: None,
    }
}

/// The wheel tag of a build (`spec/python.md` 1.1): `cp310-abi3-linux_<arch>`, the architecture the compiler's
/// `-dumpmachine` names.
fn python_tag(compiler: &cc::Compiler) -> String {
    let arch = compiler.target.split('-').next().unwrap_or("x86_64");
    format!("cp310-abi3-linux_{arch}")
}

/// The package directory beside the linked extension (`spec/python.md` 1, 3): `__init__.py`, `__init__.pyi`,
/// `py.typed`, `_types.py`, then the wheel of it all; the artifacts of every file written.
fn write_python_package(
    project: &Project,
    p: &Program,
    plan: &InteropPlan,
    compiler: &cc::Compiler,
    out: &std::path::Path,
    package: &str,
) -> std::io::Result<Vec<cpl_diag::json::BuildArtifact>> {
    use std::os::unix::fs::PermissionsExt as _;
    let manifest = &project.manifest;
    let exports: Vec<cpl_hir::DefId> = plan.exports.iter().map(|e| e.function).collect();
    let model = crate::python::PyModel::new(p, manifest, &exports);
    let dir = out.join(package);
    std::fs::create_dir_all(&dir)?;
    let texts = [
        (
            "__init__.py",
            crate::python::package_init(&model, p, plan, manifest),
        ),
        (
            "__init__.pyi",
            crate::python::package_stub(&model, p, plan, manifest),
        ),
        ("py.typed", String::new()),
        ("_types.py", model.types_module()),
    ];
    let mut artifacts = Vec::new();
    for (name, text) in &texts {
        let path = dir.join(name);
        std::fs::write(&path, text)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        artifacts.push(artifact("python_module", &path, project));
    }
    let so = dir.join("_native.abi3.so");
    artifacts.push(artifact("extension", &so, project));
    let worker_name = format!("{}-worker", crate::unit_prefix(plan));
    let worker = dir.join(&worker_name);
    let has_worker = worker.is_file();
    if has_worker {
        artifacts.push(artifact("worker", &worker, project));
    }
    // the wheel (spec/python.md 1.1)
    let mut entries: Vec<crate::wheel::Entry> = Vec::new();
    for (name, text) in &texts {
        entries.push(crate::wheel::Entry {
            path: format!("{package}/{name}"),
            data: text.clone().into_bytes(),
            executable: false,
        });
    }
    entries.push(crate::wheel::Entry {
        path: format!("{package}/_native.abi3.so"),
        data: std::fs::read(&so)?,
        executable: false,
    });
    if has_worker {
        entries.push(crate::wheel::Entry {
            path: format!("{package}/{worker_name}"),
            data: std::fs::read(&worker)?,
            executable: true,
        });
    }
    let version = manifest.python_version();
    let tag = python_tag(compiler);
    let bytes = crate::wheel::wheel(
        &crate::wheel::Metadata {
            name: package,
            version: &version,
            summary: manifest.project.description.as_deref(),
            tag: &tag,
        },
        entries,
    )?;
    let dist = out.join("dist");
    std::fs::create_dir_all(&dist)?;
    let whl = dist.join(format!("{package}-{version}-{tag}.whl"));
    std::fs::write(&whl, bytes)?;
    std::fs::set_permissions(&whl, std::fs::Permissions::from_mode(0o644))?;
    artifacts.push(artifact("wheel", &whl, project));
    Ok(artifacts)
}

/// `[targets] python` and the backend-python `[adapters]` of a Python build (`spec/python.md` 9).
fn python_lock_entries(
    project: &Project,
    plan: &InteropPlan,
    compiler: &cc::Compiler,
    cfg: &BuildConfig,
    runtime_digest: &Digest,
    probe: &PythonProbe,
    depfile: &std::path::Path,
) -> PythonLockEntries {
    let headers: BTreeSet<PathBuf> = std::fs::read_to_string(depfile)
        .map(|text| crate::lockgen::depfile_headers(&text))
        .unwrap_or_default()
        .into_iter()
        .filter(|h| h.starts_with(&probe.include))
        .collect();
    let profile: Vec<&str> = cfg.profile.flags().to_vec();
    let sanitize: Vec<&str> = cc::sanitizer_flags(&cfg.sanitize);
    let extra: Vec<String> = ["-DPy_LIMITED_API=0x030A0000".to_string()]
        .into_iter()
        .chain(PYTHON_LINK.iter().map(|s| s.to_string()))
        .collect();
    let adapters = plan
        .python_bindings
        .iter()
        .filter_map(|b| {
            cpl_project::python_input_digest(&project.manifest, project.root.as_deref(), &b.name)
                .map(|digest| {
                    (
                        b.name.clone(),
                        cpl_project::AdapterPython {
                            module: b.module.clone(),
                            digest,
                        },
                    )
                })
        })
        .collect();
    PythonLockEntries {
        target: cpl_project::TargetPython {
            package: project.manifest.python_package(),
            version: project.manifest.python_version(),
            api: format!(
                "{}.{}",
                cpl_rtspec::limits::C_API_VERSION.0,
                cpl_rtspec::limits::C_API_VERSION.1
            ),
            compiler_path: compiler.path.display().to_string(),
            compiler_digest: compiler.digest.clone(),
            flags_digest: crate::lockgen::flags_digest_with(project, &profile, &sanitize, &extra),
            runtime_digest: runtime_digest.clone(),
            python_headers_digest: crate::lockgen::files_digest(&headers, project),
        },
        adapters,
    }
}

fn rel_path(path: &std::path::Path, project: &Project) -> String {
    match &project.root {
        Some(root) => match path.strip_prefix(root) {
            Ok(rel) => rel.display().to_string(),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

fn artifact(
    kind: &str,
    path: &std::path::Path,
    project: &Project,
) -> cpl_diag::json::BuildArtifact {
    let digest = std::fs::read(path)
        .map(|b| blake3::hash(&b).to_hex().to_string())
        .unwrap_or_default();
    cpl_diag::json::BuildArtifact {
        kind: kind.to_string(),
        path: rel_path(path, project),
        digest: Digest(digest),
    }
}

fn io_error(detail: &str) -> Diagnostic {
    Diagnostic::error(
        cpl_diag::DiagCode("internal_error"),
        cpl_diag::Category::Internal,
        format!("internal error (io): {detail}"),
    )
    .with_data("kind", serde_json::json!("io"))
    .with_data("detail", serde_json::json!(detail))
}

/// A failed link: `build_native_dependency_missing` for a binding's `-l` name the linker cannot find, else
/// `build_link_failed` with the bindings' library names.
fn link_error(project: &Project, what: &str, stderr: &str) -> Diagnostic {
    for (name, c) in &project.manifest.binding_c {
        for l in &c.libraries {
            if stderr.contains(&format!("cannot find -l{l}")) {
                return Diagnostic::error(
                    crate::codes::BUILD_NATIVE_DEPENDENCY_MISSING,
                    cpl_diag::Category::Build,
                    format!("binding `{name}`: the linker cannot find the library `{l}` (-l{l})"),
                )
                .with_data("binding", serde_json::json!(name))
                .with_data("library", serde_json::json!(l));
            }
        }
    }
    let libraries: Vec<&String> = project
        .manifest
        .binding_c
        .values()
        .flat_map(|c| &c.libraries)
        .collect();
    link_failed(what, stderr).with_data("libraries", serde_json::json!(libraries))
}

fn link_failed(what: &str, stderr: &str) -> Diagnostic {
    Diagnostic::error(
        crate::codes::BUILD_LINK_FAILED,
        cpl_diag::Category::Build,
        format!("linking {what} failed"),
    )
    .with_data("stderr", serde_json::json!(stderr))
    .with_data("libraries", serde_json::json!([]))
}

/// The configuration of a driver build (the native engine, `spec/cabi.md` 3.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverConfig {
    pub cache_dir: PathBuf,
    /// `Debug` for `--engine native`.
    pub profile: cc::Profile,
    /// D8 (toolchain 12.6).
    pub sanitize: Vec<cc::Sanitizer>,
    pub coverage: bool,
    /// None from the CLI: the manifest key, `CPL_CC`, or `cc` (T72).
    pub cc: Option<PathBuf>,
    pub runtime: cpl_rt::RuntimeSource,
    /// As [`BuildConfig::werror`].
    pub werror: bool,
}

/// A driver as built: its path, and what the receipt's `native` object records (toolchain 8.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverBuild {
    pub path: PathBuf,
    pub compiler: cc::Compiler,
    pub runtime_digest: Digest,
    /// The program digest the driver's banner reports (`spec/cabi.md` 3.8), which the client checks.
    pub program_digest: Digest,
}

/// The program's driver (`spec/cabi.md` 3.8): the program emitted `with_tests`, the runtime, every backend-`c`
/// adapter of the plan with its vendored sources, and the driver main, built into
/// `<cache_dir>/driver/<key>/<prefix>-driver`, with the key and the prefix of `spec/cabi.md` 1.6 (the key covers the
/// program, the runtime, the compiler, every flag, `-Werror` included, and the bindings' `c_input_digests` of toolchain
/// 11.2), or taken from there when cabi.md 1.6's reuse rule allows, so that an edited header or adapter rebuilds it.
pub fn driver(
    project: &Project,
    p: &Program,
    plan: &InteropPlan,
    cfg: &DriverConfig,
) -> Result<DriverBuild, Vec<Diagnostic>> {
    let manifest_cc = project
        .manifest
        .interop_c
        .as_ref()
        .and_then(|c| c.compiler.as_deref());
    let compiler = cc::Compiler::discover(cfg.cc.as_deref(), manifest_cc).map_err(|d| vec![d])?;
    let profile = if cfg.sanitize.is_empty() {
        cfg.profile
    } else {
        cc::Profile::Debug
    };
    let ecfg = crate::EmitConfig {
        optimize: cfg.profile == cc::Profile::Optimized,
        with_tests: true,
        coverage: cfg.coverage,
        release: false,
        runtime: cfg.runtime.clone(),
        target: compiler.target.clone(),
        compiler: compiler.name(),
        profile: profile.as_str().to_string(),
    };
    let pkg = crate::emit(p, project, plan, &ecfg)?;
    let prefix = crate::unit_prefix(plan);
    let runtime_digest = cpl_rt::runtime_digest(&cfg.runtime)
        .map_err(|e| vec![internal(&format!("cannot read the runtime: {e}"))])?;
    // the compile flags of every unit (cabi.md 1.5): base, profile or sanitizers, tests, coverage
    let mut flags: Vec<String> = cpl_rt::flags::BASE.iter().map(|s| s.to_string()).collect();
    if cfg.sanitize.is_empty() {
        flags.extend(profile.flags().iter().map(|s| s.to_string()));
    } else {
        flags.extend(
            cc::sanitizer_flags(&cfg.sanitize)
                .iter()
                .map(|s| s.to_string()),
        );
    }
    flags.push("-DCPL_WITH_TESTS=1".to_string());
    if cfg.coverage {
        flags.push("-DCPLRT_COVERAGE=1".to_string());
    }
    if cfg.werror {
        flags.push(cpl_rt::flags::WERROR.to_string());
    }
    let mut key = blake3::Hasher::new();
    for part in [
        pkg.program_digest.as_str(),
        runtime_digest.as_str(),
        compiler.digest.as_str(),
    ] {
        key.update(part.as_bytes());
        key.update(b"\n");
    }
    for f in &flags {
        key.update(f.as_bytes());
        key.update(b"\n");
    }
    // the adapter units: a generated wrapper's text is in the program digest, and its path is per build
    let (units, binding_link) = adapter_units(project, plan, std::path::Path::new("."));
    for u in &units {
        key.update(u.binding.as_bytes());
        key.update(b"\n");
        if !u.generated {
            key.update(u.src.display().to_string().as_bytes());
            key.update(&std::fs::read(&u.src).unwrap_or_default());
        }
        for e in &u.extra {
            key.update(e.as_bytes());
            key.update(b"\n");
        }
    }
    for l in &binding_link {
        key.update(l.as_bytes());
    }
    let key = key.finalize().to_hex().to_string();
    let dir = cfg.cache_dir.join("driver").join(&key);
    let exe = dir.join(format!("{prefix}-driver"));
    if exe.is_file() {
        return Ok(DriverBuild {
            path: exe,
            compiler,
            runtime_digest,
            program_digest: pkg.program_digest.clone(),
        });
    }
    // build under a temporary name, then rename into place (cabi.md 1.6)
    let tmp =
        cfg.cache_dir
            .join("driver")
            .join(format!("{key}.tmp-{}-{}", std::process::id(), unique()));
    let _ = std::fs::remove_dir_all(&tmp);
    let io = |e: std::io::Error| vec![internal(&format!("cannot write the driver's sources: {e}"))];
    let mut files: Vec<(String, String)> =
        cpl_rt::amalgamation(&prefix, &cfg.runtime).map_err(io)?;
    files.extend(pkg.files.iter().map(|f| (f.path.clone(), f.text.clone())));
    for (rel, text) in &files {
        let path = tmp.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).map_err(io)?;
        std::fs::write(&path, text).map_err(io)?;
    }
    let units: Vec<String> = files
        .iter()
        .map(|(rel, _)| rel.clone())
        .filter(|rel| {
            rel.ends_with(".c") && !rel.ends_with("cpl_rt_exe.c") && !is_wrapper(plan, rel)
        })
        .collect();
    let mut unit_flags = flags.clone();
    unit_flags.push(format!("-ffile-prefix-map={}=.", tmp.display()));
    unit_flags.push(format!("-I{}", tmp.join("src").display()));
    unit_flags.push(format!("-I{}", tmp.join("include").display()));
    let objects: Vec<Result<PathBuf, Diagnostic>> = std::thread::scope(|scope| {
        let handles: Vec<_> = units
            .iter()
            .map(|rel| {
                let tmp = &tmp;
                let compiler = &compiler;
                let unit_flags = &unit_flags;
                scope.spawn(move || {
                    let obj = tmp.join(format!("{}.o", rel.replace('/', "_")));
                    let out = cc::command(&compiler.path)
                        .args(unit_flags)
                        .arg("-c")
                        .arg(tmp.join(rel))
                        .arg("-o")
                        .arg(&obj)
                        .output();
                    match out {
                        Ok(o) if o.status.success() => Ok(obj),
                        Ok(o) => Err(rejected(rel, &String::from_utf8_lossy(&o.stderr))),
                        Err(e) => Err(rejected(rel, &e.to_string())),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(internal("a compile thread panicked")))
            })
            .collect()
    });
    let mut objs = Vec::new();
    let mut errors = Vec::new();
    for o in objects {
        match o {
            Ok(p) => objs.push(p),
            Err(d) => errors.push(d),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let profile_flags: Vec<String> = if cfg.sanitize.is_empty() {
        profile.flags().iter().map(|s| s.to_string()).collect()
    } else {
        cc::sanitizer_flags(&cfg.sanitize)
            .iter()
            .map(|s| s.to_string())
            .collect()
    };
    objs.extend(
        compile_adapters(
            &compiler,
            plan,
            project,
            &profile_flags,
            &tmp,
            &tmp.join("obj"),
            None,
        )?
        .objs,
    );
    let (_, binding_link) = adapter_units(project, plan, &tmp);
    let link_out = tmp.join(format!("{prefix}-driver"));
    let mut link = cc::command(&compiler.path);
    if !cfg.sanitize.is_empty() {
        link.args(cc::sanitizer_flags(&cfg.sanitize));
    }
    link.args(&objs)
        .arg("-o")
        .arg(&link_out)
        .args(&binding_link)
        .arg("-lpthread");
    match link.output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => return Err(vec![rejected("link", &String::from_utf8_lossy(&o.stderr))]),
        Err(e) => return Err(vec![rejected("link", &e.to_string())]),
    }
    // keep only the driver: the sources and objects of a cached driver are rebuilt from the program
    if std::fs::rename(&tmp, &dir).is_err() {
        // another build of the same key won the rename: use its driver
        let _ = std::fs::remove_dir_all(&tmp);
    } else {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path
                .file_name()
                .is_some_and(|n| n != std::ffi::OsStr::new(&format!("{prefix}-driver")))
            {
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    Ok(DriverBuild {
        path: exe,
        compiler,
        runtime_digest,
        program_digest: pkg.program_digest,
    })
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn internal(detail: &str) -> Diagnostic {
    Diagnostic::error(
        cpl_diag::DiagCode("internal_error"),
        cpl_diag::Category::Internal,
        format!(
            "internal error ({}): {detail}",
            crate::codes::EMITTED_C_REJECTED
        ),
    )
    .with_data("kind", serde_json::json!(crate::codes::EMITTED_C_REJECTED))
    .with_data("detail", serde_json::json!(detail))
}

/// The compiler rejected emitted or runtime C: a toolchain defect (exit 2).
fn rejected(unit: &str, stderr: &str) -> Diagnostic {
    let detail = format!("the C compiler rejected {unit}");
    Diagnostic::error(
        cpl_diag::DiagCode("internal_error"),
        cpl_diag::Category::Internal,
        format!(
            "internal error ({}): {detail}",
            crate::codes::EMITTED_C_REJECTED
        ),
    )
    .with_data("kind", serde_json::json!(crate::codes::EMITTED_C_REJECTED))
    .with_data("detail", serde_json::json!(detail))
    .with_data("stderr", serde_json::json!(stderr))
}

/// One adapter unit: a reviewed wrapper, a vendored source, or a generated wrapper (`generated`, whose text the program
/// digest covers), with the flags it adds to the adapter set.
pub(crate) struct AdapterUnit {
    pub binding: String,
    pub src: PathBuf,
    pub extra: Vec<String>,
    pub generated: bool,
}

/// The adapter units of the plan's backend-c bindings (cabi.md 1.5, 6.1, 6.3): each reviewed wrapper and vendored
/// source with the adapter flag set's additions (the manifest's cflags, the binding's include_dirs as `-I`, defines,
/// and cflags); each declarative binding's generated wrapper, under `emitted` (the output root), with its include_dirs
/// as `-isystem` and the three `-Werror=` flags; and the link options of every binding (-L library_dirs, -l libraries).
pub(crate) fn adapter_units(
    project: &Project,
    plan: &InteropPlan,
    emitted: &std::path::Path,
) -> (Vec<AdapterUnit>, Vec<String>) {
    let root = project.root.clone().unwrap_or_default();
    let prefix = crate::unit_prefix(plan);
    let global: Vec<String> = project
        .manifest
        .interop_c
        .as_ref()
        .map(|c| c.cflags.clone())
        .unwrap_or_default();
    let mut units = Vec::new();
    let mut link = Vec::new();
    for b in &plan.bindings {
        let c = project
            .manifest
            .binding_c
            .get(&b.name)
            .cloned()
            .unwrap_or_default();
        let flags = |system: bool| -> Vec<String> {
            let mut extra: Vec<String> = global.clone();
            for d in &c.include_dirs {
                if system {
                    extra.push("-isystem".to_string());
                    extra.push(root.join(d).display().to_string());
                } else {
                    extra.push(format!("-I{}", root.join(d).display()));
                }
            }
            for d in &c.defines {
                extra.push(format!("-D{d}"));
            }
            extra.extend(c.cflags.iter().cloned());
            extra
        };
        match b.form {
            cpl_project::AdapterForm::Wrapper => units.push(AdapterUnit {
                binding: b.name.clone(),
                src: root.join(&b.adapter),
                extra: flags(false),
                generated: false,
            }),
            cpl_project::AdapterForm::Declarative => {
                let mut extra = flags(true);
                extra.extend(
                    cpl_rt::flags::GENERATED_WRAPPER
                        .iter()
                        .map(|s| s.to_string()),
                );
                units.push(AdapterUnit {
                    binding: b.name.clone(),
                    src: emitted.join(crate::cbind::wrapper_path(&prefix, &b.name)),
                    extra,
                    generated: true,
                });
            }
        }
        for src in &c.sources {
            units.push(AdapterUnit {
                binding: b.name.clone(),
                src: root.join(src),
                extra: flags(false),
                generated: false,
            });
        }
        for d in &c.library_dirs {
            link.push(format!("-L{}", root.join(d).display()));
        }
        for l in &c.libraries {
            link.push(format!("-l{l}"));
        }
    }
    (units, link)
}

/// Whether an emitted file is a generated wrapper, compiled with the adapter units rather than the program's.
fn is_wrapper(plan: &InteropPlan, rel: &str) -> bool {
    let prefix = crate::unit_prefix(plan);
    plan.bindings.iter().any(|b| {
        b.form == cpl_project::AdapterForm::Declarative
            && rel == crate::cbind::wrapper_path(&prefix, &b.name)
    })
}

/// The base flag set without its four `-Wno-*` flags: the adapter set (cabi.md 1.5).
pub(crate) fn adapter_base() -> Vec<String> {
    cpl_rt::flags::BASE
        .iter()
        .filter(|f| !f.starts_with("-Wno-"))
        .map(|s| s.to_string())
        .collect()
}

/// Compiles the adapter units into `obj_dir`: the objects, or the diagnostics of the sources that failed
/// (`build_adapter_compile_failed`, exit 1). `emitted` is the output root that holds the generated wrappers.
pub(crate) fn compile_adapters(
    compiler: &cc::Compiler,
    plan: &InteropPlan,
    project: &Project,
    profile_flags: &[String],
    emitted: &std::path::Path,
    obj_dir: &std::path::Path,
    cache: Option<&std::path::Path>,
) -> Result<AdapterObjects, Vec<Diagnostic>> {
    let (units, _) = adapter_units(project, plan, emitted);
    let mut reused_all = true;
    let out_include = emitted.join("include");
    let mut objs = Vec::new();
    let mut headers: BTreeMap<String, BTreeSet<PathBuf>> = plan
        .bindings
        .iter()
        .map(|b| (b.name.clone(), BTreeSet::new()))
        .collect();
    let mut errors = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let stem = u
            .src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("adapter");
        let obj = obj_dir.join(format!("adapter_{i}_{stem}.o"));
        let dep = obj_dir.join(format!("adapter_{i}_{stem}.d"));
        let _ = std::fs::create_dir_all(obj_dir);
        let mut args: Vec<String> = adapter_base();
        args.extend(profile_flags.iter().cloned());
        args.push(format!("-I{}", out_include.display()));
        args.extend(u.extra.iter().cloned());
        let out = crate::cache::compile(cache, compiler, &args, &u.src, &obj, &dep);
        // the headers the unit read (cabi.md 6.6), those under <out>/include left out
        if let Ok(text) = std::fs::read_to_string(&dep) {
            let set = headers.entry(u.binding.clone()).or_default();
            for h in crate::lockgen::depfile_headers(&text) {
                let abs = if h.is_absolute() {
                    h
                } else {
                    std::env::current_dir().unwrap_or_default().join(h)
                };
                if !abs.starts_with(&out_include) {
                    set.insert(abs);
                }
            }
        }
        let file = if u.generated {
            crate::cbind::wrapper_path(&crate::unit_prefix(plan), &u.binding)
        } else {
            rel_path(&u.src, project)
        };
        match out {
            Ok(reused) => {
                reused_all &= reused;
                objs.push(obj)
            }
            Err(stderr) => errors.push(
                Diagnostic::error(
                    crate::codes::BUILD_ADAPTER_COMPILE_FAILED,
                    cpl_diag::Category::Build,
                    format!("{file} does not compile"),
                )
                .with_data("binding", serde_json::json!(u.binding))
                .with_data("file", serde_json::json!(file))
                .with_data("stderr", serde_json::json!(stderr)),
            ),
        }
    }
    if errors.is_empty() {
        Ok(AdapterObjects {
            objs,
            headers,
            reused: reused_all,
        })
    } else {
        Err(errors)
    }
}

/// The objects of the adapter units, and the headers each binding's units read (from their depfiles).
pub(crate) struct AdapterObjects {
    pub objs: Vec<PathBuf>,
    pub headers: BTreeMap<String, BTreeSet<PathBuf>>,
    /// Whether every adapter object was reused from the object cache.
    pub reused: bool,
}
