//! C ABI for the Swift app (see ppcx.h).

use ppcx_core::{bundle, pkg, Config, Event, Machine, Outcome};
use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type EventCb = extern "C" fn(i32, *const u8, usize, *mut c_void);

struct State {
    exit_code: i32,
    message: String,
    regs: [u32; 40],
}

impl Default for State {
    fn default() -> Self {
        State { exit_code: 0, message: String::new(), regs: [0; 40] }
    }
}

pub struct Session {
    stop: Arc<AtomicBool>,
    state: Mutex<State>,
}

struct Callback {
    cb: EventCb,
    user: *mut c_void,
}
unsafe impl Send for Callback {}
impl Callback {
    fn call(&self, kind: i32, data: &[u8]) {
        (self.cb)(kind, data.as_ptr(), data.len(), self.user);
    }
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn to_c(s: String) -> *mut c_char {
    CString::new(s.replace('\0', " ")).unwrap().into_raw()
}

#[no_mangle]
pub extern "C" fn ppcx_session_new() -> *mut Session {
    Box::into_raw(Box::new(Session { stop: Arc::new(AtomicBool::new(false)), state: Mutex::new(State::default()) }))
}

#[no_mangle]
pub unsafe extern "C" fn ppcx_session_free(s: *mut Session) {
    if !s.is_null() {
        drop(Box::from_raw(s));
    }
}

#[no_mangle]
pub unsafe extern "C" fn ppcx_session_stop(s: *mut Session) {
    if let Some(s) = s.as_ref() {
        s.stop.store(true, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ppcx_session_exit_code(s: *mut Session) -> i32 {
    s.as_ref().map(|s| s.state.lock().unwrap().exit_code).unwrap_or(-1)
}

/// Caller frees with ppcx_string_free.
#[no_mangle]
pub unsafe extern "C" fn ppcx_session_message(s: *mut Session) -> *mut c_char {
    to_c(s.as_ref().map(|s| s.state.lock().unwrap().message.clone()).unwrap_or_default())
}

/// Layout: r0..r31, pc, lr, ctr, cr, xer, instructions(hi), instructions(lo). Copies up to `n` words.
#[no_mangle]
pub unsafe extern "C" fn ppcx_session_regs(s: *mut Session, out: *mut u32, n: usize) {
    if let Some(s) = s.as_ref() {
        let st = s.state.lock().unwrap();
        for i in 0..n.min(40) {
            *out.add(i) = st.regs[i];
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ppcx_string_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}

/// JSON analysis of an app bundle or Mach-O file. Never returns NULL; errors are {"error": "..."}.
#[no_mangle]
pub unsafe extern "C" fn ppcx_analyze_json(path: *const c_char) -> *mut c_char {
    let p = cstr(path);
    let r = catch_unwind(|| match bundle::analyze(Path::new(&p)) {
        Ok(a) => a.to_json(),
        Err(e) => bundle::error_json(&e),
    })
    .unwrap_or_else(|_| bundle::error_json("internal error while analysing"));
    to_c(r)
}

/// Unpack an installer package (.pkg / .mpkg) into `dest_dir` and list the PowerPC apps/programs inside.
/// JSON: {"apps":[..],"binaries":[..],"files":N,"bytes":N,"warnings":[..]} or {"error":"..."}.
#[no_mangle]
pub unsafe extern "C" fn ppcx_pkg_extract_json(pkg_path: *const c_char, dest_dir: *const c_char) -> *mut c_char {
    let (p, d) = (cstr(pkg_path), cstr(dest_dir));
    let r = catch_unwind(|| match pkg::extract_pkg(Path::new(&p), Path::new(&d)) {
        Ok(e) => e.to_json(),
        Err(e) => bundle::error_json(&e),
    })
    .unwrap_or_else(|_| bundle::error_json("internal error while opening the package"));
    to_c(r)
}

/// Returns 0 and the new bundle path in *out on success, 1 and an error message otherwise.
/// *out must be freed with ppcx_string_free.
#[no_mangle]
pub unsafe extern "C" fn ppcx_wrap_app(
    src: *const c_char,
    dest_dir: *const c_char,
    ppcx_bin: *const c_char,
    out: *mut *mut c_char,
) -> i32 {
    let (s, d, b) = (cstr(src), cstr(dest_dir), cstr(ppcx_bin));
    let r = catch_unwind(|| bundle::wrap_app(Path::new(&s), Path::new(&d), Path::new(&b)))
        .unwrap_or_else(|_| Err("internal error while wrapping".into()));
    match r {
        Ok(p) => {
            *out = to_c(p.display().to_string());
            0
        }
        Err(e) => {
            *out = to_c(e);
            1
        }
    }
}

/// Blocking. Returns 0 exited, 1 stopped, 2 instruction limit, 3 crashed, -1 failed to load.
/// Events: kind 0 = stdout, 1 = stderr, 2 = log line.
#[no_mangle]
pub unsafe extern "C" fn ppcx_run(
    s: *mut Session,
    path: *const c_char,
    args: *const *const c_char,
    nargs: usize,
    max_insns: u64,
    trace_sys: i32,
    trace_imp: i32,
    cb: Option<EventCb>,
    user: *mut c_void,
) -> i32 {
    let Some(session) = s.as_ref() else { return -1 };
    let path = cstr(path);
    let mut argv = Vec::new();
    for i in 0..nargs {
        argv.push(cstr(*args.add(i)));
    }
    let cbk = cb.map(|cb| Callback { cb, user });
    let r = catch_unwind(AssertUnwindSafe(|| run_inner(session, &path, argv, max_insns, trace_sys != 0, trace_imp != 0, cbk)));
    match r {
        Ok(code) => code,
        Err(_) => {
            session.state.lock().unwrap().message = "internal error (panic in emulator)".into();
            3
        }
    }
}

fn run_inner(
    session: &Session,
    path: &str,
    args: Vec<String>,
    max_insns: u64,
    trace_sys: bool,
    trace_imp: bool,
    cb: Option<Callback>,
) -> i32 {
    let fail = |msg: String| {
        session.state.lock().unwrap().message = msg;
        -1
    };
    let exec: PathBuf = match bundle::resolve_executable(Path::new(path)) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let data = match std::fs::read(&exec) {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot read {}: {e}", exec.display())),
    };
    let mut cfg = Config::default();
    cfg.args = std::iter::once(exec.display().to_string()).chain(args).collect();
    cfg.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    cfg.max_insns = max_insns;
    cfg.trace_syscalls = trace_sys;
    cfg.trace_imports = trace_imp;

    let sink: Box<dyn FnMut(Event) + Send> = Box::new(move |ev| {
        if let Some(c) = &cb {
            match ev {
                Event::Stdout(b) => c.call(0, &b),
                Event::Stderr(b) => c.call(1, &b),
                Event::Log(s) => c.call(2, s.as_bytes()),
            }
        }
    });
    let mut m = match Machine::load(&data, cfg, sink) {
        Ok(m) => m,
        Err(e) => return fail(e),
    };
    session.stop.store(false, Ordering::Relaxed);
    m.stop = session.stop.clone();
    let outcome = m.run();

    let snap = m.snapshot();
    let mut st = session.state.lock().unwrap();
    st.regs[..32].copy_from_slice(&snap.gpr);
    st.regs[32] = snap.pc;
    st.regs[33] = snap.lr;
    st.regs[34] = snap.ctr;
    st.regs[35] = snap.cr;
    st.regs[36] = snap.xer;
    st.regs[37] = (snap.insns >> 32) as u32;
    st.regs[38] = snap.insns as u32;
    match outcome {
        Outcome::Exited(c) => {
            st.exit_code = c;
            st.message = format!("Exited with status {c}");
            0
        }
        Outcome::Stopped => {
            st.message = "Stopped".into();
            1
        }
        Outcome::InsnLimit => {
            st.message = "Stopped: instruction limit reached".into();
            2
        }
        Outcome::Crashed(m) => {
            st.message = m;
            3
        }
    }
}
