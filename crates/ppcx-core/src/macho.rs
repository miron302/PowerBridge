//! Minimal Mach-O reader for 32-bit big-endian PowerPC executables (thin or fat).

pub const CPU_TYPE_I386: u32 = 7;
pub const CPU_TYPE_POWERPC: u32 = 18;
pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;
pub const CPU_TYPE_POWERPC64: u32 = 0x0100_0012;
pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;

pub const MH_EXECUTE: u32 = 2;

pub const S_NON_LAZY_SYMBOL_POINTERS: u32 = 6;
pub const S_LAZY_SYMBOL_POINTERS: u32 = 7;

#[derive(Debug, Clone)]
pub struct Segment {
    pub name: String,
    pub vmaddr: u32,
    pub vmsize: u32,
    pub fileoff: u32,
    pub filesize: u32,
    pub initprot: u32,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub segment: String,
    pub addr: u32,
    pub size: u32,
    pub offset: u32,
    pub flags: u32,
    pub reserved1: u32,
    pub reserved2: u32,
}

impl Section {
    pub fn kind(&self) -> u32 {
        self.flags & 0xff
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub n_type: u8,
    pub n_sect: u8,
    pub n_value: u32,
}

#[derive(Debug, Clone)]
pub struct MachO {
    /// Bytes of the PowerPC slice (the whole file for thin binaries).
    pub image: Vec<u8>,
    pub archs: Vec<String>,
    pub filetype: u32,
    pub segments: Vec<Segment>,
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
    pub indirect: Vec<u32>,
    pub entry: u32,
    pub initial_sp: Option<u32>,
    pub dylibs: Vec<String>,
}

fn be32(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("file truncated (reading offset 0x{o:x})"))
}

fn cstr16(d: &[u8], o: usize) -> Result<String, String> {
    let b = d.get(o..o + 16).ok_or("file truncated")?;
    let end = b.iter().position(|&c| c == 0).unwrap_or(16);
    Ok(String::from_utf8_lossy(&b[..end]).into_owned())
}

pub fn cpu_name(t: u32) -> String {
    match t {
        CPU_TYPE_I386 => "i386".into(),
        CPU_TYPE_POWERPC => "ppc".into(),
        CPU_TYPE_X86_64 => "x86_64".into(),
        CPU_TYPE_POWERPC64 => "ppc64".into(),
        CPU_TYPE_ARM64 => "arm64".into(),
        other => format!("cputype 0x{other:x}"),
    }
}

pub fn parse(data: &[u8]) -> Result<MachO, String> {
    if data.len() < 8 {
        return Err("file too small to be a Mach-O binary".into());
    }
    let magic = be32(data, 0)?;
    let mut archs = Vec::new();
    let image: &[u8];
    match magic {
        0xcafe_babe => {
            let n = be32(data, 4)? as usize;
            if n > 32 {
                return Err("not a universal binary (this looks like a Java .class file)".into());
            }
            let mut ppc: Option<(usize, usize)> = None;
            for i in 0..n {
                let o = 8 + i * 20;
                let cpu = be32(data, o)?;
                let off = be32(data, o + 8)? as usize;
                let size = be32(data, o + 12)? as usize;
                archs.push(cpu_name(cpu));
                if cpu == CPU_TYPE_POWERPC && ppc.is_none() {
                    ppc = Some((off, size));
                }
            }
            let (off, size) = ppc.ok_or_else(|| {
                if archs.iter().any(|a| a == "ppc64") {
                    "universal binary only has a ppc64 slice (64-bit PowerPC is not supported)".to_string()
                } else {
                    format!("universal binary has no PowerPC slice (contains: {})", archs.join(", "))
                }
            })?;
            image = data
                .get(off..off + size)
                .ok_or("PowerPC slice extends past end of file")?;
        }
        0xfeed_face => {
            image = data;
        }
        0xfeed_facf => {
            return Err("64-bit big-endian Mach-O (ppc64) is not supported".into());
        }
        0xcefa_edfe | 0xcffa_edfe => {
            return Err("little-endian Mach-O (i386/x86_64/arm): this is not a PowerPC binary".into());
        }
        _ => return Err("not a Mach-O file".into()),
    }

    let cputype = be32(image, 4)?;
    if cputype != CPU_TYPE_POWERPC {
        return Err(format!("Mach-O is for {}, not 32-bit PowerPC", cpu_name(cputype)));
    }
    if archs.is_empty() {
        archs.push("ppc".into());
    }
    let filetype = be32(image, 12)?;
    let ncmds = be32(image, 16)? as usize;

    let mut segments = Vec::new();
    let mut sections = Vec::new();
    let mut dylibs = Vec::new();
    let mut symoff = 0usize;
    let mut nsyms = 0usize;
    let mut stroff = 0usize;
    let mut indirectsymoff = 0usize;
    let mut nindirect = 0usize;
    let mut entry = None;
    let mut initial_sp = None;

    let mut off = 28usize;
    for _ in 0..ncmds {
        let cmd = be32(image, off)?;
        let size = be32(image, off + 4)? as usize;
        if size < 8 {
            return Err("corrupt load command".into());
        }
        match cmd {
            0x1 => {
                let nsects = be32(image, off + 48)? as usize;
                let segname = cstr16(image, off + 8)?;
                segments.push(Segment {
                    name: segname.clone(),
                    vmaddr: be32(image, off + 24)?,
                    vmsize: be32(image, off + 28)?,
                    fileoff: be32(image, off + 32)?,
                    filesize: be32(image, off + 36)?,
                    initprot: be32(image, off + 44)?,
                });
                for s in 0..nsects {
                    let so = off + 56 + s * 68;
                    sections.push(Section {
                        name: cstr16(image, so)?,
                        segment: cstr16(image, so + 16)?,
                        addr: be32(image, so + 32)?,
                        size: be32(image, so + 36)?,
                        offset: be32(image, so + 40)?,
                        flags: be32(image, so + 56)?,
                        reserved1: be32(image, so + 60)?,
                        reserved2: be32(image, so + 64)?,
                    });
                }
            }
            0x2 => {
                symoff = be32(image, off + 8)? as usize;
                nsyms = be32(image, off + 12)? as usize;
                stroff = be32(image, off + 16)? as usize;
            }
            0xb => {
                indirectsymoff = be32(image, off + 8 + 12 * 4)? as usize;
                nindirect = be32(image, off + 8 + 13 * 4)? as usize;
            }
            0x5 => {
                // LC_UNIXTHREAD: flavor, count, then ppc_thread_state (srr0, srr1, r0..r31, ...)
                let state = off + 16;
                entry = Some(be32(image, state)?);
                let r1 = be32(image, state + 8 + 4)?;
                if r1 != 0 {
                    initial_sp = Some(r1);
                }
            }
            0xc | 0xd | 0x18 | 0x1f => {
                // LC_LOAD_DYLIB, LC_ID_DYLIB, LC_LOAD_WEAK_DYLIB, LC_REEXPORT_DYLIB
                let name_off = be32(image, off + 8)? as usize;
                let b = image.get(off + name_off..off + size).unwrap_or(&[]);
                let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                dylibs.push(String::from_utf8_lossy(&b[..end]).into_owned());
            }
            _ => {}
        }
        off += size;
    }

    let mut symbols = Vec::with_capacity(nsyms);
    for i in 0..nsyms {
        let so = symoff + i * 12;
        let strx = be32(image, so)? as usize;
        let n_type = *image.get(so + 4).ok_or("symbol table truncated")?;
        let n_sect = *image.get(so + 5).ok_or("symbol table truncated")?;
        let n_value = be32(image, so + 8)?;
        let nb = image.get(stroff + strx..).unwrap_or(&[]);
        let end = nb.iter().position(|&c| c == 0).unwrap_or(nb.len());
        symbols.push(Symbol {
            name: String::from_utf8_lossy(&nb[..end]).into_owned(),
            n_type,
            n_sect,
            n_value,
        });
    }
    let mut indirect = Vec::with_capacity(nindirect);
    for i in 0..nindirect {
        indirect.push(be32(image, indirectsymoff + i * 4)?);
    }

    Ok(MachO {
        image: image.to_vec(),
        archs,
        filetype,
        segments,
        sections,
        symbols,
        indirect,
        entry: entry.ok_or("no LC_UNIXTHREAD entry point found (only executables are supported)")?,
        initial_sp,
        dylibs,
    })
}

impl MachO {
    /// Names of the imported (undefined) symbols, in symbol-table order.
    pub fn imports(&self) -> Vec<&str> {
        self.symbols
            .iter()
            .filter(|s| s.n_type & 0x0e == 0 && s.n_type & 0x01 != 0)
            .map(|s| s.name.as_str())
            .collect()
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("architectures : {}\n", self.archs.join(", ")));
        s.push_str(&format!(
            "file type     : {}\n",
            if self.filetype == MH_EXECUTE { "executable" } else { "other (unsupported)" }
        ));
        s.push_str(&format!("entry point   : 0x{:08x}\n", self.entry));
        if !self.dylibs.is_empty() {
            s.push_str(&format!("libraries     : {}\n", self.dylibs.join(", ")));
        }
        s.push_str("segments:\n");
        for g in &self.segments {
            s.push_str(&format!(
                "  {:<12} 0x{:08x}  size 0x{:x}\n",
                g.name, g.vmaddr, g.vmsize
            ));
        }
        let imps = self.imports();
        s.push_str(&format!("imports ({}): {}\n", imps.len(), imps.join(" ")));
        s
    }
}
