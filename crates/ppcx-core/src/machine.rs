use crate::cpu::{Cpu, Exit};
use crate::macho::{self, MachO, S_LAZY_SYMBOL_POINTERS, S_NON_LAZY_SYMBOL_POINTERS};
use crate::mem::Memory;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(crate) const TRAP_BASE: u32 = 0xFFF0_0000;
pub(crate) const SCRATCH: u32 = 0x0FF0_0000;
pub(crate) const HEAP_BASE: u32 = 0x2000_0000;
pub(crate) const HEAP_LIMIT: u32 = 0x3000_0000;
pub(crate) const MMAP_BASE: u32 = 0x3000_0000;
pub(crate) const STACK_TOP: u32 = 0xC000_0000;
pub(crate) const STACK_SIZE: u32 = 8 * 1024 * 1024;

// Fixed layout inside the scratch page.
pub(crate) const FILE_STDIN: u32 = SCRATCH;
pub(crate) const FILE_STDOUT: u32 = SCRATCH + 0x40;
pub(crate) const FILE_STDERR: u32 = SCRATCH + 0x80;
pub(crate) const CELL_STDINP: u32 = SCRATCH + 0xC0;
pub(crate) const CELL_STDOUTP: u32 = SCRATCH + 0xC4;
pub(crate) const CELL_STDERRP: u32 = SCRATCH + 0xC8;
pub(crate) const ERRNO_CELL: u32 = SCRATCH + 0xD0;
pub(crate) const CELL_PROGNAME: u32 = SCRATCH + 0xD4;
pub(crate) const CELL_ENVIRON: u32 = SCRATCH + 0xD8;
pub(crate) const CELL_NXARGC: u32 = SCRATCH + 0xDC;
pub(crate) const CELL_NXARGV: u32 = SCRATCH + 0xE0;
const PROGNAME_STR: u32 = SCRATCH + 0x100;

/// Data (non-function) symbols the loader can satisfy.
pub(crate) const DATA_SYMBOLS: &[&str] = &[
    "___stdinp", "___stdoutp", "___stderrp", "___progname", "_environ", "_NXArgc", "_NXArgv",
    "_errno", "___mh_execute_header", "__mh_execute_header",
];
const GENERIC_CELLS: u32 = SCRATCH + 0x200;

#[derive(Debug, Clone)]
pub struct Config {
    pub args: Vec<String>,
    pub env: Vec<String>,
    /// Stop after this many guest instructions (0 = unlimited).
    pub max_insns: u64,
    pub trace_syscalls: bool,
    pub trace_imports: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { args: vec![], env: vec![], max_insns: 0, trace_syscalls: false, trace_imports: false }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Log(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Exited(i32),
    Stopped,
    InsnLimit,
    Crashed(String),
}

#[derive(Debug, Clone)]
pub struct RegSnapshot {
    pub gpr: [u32; 32],
    pub pc: u32,
    pub lr: u32,
    pub ctr: u32,
    pub cr: u32,
    pub xer: u32,
    pub insns: u64,
}

pub struct Machine {
    pub cpu: Cpu,
    pub mem: Memory,
    pub stop: Arc<AtomicBool>,
    pub(crate) cfg: Config,
    pub(crate) imports: Vec<String>,
    import_map: HashMap<String, u32>,
    sink: Box<dyn FnMut(Event) + Send>,
    pub(crate) stdin: Box<dyn Read + Send>,
    pub(crate) files: HashMap<u32, std::fs::File>,
    pub(crate) next_fd: u32,
    pub(crate) heap_next: u32,
    pub(crate) heap_end: u32,
    pub(crate) mmap_next: u32,
    pub(crate) exit_code: Option<i32>,
    pub(crate) crash: Option<String>,
    pub(crate) rng: u32,
    pub(crate) warned: HashSet<String>,
    generic_next: u32,
}

impl Machine {
    pub fn load(data: &[u8], cfg: Config, sink: Box<dyn FnMut(Event) + Send>) -> Result<Machine, String> {
        let mo = macho::parse(data)?;
        if mo.filetype != macho::MH_EXECUTE {
            return Err("only MH_EXECUTE Mach-O files can be run".into());
        }
        let mut m = Machine {
            cpu: Cpu::default(),
            mem: Memory::new(),
            stop: Arc::new(AtomicBool::new(false)),
            cfg,
            imports: Vec::new(),
            import_map: HashMap::new(),
            sink,
            stdin: Box::new(std::io::empty()),
            files: HashMap::new(),
            next_fd: 3,
            heap_next: HEAP_BASE,
            heap_end: HEAP_BASE,
            mmap_next: MMAP_BASE,
            exit_code: None,
            crash: None,
            rng: 1,
            warned: HashSet::new(),
            generic_next: GENERIC_CELLS,
        };
        m.map_image(&mo)?;
        m.mem.map(SCRATCH, 0x2000);
        m.mem.map(STACK_TOP - STACK_SIZE, STACK_SIZE);
        // Returning from the entry point behaves like exit(r3).
        let exit_slot = m.import_slot("_exit");
        m.setup_scratch();
        m.bind_imports(&mo)?;
        let sp = m.setup_stack()?;
        m.cpu.gpr[1] = sp;
        m.cpu.pc = mo.entry;
        m.cpu.lr = exit_slot;
        Ok(m)
    }

    pub fn set_stdin(&mut self, r: Box<dyn Read + Send>) {
        self.stdin = r;
    }

    pub fn snapshot(&self) -> RegSnapshot {
        RegSnapshot {
            gpr: self.cpu.gpr,
            pc: self.cpu.pc,
            lr: self.cpu.lr,
            ctr: self.cpu.ctr,
            cr: self.cpu.cr,
            xer: self.cpu.xer,
            insns: self.cpu.tb,
        }
    }

    pub(crate) fn emit(&mut self, ev: Event) {
        (self.sink)(ev);
    }

    pub(crate) fn log(&mut self, msg: impl Into<String>) {
        self.emit(Event::Log(msg.into()));
    }

    pub(crate) fn warn_once(&mut self, key: &str, msg: String) {
        if self.warned.insert(key.to_string()) {
            self.log(msg);
        }
    }

    fn map_image(&mut self, mo: &MachO) -> Result<(), String> {
        for seg in &mo.segments {
            if seg.vmsize == 0 || seg.name == "__PAGEZERO" {
                continue;
            }
            self.mem.map(seg.vmaddr, seg.vmsize);
            let start = seg.fileoff as usize;
            let end = start + seg.filesize as usize;
            let bytes = mo
                .image
                .get(start..end)
                .ok_or_else(|| format!("segment {} extends past end of file", seg.name))?;
            self.mem
                .write_bytes(seg.vmaddr, bytes)
                .map_err(|_| format!("segment {} does not fit in its mapping", seg.name))?;
        }
        Ok(())
    }

    /// Address of the trap slot for an imported function (allocates one on first use).
    pub(crate) fn import_slot(&mut self, name: &str) -> u32 {
        if let Some(&a) = self.import_map.get(name) {
            return a;
        }
        let addr = TRAP_BASE + 4 * self.imports.len() as u32;
        self.imports.push(name.to_string());
        self.import_map.insert(name.to_string(), addr);
        addr
    }

    fn setup_scratch(&mut self) {
        let w = |m: &mut Memory, a: u32, v: u32| {
            let _ = m.write_u32(a, v);
        };
        w(&mut self.mem, CELL_STDINP, FILE_STDIN);
        w(&mut self.mem, CELL_STDOUTP, FILE_STDOUT);
        w(&mut self.mem, CELL_STDERRP, FILE_STDERR);
    }

    fn data_cell(&mut self, name: &str) -> Option<u32> {
        Some(match name {
            "___stdinp" => CELL_STDINP,
            "___stdoutp" => CELL_STDOUTP,
            "___stderrp" => CELL_STDERRP,
            "___progname" => CELL_PROGNAME,
            "_environ" => CELL_ENVIRON,
            "_NXArgc" => CELL_NXARGC,
            "_NXArgv" => CELL_NXARGV,
            "_errno" => ERRNO_CELL,
            "___mh_execute_header" | "__mh_execute_header" => {
                let a = self.generic_next;
                self.generic_next += 64;
                a
            }
            _ => return None,
        })
    }

    /// Replace libSystem's lazy/non-lazy symbol pointers with host trap slots (a tiny "dyld").
    fn bind_imports(&mut self, mo: &MachO) -> Result<(), String> {
        for sec in &mo.sections {
            let kind = sec.kind();
            if kind != S_LAZY_SYMBOL_POINTERS && kind != S_NON_LAZY_SYMBOL_POINTERS {
                continue;
            }
            let count = sec.size / 4;
            for k in 0..count {
                let idx = sec.reserved1 + k;
                let Some(&sym) = mo.indirect.get(idx as usize) else { continue };
                if sym & 0xC000_0000 != 0 {
                    continue; // local / absolute: already points at real code or data
                }
                let Some(s) = mo.symbols.get(sym as usize) else { continue };
                let name = s.name.clone();
                let target = if kind == S_NON_LAZY_SYMBOL_POINTERS {
                    match self.data_cell(&name) {
                        Some(c) => c,
                        None => self.import_slot(&name),
                    }
                } else {
                    self.import_slot(&name)
                };
                self.mem
                    .write_u32(sec.addr + 4 * k, target)
                    .map_err(|_| "symbol pointer section is not mapped".to_string())?;
            }
        }
        Ok(())
    }

    fn setup_stack(&mut self) -> Result<u32, String> {
        let mut args = self.cfg.args.clone();
        if args.is_empty() {
            args.push("a.out".into());
        }
        let env = self.cfg.env.clone();
        let progname = args[0].rsplit('/').next().unwrap_or("a.out").to_string();
        let _ = self.mem.write_bytes(PROGNAME_STR, progname.as_bytes());
        let _ = self.mem.write_u32(CELL_PROGNAME, PROGNAME_STR);

        let mut top = STACK_TOP - 0x100;
        let mut push_str = |mem: &mut Memory, s: &str| -> u32 {
            top -= s.len() as u32 + 1;
            let _ = mem.write_bytes(top, s.as_bytes());
            let _ = mem.write_u8(top + s.len() as u32, 0);
            top
        };
        let argv_ptrs: Vec<u32> = args.iter().map(|a| push_str(&mut self.mem, a)).collect();
        let env_ptrs: Vec<u32> = env.iter().map(|e| push_str(&mut self.mem, e)).collect();
        let apple = push_str(&mut self.mem, &format!("executable_path={}", args[0]));
        let mut words: Vec<u32> = vec![argv_ptrs.len() as u32];
        words.extend(&argv_ptrs);
        words.push(0);
        let envp_index = words.len();
        words.extend(&env_ptrs);
        words.push(0);
        words.push(apple);
        words.push(0);
        let mut sp = (top & !0xF) - (words.len() as u32 * 4);
        sp &= !0xF;
        for (i, w) in words.iter().enumerate() {
            self.mem.write_u32(sp + 4 * i as u32, *w).map_err(|_| "stack setup failed".to_string())?;
        }
        let _ = self.mem.write_u32(CELL_ENVIRON, sp + 4 * envp_index as u32);
        let _ = self.mem.write_u32(CELL_NXARGC, argv_ptrs.len() as u32);
        let _ = self.mem.write_u32(CELL_NXARGV, sp + 4);
        Ok(sp)
    }

    /// Run until the program exits, crashes, is stopped, or hits the instruction limit.
    pub fn run(&mut self) -> Outcome {
        loop {
            let pc = self.cpu.pc;
            if pc >= TRAP_BASE && pc < TRAP_BASE + 4 * self.imports.len() as u32 {
                let idx = ((pc - TRAP_BASE) / 4) as usize;
                self.hle_call(idx);
                if let Some(c) = self.crash.take() {
                    return Outcome::Crashed(c);
                }
                if let Some(code) = self.exit_code {
                    return Outcome::Exited(code);
                }
                continue;
            }
            match self.cpu.step(&mut self.mem) {
                Exit::Continue => {}
                Exit::Syscall => {
                    self.do_syscall();
                    if let Some(c) = self.crash.take() {
                        return Outcome::Crashed(c);
                    }
                    if let Some(code) = self.exit_code {
                        return Outcome::Exited(code);
                    }
                }
                Exit::Trap => return Outcome::Crashed(format!("trap instruction at 0x{pc:08x}")),
                Exit::Illegal(i) => {
                    let hint = if i >> 26 == 4 { " (AltiVec/VMX is not implemented)" } else { "" };
                    return Outcome::Crashed(format!(
                        "unsupported instruction 0x{i:08x} at 0x{pc:08x}{hint}"
                    ));
                }
                Exit::Fault(f) => {
                    return Outcome::Crashed(format!(
                        "memory fault ({}) at 0x{:08x}, pc=0x{pc:08x}",
                        if f.write { "write" } else { "read/fetch" },
                        f.addr
                    ));
                }
            }
            if self.cpu.tb & 0x1fff == 0 {
                if self.stop.load(Ordering::Relaxed) {
                    return Outcome::Stopped;
                }
                if self.cfg.max_insns != 0 && self.cpu.tb >= self.cfg.max_insns {
                    return Outcome::InsnLimit;
                }
            }
        }
    }
}
