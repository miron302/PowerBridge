//! High-level emulation of common libSystem imports (printf, malloc, string.h, ...).
//! Calls arrive here when the guest jumps to a bound trap slot; arguments follow the
//! PowerPC Darwin ABI (r3..r10, then the caller's parameter area at 24(r1)).

use crate::machine::*;
use crate::mem::Fault;

/// Names (without the leading underscore) of every import handled by `hle_inner`.
/// Used by the bundle analyzer; a unit test keeps it in sync with the implementation.
pub const SUPPORTED_IMPORTS: &[&str] = &[
    "exit", "_exit", "abort", "puts", "putchar", "fputs", "fputc", "putc", "fwrite", "fflush",
    "setvbuf", "setbuf", "fclose", "printf", "fprintf", "sprintf", "snprintf", "write", "read",
    "open", "close", "malloc", "calloc", "realloc", "free", "strlen", "strcmp", "strncmp",
    "strcpy", "strncpy", "strcat", "strchr", "strrchr", "strdup", "memcpy", "memmove", "bcopy",
    "memset", "memcmp", "toupper", "tolower", "atoi", "atol", "abs", "labs", "srand", "rand",
    "time", "getpid", "getenv", "signal", "sleep", "usleep", "fopen", "_error", "atexit",
    "_keymgr_dwarf2_register_sections", "_dyld_make_delayed_module_initializer_calls",
    "_dyld_func_lookup", "mach_init_routine", "_cthread_init_routine", "_dyld_init_check",
    "dyld_stub_binding_helper", "_cxa_atexit", "_keymgr_unregister_sections",
];

/// True if the host provides this imported symbol (symbol name as in the Mach-O, with `_` prefix).
pub fn is_supported_import(sym: &str) -> bool {
    let n = sym.strip_prefix('_').unwrap_or(sym);
    SUPPORTED_IMPORTS.contains(&n) || DATA_SYMBOLS.contains(&sym)
}

/// Walks the integer/FP argument registers (and stack) for varargs functions.
struct Args {
    idx: usize,
    fidx: usize,
    sp: u32,
}

impl Machine {
    fn arg(&self, i: usize) -> u32 {
        self.cpu.gpr[3 + i]
    }

    fn next_u32(&self, a: &mut Args) -> Result<u32, Fault> {
        let v = if a.idx < 8 {
            self.cpu.gpr[3 + a.idx]
        } else {
            self.mem.read_u32(a.sp.wrapping_add(24 + 4 * (a.idx as u32 - 8)))?
        };
        a.idx += 1;
        Ok(v)
    }

    fn next_f64(&self, a: &mut Args) -> f64 {
        let v = self.cpu.fpr[(1 + a.fidx).min(31)];
        a.fidx += 1;
        a.idx += 2; // a double also occupies two integer argument slots
        v
    }

    pub(crate) fn hle_call(&mut self, idx: usize) {
        let full = self.imports[idx].clone();
        let name = full.strip_prefix('_').unwrap_or(&full).to_string();
        if self.cfg.trace_imports {
            let (a0, a1, a2) = (self.arg(0), self.arg(1), self.arg(2));
            self.log(format!("[imp] {full}({a0:#x}, {a1:#x}, {a2:#x})"));
        }
        match self.hle_inner(&name) {
            Ok(ret) => {
                self.cpu.gpr[3] = ret;
                self.cpu.pc = self.cpu.lr;
            }
            Err(f) => {
                self.crash = Some(format!(
                    "memory fault ({}) at 0x{:08x} inside imported function {full}()",
                    if f.write { "write" } else { "read" },
                    f.addr
                ));
            }
        }
    }

    fn set_errno(&mut self, e: i32) {
        let _ = self.mem.write_u32(ERRNO_CELL, e as u32);
    }

    fn out_fd(&mut self, fd: u32, data: Vec<u8>) {
        match fd {
            2 => self.emit(Event::Stderr(data)),
            _ => self.emit(Event::Stdout(data)),
        }
    }

    fn file_fd(file: u32) -> Option<u32> {
        match file {
            FILE_STDIN => Some(0),
            FILE_STDOUT => Some(1),
            FILE_STDERR => Some(2),
            _ => None,
        }
    }

    fn malloc(&mut self, n: u32) -> Result<u32, Fault> {
        let size = ((n as u64 + 15) & !15) + 16;
        if self.heap_next as u64 + size > HEAP_LIMIT as u64 {
            return Ok(0);
        }
        let end = self.heap_next + size as u32;
        if end > self.heap_end {
            let new_end = (end + 0xF_FFFF) & !0xF_FFFF;
            self.mem.map(self.heap_end, new_end - self.heap_end);
            self.heap_end = new_end;
        }
        let p = self.heap_next + 16;
        self.mem.write_u32(p - 4, n)?;
        self.heap_next = end;
        Ok(p)
    }

    fn hle_inner(&mut self, name: &str) -> Result<u32, Fault> {
        let (r3, r4, r5) = (self.arg(0), self.arg(1), self.arg(2));
        Ok(match name {
            "exit" | "_exit" => {
                self.exit_code = Some(r3 as i32);
                0
            }
            "abort" => {
                self.log("[abort] guest called abort()");
                self.exit_code = Some(134);
                0
            }
            "puts" => {
                let mut s = self.mem.read_cstr(r3, 1 << 20)?;
                s.push(b'\n');
                self.emit(Event::Stdout(s));
                1
            }
            "putchar" => {
                self.emit(Event::Stdout(vec![r3 as u8]));
                r3 & 0xff
            }
            "fputs" => {
                let s = self.mem.read_cstr(r3, 1 << 20)?;
                if let Some(fd) = Self::file_fd(r4) {
                    self.out_fd(fd, s);
                }
                1
            }
            "fputc" | "putc" => {
                if let Some(fd) = Self::file_fd(r4) {
                    self.out_fd(fd, vec![r3 as u8]);
                }
                r3 & 0xff
            }
            "fwrite" => {
                let total = r4.wrapping_mul(r5);
                let data = self.mem.read_bytes(r3, total.min(1 << 24))?;
                if let Some(fd) = Self::file_fd(self.arg(3)) {
                    self.out_fd(fd, data);
                }
                r5
            }
            "fflush" | "setvbuf" | "setbuf" | "fclose" => 0,
            "printf" => {
                let fmt = self.mem.read_cstr(r3, 1 << 16)?;
                let mut a = Args { idx: 1, fidx: 0, sp: self.cpu.gpr[1] };
                let out = self.format(&fmt, &mut a)?;
                let n = out.len() as u32;
                self.emit(Event::Stdout(out));
                n
            }
            "fprintf" => {
                let fmt = self.mem.read_cstr(r4, 1 << 16)?;
                let mut a = Args { idx: 2, fidx: 0, sp: self.cpu.gpr[1] };
                let out = self.format(&fmt, &mut a)?;
                let n = out.len() as u32;
                if let Some(fd) = Self::file_fd(r3) {
                    self.out_fd(fd, out);
                }
                n
            }
            "sprintf" | "snprintf" => {
                let (dst, cap, fmt_arg, first) = if name == "sprintf" {
                    (r3, u32::MAX, r4, 2)
                } else {
                    (r3, r4, r5, 3)
                };
                let fmt = self.mem.read_cstr(fmt_arg, 1 << 16)?;
                let mut a = Args { idx: first, fidx: 0, sp: self.cpu.gpr[1] };
                let out = self.format(&fmt, &mut a)?;
                let n = out.len() as u32;
                if cap != 0 {
                    let keep = (out.len() as u32).min(cap.saturating_sub(1));
                    self.mem.write_bytes(dst, &out[..keep as usize])?;
                    self.mem.write_u8(dst + keep, 0)?;
                }
                n
            }
            "write" | "read" => {
                let num = if name == "write" { 4 } else { 3 };
                match self.sys_dispatch(num, [r3, r4, r5, 0, 0, 0]) {
                    Ok((v, _)) => v,
                    Err(e) => {
                        self.set_errno(e);
                        u32::MAX
                    }
                }
            }
            "open" => {
                let mode = self.arg(2);
                match self.sys_dispatch(5, [r3, r4, mode, 0, 0, 0]) {
                    Ok((v, _)) => v,
                    Err(e) => {
                        self.set_errno(e);
                        u32::MAX
                    }
                }
            }
            "close" => match self.sys_dispatch(6, [r3, 0, 0, 0, 0, 0]) {
                Ok(_) => 0,
                Err(e) => {
                    self.set_errno(e);
                    u32::MAX
                }
            },
            "malloc" => self.malloc(r3)?,
            "calloc" => {
                let n = r3.wrapping_mul(r4);
                self.malloc(n)? // heap pages start zeroed and are never reused
            }
            "realloc" => {
                let np = self.malloc(r4)?;
                if r3 != 0 && np != 0 {
                    let old = self.mem.read_u32(r3 - 4)?;
                    let n = old.min(r4);
                    let data = self.mem.read_bytes(r3, n)?;
                    self.mem.write_bytes(np, &data)?;
                }
                np
            }
            "free" => 0,
            "strlen" => self.mem.read_cstr(r3, usize::MAX >> 1)?.len() as u32,
            "strcmp" | "strncmp" => {
                let limit = if name == "strncmp" { r5 as usize } else { usize::MAX >> 1 };
                let mut a = self.mem.read_cstr(r3, limit)?;
                let mut b = self.mem.read_cstr(r4, limit)?;
                a.push(0);
                b.push(0);
                let n = a.len().min(b.len()).min(if name == "strncmp" { limit } else { usize::MAX });
                let mut r = 0i32;
                for i in 0..n {
                    if a[i] != b[i] {
                        r = a[i] as i32 - b[i] as i32;
                        break;
                    }
                }
                r as u32
            }
            "strcpy" => {
                let mut s = self.mem.read_cstr(r4, 1 << 24)?;
                s.push(0);
                self.mem.write_bytes(r3, &s)?;
                r3
            }
            "strncpy" => {
                let s = self.mem.read_cstr(r4, r5 as usize)?;
                let mut buf = vec![0u8; r5 as usize];
                buf[..s.len()].copy_from_slice(&s);
                self.mem.write_bytes(r3, &buf)?;
                r3
            }
            "strcat" => {
                let dl = self.mem.read_cstr(r3, 1 << 24)?.len() as u32;
                let mut s = self.mem.read_cstr(r4, 1 << 24)?;
                s.push(0);
                self.mem.write_bytes(r3 + dl, &s)?;
                r3
            }
            "strchr" | "strrchr" => {
                let mut s = self.mem.read_cstr(r3, 1 << 24)?;
                s.push(0);
                let c = r4 as u8;
                let pos = if name == "strchr" {
                    s.iter().position(|&x| x == c)
                } else {
                    s.iter().rposition(|&x| x == c)
                };
                pos.map(|p| r3 + p as u32).unwrap_or(0)
            }
            "strdup" => {
                let mut s = self.mem.read_cstr(r3, 1 << 24)?;
                s.push(0);
                let p = self.malloc(s.len() as u32)?;
                if p != 0 {
                    self.mem.write_bytes(p, &s)?;
                }
                p
            }
            "memcpy" | "memmove" | "bcopy" => {
                let (dst, src) = if name == "bcopy" { (r4, r3) } else { (r3, r4) };
                let data = self.mem.read_bytes(src, r5)?;
                self.mem.write_bytes(dst, &data)?;
                dst
            }
            "memset" => {
                for i in 0..r5 {
                    self.mem.write_u8(r3.wrapping_add(i), r4 as u8)?;
                }
                r3
            }
            "memcmp" => {
                let a = self.mem.read_bytes(r3, r5)?;
                let b = self.mem.read_bytes(r4, r5)?;
                let mut r = 0i32;
                for i in 0..a.len() {
                    if a[i] != b[i] {
                        r = a[i] as i32 - b[i] as i32;
                        break;
                    }
                }
                r as u32
            }
            "toupper" => (r3 as u8).to_ascii_uppercase() as u32,
            "tolower" => (r3 as u8).to_ascii_lowercase() as u32,
            "atoi" | "atol" => {
                let s = self.mem.read_cstr(r3, 64)?;
                let s = String::from_utf8_lossy(&s).trim_start().to_string();
                let mut end = 0;
                for (i, c) in s.char_indices() {
                    if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
                        end = i + 1;
                    } else {
                        break;
                    }
                }
                s[..end].parse::<i32>().unwrap_or(0) as u32
            }
            "abs" | "labs" => (r3 as i32).unsigned_abs(),
            "srand" => {
                self.rng = r3;
                0
            }
            "rand" => {
                self.rng = self.rng.wrapping_mul(1103515245).wrapping_add(12345);
                (self.rng >> 16) & 0x7fff
            }
            "time" => {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as u32;
                if r3 != 0 {
                    self.mem.write_u32(r3, t)?;
                }
                t
            }
            "getpid" => 1000,
            "getenv" | "signal" | "sleep" | "usleep" | "fopen" => {
                if name == "fopen" {
                    self.set_errno(2);
                }
                0
            }
            "_error" => ERRNO_CELL,
            "atexit"
            | "_keymgr_dwarf2_register_sections"
            | "_dyld_make_delayed_module_initializer_calls"
            | "_dyld_func_lookup"
            | "mach_init_routine"
            | "_cthread_init_routine"
            | "_dyld_init_check"
            | "dyld_stub_binding_helper"
            | "_cxa_atexit"
            | "_keymgr_unregister_sections" => 0,
            other => {
                let full = format!("_{other}");
                self.warn_once(&full, format!("[imp] unimplemented import {full}() called; returning 0"));
                0
            }
        })
    }

    /// printf-style formatter reading arguments per the PowerPC Darwin varargs rules.
    fn format(&self, fmt: &[u8], a: &mut Args) -> Result<Vec<u8>, Fault> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < fmt.len() {
            let c = fmt[i];
            i += 1;
            if c != b'%' {
                out.push(c);
                continue;
            }
            let (mut left, mut plus, mut space, mut alt, mut zero) = (false, false, false, false, false);
            while i < fmt.len() {
                match fmt[i] {
                    b'-' => left = true,
                    b'+' => plus = true,
                    b' ' => space = true,
                    b'#' => alt = true,
                    b'0' => zero = true,
                    _ => break,
                }
                i += 1;
            }
            let mut width = 0usize;
            if i < fmt.len() && fmt[i] == b'*' {
                let w = self.next_u32(a)? as i32;
                if w < 0 { left = true; }
                width = w.unsigned_abs() as usize;
                i += 1;
            } else {
                while i < fmt.len() && fmt[i].is_ascii_digit() {
                    width = width * 10 + (fmt[i] - b'0') as usize;
                    i += 1;
                }
            }
            let mut prec: Option<usize> = None;
            if i < fmt.len() && fmt[i] == b'.' {
                i += 1;
                if i < fmt.len() && fmt[i] == b'*' {
                    let p = self.next_u32(a)? as i32;
                    prec = if p < 0 { None } else { Some(p as usize) };
                    i += 1;
                } else {
                    let mut p = 0usize;
                    while i < fmt.len() && fmt[i].is_ascii_digit() {
                        p = p * 10 + (fmt[i] - b'0') as usize;
                        i += 1;
                    }
                    prec = Some(p);
                }
            }
            let (mut hh, mut h, mut ll) = (false, false, false);
            while i < fmt.len() {
                match fmt[i] {
                    b'h' => { if h { hh = true } else { h = true } }
                    b'l' => { if ll { } ; ll = ll || fmt.get(i + 1) == Some(&b'l') || ll }
                    b'z' | b'j' | b't' | b'L' | b'q' => {}
                    _ => break,
                }
                i += 1;
            }
            if i >= fmt.len() {
                break;
            }
            let conv = fmt[i];
            i += 1;
            let mut body: Vec<u8>;
            let mut sign = String::new();
            let mut numeric = false;
            match conv {
                b'%' => body = b"%".to_vec(),
                b'c' => body = vec![self.next_u32(a)? as u8],
                b's' => {
                    let p = self.next_u32(a)?;
                    body = if p == 0 { b"(null)".to_vec() } else { self.mem.read_cstr(p, 1 << 20)? };
                    if let Some(pr) = prec {
                        body.truncate(pr);
                    }
                }
                b'p' => {
                    let v = self.next_u32(a)?;
                    body = format!("0x{v:x}").into_bytes();
                }
                b'd' | b'i' => {
                    numeric = true;
                    let v: i64 = if ll {
                        let hi = self.next_u32(a)? as u64;
                        let lo = self.next_u32(a)? as u64;
                        ((hi << 32) | lo) as i64
                    } else {
                        let r = self.next_u32(a)?;
                        if hh { r as i8 as i64 } else if h { r as i16 as i64 } else { r as i32 as i64 }
                    };
                    if v < 0 { sign.push('-') } else if plus { sign.push('+') } else if space { sign.push(' ') }
                    let mut digits = v.unsigned_abs().to_string();
                    if let Some(p) = prec { while digits.len() < p { digits.insert(0, '0') } }
                    body = digits.into_bytes();
                }
                b'u' | b'x' | b'X' | b'o' => {
                    numeric = true;
                    let v: u64 = if ll {
                        let hi = self.next_u32(a)? as u64;
                        let lo = self.next_u32(a)? as u64;
                        (hi << 32) | lo
                    } else {
                        let r = self.next_u32(a)?;
                        if hh { r as u8 as u64 } else if h { r as u16 as u64 } else { r as u64 }
                    };
                    let mut digits = match conv {
                        b'u' => v.to_string(),
                        b'x' => format!("{v:x}"),
                        b'X' => format!("{v:X}"),
                        _ => format!("{v:o}"),
                    };
                    if let Some(p) = prec { while digits.len() < p { digits.insert(0, '0') } }
                    if alt && v != 0 {
                        match conv {
                            b'x' => sign.push_str("0x"),
                            b'X' => sign.push_str("0X"),
                            b'o' => digits.insert(0, '0'),
                            _ => {}
                        }
                    }
                    body = digits.into_bytes();
                }
                b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                    numeric = true;
                    let v = self.next_f64(a);
                    if v.is_sign_negative() && !v.is_nan() { sign.push('-') } else if plus { sign.push('+') } else if space { sign.push(' ') }
                    let av = v.abs();
                    let p = prec.unwrap_or(6);
                    let s = if av.is_nan() {
                        "nan".to_string()
                    } else if av.is_infinite() {
                        "inf".to_string()
                    } else {
                        match conv {
                            b'f' | b'F' => format!("{av:.p$}"),
                            b'e' | b'E' => fmt_exp(av, p, conv == b'E'),
                            _ => fmt_g(av, if p == 0 { 1 } else { p }, alt, conv == b'G'),
                        }
                    };
                    body = s.into_bytes();
                }
                other => {
                    body = vec![b'%', other];
                }
            }
            let int_conv = matches!(conv, b'd' | b'i' | b'u' | b'x' | b'X' | b'o');
            let pad_zero = zero && !left && numeric && !(int_conv && prec.is_some());
            let total = sign.len() + body.len();
            if total >= width {
                out.extend_from_slice(sign.as_bytes());
                out.extend_from_slice(&body);
            } else if left {
                out.extend_from_slice(sign.as_bytes());
                out.extend_from_slice(&body);
                out.extend(std::iter::repeat(b' ').take(width - total));
            } else if pad_zero {
                out.extend_from_slice(sign.as_bytes());
                out.extend(std::iter::repeat(b'0').take(width - total));
                out.extend_from_slice(&body);
            } else {
                out.extend(std::iter::repeat(b' ').take(width - total));
                out.extend_from_slice(sign.as_bytes());
                out.extend_from_slice(&body);
            }
        }
        Ok(out)
    }
}

fn fmt_exp(v: f64, prec: usize, upper: bool) -> String {
    let s = format!("{v:.prec$e}");
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    let ev: i32 = e.parse().unwrap_or(0);
    let r = format!("{m}e{}{:02}", if ev < 0 { '-' } else { '+' }, ev.abs());
    if upper { r.to_uppercase() } else { r }
}

fn fmt_g(v: f64, p: usize, alt: bool, upper: bool) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let exp = v.log10().floor() as i32;
    let es = format!("{v:.prec$e}", prec = p - 1);
    let real_exp: i32 = es.split_once('e').map(|x| x.1.parse().unwrap_or(exp)).unwrap_or(exp);
    let mut s = if real_exp < -4 || real_exp >= p as i32 {
        fmt_exp(v, p - 1, upper)
    } else {
        format!("{v:.prec$}", prec = (p as i32 - 1 - real_exp).max(0) as usize)
    };
    if !alt {
        if let Some(epos) = s.find(|c| c == 'e' || c == 'E') {
            let (m, e) = s.split_at(epos);
            let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
            s = format!("{m}{e}");
        } else if s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Config, Machine};

    #[test]
    fn supported_list_matches_implementation() {
        let data = include_bytes!("../../../tests/fixtures/hello_sc.bin");
        let mut m = Machine::load(data, Config::default(), Box::new(|_| {})).unwrap();
        for name in SUPPORTED_IMPORTS {
            let _ = m.hle_inner(name); // faults on null pointers are fine; "unimplemented" is not
            assert!(
                !m.warned.contains(&format!("_{name}")),
                "SUPPORTED_IMPORTS lists {name} but hle_inner does not handle it"
            );
        }
        assert!(!is_supported_import("_NSApplicationMain"));
        assert!(is_supported_import("_printf"));
    }
}
