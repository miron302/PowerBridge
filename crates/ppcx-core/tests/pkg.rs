use ppcx_core::bundle::{self, Verdict};
use ppcx_core::inflate;
use ppcx_core::pkg::{self, Extraction};
use ppcx_core::{Config, Event, Machine, Outcome};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn fx(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pkgs").join(rel)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ppcx-pkgtest-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn names(v: &[PathBuf]) -> Vec<String> {
    v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
}

fn extract(pkg_name: &str, tag: &str) -> (Result<Extraction, String>, PathBuf) {
    let base = tmp(tag);
    let dest = base.join("out");
    (pkg::extract_pkg(&fx(pkg_name), &dest), dest)
}

#[test]
fn inflate_matches_python_for_gzip_and_zlib() {
    for name in ["hello", "text", "random_stored", "mixed", "empty"] {
        let raw = std::fs::read(fx(&format!("inflate/{name}.raw"))).unwrap();
        let gz = std::fs::read(fx(&format!("inflate/{name}.gz"))).unwrap();
        let zl = std::fs::read(fx(&format!("inflate/{name}.zlib"))).unwrap();
        assert_eq!(inflate::gunzip(&gz, 1 << 28).unwrap(), raw, "gzip {name}");
        assert_eq!(inflate::zlib_decompress(&zl, 1 << 28).unwrap(), raw, "zlib {name}");
    }
}

#[test]
fn inflate_rejects_truncated_corrupt_and_oversized() {
    let gz = std::fs::read(fx("inflate/text.gz")).unwrap();
    assert!(inflate::gunzip(&gz[..gz.len() / 2], 1 << 28).is_err(), "truncated");
    let mut bad = gz.clone();
    for b in bad.iter_mut().skip(20).take(64) {
        *b ^= 0x5a;
    }
    let _ = inflate::gunzip(&bad, 1 << 28); // must not panic
    let err = inflate::gunzip(&gz, 1000).unwrap_err();
    assert!(err.contains("too large"), "{err}");
}

#[test]
fn bundle_style_pkg() {
    let (r, dest) = extract("Bundle.pkg", "bundle");
    let ex = r.unwrap();
    assert_eq!(names(&ex.apps), vec!["Hello.app"]);
    assert!(ex.files >= 3);
    let a = bundle::analyze(&ex.apps[0]).unwrap();
    assert_eq!(a.verdict, Verdict::Runs);
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
}

#[test]
fn metapackage_finds_app_and_command_line_tool() {
    let (r, dest) = extract("Suite.mpkg", "suite");
    let ex = r.unwrap();
    assert_eq!(names(&ex.apps), vec!["Bin Tool.app"]);
    assert_eq!(names(&ex.binaries), vec!["ppctool"], "only the PowerPC executable, not notes.txt or app internals");
    assert_eq!(bundle::analyze(&ex.binaries[0]).unwrap().verdict, Verdict::Runs);
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
}

#[test]
fn flat_package_with_gzip_newc_payload() {
    let (r, dest) = extract("flat_simple.pkg", "flat_simple");
    let ex = r.unwrap();
    assert_eq!(names(&ex.apps), vec!["Hello.app"]);
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
}

#[test]
fn flat_product_package_with_zlib_wrapped_odc_payload() {
    let (r, dest) = extract("flat_product.pkg", "flat_product");
    let ex = r.unwrap();
    assert_eq!(names(&ex.apps), vec!["Cocoa.app", "Hello.app"]);
    let cocoa = ex.apps.iter().find(|p| p.ends_with("Cocoa.app")).unwrap();
    assert_eq!(bundle::analyze(cocoa).unwrap().verdict, Verdict::Unsupported);
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
}

#[test]
fn app_extracted_from_pkg_actually_runs() {
    let (r, dest) = extract("Bundle.pkg", "runpkg");
    let ex = r.unwrap();
    let exec = bundle::resolve_executable(&ex.apps[0]).unwrap();
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = out.clone();
    let sink = Box::new(move |ev: Event| {
        if let Event::Stdout(b) = ev {
            o2.lock().unwrap().extend(b)
        }
    });
    let mut m = Machine::load(&std::fs::read(&exec).unwrap(), Config::default(), sink).unwrap();
    assert_eq!(m.run(), Outcome::Exited(7));
    assert!(String::from_utf8_lossy(&out.lock().unwrap()).starts_with("sum=55"));
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
}

#[test]
fn hostile_package_cannot_write_outside_destination() {
    let (r, dest) = extract("evil.pkg", "evil");
    let ex = r.unwrap();
    let base = dest.parent().unwrap();
    assert!(!base.join("escape1.txt").exists());
    assert!(!base.join("escape2.txt").exists());
    assert!(!dest.parent().unwrap().parent().unwrap().join("escape2.txt").exists());
    assert!(!base.join("pwned.txt").exists(), "wrote through a symlink");
    assert!(!Path::new("/etc/pwned.txt").exists());
    assert!(!dest.join("00_package/evil_abs").exists(), "absolute-target symlink must be skipped");
    assert!(dest.join("00_package/safe/ok.txt").is_file(), "legitimate files still extracted");
    assert!(dest.join("00_package/abs_name.txt").is_file(), "absolute names are re-rooted inside the folder");
    assert!(ex.warnings.len() >= 3, "{:?}", ex.warnings);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn clear_errors_for_unsupported_or_invalid_packages() {
    let (r, d) = extract("bz.pkg", "bz");
    assert!(r.unwrap_err().contains("bzip2"));
    let _ = std::fs::remove_dir_all(d.parent().unwrap());
    let (r, d) = extract("notapkg.pkg", "notapkg");
    assert!(r.unwrap_err().contains("doesn't look like an installer"));
    let _ = std::fs::remove_dir_all(d.parent().unwrap());
    assert!(pkg::is_pkg_path(Path::new("X.PKG")) && pkg::is_pkg_path(Path::new("a/b.mpkg")) && !pkg::is_pkg_path(Path::new("a.app")));
}

#[test]
fn extraction_json_is_well_formed() {
    let (r, dest) = extract("Suite.mpkg", "json");
    let j = r.unwrap().to_json();
    assert!(j.starts_with("{\"apps\":[") && j.contains("\"binaries\":[") && j.contains("\"warnings\":[]") && j.ends_with('}'));
    let _ = std::fs::remove_dir_all(dest.parent().unwrap());
}
