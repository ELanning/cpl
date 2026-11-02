//! Git (`spec/toolchain.md` 6.3, 9.3, 12): against temporary repositories
//! created in the test (`git init -b main`, two commits): `discover_repo`,
//! `tree_at` restricted to the project files, `current_revision` clean and
//! dirty, `resolve_rev`, `merge_base`, the error cases, and a semantic diff
//! between two commits. Every test is skipped, with a message, when `git`
//! is not installed (`tests/git_missing.rs` covers that case).

mod common;

use common::*;
use cpl_diag::json::{DiffStatementKind, Revision, RevisionState};
use cpl_diag::FileOrigin;
use cpl_index::{
    current_revision, discover_repo, facet_map, merge_base, resolve_rev, semantic_diff,
    source_digest, tree_at, GitError, GitRepo,
};
use cpl_project::{load_project_from_tree, LoadError, LoadOptions, Project};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git_available() -> bool {
    let found = Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: git is not installed");
    }
    found
}

/// A temporary repository driven through the git command line, with a
/// fixed identity and no hooks, signing, or line-ending conversion.
struct TestRepo {
    dir: PathBuf,
}

impl TestRepo {
    fn init(name: &str) -> TestRepo {
        let dir = temp_dir(name);
        let repo = TestRepo { dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args([
                "-c",
                "user.name=cpl test",
                "-c",
                "user.email=cpl@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("LC_ALL", "C")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf-8")
            .trim()
            .to_string()
    }

    fn write(&self, files: &[(&str, &str)]) {
        write_files(&self.dir, files);
    }

    fn remove(&self, path: &str) {
        std::fs::remove_file(self.dir.join(path)).expect("remove file");
    }

    /// Commit everything; the new HEAD.
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "-q",
            "--no-verify",
            "--allow-empty",
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }
}

const MANIFEST: &str = "[project]\nname = \"demo\"\nedition = \"0.1\"\n";

const PAGES_V1: &str = "module demo.pages

@id(\"fn_aaaaaaaaaaaaaaaa\")
pub fn first(items: List<Int>) -> Int
    requires items.len > 0
{
    0
}

pub fn other() -> Int {
    1
}
";

const PAGES_V2: &str = "module demo.pages

@id(\"fn_aaaaaaaaaaaaaaaa\")
pub fn first(items: List<Int>) -> Int
    requires items.len > 1
{
    0
}

pub fn other() -> Int {
    1
}
";

/// A repository whose project lives in `cpl/`, beside files that are not
/// part of it, with two commits: the second changes `first`'s contract.
struct Scenario {
    repo: TestRepo,
    first: String,
    second: String,
}

fn scenario(name: &str) -> Scenario {
    let repo = TestRepo::init(name);
    repo.write(&[
        ("README.md", "not a project file\n"),
        ("other/z.cpl", "module other\n"),
        ("cpl/cpl.toml", MANIFEST),
        ("cpl/src/demo/pages.cpl", PAGES_V1),
        ("cpl/src/notes.txt", "not a source\n"),
        (
            "cpl/work/plan.work",
            "change c {\n    base git(\"main\")\n    target git(\"main\")\n}\n",
        ),
        (
            "cpl/work/fix.patch",
            "patch first.body\n    expect base(first.body)\n{\n    1\n}\n",
        ),
        ("cpl/work/deep/nested.work", "change d {\n}\n"),
        ("cpl/work/readme.md", "not a work file\n"),
    ]);
    let first = repo.commit("first");
    repo.write(&[("cpl/src/demo/pages.cpl", PAGES_V2)]);
    let second = repo.commit("second");
    Scenario {
        repo,
        first,
        second,
    }
}

fn project_root(s: &Scenario) -> PathBuf {
    s.repo.dir.join("cpl")
}

fn discovered(root: &Path) -> GitRepo {
    discover_repo(root)
        .expect("discover")
        .expect("inside a repository")
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).expect("canonicalize")
}

fn load_at(repo: &GitRepo, commit: &str) -> Project {
    let tree = tree_at(repo, commit).expect("tree");
    load_project_from_tree(
        &tree,
        FileOrigin::Revision(commit.to_string()),
        &LoadOptions::default(),
    )
    .expect("load the tree")
}

// ---------------------------------------------------------------------------
// discover_repo
// ---------------------------------------------------------------------------

#[test]
fn discover_repo_finds_the_toplevel_and_the_project_prefix() {
    if !git_available() {
        return;
    }
    let s = scenario("discover");
    let repo = discovered(&project_root(&s));
    assert_eq!(canonical(&repo.toplevel), canonical(&s.repo.dir));
    assert_eq!(repo.project_prefix, "cpl/");
    let at_top = discovered(&s.repo.dir);
    assert_eq!(at_top.project_prefix, "");
    let deeper = discovered(&project_root(&s).join("src/demo"));
    assert_eq!(deeper.project_prefix, "cpl/src/demo/");
    // Outside any repository.
    let outside = temp_dir("discover-outside");
    assert_eq!(discover_repo(&outside).expect("no error"), None);
}

// ---------------------------------------------------------------------------
// tree_at
// ---------------------------------------------------------------------------

#[test]
fn tree_at_reads_only_the_project_files_of_a_commit() {
    if !git_available() {
        return;
    }
    let s = scenario("tree");
    let repo = discovered(&project_root(&s));
    let tree = tree_at(&repo, &s.first).expect("tree");
    let paths: Vec<&str> = tree.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "cpl.toml",
            "src/demo/pages.cpl",
            "work/fix.patch",
            "work/plan.work"
        ]
    );
    assert_eq!(tree.files["src/demo/pages.cpl"], PAGES_V1.as_bytes());
    assert_eq!(tree.files["cpl.toml"], MANIFEST.as_bytes());
    // Any name of the commit works; the second commit has the new text.
    let head = tree_at(&repo, "HEAD").expect("tree");
    assert_eq!(head.files["src/demo/pages.cpl"], PAGES_V2.as_bytes());
    assert_eq!(head, tree_at(&repo, &s.second).expect("tree"));
    assert_eq!(tree, tree_at(&repo, "HEAD~1").expect("tree"));
}

#[test]
fn a_tree_loads_like_the_checkout_it_was_committed_from() {
    if !git_available() {
        return;
    }
    let s = scenario("tree-load");
    let root = project_root(&s);
    let repo = discovered(&root);
    let from_git = load_at(&repo, &s.second);
    let from_disk = load_dir(&root);
    assert_eq!(source_digest(&from_git), source_digest(&from_disk));
    let revision = Revision {
        commit: Some(s.second.clone()),
        state: RevisionState::Clean,
        digest: source_digest(&from_git),
    };
    assert_eq!(
        facet_map(&from_git, revision.clone()),
        facet_map(&from_disk, revision)
    );
    // The files carry the revision as their origin.
    assert!(from_git
        .files
        .iter()
        .filter(|e| e.origin != FileOrigin::Std)
        .all(|e| e.origin == FileOrigin::Revision(s.second.clone())));
}

#[test]
fn tree_at_follows_the_manifest_of_the_commit() {
    if !git_available() {
        return;
    }
    let repo_dir = TestRepo::init("tree-sources");
    repo_dir.write(&[("README.md", "nothing yet\n")]);
    let empty = repo_dir.commit("no project");
    repo_dir.write(&[
        (
            "cpl.toml",
            "[project]\nname = \"demo\"\nedition = \"0.1\"\n\n[build]\nsources = [\"lib\", \"more/stuff\"]\n",
        ),
        ("cpl.lock", "# not read by tree_at\n"),
        ("lib/x.cpl", "module x\n"),
        ("lib/deep/w.cpl", "module x.deep\n"),
        ("lib/readme.md", "not a source\n"),
        ("lib/.cpl", "a file named only .cpl is not a source\n"),
        ("more/stuff/y.cpl", "module y\n"),
        ("more/other.cpl", "module outside\n"),
        ("src/z.cpl", "module z\n"),
    ]);
    std::os::unix::fs::symlink("x.cpl", repo_dir.dir.join("lib/link.cpl")).expect("symlink");
    let custom = repo_dir.commit("custom sources");
    let repo = discovered(&repo_dir.dir);
    // A commit without cpl.toml has no project: an empty tree, which loads
    // as NoManifest (cpl diff's git_no_project_at_revision).
    let none = tree_at(&repo, &empty).expect("tree");
    assert!(none.files.is_empty());
    assert!(matches!(
        load_project_from_tree(&none, FileOrigin::Revision(empty), &LoadOptions::default()),
        Err(LoadError::NoManifest(_))
    ));
    // The commit's own [build] sources decide. A symbolic link to a file is
    // read through, as `load_project` reads it in a checkout (Section 5.4:
    // the facet map is the same from a directory and from a git tree).
    let tree = tree_at(&repo, &custom).expect("tree");
    let paths: Vec<&str> = tree.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "cpl.lock",
            "cpl.toml",
            "lib/deep/w.cpl",
            "lib/link.cpl",
            "lib/x.cpl",
            "more/stuff/y.cpl"
        ]
    );
    assert_eq!(tree.files["lib/link.cpl"], tree.files["lib/x.cpl"]);
    // "." makes every .cpl file of the project a source.
    repo_dir.write(&[(
        "cpl.toml",
        "[project]\nname = \"demo\"\nedition = \"0.1\"\n\n[build]\nsources = [\".\"]\n",
    )]);
    let dot = repo_dir.commit("dot");
    let tree = tree_at(&repo, &dot).expect("tree");
    let cpl_files: Vec<&str> = tree
        .files
        .keys()
        .map(String::as_str)
        .filter(|p| p.ends_with(".cpl"))
        .collect();
    assert_eq!(
        cpl_files,
        [
            "lib/deep/w.cpl",
            "lib/link.cpl",
            "lib/x.cpl",
            "more/other.cpl",
            "more/stuff/y.cpl",
            "src/z.cpl"
        ]
    );
    // A manifest that does not parse: cpl.toml and cpl.lock alone, so that
    // loading reports the manifest error as it would for a checkout.
    repo_dir.write(&[("cpl.toml", "[project\n")]);
    let broken = repo_dir.commit("broken manifest");
    let tree = tree_at(&repo, &broken).expect("tree");
    let paths: Vec<&str> = tree.files.keys().map(String::as_str).collect();
    assert_eq!(paths, ["cpl.lock", "cpl.toml"]);
    assert!(matches!(
        load_project_from_tree(&tree, FileOrigin::Revision(broken), &LoadOptions::default()),
        Err(LoadError::Manifest { .. })
    ));
}

#[test]
fn symbolic_links_are_followed_inside_the_commit() {
    if !git_available() {
        return;
    }
    // Section 5.4: the facet map "is the same whether built from a directory
    // or from a git tree", and `load_project` reads a symbolic link to a file
    // as that file; `tree_at` follows the links of a commit inside its tree,
    // wherever in the repository they lead, the manifest's included.
    let repo_dir = TestRepo::init("tree-links");
    repo_dir.write(&[
        (
            "config/cpl.toml",
            "[project]\nname = \"demo\"\nedition = \"0.1\"\n",
        ),
        (
            "shared/lib.cpl",
            "module shared\n\npub fn g() -> Int {\n    2\n}\n",
        ),
        ("src/m.cpl", "module m\n\npub fn f() -> Int {\n    1\n}\n"),
        ("plans/a.work", "change c {\n}\n"),
    ]);
    std::fs::create_dir_all(repo_dir.dir.join("work")).expect("work directory");
    let link = |target: &str, path: &str| {
        std::os::unix::fs::symlink(target, repo_dir.dir.join(path)).expect("symlink")
    };
    link("config/cpl.toml", "cpl.toml");
    link("../shared/lib.cpl", "src/lib.cpl");
    link("lib.cpl", "src/chain.cpl");
    link("../plans/a.work", "work/a.work");
    let first = repo_dir.commit("links that lead to files");
    let repo = discovered(&repo_dir.dir);
    let expected = [
        "cpl.toml",
        "src/chain.cpl",
        "src/lib.cpl",
        "src/m.cpl",
        "work/a.work",
    ];
    let tree = tree_at(&repo, &first).expect("tree");
    let paths: Vec<&str> = tree.files.keys().map(String::as_str).collect();
    assert_eq!(paths, expected);
    assert_eq!(
        tree.files["cpl.toml"],
        b"[project]\nname = \"demo\"\nedition = \"0.1\"\n"
    );
    assert_eq!(
        tree.files["src/lib.cpl"],
        b"module shared\n\npub fn g() -> Int {\n    2\n}\n"
    );
    assert_eq!(tree.files["src/chain.cpl"], tree.files["src/lib.cpl"]);
    assert_eq!(tree.files["work/a.work"], b"change c {\n}\n");
    // The commit and its clean checkout are one revision.
    let disk = load_dir(&repo_dir.dir);
    let revision = current_revision(&disk, Some(&repo)).expect("revision");
    assert_eq!(revision.state, RevisionState::Clean);
    let committed = load_at(&repo, &first);
    assert_eq!(source_digest(&committed), revision.digest);
    assert_eq!(
        facet_map(&committed, revision.clone()),
        facet_map(&disk, revision)
    );
    // A link that leads to no file of the commit is not part of its tree: one
    // to a directory, to nothing, around a loop, or out of the repository
    // (whose files no commit holds).
    link("../shared", "src/dir.cpl");
    link("nothing.cpl", "src/dangling.cpl");
    link("loop_b.cpl", "src/loop_a.cpl");
    link("loop_a.cpl", "src/loop_b.cpl");
    link("m.cpl/inner.cpl", "src/through.cpl");
    link("../../outside.cpl", "src/out.cpl");
    link("/etc/hostname", "src/absolute.cpl");
    let second = repo_dir.commit("links that lead to no file");
    let tree = tree_at(&repo, &second).expect("tree");
    let paths: Vec<&str> = tree.files.keys().map(String::as_str).collect();
    assert_eq!(paths, expected);
}

#[test]
fn pattern_characters_in_paths_name_themselves() {
    if !git_available() {
        return;
    }
    // A project directory and a source directory whose names are git
    // pathspec patterns; neighbours they would match as patterns.
    let repo_dir = TestRepo::init("literal");
    repo_dir.write(&[
        (
            "proj[1]/cpl.toml",
            "[project]\nname = \"demo\"\nedition = \"0.1\"\n\n[build]\nsources = [\"lib*\"]\n",
        ),
        ("proj[1]/lib*/a.cpl", "module a\n"),
        ("proj[1]/libx/b.cpl", "module b\n"),
        ("proj1/cpl.toml", MANIFEST),
        ("proj1/lib*/c.cpl", "module c\n"),
    ]);
    let commit = repo_dir.commit("patterns");
    let root = repo_dir.dir.join("proj[1]");
    let repo = discovered(&root);
    assert_eq!(repo.project_prefix, "proj[1]/");
    let tree = tree_at(&repo, &commit).expect("tree");
    let paths: Vec<&str> = tree.files.keys().map(String::as_str).collect();
    assert_eq!(paths, ["cpl.toml", "lib*/a.cpl"]);
    let state = || {
        let project = load_dir(&root);
        current_revision(&project, Some(&repo))
            .expect("revision")
            .state
    };
    assert_eq!(state(), RevisionState::Clean);
    repo_dir.write(&[
        ("proj[1]/libx/b.cpl", "module b2\n"),
        ("proj1/lib*/c.cpl", "module c2\n"),
    ]);
    assert_eq!(state(), RevisionState::Clean);
    repo_dir.write(&[("proj[1]/lib*/a.cpl", "module a2\n")]);
    assert_eq!(state(), RevisionState::Worktree);
}

// ---------------------------------------------------------------------------
// current_revision
// ---------------------------------------------------------------------------

#[test]
fn current_revision_is_clean_until_the_source_set_changes() {
    if !git_available() {
        return;
    }
    let s = scenario("revision");
    let root = project_root(&s);
    let repo = discovered(&root);
    let revision = || {
        let project = load_dir(&root);
        let revision = current_revision(&project, Some(&repo)).expect("revision");
        assert_eq!(revision.digest, source_digest(&project));
        revision
    };
    let clean = revision();
    assert_eq!(clean.commit.as_deref(), Some(s.second.as_str()));
    assert_eq!(clean.state, RevisionState::Clean);
    assert_eq!(s.second.len(), 40);

    // A modified source file.
    s.repo.write(&[("cpl/src/demo/pages.cpl", PAGES_V1)]);
    let dirty = revision();
    assert_eq!(dirty.state, RevisionState::Worktree);
    assert_eq!(dirty.commit.as_deref(), Some(s.second.as_str()));
    assert_ne!(dirty.digest, clean.digest);
    s.repo.write(&[("cpl/src/demo/pages.cpl", PAGES_V2)]);
    assert_eq!(revision(), clean);

    // An untracked source file, then a modified work file and manifest.
    s.repo
        .write(&[("cpl/src/demo/extra.cpl", "module demo.extra\n")]);
    assert_eq!(revision().state, RevisionState::Worktree);
    s.repo.remove("cpl/src/demo/extra.cpl");
    assert_eq!(revision().state, RevisionState::Clean);
    s.repo.write(&[("cpl/work/plan.work", "change c {\n}\n")]);
    assert_eq!(revision().state, RevisionState::Worktree);
    s.repo.git(&["checkout", "--", "cpl/work/plan.work"]);
    s.repo.write(&[(
        "cpl/cpl.toml",
        "[project]\nname = \"demo2\"\nedition = \"0.1\"\n",
    )]);
    assert_eq!(revision().state, RevisionState::Worktree);
    s.repo.git(&["checkout", "--", "cpl/cpl.toml"]);
    assert_eq!(revision(), clean);

    // Files outside the source-set roots do not count; files under a root
    // do, members or not (the roots are what 6.3 passes to `git status`).
    s.repo
        .write(&[("README.md", "changed\n"), ("other/new.cpl", "module n\n")]);
    assert_eq!(revision(), clean);
    s.repo.write(&[("cpl/src/notes.txt", "changed\n")]);
    let noted = revision();
    assert_eq!(noted.state, RevisionState::Worktree);
    assert_eq!(noted.digest, clean.digest);
}

#[test]
fn current_revision_without_a_repository_or_without_commits() {
    if !git_available() {
        return;
    }
    let outside = temp_dir("revision-outside");
    write_files(
        &outside,
        &[
            ("cpl.toml", MANIFEST),
            ("src/demo/a.cpl", "module demo.a\n"),
        ],
    );
    let project = load_dir(&outside);
    let unversioned = Revision {
        commit: None,
        state: RevisionState::Worktree,
        digest: source_digest(&project),
    };
    assert_eq!(discover_repo(&outside).expect("no error"), None);
    assert_eq!(
        current_revision(&project, None).expect("revision"),
        unversioned
    );
    // A repository without commits.
    let fresh = TestRepo::init("revision-fresh");
    fresh.write(&[
        ("cpl.toml", MANIFEST),
        ("src/demo/a.cpl", "module demo.a\n"),
    ]);
    let repo = discovered(&fresh.dir);
    let project = load_dir(&fresh.dir);
    assert_eq!(
        current_revision(&project, Some(&repo)).expect("revision"),
        unversioned
    );
}

// ---------------------------------------------------------------------------
// resolve_rev and merge_base
// ---------------------------------------------------------------------------

#[test]
fn resolve_rev_names_commits_only() {
    if !git_available() {
        return;
    }
    let s = scenario("resolve");
    let repo = discovered(&project_root(&s));
    s.repo.git(&["tag", "light", &s.first]);
    s.repo
        .git(&["tag", "-a", "-m", "annotated", "heavy", &s.second]);
    for (rev, expected) in [
        ("main", &s.second),
        ("HEAD", &s.second),
        ("HEAD~1", &s.first),
        ("light", &s.first),
        ("heavy", &s.second),
        (&s.first[..10], &s.first),
        (s.second.as_str(), &s.second),
    ] {
        assert_eq!(&resolve_rev(&repo, rev).expect(rev), expected, "{rev}");
    }
    let tree = s.repo.git(&["rev-parse", "HEAD^{tree}"]);
    for unknown in [
        "nope",
        "HEAD~5",
        "",
        "-q",
        "--all",
        tree.as_str(),
        "main:cpl/cpl.toml",
    ] {
        match resolve_rev(&repo, unknown) {
            Err(GitError::UnknownRevision(rev)) => assert_eq!(rev, unknown),
            other => panic!("{unknown:?}: {other:?}"),
        }
    }
}

#[test]
fn merge_base_of_two_branches() {
    if !git_available() {
        return;
    }
    let s = scenario("merge-base");
    let repo = discovered(&project_root(&s));
    s.repo.git(&["checkout", "-q", "-b", "feature", &s.first]);
    s.repo
        .write(&[("cpl/src/demo/feature.cpl", "module demo.feature\n")]);
    let feature = s.repo.commit("feature");
    assert_eq!(merge_base(&repo, "main", "feature").expect("base"), s.first);
    assert_eq!(merge_base(&repo, &feature, "main").expect("base"), s.first);
    assert_eq!(merge_base(&repo, "main", "main").expect("base"), s.second);
    assert!(matches!(
        merge_base(&repo, "main", "nope"),
        Err(GitError::UnknownRevision(rev)) if rev == "nope"
    ));
    // Unrelated histories have no merge base.
    s.repo.git(&["checkout", "-q", "--orphan", "unrelated"]);
    s.repo.git(&["rm", "-q", "-r", "--cached", "."]);
    s.repo.write(&[("lonely.txt", "x\n")]);
    s.repo.git(&["add", "lonely.txt"]);
    s.repo.git(&["commit", "-q", "--no-verify", "-m", "orphan"]);
    match merge_base(&repo, "main", "unrelated") {
        Err(GitError::Command {
            args,
            status,
            stderr,
        }) => {
            assert_eq!(status, 1);
            assert_eq!(args[0], "merge-base");
            assert!(stderr.contains("no common ancestor"), "{stderr}");
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[test]
fn git_errors_map_to_git_error() {
    if !git_available() {
        return;
    }
    let s = scenario("errors");
    let repo = discovered(&project_root(&s));
    // An unknown commit.
    assert!(matches!(
        tree_at(&repo, "nope"),
        Err(GitError::UnknownRevision(rev)) if rev == "nope"
    ));
    // A directory that is not a repository.
    let outside = temp_dir("errors-outside");
    let not_a_repo = GitRepo {
        toplevel: outside.clone(),
        project_prefix: String::new(),
    };
    match resolve_rev(&not_a_repo, "HEAD") {
        Err(GitError::NotARepository(dir)) => assert_eq!(dir, outside.display().to_string()),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        tree_at(&not_a_repo, "HEAD"),
        Err(GitError::NotARepository(_))
    ));
    write_files(&outside, &[("cpl.toml", MANIFEST)]);
    let project = load_dir(&outside);
    assert!(matches!(
        current_revision(&project, Some(&not_a_repo)),
        Err(GitError::NotARepository(_))
    ));
    // A directory that does not exist is some other git failure.
    match discover_repo(&outside.join("missing")) {
        Err(GitError::Command { status, stderr, .. }) => {
            assert_eq!(status, 128);
            assert!(!stderr.is_empty());
        }
        other => panic!("{other:?}"),
    }
    // The messages of Section 1.8.
    assert_eq!(
        GitError::UnknownRevision("nope".into()).to_string(),
        "unknown revision nope"
    );
    assert_eq!(
        GitError::NotARepository("/x".into()).to_string(),
        "not a git repository: /x"
    );
    assert_eq!(
        GitError::NotInstalled.to_string(),
        "git is not installed or not on PATH"
    );
    assert_eq!(
        GitError::Command {
            args: vec!["merge-base".into(), "a".into()],
            status: 1,
            stderr: "boom".into()
        }
        .to_string(),
        "git [\"merge-base\", \"a\"] failed with status 1: boom"
    );
}

// ---------------------------------------------------------------------------
// The semantic diff of two commits (Section 9.3)
// ---------------------------------------------------------------------------

#[test]
fn semantic_diff_between_two_commits() {
    if !git_available() {
        return;
    }
    let s = scenario("diff");
    let repo = discovered(&project_root(&s));
    let map_at = |commit: &str| {
        let project = load_at(&repo, commit);
        let revision = Revision {
            commit: Some(commit.to_string()),
            state: RevisionState::Clean,
            digest: source_digest(&project),
        };
        facet_map(&project, revision)
    };
    // `cpl diff main~1..main`: the contract edit.
    let old = map_at(&resolve_rev(&repo, "main~1").expect("old"));
    let new = map_at(&resolve_rev(&repo, "main").expect("new"));
    let diff = semantic_diff(&old, &new);
    assert_eq!(diff.statements.len(), 1);
    let statement = &diff.statements[0];
    assert_eq!(statement.statement, DiffStatementKind::ContractChanged);
    assert_eq!(statement.id.as_str(), "fn_aaaaaaaaaaaaaaaa");
    assert_eq!(statement.path, "demo.pages.first");
    assert_eq!(diff.old.commit.as_deref(), Some(s.first.as_str()));
    assert_eq!(diff.new.commit.as_deref(), Some(s.second.as_str()));
    assert_eq!(diff.old.state, RevisionState::Clean);
    // `cpl diff main`: the working tree against main, after an edit.
    s.repo.write(&[(
        "cpl/src/demo/pages.cpl",
        &PAGES_V2.replace(
            "pub fn other() -> Int {\n    1",
            "pub fn other() -> Int {\n    2",
        ),
    )]);
    let project = load_dir(&project_root(&s));
    let worktree = current_revision(&project, Some(&repo)).expect("revision");
    assert_eq!(worktree.state, RevisionState::Worktree);
    let diff = semantic_diff(&new, &facet_map(&project, worktree));
    let kinds: Vec<(DiffStatementKind, &str)> = diff
        .statements
        .iter()
        .map(|s| (s.statement, s.path.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [(DiffStatementKind::BodyChanged, "demo.pages.other")]
    );
}
