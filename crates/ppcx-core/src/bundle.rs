//! `.app` bundle support: Info.plist parsing (XML + binary), compatibility analysis,
//! and generation of a standalone wrapper app around a PowerPC app.

use crate::{hle, macho};
use std::fs;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ plist

#[derive(Debug, Clone, PartialEq)]
pub enum Plist {
    Str(String),
    Int(i64),
    Bool(bool),
    Arr(Vec<Plist>),
    Dict(Vec<(String, Plist)>),
    Other,
}

impl Plist {
    pub fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Plist::Str(s) => Some(s),
            _ => None,
        }
    }
}

pub fn parse_plist(data: &[u8]) -> Result<Plist, String> {
    if data.starts_with(b"bplist00") {
        parse_bplist(data)
    } else {
        parse_xml_plist(&String::from_utf8_lossy(data))
    }
}

enum Tok {
    Open(String),
    Close(String),
    Empty(String),
    Text(String),
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i] == b'<' {
            if src[i..].starts_with("<!--") {
                match src[i..].find("-->") {
                    Some(e) => i += e + 3,
                    None => break,
                }
                continue;
            }
            let Some(e) = src[i..].find('>') else { break };
            let inner = &src[i + 1..i + e];
            i += e + 1;
            if inner.starts_with('?') || inner.starts_with('!') {
                continue;
            }
            if let Some(n) = inner.strip_prefix('/') {
                out.push(Tok::Close(n.trim().to_string()));
            } else if let Some(n) = inner.strip_suffix('/') {
                out.push(Tok::Empty(n.split_whitespace().next().unwrap_or("").to_string()));
            } else {
                out.push(Tok::Open(inner.split_whitespace().next().unwrap_or("").to_string()));
            }
        } else {
            let end = src[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            let t = &src[i..end];
            if !t.trim().is_empty() {
                out.push(Tok::Text(unescape(t)));
            }
            i = end;
        }
    }
    out
}

fn text_until_close(t: &[Tok], p: &mut usize, name: &str) -> Result<String, String> {
    let mut s = String::new();
    loop {
        match t.get(*p) {
            Some(Tok::Text(x)) => {
                s.push_str(x);
                *p += 1;
            }
            Some(Tok::Close(c)) if c == name => {
                *p += 1;
                return Ok(s);
            }
            _ => return Err(format!("malformed <{name}> element in plist")),
        }
    }
}

fn parse_val(t: &[Tok], p: &mut usize) -> Result<Plist, String> {
    match t.get(*p) {
        Some(Tok::Open(n)) => {
            let n = n.clone();
            *p += 1;
            match n.as_str() {
                "dict" => {
                    let mut v = Vec::new();
                    loop {
                        match t.get(*p) {
                            Some(Tok::Close(c)) if c == "dict" => {
                                *p += 1;
                                break;
                            }
                            Some(Tok::Open(k)) if k == "key" => {
                                *p += 1;
                                let key = text_until_close(t, p, "key")?;
                                let val = parse_val(t, p)?;
                                v.push((key, val));
                            }
                            _ => return Err("malformed <dict> in plist".into()),
                        }
                    }
                    Ok(Plist::Dict(v))
                }
                "array" => {
                    let mut v = Vec::new();
                    loop {
                        if let Some(Tok::Close(c)) = t.get(*p) {
                            if c == "array" {
                                *p += 1;
                                break;
                            }
                        }
                        if *p >= t.len() {
                            return Err("unterminated <array> in plist".into());
                        }
                        v.push(parse_val(t, p)?);
                    }
                    Ok(Plist::Arr(v))
                }
                "string" | "integer" | "real" | "date" | "data" => {
                    let text = text_until_close(t, p, &n)?;
                    Ok(match n.as_str() {
                        "string" => Plist::Str(text),
                        "integer" => text.trim().parse().map(Plist::Int).unwrap_or(Plist::Other),
                        _ => Plist::Other,
                    })
                }
                "plist" => {
                    let v = parse_val(t, p)?;
                    if let Some(Tok::Close(c)) = t.get(*p) {
                        if c == "plist" {
                            *p += 1;
                        }
                    }
                    Ok(v)
                }
                other => Err(format!("unexpected <{other}> in plist")),
            }
        }
        Some(Tok::Empty(n)) => {
            *p += 1;
            Ok(match n.as_str() {
                "true" => Plist::Bool(true),
                "false" => Plist::Bool(false),
                "string" => Plist::Str(String::new()),
                "dict" => Plist::Dict(vec![]),
                "array" => Plist::Arr(vec![]),
                _ => Plist::Other,
            })
        }
        _ => Err("unexpected end of plist".into()),
    }
}

fn parse_xml_plist(src: &str) -> Result<Plist, String> {
    let toks = tokenize(src);
    let mut p = 0;
    parse_val(&toks, &mut p)
}

fn be_n(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |a, &x| (a << 8) | x as u64)
}

struct BPlist<'a> {
    d: &'a [u8],
    osz: usize,
    rsz: usize,
    n: usize,
    otoff: usize,
}

impl<'a> BPlist<'a> {
    fn slice(&self, off: usize, len: usize) -> Result<&'a [u8], String> {
        self.d
            .get(off..off.checked_add(len).ok_or("bad plist offset")?)
            .ok_or_else(|| "binary plist truncated".to_string())
    }

    fn offset(&self, idx: usize) -> Result<usize, String> {
        if idx >= self.n {
            return Err("bad object reference in binary plist".into());
        }
        Ok(be_n(self.slice(self.otoff + idx * self.osz, self.osz)?) as usize)
    }

    fn len_at(&self, off: usize, lo: usize) -> Result<(usize, usize), String> {
        if lo < 0xF {
            return Ok((lo, off + 1));
        }
        let m = *self.slice(off + 1, 1)?.first().unwrap();
        if m >> 4 != 1 {
            return Err("bad length marker in binary plist".into());
        }
        let nb = 1usize << (m & 0xF);
        Ok((be_n(self.slice(off + 2, nb)?) as usize, off + 2 + nb))
    }

    fn obj(&self, idx: usize, depth: u32) -> Result<Plist, String> {
        if depth > 32 {
            return Err("binary plist nested too deeply".into());
        }
        let off = self.offset(idx)?;
        let m = self.slice(off, 1)?[0];
        let (hi, lo) = (m >> 4, (m & 0xF) as usize);
        Ok(match hi {
            0x0 => match m {
                0x08 => Plist::Bool(false),
                0x09 => Plist::Bool(true),
                _ => Plist::Other,
            },
            0x1 => {
                let len = 1usize << lo;
                if len > 8 {
                    Plist::Other
                } else {
                    Plist::Int(be_n(self.slice(off + 1, len)?) as i64)
                }
            }
            0x5 => {
                let (len, start) = self.len_at(off, lo)?;
                Plist::Str(String::from_utf8_lossy(self.slice(start, len)?).into_owned())
            }
            0x6 => {
                let (len, start) = self.len_at(off, lo)?;
                let raw = self.slice(start, len * 2)?;
                let u: Vec<u16> = raw.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Plist::Str(String::from_utf16_lossy(&u))
            }
            0xA => {
                let (len, start) = self.len_at(off, lo)?;
                let mut v = Vec::new();
                for i in 0..len {
                    let r = be_n(self.slice(start + i * self.rsz, self.rsz)?) as usize;
                    v.push(self.obj(r, depth + 1)?);
                }
                Plist::Arr(v)
            }
            0xD => {
                let (len, start) = self.len_at(off, lo)?;
                let mut v = Vec::new();
                for i in 0..len {
                    let kr = be_n(self.slice(start + i * self.rsz, self.rsz)?) as usize;
                    let vr = be_n(self.slice(start + (len + i) * self.rsz, self.rsz)?) as usize;
                    if let Plist::Str(k) = self.obj(kr, depth + 1)? {
                        v.push((k, self.obj(vr, depth + 1)?));
                    }
                }
                Plist::Dict(v)
            }
            _ => Plist::Other,
        })
    }
}

fn parse_bplist(d: &[u8]) -> Result<Plist, String> {
    if d.len() < 40 {
        return Err("binary plist too small".into());
    }
    let tr = &d[d.len() - 32..];
    let b = BPlist {
        d,
        osz: tr[6] as usize,
        rsz: tr[7] as usize,
        n: be_n(&tr[8..16]) as usize,
        otoff: be_n(&tr[24..32]) as usize,
    };
    if b.osz == 0 || b.osz > 8 || b.rsz == 0 || b.rsz > 8 {
        return Err("corrupt binary plist trailer".into());
    }
    b.obj(be_n(&tr[16..24]) as usize, 0)
}

// ------------------------------------------------------------------ bundle

#[derive(Debug, Clone)]
pub struct AppBundle {
    pub path: PathBuf,
    pub plist: Plist,
    pub exec_path: PathBuf,
}

impl AppBundle {
    pub fn open(path: &Path) -> Result<AppBundle, String> {
        if !path.is_dir() {
            return Err(format!("{} is not an app bundle (not a directory)", path.display()));
        }
        let pl = path.join("Contents/Info.plist");
        let data = fs::read(&pl).map_err(|e| format!("cannot read {}: {e}", pl.display()))?;
        let plist = parse_plist(&data)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let exe = plist
            .get("CFBundleExecutable")
            .and_then(Plist::as_str)
            .map(String::from)
            .unwrap_or(stem);
        let exec_path = path.join("Contents/MacOS").join(&exe);
        if !exec_path.is_file() {
            return Err(format!("bundle executable not found: Contents/MacOS/{exe}"));
        }
        Ok(AppBundle { path: path.to_path_buf(), plist, exec_path })
    }

    fn key(&self, k: &str) -> Option<String> {
        self.plist.get(k).and_then(Plist::as_str).map(String::from)
    }

    pub fn name(&self) -> String {
        self.key("CFBundleDisplayName")
            .or_else(|| self.key("CFBundleName"))
            .unwrap_or_else(|| {
                self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("App").to_string()
            })
    }
}

/// Accepts either a Mach-O file or an `.app` bundle and returns the executable to run.
pub fn resolve_executable(path: &Path) -> Result<PathBuf, String> {
    if path.is_dir() {
        Ok(AppBundle::open(path)?.exec_path)
    } else {
        Ok(path.to_path_buf())
    }
}

// ------------------------------------------------------------------ analysis

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Runs,
    Partial,
    Unsupported,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Runs => "runs",
            Verdict::Partial => "partial",
            Verdict::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub kind: String, // "app" | "binary"
    pub path: String,
    pub exec_path: String,
    pub name: String,
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    pub min_os: Option<String>,
    pub archs: Vec<String>,
    pub libraries: Vec<String>,
    pub frameworks: Vec<String>,
    pub imports_total: usize,
    pub missing: Vec<String>,
    pub verdict: Verdict,
    pub verdict_text: String,
}

fn framework_name(path: &str) -> Option<String> {
    let idx = path.find(".framework/")?;
    let head = &path[..idx];
    Some(head.rsplit('/').next().unwrap_or(head).to_string())
}

pub fn analyze(path: &Path) -> Result<Analysis, String> {
    let bundle = if path.is_dir() { Some(AppBundle::open(path)?) } else { None };
    let exec = bundle.as_ref().map(|b| b.exec_path.clone()).unwrap_or_else(|| path.to_path_buf());
    let data = fs::read(&exec).map_err(|e| format!("cannot read {}: {e}", exec.display()))?;
    let mo = macho::parse(&data)?;

    let mut frameworks: Vec<String> = Vec::new();
    let mut other_libs: Vec<String> = Vec::new();
    for l in &mo.dylibs {
        if let Some(f) = framework_name(l) {
            if !frameworks.contains(&f) {
                frameworks.push(f);
            }
        } else if !["libSystem", "libgcc_s", "libstdc++", "libm.", "libc."]
            .iter()
            .any(|p| l.rsplit('/').next().unwrap_or("").starts_with(p))
        {
            other_libs.push(l.clone());
        }
    }
    let imports = mo.imports();
    let missing: Vec<String> = imports
        .iter()
        .filter(|s| !hle::is_supported_import(s))
        .map(|s| s.to_string())
        .collect();

    let (verdict, verdict_text) = if !frameworks.is_empty() {
        (
            Verdict::Unsupported,
            format!(
                "Links PowerPC system frameworks ({}). Running GUI/Cocoa/Carbon apps would need those \
                 frameworks and the Objective-C runtime, which this translator does not provide.",
                frameworks.join(", ")
            ),
        )
    } else if !missing.is_empty() || !other_libs.is_empty() {
        (
            Verdict::Partial,
            format!(
                "Uses {} import(s) the translator doesn't provide{}; it will run until it calls one.",
                missing.len(),
                if other_libs.is_empty() { String::new() } else { format!(" and links {}", other_libs.join(", ")) }
            ),
        )
    } else {
        (
            Verdict::Runs,
            "Only uses features the translator provides (syscalls and the supported libSystem subset).".into(),
        )
    };

    let (name, bundle_id, version, min_os) = match &bundle {
        Some(b) => (
            b.name(),
            b.key("CFBundleIdentifier"),
            b.key("CFBundleShortVersionString").or_else(|| b.key("CFBundleVersion")),
            b.key("LSMinimumSystemVersion"),
        ),
        None => (
            path.file_name().and_then(|s| s.to_str()).unwrap_or("binary").to_string(),
            None,
            None,
            None,
        ),
    };

    Ok(Analysis {
        kind: if bundle.is_some() { "app".into() } else { "binary".into() },
        path: path.display().to_string(),
        exec_path: exec.display().to_string(),
        name,
        bundle_id,
        version,
        min_os,
        archs: mo.archs.clone(),
        libraries: mo.dylibs.clone(),
        frameworks,
        imports_total: imports.len(),
        missing,
        verdict,
        verdict_text,
    })
}

pub fn json_string(s: &str) -> String {
    js(s)
}

fn js(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn js_opt(s: &Option<String>) -> String {
    s.as_deref().map(js).unwrap_or_else(|| "null".into())
}

fn js_arr(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| js(s)).collect::<Vec<_>>().join(","))
}

pub fn error_json(msg: &str) -> String {
    format!("{{\"error\":{}}}", js(msg))
}

impl Analysis {
    /// JSON with snake_case keys (consumed by the Swift app).
    pub fn to_json(&self) -> String {
        format!(
            "{{\"kind\":{},\"path\":{},\"exec_path\":{},\"name\":{},\"bundle_id\":{},\"version\":{},\"min_os\":{},\
             \"archs\":{},\"libraries\":{},\"frameworks\":{},\"imports_total\":{},\"missing\":{},\
             \"verdict\":{},\"verdict_text\":{}}}",
            js(&self.kind), js(&self.path), js(&self.exec_path), js(&self.name),
            js_opt(&self.bundle_id), js_opt(&self.version), js_opt(&self.min_os),
            js_arr(&self.archs), js_arr(&self.libraries), js_arr(&self.frameworks),
            self.imports_total, js_arr(&self.missing),
            js(self.verdict.as_str()), js(&self.verdict_text)
        )
    }

    pub fn report(&self) -> String {
        let mut s = format!("{} ({})\n", self.name, self.kind);
        if let Some(v) = &self.bundle_id { s.push_str(&format!("  identifier : {v}\n")); }
        if let Some(v) = &self.version { s.push_str(&format!("  version    : {v}\n")); }
        s.push_str(&format!("  executable : {}\n", self.exec_path));
        s.push_str(&format!("  archs      : {}\n", self.archs.join(", ")));
        if !self.frameworks.is_empty() { s.push_str(&format!("  frameworks : {}\n", self.frameworks.join(", "))); }
        s.push_str(&format!("  imports    : {} total, {} unsupported\n", self.imports_total, self.missing.len()));
        if !self.missing.is_empty() {
            s.push_str(&format!("  unsupported: {}\n", self.missing.join(" ")));
        }
        s.push_str(&format!("  verdict    : {} - {}\n", self.verdict.as_str(), self.verdict_text));
        s
    }
}

// ------------------------------------------------------------------ wrapper generation

fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let ft = e.file_type()?;
        if ft.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(&from)?, &to)?;
        } else if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn set_exec(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o755))?;
    }
    let _ = p;
    Ok(())
}

/// Create "<Name> (PowerBridge).app": a normal Mac app whose launcher runs the embedded PowerPC app
/// through the `ppcx` translator binary. Returns the path of the new bundle.
pub fn wrap_app(src: &Path, dest_dir: &Path, ppcx_bin: &Path) -> Result<PathBuf, String> {
    let b = AppBundle::open(src)?;
    // Validate it is actually a PowerPC executable before wrapping.
    let data = fs::read(&b.exec_path).map_err(|e| e.to_string())?;
    macho::parse(&data)?;
    if !ppcx_bin.is_file() {
        return Err(format!("translator binary not found: {}", ppcx_bin.display()));
    }
    let name: String = b.name().chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
    let out = dest_dir.join(format!("{name} (PowerBridge).app"));
    if out.starts_with(src) {
        return Err("destination must not be inside the source app".into());
    }
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("cannot replace existing {}: {e}", out.display()))?;
    }
    let io = |e: std::io::Error| e.to_string();
    let macos = out.join("Contents/MacOS");
    let resources = out.join("Contents/Resources");
    let inner_dir = out.join("Contents/PowerPC");
    fs::create_dir_all(&macos).map_err(io)?;
    fs::create_dir_all(&resources).map_err(io)?;
    fs::create_dir_all(&inner_dir).map_err(io)?;

    let app_file = src.file_name().and_then(|s| s.to_str()).unwrap_or("App.app").to_string();
    copy_dir(src, &inner_dir.join(&app_file)).map_err(io)?;
    fs::copy(ppcx_bin, macos.join("ppcx")).map_err(io)?;
    set_exec(&macos.join("ppcx")).map_err(io)?;

    let exe_rel = b.exec_path.strip_prefix(src).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         # Generated by PowerBridge: runs the embedded PowerPC app through the ppcx translator.\n\
         HERE=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
         EXE=\"$HERE/../PowerPC\"/{app}/{exe}\n\
         if [ -z \"$PPCX_IN_TERMINAL\" ] && [ ! -t 1 ]; then\n  \
           # launched from Finder: show the program's console in Terminal\n  \
           TMP=\"$(mktemp /tmp/ppcx.XXXXXX)\"; mv \"$TMP\" \"$TMP.command\"; TMP=\"$TMP.command\"\n  \
           printf '#!/bin/sh\\nPPCX_IN_TERMINAL=1 \"%s\"\\necho\\necho \"[ppcx: process finished]\"\\n' \"$0\" > \"$TMP\"\n  \
           chmod +x \"$TMP\"; open -a Terminal \"$TMP\"; exit 0\n\
         fi\n\
         exec \"$HERE/ppcx\" \"$EXE\" \"$@\"\n",
        app = sq(&app_file),
        exe = sq(&exe_rel)
    );
    fs::write(macos.join("launcher"), script).map_err(io)?;
    set_exec(&macos.join("launcher")).map_err(io)?;

    // Icon (best effort)
    let mut icon_key = String::new();
    if let Some(icon) = b.key("CFBundleIconFile") {
        let file = if icon.contains('.') { icon.clone() } else { format!("{icon}.icns") };
        let from = src.join("Contents/Resources").join(&file);
        if from.is_file() && fs::copy(&from, resources.join(&file)).is_ok() {
            icon_key = format!("<key>CFBundleIconFile</key><string>{}</string>\n", xml_esc(&file));
        }
    }
    let ident: String = b
        .key("CFBundleIdentifier")
        .unwrap_or_else(|| name.chars().filter(|c| c.is_ascii_alphanumeric()).collect());
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>CFBundleName</key><string>{name} (PowerBridge)</string>\n\
         <key>CFBundleExecutable</key><string>launcher</string>\n\
         <key>CFBundleIdentifier</key><string>local.powerbridge.{ident}</string>\n\
         <key>CFBundlePackageType</key><string>APPL</string>\n\
         <key>CFBundleVersion</key><string>1</string>\n\
         <key>LSUIElement</key><false/>\n\
         {icon_key}</dict></plist>\n",
        name = xml_esc(&name),
        ident = xml_esc(&ident),
        icon_key = icon_key
    );
    fs::write(out.join("Contents/Info.plist"), plist).map_err(io)?;
    Ok(out)
}
