//! `cpl build --target rust|go|java` through the built binary (`spec/hosts.md` 1): the hosts example writes
//! each package and reports it, a rebuild keeps unchanged files and removes stale ones, `[interop] targets` gates the
//! build (`kotlin` enables `java`), and the flags of the C build are refused. Compiling and running the packages is
//! `cpl-emit-host`'s test.

mod common;

use common::*;
use std::path::{Path, PathBuf};

/// A fresh copy of `examples/hosts`.
fn hosts(name: &str) -> PathBuf {
    let dir = temp_dir(name).join("hosts");
    copy_dir(&workspace().join("examples/hosts"), &dir);
    // a copy that ran an earlier build keeps no generated files
    let _ = std::fs::remove_dir_all(dir.join(".cpl"));
    dir
}

fn set_targets(dir: &Path, targets: &str) {
    std::fs::write(
        dir.join("cpl.toml"),
        format!(
            "[project]\nname = \"pricing\"\nedition = \"0.1\"\n\n[interop]\ntargets = {targets}\n"
        ),
    )
    .expect("cpl.toml");
}

#[test]
fn each_host_target_writes_its_package() {
    let dir = hosts("build-hosts");
    for (target, files) in [
        ("rust", vec!["Cargo.toml", "src/lib.rs", "src/cpl.rs"]),
        ("go", vec!["go.mod", "shop.pricing.cpl.go", "cpl/cpl.go"]),
        (
            "java",
            vec![
                "src/pricing/shop/pricing/Pricing.java",
                "src/pricing/shop/pricing/Discount.java",
                "src/pricing/cpl/Int.java",
            ],
        ),
    ] {
        let r = cpl(&dir, &["build", "--target", target, "--json"]);
        r.expect_code(0);
        let json = r.json();
        assert_eq!(json["target"], target);
        assert_eq!(json["profile"], "source");
        let out = dir.join(".cpl/build").join(target);
        for f in files {
            assert!(out.join(f).is_file(), "{target}: {f} is missing");
        }
        let exports: Vec<&str> = json["exports"]
            .as_array()
            .expect("exports")
            .iter()
            .map(|e| e["function"].as_str().expect("a function"))
            .collect();
        assert!(exports.contains(&"shop.pricing.checkout"), "{exports:?}");
        let artifacts = json["artifacts"].as_array().expect("artifacts");
        assert!(artifacts.iter().all(|a| a["kind"] == "source"));
        assert!(artifacts.iter().all(|a| a["path"]
            .as_str()
            .is_some_and(|p| p.starts_with(".cpl/build/"))));
        // the human output says how the host uses the package
        let r = cpl(&dir, &["build", "--target", target]);
        r.expect_code(0);
        assert!(
            r.stdout.contains(&format!("({target} source)")),
            "{}",
            r.stdout
        );
    }
}

#[test]
fn a_rebuild_keeps_unchanged_files_and_removes_stale_ones() {
    let dir = hosts("build-hosts-rebuild");
    cpl(&dir, &["build", "--target", "java"]).expect_code(0);
    let out = dir.join(".cpl/build/java");
    let kept = out.join("src/pricing/shop/pricing/Line.java");
    let stamp = std::fs::metadata(&kept)
        .and_then(|m| m.modified())
        .expect("a time stamp");
    // a file of the user's in the package directory is not the build's to remove
    std::fs::write(out.join("NOTES.txt"), "mine").expect("a user file");
    // Entry and the capabilities go away: their files go with them; Line stays as it was
    std::fs::write(
        dir.join("src/shop/pricing.cpl"),
        r#"module shop.pricing

/// An amount of money in cents.
pub type Cents = Int where self >= 0

/// One line of an order.
pub struct Line {
    sku: Text
    unit_price: Cents
    quantity: Int
} where self.quantity > 0

pub pure fn gross(line: Line) -> Int {
    line.unit_price * line.quantity
}
"#,
    )
    .expect("the smaller module");
    let r = cpl(&dir, &["build", "--target", "java"]);
    r.expect_code(0);
    assert!(
        !out.join("src/pricing/shop/pricing/Entry.java").exists(),
        "the stale record stayed"
    );
    assert!(
        out.join("NOTES.txt").is_file(),
        "the user's file was removed"
    );
    let again = std::fs::metadata(&kept)
        .and_then(|m| m.modified())
        .expect("a time stamp");
    assert_eq!(stamp, again, "an unchanged file was rewritten");
}

#[test]
fn interop_targets_gate_the_host_targets() {
    let dir = hosts("build-hosts-gate");
    set_targets(&dir, "[\"c\"]");
    let r = cpl(&dir, &["build", "--target", "go", "--json"]);
    r.expect_code(1);
    assert_eq!(
        r.json()["diagnostics"][0]["code"],
        "build_target_not_enabled"
    );
    // kotlin enables java, and --target kotlin is java
    set_targets(&dir, "[\"kotlin\"]");
    cpl(&dir, &["build", "--target", "java"]).expect_code(0);
    let r = cpl(&dir, &["build", "--target", "kotlin", "--json"]);
    r.expect_code(0);
    assert_eq!(r.json()["target"], "java");
    // an unknown target names the five
    let r = cpl(&dir, &["build", "--target", "ruby", "--json"]);
    r.expect_code(2);
    let message = r.json()["diagnostics"][0]["message"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(message.contains("rust, go, or java"), "{message}");
}

#[test]
fn the_c_build_flags_do_not_apply() {
    let dir = hosts("build-hosts-flags");
    for flags in [
        &["--artifact", "shared"][..],
        &["--sanitize", "address"][..],
        &["--lock"][..],
    ] {
        let mut args = vec!["build", "--target", "rust", "--json"];
        args.extend_from_slice(flags);
        let r = cpl(&dir, &args);
        r.expect_code(2);
        let message = r.json()["diagnostics"][0]["message"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            message.contains("does not apply to --target rust"),
            "{message}"
        );
    }
}
