use ppcx_core::bundle::{self, Verdict};
use ppcx_core::{Config, Event, Machine, Outcome};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn app(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/apps").join(name)
}

#[test]
fn xml_plist_app_runs_verdict() {
    let a = bundle::analyze(&app("Hello.app")).unwrap();
    assert_eq!(a.kind, "app");
    assert_eq!(a.name, "Hello");
    assert_eq!(a.bundle_id.as_deref(), Some("test.ppcx.hello"));
    assert_eq!(a.version.as_deref(), Some("1.2"));
    assert_eq!(a.min_os.as_deref(), Some("10.4"));
    assert_eq!(a.archs, vec!["ppc"]);
    assert!(a.frameworks.is_empty());
    assert!(a.missing.is_empty(), "{:?}", a.missing);
    assert_eq!(a.verdict, Verdict::Runs);
}

#[test]
fn binary_plist_with_spaces_in_name() {
    let a = bundle::analyze(&app("Bin Tool.app")).unwrap();
    assert_eq!(a.name, "Bin Tool");
    assert_eq!(a.bundle_id.as_deref(), Some("test.ppcx.bintool"));
    assert_eq!(a.verdict, Verdict::Runs);
}

#[test]
fn cocoa_app_is_reported_unsupported() {
    let a = bundle::analyze(&app("Cocoa.app")).unwrap();
    assert_eq!(a.verdict, Verdict::Unsupported);
    assert_eq!(a.frameworks, vec!["Cocoa", "Foundation"]);
    assert!(a.missing.contains(&"_NSApplicationMain".to_string()));
    assert!(a.verdict_text.contains("Cocoa"));
}

#[test]
fn unknown_import_is_partial() {
    let a = bundle::analyze(&app("Partial.app")).unwrap();
    assert_eq!(a.verdict, Verdict::Partial);
    assert_eq!(a.missing, vec!["_mystery_fn"]);
}

#[test]
fn json_is_well_formed_and_escaped() {
    let a = bundle::analyze(&app("Bin Tool.app")).unwrap();
    let j = a.to_json();
    assert!(j.starts_with('{') && j.ends_with('}'));
    assert!(j.contains("\"verdict\":\"runs\""));
    assert!(j.contains("\"name\":\"Bin Tool\""));
    assert!(bundle::error_json("a \"quoted\" \\ msg").contains("\\\"quoted\\\""));
}

#[test]
fn running_an_app_bundle_directly() {
    let exec = bundle::resolve_executable(&app("Hello.app")).unwrap();
    assert!(exec.ends_with("Contents/MacOS/hello"));
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = out.clone();
    let sink = Box::new(move |ev: Event| {
        if let Event::Stdout(b) = ev {
            o2.lock().unwrap().extend(b)
        }
    });
    let mut m = Machine::load(&std::fs::read(&exec).unwrap(), Config::default(), sink).unwrap();
    assert_eq!(m.run(), Outcome::Exited(7));
    assert_eq!(String::from_utf8_lossy(&out.lock().unwrap()), "sum=55 fact=3628800 hex=0xff str=PowerPC\n");
}

#[test]
fn wrapper_has_expected_structure() {
    let tmp = std::env::temp_dir().join(format!("ppcx-wrap-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fake_ppcx = tmp.join("ppcx");
    std::fs::write(&fake_ppcx, b"#!/bin/sh\n").unwrap();

    let out = bundle::wrap_app(&app("Hello.app"), &tmp, &fake_ppcx).unwrap();
    assert_eq!(out.file_name().unwrap(), "Hello (Intel).app");
    assert!(out.join("Contents/MacOS/launcher").is_file());
    assert!(out.join("Contents/MacOS/ppcx").is_file());
    assert!(out.join("Contents/PowerPC/Hello.app/Contents/MacOS/hello").is_file());
    assert!(out.join("Contents/Resources/app.icns").is_file(), "icon copied");
    let plist = std::fs::read_to_string(out.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("Hello (Intel)") && plist.contains("<string>launcher</string>"));
    let parsed = bundle::parse_plist(plist.as_bytes()).unwrap();
    assert_eq!(parsed.get("CFBundleExecutable").and_then(|p| p.as_str()), Some("launcher"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(out.join("Contents/MacOS/launcher")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "launcher must be executable");
    }

    // names with spaces survive quoting
    let out2 = bundle::wrap_app(&app("Bin Tool.app"), &tmp, &fake_ppcx).unwrap();
    let script = std::fs::read_to_string(out2.join("Contents/MacOS/launcher")).unwrap();
    assert!(script.contains("'Bin Tool.app'"), "{script}");

    // refuses to wrap a non-PowerPC / missing executable
    assert!(bundle::wrap_app(&tmp, &tmp, &fake_ppcx).is_err());
    let _ = std::fs::remove_dir_all(&tmp);
}
