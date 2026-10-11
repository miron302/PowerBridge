use ppcx_core::{bundle, macho, pkg, Config, Event, Machine, Outcome};
use std::io::Write;

fn usage() -> ! {
    eprintln!(
        "ppcx - PowerBridge command-line translator: run 32-bit PowerPC Mach-O programs on an Intel Mac\n\n\
         usage: ppcx [options] <ppc-binary-or-.app> [args...]\n\n\
         options:\n  \
           --info             print compatibility report and exit\n  \
           --analyze          print the report as JSON and exit\n  \
           --wrap DIR         create an Intel-launchable wrapper app for a PowerPC .app in DIR\n  \
           --extract-pkg DIR  unpack an installer package (.pkg/.mpkg) into DIR and list what's inside\n  \
           --trace-syscalls   log every syscall\n  \
           --trace-imports    log every libSystem import call\n  \
           --max-insns N      stop after N guest instructions\n  \
           --regs             dump registers on exit"
    );
    std::process::exit(2)
}

fn main() {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = Config::default();
    let (mut info, mut regs, mut json) = (false, false, false);
    let mut wrap_dir: Option<String> = None;
    let mut pkg_dir: Option<String> = None;
    while !argv.is_empty() && argv[0].starts_with("--") {
        match argv.remove(0).as_str() {
            "--info" => info = true,
            "--regs" => regs = true,
            "--analyze" => json = true,
            "--extract-pkg" => {
                if argv.is_empty() { usage() }
                pkg_dir = Some(argv.remove(0));
            }
            "--wrap" => {
                if argv.is_empty() { usage() }
                wrap_dir = Some(argv.remove(0));
            }
            "--trace-syscalls" => cfg.trace_syscalls = true,
            "--trace-imports" => cfg.trace_imports = true,
            "--max-insns" => {
                if argv.is_empty() { usage() }
                cfg.max_insns = argv.remove(0).parse().unwrap_or_else(|_| usage());
            }
            _ => usage(),
        }
    }
    if argv.is_empty() {
        usage();
    }
    let path = argv.remove(0);
    let die = |msg: String| -> ! {
        eprintln!("ppcx: {msg}");
        std::process::exit(1)
    };
    if let Some(dir) = pkg_dir {
        match pkg::extract_pkg(std::path::Path::new(&path), std::path::Path::new(&dir)) {
            Ok(e) => {
                if json {
                    println!("{}", e.to_json());
                } else {
                    println!("unpacked {} files ({} bytes) into {dir}", e.files, e.bytes);
                    for a in &e.apps { println!("  app      {}", a.display()); }
                    for b in &e.binaries { println!("  program  {}", b.display()); }
                    if e.apps.is_empty() && e.binaries.is_empty() { println!("  (no PowerPC apps or programs found)"); }
                    for w in &e.warnings { println!("  skipped  {w}"); }
                }
            }
            Err(e) => die(e),
        }
        return;
    }
    if pkg::is_pkg_path(std::path::Path::new(&path)) {
        if !info && !json {
            die("this is an installer package. Use --extract-pkg DIR to unpack it, then run an app inside.".into());
        }
        let tmp = std::env::temp_dir().join(format!("ppcx-pkg-{}", std::process::id()));
        let res = pkg::extract_pkg(std::path::Path::new(&path), &tmp);
        match res {
            Ok(e) => {
                for item in e.apps.iter().chain(e.binaries.iter()) {
                    match bundle::analyze(item) {
                        Ok(a) => if json { println!("{}", a.to_json()) } else { print!("{}", a.report()) },
                        Err(err) => eprintln!("ppcx: {}: {err}", item.display()),
                    }
                }
                if e.apps.is_empty() && e.binaries.is_empty() { println!("no PowerPC apps or programs found in this package"); }
                let _ = std::fs::remove_dir_all(&tmp);
            }
            Err(err) => {
                let _ = std::fs::remove_dir_all(&tmp);
                die(err)
            }
        }
        return;
    }
    if let Some(dir) = wrap_dir {
        let me = std::env::current_exe().unwrap_or_else(|e| die(e.to_string()));
        match bundle::wrap_app(std::path::Path::new(&path), std::path::Path::new(&dir), &me) {
            Ok(p) => println!("created {}", p.display()),
            Err(e) => die(e),
        }
        return;
    }
    if json || info {
        match bundle::analyze(std::path::Path::new(&path)) {
            Ok(a) => {
                if json {
                    println!("{}", a.to_json());
                } else {
                    print!("{}", a.report());
                    if let Ok(d) = std::fs::read(&a.exec_path) {
                        if let Ok(m) = macho::parse(&d) {
                            print!("{}", m.describe());
                        }
                    }
                }
            }
            Err(e) => {
                if json { println!("{}", bundle::error_json(&e)); }
                die(e)
            }
        }
        return;
    }
    let exec = bundle::resolve_executable(std::path::Path::new(&path)).unwrap_or_else(|e| die(e));
    let data = std::fs::read(&exec).unwrap_or_else(|e| die(format!("cannot read {}: {e}", exec.display())));
    cfg.args = std::iter::once(exec.display().to_string()).chain(argv).collect();
    cfg.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();

    let sink = Box::new(|ev: Event| match ev {
        Event::Stdout(b) => {
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(&b);
            let _ = o.flush();
        }
        Event::Stderr(b) => {
            let _ = std::io::stderr().write_all(&b);
        }
        Event::Log(s) => eprintln!("[ppcx] {s}"),
    });
    let mut m = match Machine::load(&data, cfg, sink) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ppcx: {e}");
            std::process::exit(1)
        }
    };
    m.set_stdin(Box::new(std::io::stdin()));
    let out = m.run();
    if regs {
        let s = m.snapshot();
        eprintln!("pc={:08x} lr={:08x} ctr={:08x} cr={:08x} xer={:08x} insns={}", s.pc, s.lr, s.ctr, s.cr, s.xer, s.insns);
        for r in 0..32 {
            eprint!("r{r:<2}={:08x}{}", s.gpr[r], if r % 4 == 3 { "\n" } else { "  " });
        }
    }
    match out {
        Outcome::Exited(c) => std::process::exit(c),
        Outcome::Stopped => std::process::exit(130),
        Outcome::InsnLimit => {
            eprintln!("ppcx: instruction limit reached");
            std::process::exit(124)
        }
        Outcome::Crashed(msg) => {
            eprintln!("ppcx: guest crashed: {msg}");
            std::process::exit(139)
        }
    }
}
