//! The trivia model: ordinary comments attach as leading trivia to the
//! following node or as trailing trivia to the node on the same line, blank
//! lines are recorded, doc comments become `Docs`, and every comment of the
//! source appears exactly once in the tree.

use cpl_diag::FileId;
use cpl_syntax::ast::*;
use cpl_syntax::{dump_module, dump_patch, dump_work, parse_module, parse_patch, parse_work};

fn module(src: &str) -> Module {
    let r = parse_module(src, FileId(0));
    assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
    r.value.expect("module")
}

fn texts(comments: &[Comment]) -> Vec<&str> {
    comments.iter().map(|c| c.text.as_str()).collect()
}

fn fn_decl(decl: &Declaration) -> &FnDecl {
    match &decl.kind {
        DeclKind::Fn(f) => f,
        other => panic!("expected a fn, got {other:?}"),
    }
}

#[test]
fn comments_attach_to_items_statements_and_bodies() {
    let src = "// license\nmodule m // header\n\n// leading a\n// leading b\nfn f() -> Int { // after brace\n    // before let\n    let x = 1 // trailing let\n\n    // before tail\n    x\n    // dangling in block\n}\n// at end\n";
    let m = module(src);
    assert_eq!(texts(&m.trivia.leading), ["// license"]);
    assert_eq!(texts(&m.trivia.trailing), ["// header"]);
    assert_eq!(m.trailing_comments.len(), 1);
    assert_eq!(m.trailing_comments[0].text, "// at end");

    let decl = &m.declarations[0];
    assert_eq!(
        texts(&decl.trivia.leading),
        ["// leading a", "// leading b"]
    );
    // The blank line sits between the header and the first comment.
    assert!(decl.trivia.leading[0].blank_line_before);
    assert!(!decl.trivia.leading[1].blank_line_before);
    assert!(!decl.trivia.blank_line_before);
    assert!(decl.trivia.trailing.is_empty());

    let body = fn_decl(decl).body.as_ref().expect("body");
    let Statement::Let(let_stmt) = &body.statements[0] else {
        panic!("expected let")
    };
    assert_eq!(
        texts(&let_stmt.trivia.leading),
        ["// after brace", "// before let"]
    );
    assert_eq!(texts(&let_stmt.trivia.trailing), ["// trailing let"]);
    assert!(!let_stmt.trivia.blank_line_before);

    let tail = body.tail.as_ref().expect("tail");
    assert_eq!(texts(&tail.trivia.leading), ["// before tail"]);
    assert!(
        tail.trivia.leading[0].blank_line_before,
        "the blank line precedes the comment"
    );
    assert!(
        !tail.trivia.blank_line_before,
        "no blank line between the comment and the node"
    );
    assert_eq!(texts(&body.trailing_comments), ["// dangling in block"]);
}

#[test]
fn comments_around_headers_clauses_parameters_and_sets() {
    let src = "module m\n\nfn g(\n    // the id\n    id: PageId, // trailing param\n    using db: PageRead,\n) -> Int\n    // why\n    requires id != x\n    effects { db.get, // first\n        db.put }\n{\n    1\n}\n";
    let m = module(src);
    let f = fn_decl(&m.declarations[0]);
    assert_eq!(texts(&f.params.positional[0].trivia.leading), ["// the id"]);
    assert_eq!(
        texts(&f.params.positional[0].trivia.trailing),
        ["// trailing param"]
    );
    assert!(f.params.using[0].trivia.is_empty());
    let FnClause::Contract(requires) = &f.clauses[0] else {
        panic!("expected requires")
    };
    assert_eq!(texts(&requires.trivia.leading), ["// why"]);
    let effects = f.effects().expect("effects");
    assert_eq!(texts(&effects.effects[0].trivia.trailing), ["// first"]);
    assert!(effects.effects[1].trivia.is_empty());
}

#[test]
fn members_arms_and_struct_literal_fields_carry_trivia() {
    let src = "module m\n\nstruct Page {\n    // the id\n    id: PageId // trailing\n\n    /// documented\n    text: Text\n    // dangling\n}\n\nenum E {\n    /// doc a\n    // plain\n    A\n    B // after b\n}\n\nfn f(r: R) -> Int {\n    match r {\n        // first arm\n        Ok(v) => v // ok\n\n        Err(_) => 0\n        // after arms\n    }\n}\n\nfn g(p: Page) -> Page {\n    Page {\n        ..p,\n        // new text\n        text: t, // set\n    }\n}\n";
    let m = module(src);
    let DeclKind::Struct(s) = &m.declarations[0].kind else {
        panic!()
    };
    assert_eq!(texts(&s.fields[0].trivia.leading), ["// the id"]);
    assert_eq!(texts(&s.fields[0].trivia.trailing), ["// trailing"]);
    assert!(s.fields[1].trivia.blank_line_before);
    assert_eq!(
        s.fields[1].docs.as_ref().map(|d| d.text()),
        Some("documented".to_string())
    );
    assert_eq!(texts(&s.trailing_comments), ["// dangling"]);

    let DeclKind::Enum(e) = &m.declarations[1].kind else {
        panic!()
    };
    assert_eq!(
        e.variants[0].docs.as_ref().map(|d| d.text()),
        Some("doc a".to_string())
    );
    assert_eq!(texts(&e.variants[0].trivia.leading), ["// plain"]);
    assert_eq!(texts(&e.variants[1].trivia.trailing), ["// after b"]);

    let f = fn_decl(&m.declarations[2]);
    let ExprKind::Match(mt) = &f.body.as_ref().unwrap().tail.as_ref().unwrap().expr.kind else {
        panic!()
    };
    assert_eq!(texts(&mt.arms[0].trivia.leading), ["// first arm"]);
    assert_eq!(texts(&mt.arms[0].trivia.trailing), ["// ok"]);
    assert!(mt.arms[1].trivia.blank_line_before);
    assert_eq!(texts(&mt.trailing_comments), ["// after arms"]);

    let g = fn_decl(&m.declarations[3]);
    let ExprKind::Struct(sl) = &g.body.as_ref().unwrap().tail.as_ref().unwrap().expr.kind else {
        panic!()
    };
    assert_eq!(texts(&sl.entries.fields[0].trivia.leading), ["// new text"]);
    assert_eq!(texts(&sl.entries.fields[0].trivia.trailing), ["// set"]);
}

#[test]
fn docs_concatenate_across_annotations_and_attach_to_modules() {
    let m = module("/// Module docs\n/// second line\nmodule m\n\n/// a\n/// b\n@id(\"x\")\n/// c\n@prove\npub fn f() -> Int { 1 }\n");
    assert_eq!(
        m.docs.as_ref().map(|d| d.text()),
        Some("Module docs\nsecond line".to_string())
    );
    let decl = &m.declarations[0];
    assert_eq!(
        decl.docs.as_ref().map(|d| d.text()),
        Some("a\nb\nc".to_string())
    );
    assert_eq!(decl.docs.as_ref().unwrap().lines.len(), 3);
    assert_eq!(decl.annotations.len(), 2);
    assert_eq!(decl.id(), Some("x"));
    assert!(decl.has_annotation("prove"));
    assert!(decl.is_pub());
    assert_eq!(decl.name().map(|n| n.text.as_str()), Some("f"));
    // Comments between prefix lines and the keyword are leading trivia of the declaration.
    let m = module("module m\n\n/// doc\n// note\n@prove\n// another\nfn f() -> Int { 1 }\n");
    assert_eq!(
        texts(&m.declarations[0].trivia.leading),
        ["// note", "// another"]
    );
    assert_eq!(
        m.declarations[0].docs.as_ref().map(|d| d.text()),
        Some("doc".to_string())
    );
}

#[test]
fn blank_lines_between_items_and_statements_are_recorded() {
    let m = module("module m\nfn a() -> Int { 1 }\nfn b() -> Int { 2 }\n\n\nfn c() -> Int {\n    let x = 1\n\n    let y = 2\n    let z = 3\n    x\n}\n");
    assert!(!m.declarations[0].trivia.blank_line_before);
    assert!(!m.declarations[1].trivia.blank_line_before);
    assert!(m.declarations[2].trivia.blank_line_before);
    let body = fn_decl(&m.declarations[2]).body.as_ref().expect("body");
    assert!(!body.statements[0].trivia().blank_line_before);
    assert!(body.statements[1].trivia().blank_line_before);
    assert!(!body.statements[2].trivia().blank_line_before);
    assert!(!body.tail.as_ref().unwrap().trivia.blank_line_before);
    // The dump records the blank line too.
    let d = dump_module(&m);
    assert!(d.contains("(let y (int 2) (trivia blank))"), "{d}");
}

#[test]
fn work_and_patch_trivia() {
    let src = "// top\nuse a.b.{f} // uses\n\n/// The change.\nchange c {\n    // where\n    base git(\"main\") // b\n    target git(\"main\")\n\n    /// A task.\n    task t {\n        preserve {\n            // sig\n            f.signature,\n            f.contract, // c\n        }\n        goal \"g\"\n    }\n    // end of change\n}\n// eof\n";
    let r = parse_work(src, FileId(0));
    assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
    let w = r.value.unwrap();
    assert_eq!(texts(&w.uses[0].trivia.leading), ["// top"]);
    assert_eq!(texts(&w.uses[0].trivia.trailing), ["// uses"]);
    let c = &w.changes[0];
    assert_eq!(
        c.docs.as_ref().map(|d| d.text()),
        Some("The change.".to_string())
    );
    assert!(c.trivia.blank_line_before);
    assert_eq!(texts(&c.items[0].trivia.leading), ["// where"]);
    assert_eq!(texts(&c.items[0].trivia.trailing), ["// b"]);
    assert!(c.items[2].trivia.blank_line_before);
    let ChangeItemKind::Task(t) = &c.items[2].kind else {
        panic!()
    };
    assert_eq!(
        t.docs.as_ref().map(|d| d.text()),
        Some("A task.".to_string())
    );
    let TaskItemKind::Preserve(set) = &t.items[0].kind else {
        panic!()
    };
    assert_eq!(texts(&set.selectors[0].trivia.leading), ["// sig"]);
    assert_eq!(texts(&set.selectors[1].trivia.trailing), ["// c"]);
    assert_eq!(texts(&c.trailing_comments), ["// end of change"]);
    assert_eq!(texts(&w.trailing_comments), ["// eof"]);

    let src = "/// Replace the body.\npatch f.body\n    // check\n    expect base(f.body) // e\n{\n    // inside\n    1\n}\n";
    let r = parse_patch(src, FileId(0));
    assert!(r.diagnostics.is_empty(), "{:#?}", r.diagnostics);
    let p = r.value.unwrap();
    assert_eq!(
        p.forms[0].docs.as_ref().map(|d| d.text()),
        Some("Replace the body.".to_string())
    );
    assert_eq!(texts(&p.forms[0].expect.trivia.leading), ["// check"]);
    assert_eq!(texts(&p.forms[0].expect.trivia.trailing), ["// e"]);
    assert_eq!(
        texts(&p.forms[0].body.tail.as_ref().unwrap().trivia.leading),
        ["// inside"]
    );
}

/// Count `//` comment starts in a source that has no `//` inside text
/// literals and no `////` banners (doc comments are excluded).
fn comment_starts(src: &str) -> usize {
    let bytes = src.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'/' {
            let doc = i + 2 < bytes.len() && bytes[i + 2] == b'/';
            if !doc {
                count += 1;
            }
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        }
        i += 1;
    }
    count
}

#[test]
fn every_comment_appears_exactly_once_in_the_tree() {
    let mut sources: Vec<(String, String)> = Vec::new();
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "cpl" | "work" | "patch") {
                    out.push((
                        ext.to_string(),
                        std::fs::read_to_string(&path).expect("read"),
                    ));
                }
            }
        }
    }
    walk(&examples, &mut sources);
    sources.push((
        "cpl".into(),
        "// a\nmodule m // b\n// c\n\n// d\nuse x.y.{z} // e\n\n/// doc\n// f\n@id(\"i\") // g\n// h\npub fn f( // i\n    a: Int, // j\n    // k\n    b: Int,\n) -> Int // l\n    // m\n    requires a > 0 // n\n    // o\n{ // p\n    // q\n    let x = [ // r\n        1, // s\n        // t\n        2,\n    ] // u\n    // v\n    if a > b { // w\n        1 // x\n    } else { // y\n        2\n        // z\n    }\n    // aa\n}\n// bb\n// cc\n"
            .into(),
    ));
    sources.push((
        "work".into(),
        "// a\nuse p.{f}\n// b\nchange c { // c\n    // d\n    base git(\"m\") // e\n    target git(\"m\")\n    task t after x { // f\n        write { // g\n            f.body, // h\n            // i\n        }\n        // j\n        goal \"g\" // k\n    } // l\n    integrate { // m\n        require typecheck // n\n        // o\n    }\n    // p\n}\n// q\n"
            .into(),
    ));
    assert!(sources.len() >= 8);
    for (ext, src) in &sources {
        let dump = match ext.as_str() {
            "cpl" => {
                let r = parse_module(src, FileId(0));
                assert!(r.diagnostics.is_empty(), "{src}\n{:#?}", r.diagnostics);
                dump_module(&r.value.unwrap())
            }
            "work" => {
                let r = parse_work(src, FileId(0));
                assert!(r.diagnostics.is_empty(), "{src}\n{:#?}", r.diagnostics);
                dump_work(&r.value.unwrap())
            }
            _ => {
                let r = parse_patch(src, FileId(0));
                assert!(r.diagnostics.is_empty(), "{src}\n{:#?}", r.diagnostics);
                dump_patch(&r.value.unwrap())
            }
        };
        let in_dump = dump.matches("\"//").count();
        assert_eq!(
            in_dump,
            comment_starts(src),
            "comment count mismatch for:\n{src}\n{dump}"
        );
    }
}

#[test]
fn comments_inside_closed_or_enclosing_brackets_trail_the_innermost_node_containing_them() {
    // `spec/toolchain.md` 2.3 rules D and G: no node after a bracket pair
    // that owns no comments (a parenthesized expression, a generic list)
    // claims a comment written inside it, and no node inside a bracket pair
    // claims a comment of the enclosing sequence written before the pair
    // ("within the same enclosing sequence"); each is trailing trivia of the
    // innermost attachable node containing it.
    let src = "module m\n\nfn f(x: List<Int // in generic\n>) -> Int {\n    if (c // in header parens\n    ) {\n        let p = (// before brackets\n            [b])\n        g(p)\n    }\n    0\n}\n";
    let m = module(src);
    let f = fn_decl(&m.declarations[0]);
    assert_eq!(
        texts(&f.params.positional[0].trivia.trailing),
        ["// in generic"]
    );
    let body = f.body.as_ref().expect("body");
    let Statement::Expr(if_stmt) = &body.statements[0] else {
        panic!("expected the if statement")
    };
    assert!(if_stmt.trivia.leading.is_empty());
    assert_eq!(texts(&if_stmt.trivia.trailing), ["// in header parens"]);
    let ExprKind::If(i) = &if_stmt.expr.kind else {
        panic!("expected an if")
    };
    let Statement::Let(p) = &i.then_block.statements[0] else {
        panic!("expected let")
    };
    assert!(p.trivia.leading.is_empty());
    assert_eq!(texts(&p.trivia.trailing), ["// before brackets"]);
    let ExprKind::Paren(inner) = &p.value.kind else {
        panic!("expected a parenthesized expression")
    };
    let ExprKind::List(list) = &inner.kind else {
        panic!("expected a list")
    };
    assert!(list.elements[0].trivia.is_empty());
    let tail = i.then_block.tail.as_ref().expect("tail");
    let ExprKind::Call(call) = &tail.expr.kind else {
        panic!("expected a call")
    };
    assert!(call.args.positional[0].trivia.is_empty());
}
