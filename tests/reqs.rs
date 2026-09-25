//! End-to-end: drive `multi reqs` against a temporary database.

use std::path::Path;
use std::process::Command;

fn reqs(db: &Path, args: &[&str]) -> (bool, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_multi"))
        .arg("reqs")
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("run multi reqs");
    (
        o.status.success(),
        String::from_utf8(o.stdout).unwrap(),
        String::from_utf8(o.stderr).unwrap(),
    )
}

fn ok(db: &Path, args: &[&str]) -> String {
    let (success, out, err) = reqs(db, args);
    assert!(success, "multi reqs {args:?} failed:\n{err}");
    out
}

fn example() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/reqs/examples/api.dl")
        .display()
        .to_string()
}

#[test]
fn end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db");

    assert!(
        !reqs(&db, &["status"]).0,
        "commands other than init need an existing db"
    );
    ok(&db, &["init"]);
    ok(&db, &["load", &example()]);
    ok(&db, &["dl", "check"]);
    ok(&db, &["compute"]);

    let status = ok(&db, &["status", "--required"]);
    assert!(
        status.contains("partial  required  api_ready  ready 1/3 partial"),
        "{status}"
    );

    let trace = ok(&db, &["trace", r#"handler(put, "/posts/{id}")"#]);
    assert!(
        trace.contains("resource(posts)  [partial]  via resource_crud 3/5 partial"),
        "{trace}"
    );
    assert!(
        trace.contains("api_ready  [partial] required  via ready 1/3 partial"),
        "{trace}"
    );

    let explain = ok(&db, &["explain", "authenticated_requests"]);
    assert!(
        explain.contains("alternative auth_via_token 1/1 satisfied"),
        "{explain}"
    );

    let why = ok(&db, &["dl", "why", "sat(authenticated_requests)"]);
    assert!(
        why.contains("evidence(token_auth, \"src/auth/jwt.rs\")  (asserted)"),
        "{why}"
    );

    assert_eq!(ok(&db, &["violations"]).trim(), "no violations");
    ok(
        &db,
        &["evidence", "add", "session_auth", "--source", "legacy.rs"],
    );
    let (_, _, warn) = reqs(&db, &["violations"]);
    assert!(warn.contains("out of date"), "{warn}");
    ok(&db, &["compute"]);
    let v = ok(&db, &["violations"]);
    assert!(
        v.contains("exclusive(session_auth, token_auth)  (rule prelude_exclusive)"),
        "{v}"
    );

    let q = ok(
        &db,
        &[
            "--format",
            "json",
            "dl",
            "query",
            "candidate(D, resource(R))",
        ],
    );
    let rows: serde_json::Value = serde_json::from_str(&q).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);

    // Invalid changes are rejected and leave the store untouched.
    let (success, _, err) = reqs(&db, &["require", "handler(get, 5)"]);
    assert!(!success && err.contains("expects string"), "{err}");

    // Export round-trips through a fresh database.
    let exported = ok(&db, &["export"]);
    let file = dir.path().join("export.dl");
    std::fs::write(&file, &exported).unwrap();
    let db2 = dir.path().join("t2.db");
    ok(&db2, &["init"]);
    ok(&db2, &["load", file.to_str().unwrap()]);
    assert_eq!(ok(&db2, &["export"]), exported);
}
