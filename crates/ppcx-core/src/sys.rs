//! Darwin/PowerPC syscall translation (BSD syscalls via `sc`, Mach traps stubbed).

use crate::machine::{Event, Machine, MMAP_BASE};
use std::io::{Read, Seek, SeekFrom, Write};

const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const EIO: i32 = 5;
const EBADF: i32 = 9;
const EACCES: i32 = 13;
const EFAULT: i32 = 14;
const EEXIST: i32 = 17;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const ENOTTY: i32 = 25;
const ENOSYS: i32 = 78;

pub(crate) type SysResult = Result<(u32, u32), i32>;

fn io_errno(e: &std::io::Error) -> i32 {
    match e.kind() {
        std::io::ErrorKind::NotFound => ENOENT,
        std::io::ErrorKind::PermissionDenied => EACCES,
        std::io::ErrorKind::AlreadyExists => EEXIST,
        _ => EIO,
    }
}

fn sys_name(n: u32) -> &'static str {
    match n {
        1 => "exit",
        3 => "read",
        4 => "write",
        5 => "open",
        6 => "close",
        20 => "getpid",
        24 => "getuid",
        25 => "geteuid",
        33 => "access",
        43 => "getegid",
        46 => "sigaction",
        47 => "getgid",
        48 => "sigprocmask",
        54 => "ioctl",
        73 => "munmap",
        74 => "mprotect",
        75 => "madvise",
        116 => "gettimeofday",
        194 => "getrlimit",
        197 => "mmap",
        199 => "lseek",
        327 => "issetugid",
        _ => "?",
    }
}

impl Machine {
    /// Handle an `sc` instruction. PowerPC Darwin convention: r0 = number, args in r3.., result in r3;
    /// on success the kernel resumes at pc+8 (skipping the error branch), on error at pc+4.
    pub(crate) fn do_syscall(&mut self) {
        let num = self.cpu.gpr[0];
        let a: [u32; 6] = [
            self.cpu.gpr[3], self.cpu.gpr[4], self.cpu.gpr[5],
            self.cpu.gpr[6], self.cpu.gpr[7], self.cpu.gpr[8],
        ];
        let pc = self.cpu.pc;
        let res = if (num as i32) < 0 {
            // Mach trap
            self.warn_once(&format!("mach{num}"), format!("[mach] trap {} stubbed", num as i32));
            Ok((0x103, 0))
        } else {
            self.sys_dispatch(num, a)
        };
        if self.cfg.trace_syscalls {
            let r = match &res {
                Ok((v, _)) => format!("{}", *v as i32),
                Err(e) => format!("-1 (errno {e})"),
            };
            self.log(format!(
                "[sys] {}({:#x}, {:#x}, {:#x}) = {}",
                sys_name(num), a[0], a[1], a[2], r
            ));
        }
        match res {
            Ok((r3, r4)) => {
                self.cpu.gpr[3] = r3;
                self.cpu.gpr[4] = r4;
                self.cpu.pc = pc.wrapping_add(8);
            }
            Err(e) => {
                self.cpu.gpr[3] = e as u32;
                self.cpu.pc = pc.wrapping_add(4);
            }
        }
    }

    pub(crate) fn sys_dispatch(&mut self, num: u32, a: [u32; 6]) -> SysResult {
        match num {
            0 => {
                // indirect syscall(number, args...)
                let n = a[0];
                self.sys_dispatch(n, [a[1], a[2], a[3], a[4], a[5], 0])
            }
            1 => {
                self.exit_code = Some(a[0] as i32);
                Ok((0, 0))
            }
            3 => self.sys_read(a[0], a[1], a[2]),
            4 => self.sys_write(a[0], a[1], a[2]),
            5 => self.sys_open(a[0], a[1], a[2]),
            6 => {
                if (0..3).contains(&a[0]) || self.files.remove(&a[0]).is_some() {
                    Ok((0, 0))
                } else {
                    Err(EBADF)
                }
            }
            20 => Ok((1000, 0)),
            24 | 25 => Ok((501, 0)),
            43 | 47 => Ok((20, 0)),
            327 => Ok((0, 0)),
            33 => {
                let p = self.mem.read_cstr(a[0], 4096).map_err(|_| EFAULT)?;
                if std::path::Path::new(&String::from_utf8_lossy(&p).into_owned()).exists() {
                    Ok((0, 0))
                } else {
                    Err(ENOENT)
                }
            }
            46 | 48 | 329 | 73 | 74 | 75 => Ok((0, 0)),
            54 => Err(ENOTTY),
            116 => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                if a[0] != 0 {
                    self.mem.write_u32(a[0], now.as_secs() as u32).map_err(|_| EFAULT)?;
                    self.mem.write_u32(a[0] + 4, now.subsec_micros()).map_err(|_| EFAULT)?;
                }
                Ok((0, 0))
            }
            194 => {
                if a[1] != 0 {
                    self.mem.write_u64(a[1], 0x7fff_ffff_ffff_ffff).map_err(|_| EFAULT)?;
                    self.mem.write_u64(a[1] + 8, 0x7fff_ffff_ffff_ffff).map_err(|_| EFAULT)?;
                }
                Ok((0, 0))
            }
            197 => self.sys_mmap(a),
            199 => self.sys_lseek(a),
            other => {
                self.warn_once(&format!("sys{other}"), format!("[sys] unimplemented syscall {other}"));
                Err(ENOSYS)
            }
        }
    }

    fn sys_read(&mut self, fd: u32, buf: u32, len: u32) -> SysResult {
        let len = len.min(1 << 24) as usize;
        let mut tmp = vec![0u8; len];
        let n = if fd == 0 {
            self.stdin.read(&mut tmp).map_err(|e| io_errno(&e))?
        } else if let Some(f) = self.files.get_mut(&fd) {
            f.read(&mut tmp).map_err(|e| io_errno(&e))?
        } else {
            return Err(EBADF);
        };
        self.mem.write_bytes(buf, &tmp[..n]).map_err(|_| EFAULT)?;
        Ok((n as u32, 0))
    }

    fn sys_write(&mut self, fd: u32, buf: u32, len: u32) -> SysResult {
        let data = self.mem.read_bytes(buf, len.min(1 << 26)).map_err(|_| EFAULT)?;
        match fd {
            1 => self.emit(Event::Stdout(data)),
            2 => self.emit(Event::Stderr(data)),
            _ => {
                let f = self.files.get_mut(&fd).ok_or(EBADF)?;
                f.write_all(&data).map_err(|e| io_errno(&e))?;
            }
        }
        Ok((len, 0))
    }

    fn sys_open(&mut self, path: u32, flags: u32, _mode: u32) -> SysResult {
        let p = self.mem.read_cstr(path, 4096).map_err(|_| EFAULT)?;
        let p = String::from_utf8_lossy(&p).into_owned();
        let mut o = std::fs::OpenOptions::new();
        match flags & 3 {
            0 => { o.read(true); }
            1 => { o.write(true); }
            _ => { o.read(true).write(true); }
        }
        if flags & 0x8 != 0 { o.append(true); }
        if flags & 0x200 != 0 {
            if flags & 0x800 != 0 { o.create_new(true); } else { o.create(true); }
        }
        if flags & 0x400 != 0 { o.truncate(true); }
        match o.open(&p) {
            Ok(f) => {
                let fd = self.next_fd;
                self.next_fd += 1;
                self.files.insert(fd, f);
                Ok((fd, 0))
            }
            Err(e) => Err(io_errno(&e)),
        }
    }

    fn sys_lseek(&mut self, a: [u32; 6]) -> SysResult {
        // fd in r3, 64-bit offset in r4:r5, whence in r6 (best effort)
        let off = (((a[1] as u64) << 32) | a[2] as u64) as i64;
        let f = self.files.get_mut(&a[0]).ok_or(EBADF)?;
        let pos = match a[3] {
            0 => SeekFrom::Start(off as u64),
            1 => SeekFrom::Current(off),
            2 => SeekFrom::End(off),
            _ => return Err(EINVAL),
        };
        let r = f.seek(pos).map_err(|e| io_errno(&e))?;
        Ok(((r >> 32) as u32, r as u32))
    }

    fn sys_mmap(&mut self, a: [u32; 6]) -> SysResult {
        let (addr, len, flags, fd) = (a[0], a[1], a[3], a[4]);
        if len == 0 {
            return Err(EINVAL);
        }
        let len = (len + 0xfff) & !0xfff;
        let base = if flags & 0x10 != 0 && addr != 0 {
            addr & !0xfff
        } else {
            let b = self.mmap_next;
            if b as u64 + len as u64 > 0xA000_0000 {
                return Err(ENOMEM);
            }
            self.mmap_next += len;
            b
        };
        self.mem.map(base, len);
        if flags & 0x1000 == 0 && fd != 0xFFFF_FFFF {
            // file-backed: copy contents in (private mapping semantics)
            if let Some(f) = self.files.get_mut(&fd) {
                let mut buf = vec![0u8; len as usize];
                let _ = f.seek(SeekFrom::Start(0));
                let n = f.read(&mut buf).unwrap_or(0);
                let _ = self.mem.write_bytes(base, &buf[..n]);
            } else {
                return Err(EBADF);
            }
        }
        let _ = (MMAP_BASE, EPERM);
        Ok((base, 0))
    }
}
