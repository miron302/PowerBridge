use ppcx_core::{Config, Event, Machine, Outcome};
use std::sync::{Arc, Mutex};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/").to_string() + name).unwrap()
}

fn run(name: &str) -> (Outcome, String, String) {
    let out = Arc::new(Mutex::new(Vec::new()));
    let logs = Arc::new(Mutex::new(Vec::new()));
    let (o2, l2) = (out.clone(), logs.clone());
    let sink = Box::new(move |ev: Event| match ev {
        Event::Stdout(b) => o2.lock().unwrap().extend(b),
        Event::Stderr(b) => o2.lock().unwrap().extend(b),
        Event::Log(s) => l2.lock().unwrap().push(s),
    });
    let mut cfg = Config::default();
    cfg.args = vec![name.to_string()];
    cfg.max_insns = 10_000_000;
    let mut m = Machine::load(&fixture(name), cfg, sink).expect("load");
    let outcome = m.run();
    let s = String::from_utf8_lossy(&out.lock().unwrap()).into_owned();
    let l = logs.lock().unwrap().join("\n");
    (outcome, s, l)
}

#[test]
fn hello_via_raw_syscalls() {
    let (o, s, _) = run("hello_sc.bin");
    assert_eq!(o, Outcome::Exited(0));
    assert_eq!(s, "Hello from PowerPC!\n");
}

#[test]
fn printf_via_picsymbolstub() {
    let (o, s, l) = run("calc_printf.bin");
    assert_eq!(s, "sum=55 fact=3628800 hex=0xff str=PowerPC\n", "logs: {l}");
    assert_eq!(o, Outcome::Exited(7));
}

#[test]
fn universal_binary_picks_ppc_slice() {
    let (o, s, _) = run("calc_printf_fat.bin");
    assert_eq!(o, Outcome::Exited(7));
    assert!(s.starts_with("sum=55"));
}

#[test]
fn integer_instruction_semantics() {
    let (o, s, l) = run("ops.bin");
    assert_eq!(s, "18 -4 14 1 -5 15 -128\n7 44332211\n", "logs: {l}");
    assert_eq!(o, Outcome::Exited(0));
}

#[test]
fn rejects_non_ppc() {
    let mut x86 = vec![0xCE, 0xFA, 0xED, 0xFE];
    x86.extend(vec![0u8; 64]);
    let sink = Box::new(|_| {});
    let err = Machine::load(&x86, Config::default(), sink).err().expect("must fail");
    assert!(err.contains("not a PowerPC"), "{err}");
}
