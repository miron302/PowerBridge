//! Installer package (.pkg / .mpkg) support. PowerBridge only *looks inside* a package: it unpacks the
//! payload into a folder you choose, never runs installer scripts, and reports the PowerPC apps and
//! programs it finds.

use crate::inflate;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_PAYLOAD: usize = 1 << 31; // 2 GiB of decompressed data per payload
const MAX_FILES: usize = 400_000;
const MAX_WARNINGS: usize = 50;

#[derive(Debug, Default, Clone)]
pub struct Extraction {
    /// `.app` bundles found inside the package (absolute paths in the extraction folder).
    pub apps: Vec<PathBuf>,
    /// PowerPC command-line programs found outside any `.app`.
    pub binaries: Vec<PathBuf>,
    pub files: usize,
    pub bytes: u64,
    pub warnings: Vec<String>,
}

impl Extraction {
    fn warn(&mut self, msg: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(msg);
        }
    }

    pub fn to_json(&self) -> String {
        let arr = |v: &[PathBuf]| {
            format!(
                "[{}]",
                v.iter().map(|p| crate::bundle::json_string(&p.display().to_string())).collect::<Vec<_>>().join(",")
            )
        };
        let warns = format!(
            "[{}]",
            self.warnings.iter().map(|w| crate::bundle::json_string(w)).collect::<Vec<_>>().join(",")
        );
        format!(
            "{{\"apps\":{},\"binaries\":{},\"files\":{},\"bytes\":{},\"warnings\":{}}}",
            arr(&self.apps), arr(&self.binaries), self.files, self.bytes, warns
        )
    }
}

pub fn is_pkg_path(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("pkg") | Some("mpkg")
    )
}

/// Unpack `pkg` into `dest` and report what was found.
pub fn extract_pkg(pkg: &Path, dest: &Path) -> Result<Extraction, String> {
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let mut ex = Extraction::default();
    let mut payloads = 0usize;
    if pkg.is_dir() {
        bundle_payloads(pkg, pkg, dest, &mut ex, &mut payloads, 0)?;
    } else {
        let len = fs::metadata(pkg).map_err(|e| format!("cannot read {}: {e}", pkg.display()))?.len();
        if len > (4u64 << 30) {
            return Err("this installer package is too large to open (over 4 GB)".into());
        }
        let data = fs::read(pkg).map_err(|e| format!("cannot read {}: {e}", pkg.display()))?;
        if data.starts_with(b"xar!") {
            flat_payloads(&data, dest, &mut ex, &mut payloads)?;
        } else {
            return Err("this doesn't look like an installer package (.pkg or .mpkg)".into());
        }
    }
    if payloads == 0 {
        return Err("this package doesn't contain any files to look at".into());
    }
    ex.apps = find_apps(dest);
    ex.binaries.sort();
    Ok(ex)
}

// ------------------------------------------------------------------ old-style (bundle) packages

fn bundle_payloads(
    root: &Path,
    dir: &Path,
    dest: &Path,
    ex: &mut Extraction,
    payloads: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 10 {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let ft = match e.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let p = e.path();
        if ft.is_dir() {
            bundle_payloads(root, &p, dest, ex, payloads, depth + 1)?;
        } else if ft.is_file() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("Archive.") && !name.ends_with(".bom") {
                let rel = dir.strip_prefix(root).unwrap_or(Path::new(""));
                let label = label_for(rel.to_string_lossy().as_ref());
                let sub = dest.join(format!("{:02}_{label}", *payloads));
                *payloads += 1;
                let data = fs::read(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
                extract_archive_bytes(&data, &sub, ex)?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ flat (xar) packages

struct XarFile {
    path: String,
    offset: u64,
    length: u64,
    style: String,
}

#[derive(Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &s[ks..i];
        while i < b.len() && (b[i] == b'=' || b[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
            let q = b[i];
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            out.push((key.to_string(), unescape(&s[vs..i.min(b.len())])));
            i += 1;
        } else if key.is_empty() {
            break;
        }
    }
    out
}

fn parse_xml(src: &str) -> Result<Node, String> {
    let mut stack: Vec<Node> = vec![Node::default()];
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if src[i..].starts_with("<!--") {
                i += src[i..].find("-->").map(|e| e + 3).ok_or("bad package table of contents")?;
                continue;
            }
            let end = src[i..].find('>').ok_or("bad package table of contents")? + i;
            let inner = &src[i + 1..end];
            i = end + 1;
            if inner.starts_with('?') || inner.starts_with('!') {
                continue;
            }
            if let Some(name) = inner.strip_prefix('/') {
                let node = stack.pop().ok_or("bad package table of contents")?;
                if node.name != name.trim() || stack.is_empty() {
                    return Err("bad package table of contents".into());
                }
                stack.last_mut().unwrap().children.push(node);
            } else {
                let selfclose = inner.ends_with('/');
                let body = inner.trim_end_matches('/');
                let (name, rest) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
                let node = Node { name: name.to_string(), attrs: parse_attrs(rest), ..Default::default() };
                if selfclose {
                    stack.last_mut().unwrap().children.push(node);
                } else {
                    stack.push(node);
                }
            }
        } else {
            let end = src[i..].find('<').map(|e| e + i).unwrap_or(b.len());
            stack.last_mut().unwrap().text.push_str(&unescape(&src[i..end]));
            i = end;
        }
    }
    if stack.len() != 1 {
        return Err("bad package table of contents".into());
    }
    Ok(stack.pop().unwrap())
}

fn child<'a>(n: &'a Node, name: &str) -> Option<&'a Node> {
    n.children.iter().find(|c| c.name == name)
}

fn collect(node: &Node, prefix: &str, out: &mut Vec<XarFile>) {
    for ch in node.children.iter().filter(|c| c.name == "file") {
        let name = child(ch, "name").map(|n| n.text.trim().to_string()).unwrap_or_default();
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if let Some(d) = child(ch, "data") {
            let num = |k: &str| child(d, k).and_then(|n| n.text.trim().parse::<u64>().ok()).unwrap_or(0);
            let style = child(d, "encoding")
                .and_then(|e| e.attrs.iter().find(|(k, _)| k == "style").map(|(_, v)| v.clone()))
                .unwrap_or_default();
            out.push(XarFile { path: path.clone(), offset: num("offset"), length: num("length"), style });
        }
        collect(ch, &path, out);
    }
}

fn flat_payloads(data: &[u8], dest: &Path, ex: &mut Extraction, payloads: &mut usize) -> Result<(), String> {
    let hdr = |o: usize, n: usize| data.get(o..o + n).ok_or_else(|| "installer package is truncated".to_string());
    let header_size = u16::from_be_bytes(hdr(4, 2)?.try_into().unwrap()) as usize;
    let toc_clen = u64::from_be_bytes(hdr(8, 8)?.try_into().unwrap()) as usize;
    let toc_z = hdr(header_size, toc_clen)?;
    let toc = inflate::zlib_decompress(toc_z, 256 << 20)?;
    let toc = String::from_utf8_lossy(&toc).into_owned();
    let root = parse_xml(&toc)?;
    let xar = child(&root, "xar").ok_or("bad package table of contents")?;
    let toc_node = child(xar, "toc").ok_or("bad package table of contents")?;
    let mut files = Vec::new();
    collect(toc_node, "", &mut files);
    let heap = header_size + toc_clen;

    for f in files.iter().filter(|f| f.path == "Payload" || f.path.ends_with("/Payload")) {
        if f.length == 0 {
            continue; // meta-packages have empty payloads
        }
        let start = heap.checked_add(f.offset as usize).ok_or("bad package offset")?;
        let raw = data
            .get(start..start.checked_add(f.length as usize).ok_or("bad package offset")?)
            .ok_or("installer package is truncated")?;
        let bytes: Vec<u8>;
        let body: &[u8] = match f.style.as_str() {
            "" | "application/octet-stream" => raw,
            "application/x-gzip" => {
                bytes = if raw.starts_with(&[0x1f, 0x8b]) {
                    inflate::gunzip(raw, MAX_PAYLOAD)?
                } else {
                    inflate::zlib_decompress(raw, MAX_PAYLOAD)?
                };
                &bytes
            }
            other => return Err(format!("this package uses a compression method ({other}) that PowerBridge can't open yet")),
        };
        if body.is_empty() {
            continue;
        }
        let parent = f.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("package");
        let sub = dest.join(format!("{:02}_{}", *payloads, label_for(parent)));
        *payloads += 1;
        extract_archive_bytes(body, &sub, ex)?;
    }
    Ok(())
}

fn label_for(s: &str) -> String {
    let l: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    let l = l.trim_matches('_').to_string();
    if l.is_empty() { "package".into() } else { l }
}

// ------------------------------------------------------------------ payload (gzip + cpio)

fn extract_archive_bytes(data: &[u8], dest: &Path, ex: &mut Extraction) -> Result<(), String> {
    let owned: Vec<u8>;
    let raw: &[u8] = if data.starts_with(&[0x1f, 0x8b]) {
        owned = inflate::gunzip(data, MAX_PAYLOAD)?;
        &owned
    } else if data.starts_with(b"BZh") {
        return Err("this package is compressed with bzip2, which PowerBridge can't open yet".into());
    } else if data.starts_with(b"pbzx") || data.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        return Err("this package uses a newer compression format that PowerBridge can't open".into());
    } else {
        data
    };
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    extract_cpio(raw, dest, ex)
}

fn num(b: &[u8], radix: u32) -> Result<u64, String> {
    let s = std::str::from_utf8(b).map_err(|_| "corrupt archive")?;
    u64::from_str_radix(s.trim_matches(|c: char| c == ' ' || c == '\0'), radix).map_err(|_| "corrupt archive (bad number)".to_string())
}

fn clean_rel(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains('\0') => return None,
            p => out.push(p),
        }
    }
    if out.as_os_str().is_empty() { None } else { Some(out) }
}

/// Resolve `rel` under `dest`, refusing to pass through any symlink.
fn safe_path(dest: &Path, rel: &Path) -> Option<PathBuf> {
    let mut cur = dest.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(p) => cur.push(p),
            _ => return None,
        }
        if let Ok(md) = fs::symlink_metadata(&cur) {
            if md.file_type().is_symlink() {
                return None;
            }
        }
    }
    Some(cur)
}

fn looks_like_ppc_exe(d: &[u8]) -> bool {
    let be = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    match be(0) {
        Some(0xfeed_face) => be(4) == Some(18) && be(12) == Some(2),
        Some(0xcafe_babe) => {
            let n = be(4).unwrap_or(99);
            n <= 8 && (0..n as usize).any(|i| be(8 + i * 20) == Some(18))
        }
        _ => false,
    }
}

fn extract_cpio(data: &[u8], dest: &Path, ex: &mut Extraction) -> Result<(), String> {
    let mut off = 0usize;
    let bad = || "corrupt archive inside the package".to_string();
    loop {
        if off + 6 > data.len() || data[off..].iter().all(|&b| b == 0) {
            break;
        }
        let magic = &data[off..off + 6];
        let (mode, filesize, namesize, name_off, newc) = if magic == b"070707" {
            if off + 76 > data.len() {
                return Err(bad());
            }
            (
                num(&data[off + 18..off + 24], 8)?,
                num(&data[off + 65..off + 76], 8)? as usize,
                num(&data[off + 59..off + 65], 8)? as usize,
                off + 76,
                false,
            )
        } else if magic == b"070701" || magic == b"070702" {
            if off + 110 > data.len() {
                return Err(bad());
            }
            (
                num(&data[off + 14..off + 22], 16)?,
                num(&data[off + 54..off + 62], 16)? as usize,
                num(&data[off + 94..off + 102], 16)? as usize,
                off + 110,
                true,
            )
        } else {
            return Err("unrecognised archive format inside the package".into());
        };
        let align = |n: usize| if newc { (n + 3) & !3 } else { n };
        let name_end = name_off.checked_add(namesize).ok_or_else(bad)?;
        let data_off = align(name_end);
        let data_end = data_off.checked_add(filesize).ok_or_else(bad)?;
        if name_end > data.len() || data_end > data.len() || namesize == 0 {
            return Err(bad());
        }
        let name = String::from_utf8_lossy(&data[name_off..name_end - 1]).into_owned();
        let body = &data[data_off..data_end];
        off = align(data_end);
        if name == "TRAILER!!!" {
            break;
        }
        if ex.files >= MAX_FILES {
            return Err("this package contains too many files".into());
        }
        let Some(rel) = clean_rel(&name) else {
            if name.split('/').any(|p| p == "..") {
                ex.warn(format!("skipped unsafe path: {name}"));
            }
            continue;
        };
        let Some(target) = safe_path(dest, &rel) else {
            ex.warn(format!("skipped path that goes through a link: {name}"));
            continue;
        };
        let kind = mode & 0o170000;
        let io = |e: std::io::Error| format!("cannot write {}: {e}", target.display());
        match kind {
            0o040000 => {
                fs::create_dir_all(&target).map_err(io)?;
                set_mode(&target, (mode as u32 & 0o777) | 0o700);
            }
            0o100000 => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(io)?;
                }
                fs::write(&target, body).map_err(io)?;
                set_mode(&target, (mode as u32 & 0o777) | 0o600);
                ex.files += 1;
                ex.bytes += body.len() as u64;
                let inside_app = rel.components().any(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase().ends_with(".app"));
                if !inside_app && ex.binaries.len() < 200 && looks_like_ppc_exe(body) {
                    ex.binaries.push(target.clone());
                }
            }
            0o120000 => {
                let link = String::from_utf8_lossy(body).into_owned();
                if link.starts_with('/') || link.contains('\0') || link.is_empty() {
                    ex.warn(format!("skipped link with an absolute target: {name}"));
                    continue;
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(io)?;
                }
                if fs::symlink_metadata(&target).is_ok() {
                    let _ = fs::remove_file(&target);
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(&link, &target).map_err(io)?;
                #[cfg(not(unix))]
                let _ = link;
            }
            _ => {}
        }
    }
    Ok(())
}

fn set_mode(p: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode & 0o777));
    }
    let _ = (p, mode);
}

fn find_apps(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_dir() {
                continue; // symlinks are not followed
            }
            let p = e.path();
            if e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".app") {
                out.push(p);
            } else {
                walk(&p, depth + 1, out);
            }
        }
    }
    let mut v = Vec::new();
    walk(root, 0, &mut v);
    v.sort();
    v
}
