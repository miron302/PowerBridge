//! 32-bit PowerPC user-mode interpreter (integer core + scalar floating point).

use crate::mem::{Fault, Memory};

pub const XER_SO: u32 = 0x8000_0000;
pub const XER_OV: u32 = 0x4000_0000;
pub const XER_CA: u32 = 0x2000_0000;

#[derive(Clone)]
pub struct Cpu {
    pub gpr: [u32; 32],
    pub fpr: [f64; 32],
    pub cr: u32,
    pub lr: u32,
    pub ctr: u32,
    pub xer: u32,
    pub pc: u32,
    pub vrsave: u32,
    /// Free-running counter, incremented once per executed instruction (also serves as the time base).
    pub tb: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu { gpr: [0; 32], fpr: [0.0; 32], cr: 0, lr: 0, ctr: 0, xer: 0, pc: 0, vrsave: 0, tb: 0 }
    }
}

#[derive(Debug)]
pub enum Exit {
    Continue,
    Syscall,
    Trap,
    Illegal(u32),
    Fault(Fault),
}

#[inline]
fn addc3(a: u32, b: u32, c: u32) -> (u32, bool) {
    let s = a as u64 + b as u64 + c as u64;
    (s as u32, (s >> 32) != 0)
}

#[inline]
fn mask(mb: u32, me: u32) -> u32 {
    let m1 = 0xFFFF_FFFFu32 >> mb;
    let m2 = 0xFFFF_FFFFu32 << (31 - me);
    if mb <= me { m1 & m2 } else { m1 | m2 }
}

impl Cpu {
    #[inline]
    fn rd0(&self, r: usize) -> u32 {
        if r == 0 { 0 } else { self.gpr[r] }
    }

    #[inline]
    fn set_cr_field(&mut self, n: u32, v: u32) {
        let sh = 28 - 4 * n;
        self.cr = (self.cr & !(0xF << sh)) | ((v & 0xF) << sh);
    }

    #[inline]
    fn cr_bit(&self, i: u32) -> u32 {
        (self.cr >> (31 - i)) & 1
    }

    #[inline]
    fn set_cr_bit(&mut self, i: u32, v: u32) {
        let sh = 31 - i;
        self.cr = (self.cr & !(1 << sh)) | ((v & 1) << sh);
    }

    #[inline]
    fn so(&self) -> u32 {
        (self.xer >> 31) & 1
    }

    #[inline]
    fn set_cr0(&mut self, r: u32) {
        let v = if (r as i32) < 0 { 8 } else if r == 0 { 2 } else { 4 };
        self.set_cr_field(0, v | self.so());
    }

    #[inline]
    fn set_ca(&mut self, c: bool) {
        if c { self.xer |= XER_CA } else { self.xer &= !XER_CA }
    }

    #[inline]
    fn ca(&self) -> u32 {
        (self.xer >> 29) & 1
    }

    fn cmp_s(&mut self, crf: u32, a: i32, b: i32) {
        let v = if a < b { 8 } else if a > b { 4 } else { 2 };
        self.set_cr_field(crf, v | self.so());
    }

    fn cmp_u(&mut self, crf: u32, a: u32, b: u32) {
        let v = if a < b { 8 } else if a > b { 4 } else { 2 };
        self.set_cr_field(crf, v | self.so());
    }

    fn trap_cond(&self, to: u32, a: u32, b: u32) -> bool {
        ((a as i32) < (b as i32) && to & 16 != 0)
            || ((a as i32) > (b as i32) && to & 8 != 0)
            || (a == b && to & 4 != 0)
            || (a < b && to & 2 != 0)
            || (a > b && to & 1 != 0)
    }

    fn branch_ok(&mut self, bo: u32, bi: u32, use_ctr: bool) -> bool {
        if use_ctr && bo & 4 == 0 {
            self.ctr = self.ctr.wrapping_sub(1);
        }
        let ctr_ok = !use_ctr || bo & 4 != 0 || ((self.ctr != 0) ^ (bo & 2 != 0));
        let cond_ok = bo & 0x10 != 0 || (self.cr_bit(bi) == ((bo >> 3) & 1));
        ctr_ok && cond_ok
    }

    fn spr_read(&self, spr: u32) -> u32 {
        match spr {
            1 => self.xer,
            8 => self.lr,
            9 => self.ctr,
            256 => self.vrsave,
            268 => self.tb as u32,
            269 => (self.tb >> 32) as u32,
            _ => 0,
        }
    }

    fn spr_write(&mut self, spr: u32, v: u32) {
        match spr {
            1 => self.xer = v,
            8 => self.lr = v,
            9 => self.ctr = v,
            256 => self.vrsave = v,
            _ => {}
        }
    }

    pub fn step(&mut self, mem: &mut Memory) -> Exit {
        let insn = match mem.read_u32(self.pc) {
            Ok(v) => v,
            Err(f) => return Exit::Fault(f),
        };
        self.tb = self.tb.wrapping_add(1);
        match self.exec(mem, insn) {
            Ok(e) => e,
            Err(f) => Exit::Fault(f),
        }
    }

    fn exec(&mut self, mem: &mut Memory, insn: u32) -> Result<Exit, Fault> {
        let op = insn >> 26;
        let rt = ((insn >> 21) & 31) as usize;
        let ra = ((insn >> 16) & 31) as usize;
        let rb = ((insn >> 11) & 31) as usize;
        let simm = insn as u16 as i16 as i32;
        let uimm = insn & 0xffff;
        let rc = insn & 1 != 0;
        let mut next = self.pc.wrapping_add(4);

        match op {
            3 => {
                if self.trap_cond(rt as u32, self.gpr[ra], simm as u32) {
                    return Ok(Exit::Trap);
                }
            }
            7 => self.gpr[rt] = (self.gpr[ra] as i32).wrapping_mul(simm) as u32,
            8 => {
                let (r, c) = addc3(!self.gpr[ra], simm as u32, 1);
                self.gpr[rt] = r;
                self.set_ca(c);
            }
            10 => self.cmp_u((rt >> 2) as u32, self.gpr[ra], uimm),
            11 => self.cmp_s((rt >> 2) as u32, self.gpr[ra] as i32, simm),
            12 | 13 => {
                let (r, c) = addc3(self.gpr[ra], simm as u32, 0);
                self.gpr[rt] = r;
                self.set_ca(c);
                if op == 13 {
                    self.set_cr0(r);
                }
            }
            14 => self.gpr[rt] = self.rd0(ra).wrapping_add(simm as u32),
            15 => self.gpr[rt] = self.rd0(ra).wrapping_add(uimm << 16),
            16 => {
                let bo = rt as u32;
                let bi = ra as u32;
                let bd = (insn & 0xfffc) as u16 as i16 as i32;
                let target = if insn & 2 != 0 { bd as u32 } else { self.pc.wrapping_add(bd as u32) };
                let taken = self.branch_ok(bo, bi, true);
                if insn & 1 != 0 {
                    self.lr = next;
                }
                if taken {
                    next = target;
                }
            }
            17 => {
                if insn & 2 != 0 {
                    return Ok(Exit::Syscall);
                }
                return Ok(Exit::Illegal(insn));
            }
            18 => {
                let li = (((insn & 0x03ff_fffc) << 6) as i32) >> 6;
                let target = if insn & 2 != 0 { li as u32 } else { self.pc.wrapping_add(li as u32) };
                if insn & 1 != 0 {
                    self.lr = next;
                }
                next = target;
            }
            19 => {
                let xo = (insn >> 1) & 0x3ff;
                match xo {
                    16 | 528 => {
                        let use_ctr = xo == 16;
                        let dest = if use_ctr { self.lr & !3 } else { self.ctr & !3 };
                        let taken = self.branch_ok(rt as u32, ra as u32, use_ctr);
                        if insn & 1 != 0 {
                            self.lr = next;
                        }
                        if taken {
                            next = dest;
                        }
                    }
                    150 => {}
                    0 => {
                        let src = (self.cr >> (28 - 4 * ((ra >> 2) as u32))) & 0xF;
                        self.set_cr_field((rt >> 2) as u32, src);
                    }
                    33 | 129 | 193 | 225 | 257 | 289 | 417 | 449 => {
                        let a = self.cr_bit(ra as u32);
                        let b = self.cr_bit(rb as u32);
                        let r = match xo {
                            33 => !(a | b),
                            129 => a & !b,
                            193 => a ^ b,
                            225 => !(a & b),
                            257 => a & b,
                            289 => !(a ^ b),
                            417 => a | !b,
                            _ => a | b,
                        };
                        self.set_cr_bit(rt as u32, r);
                    }
                    _ => return Ok(Exit::Illegal(insn)),
                }
            }
            20 => {
                let sh = (insn >> 11) & 31;
                let m = mask((insn >> 6) & 31, (insn >> 1) & 31);
                let r = self.gpr[rt].rotate_left(sh);
                let v = (r & m) | (self.gpr[ra] & !m);
                self.gpr[ra] = v;
                if rc { self.set_cr0(v) }
            }
            21 => {
                let sh = (insn >> 11) & 31;
                let m = mask((insn >> 6) & 31, (insn >> 1) & 31);
                let v = self.gpr[rt].rotate_left(sh) & m;
                self.gpr[ra] = v;
                if rc { self.set_cr0(v) }
            }
            23 => {
                let sh = self.gpr[rb] & 31;
                let m = mask((insn >> 6) & 31, (insn >> 1) & 31);
                let v = self.gpr[rt].rotate_left(sh) & m;
                self.gpr[ra] = v;
                if rc { self.set_cr0(v) }
            }
            24 => self.gpr[ra] = self.gpr[rt] | uimm,
            25 => self.gpr[ra] = self.gpr[rt] | (uimm << 16),
            26 => self.gpr[ra] = self.gpr[rt] ^ uimm,
            27 => self.gpr[ra] = self.gpr[rt] ^ (uimm << 16),
            28 => {
                let v = self.gpr[rt] & uimm;
                self.gpr[ra] = v;
                self.set_cr0(v);
            }
            29 => {
                let v = self.gpr[rt] & (uimm << 16);
                self.gpr[ra] = v;
                self.set_cr0(v);
            }
            31 => {
                if let Some(e) = self.exec31(mem, insn)? {
                    return Ok(e);
                }
            }
            32 | 33 | 34 | 35 | 40 | 41 | 42 | 43 => {
                let ea = if op & 1 == 1 { self.gpr[ra] } else { self.rd0(ra) }.wrapping_add(simm as u32);
                let v = match op {
                    32 | 33 => mem.read_u32(ea)?,
                    34 | 35 => mem.read_u8(ea)? as u32,
                    40 | 41 => mem.read_u16(ea)? as u32,
                    _ => mem.read_u16(ea)? as i16 as i32 as u32,
                };
                if op & 1 == 1 {
                    self.gpr[ra] = ea;
                }
                self.gpr[rt] = v;
            }
            36 | 37 | 38 | 39 | 44 | 45 => {
                let ea = if op & 1 == 1 { self.gpr[ra] } else { self.rd0(ra) }.wrapping_add(simm as u32);
                let v = self.gpr[rt];
                match op {
                    36 | 37 => mem.write_u32(ea, v)?,
                    38 | 39 => mem.write_u8(ea, v as u8)?,
                    _ => mem.write_u16(ea, v as u16)?,
                }
                if op & 1 == 1 {
                    self.gpr[ra] = ea;
                }
            }
            46 => {
                let mut ea = self.rd0(ra).wrapping_add(simm as u32);
                for r in rt..32 {
                    self.gpr[r] = mem.read_u32(ea)?;
                    ea = ea.wrapping_add(4);
                }
            }
            47 => {
                let mut ea = self.rd0(ra).wrapping_add(simm as u32);
                for r in rt..32 {
                    mem.write_u32(ea, self.gpr[r])?;
                    ea = ea.wrapping_add(4);
                }
            }
            48..=55 => {
                let upd = op & 1 == 1;
                let ea = if upd { self.gpr[ra] } else { self.rd0(ra) }.wrapping_add(simm as u32);
                match op {
                    48 | 49 => self.fpr[rt] = f32::from_bits(mem.read_u32(ea)?) as f64,
                    50 | 51 => self.fpr[rt] = f64::from_bits(mem.read_u64(ea)?),
                    52 | 53 => mem.write_u32(ea, (self.fpr[rt] as f32).to_bits())?,
                    _ => mem.write_u64(ea, self.fpr[rt].to_bits())?,
                }
                if upd {
                    self.gpr[ra] = ea;
                }
            }
            59 | 63 => {
                if let Some(e) = self.exec_fp(op, insn) {
                    return Ok(e);
                }
            }
            _ => return Ok(Exit::Illegal(insn)),
        }
        self.pc = next;
        Ok(Exit::Continue)
    }

    /// Opcode 31 (X/XO forms). Returns Some(exit) to abort normal pc advance.
    fn exec31(&mut self, mem: &mut Memory, insn: u32) -> Result<Option<Exit>, Fault> {
        let rt = ((insn >> 21) & 31) as usize;
        let ra = ((insn >> 16) & 31) as usize;
        let rb = ((insn >> 11) & 31) as usize;
        let rc = insn & 1 != 0;
        let xo = (insn >> 1) & 0x3ff;
        let (a, b, s) = (self.gpr[ra], self.gpr[rb], self.gpr[rt]);
        let ea_x = self.rd0(ra).wrapping_add(b);

        macro_rules! set_ra {
            ($v:expr) => {{
                let v: u32 = $v;
                self.gpr[ra] = v;
                if rc { self.set_cr0(v) }
            }};
        }
        macro_rules! set_rt {
            ($v:expr) => {{
                let v: u32 = $v;
                self.gpr[rt] = v;
                if rc { self.set_cr0(v) }
            }};
        }

        match xo {
            0 => self.cmp_s((rt >> 2) as u32, a as i32, b as i32),
            32 => self.cmp_u((rt >> 2) as u32, a, b),
            4 => {
                if self.trap_cond(rt as u32, a, b) {
                    return Ok(Some(Exit::Trap));
                }
            }
            19 => self.gpr[rt] = self.cr,
            144 => {
                let fxm = (insn >> 12) & 0xff;
                let mut m = 0u32;
                for i in 0..8 {
                    if fxm & (0x80 >> i) != 0 {
                        m |= 0xF << (28 - 4 * i);
                    }
                }
                self.cr = (self.cr & !m) | (s & m);
            }
            83 => self.gpr[rt] = 0x0000_D032,
            339 => {
                let spr = ((insn >> 16) & 31) | (((insn >> 11) & 31) << 5);
                self.gpr[rt] = self.spr_read(spr);
            }
            371 => {
                let tbr = ((insn >> 16) & 31) | (((insn >> 11) & 31) << 5);
                self.gpr[rt] = self.spr_read(tbr);
            }
            467 => {
                let spr = ((insn >> 16) & 31) | (((insn >> 11) & 31) << 5);
                self.spr_write(spr, s);
            }
            512 => {
                let f = (rt >> 2) as u32;
                let v = self.xer >> 28;
                self.set_cr_field(f, v);
                self.xer &= 0x0FFF_FFFF;
            }
            // logical
            28 => set_ra!(s & b),
            60 => set_ra!(s & !b),
            124 => set_ra!(!(s | b)),
            284 => set_ra!(!(s ^ b)),
            316 => set_ra!(s ^ b),
            412 => set_ra!(s | !b),
            444 => set_ra!(s | b),
            476 => set_ra!(!(s & b)),
            26 => set_ra!(s.leading_zeros()),
            922 => set_ra!(s as u16 as i16 as i32 as u32),
            954 => set_ra!(s as u8 as i8 as i32 as u32),
            24 => {
                let n = b & 0x3f;
                set_ra!(if n > 31 { 0 } else { s << n })
            }
            536 => {
                let n = b & 0x3f;
                set_ra!(if n > 31 { 0 } else { s >> n })
            }
            792 | 824 => {
                let n = if xo == 824 { rb as u32 } else { b & 0x3f };
                let si = s as i32;
                let (r, ca) = if n > 31 {
                    (((si >> 31) as u32), si < 0)
                } else {
                    ((si >> n) as u32, si < 0 && n > 0 && (s & ((1u32 << n) - 1)) != 0)
                };
                self.set_ca(ca);
                set_ra!(r)
            }
            // loads / stores (indexed)
            23 => self.gpr[rt] = mem.read_u32(ea_x)?,
            55 => {
                let v = mem.read_u32(ea_x)?;
                self.gpr[ra] = ea_x;
                self.gpr[rt] = v;
            }
            87 => self.gpr[rt] = mem.read_u8(ea_x)? as u32,
            119 => {
                let v = mem.read_u8(ea_x)? as u32;
                self.gpr[ra] = ea_x;
                self.gpr[rt] = v;
            }
            279 => self.gpr[rt] = mem.read_u16(ea_x)? as u32,
            311 => {
                let v = mem.read_u16(ea_x)? as u32;
                self.gpr[ra] = ea_x;
                self.gpr[rt] = v;
            }
            343 => self.gpr[rt] = mem.read_u16(ea_x)? as i16 as i32 as u32,
            375 => {
                let v = mem.read_u16(ea_x)? as i16 as i32 as u32;
                self.gpr[ra] = ea_x;
                self.gpr[rt] = v;
            }
            20 => self.gpr[rt] = mem.read_u32(ea_x)?,
            534 => self.gpr[rt] = mem.read_u32(ea_x)?.swap_bytes(),
            790 => self.gpr[rt] = (mem.read_u16(ea_x)?).swap_bytes() as u32,
            151 => mem.write_u32(ea_x, s)?,
            183 => {
                mem.write_u32(ea_x, s)?;
                self.gpr[ra] = ea_x;
            }
            215 => mem.write_u8(ea_x, s as u8)?,
            247 => {
                mem.write_u8(ea_x, s as u8)?;
                self.gpr[ra] = ea_x;
            }
            407 => mem.write_u16(ea_x, s as u16)?,
            439 => {
                mem.write_u16(ea_x, s as u16)?;
                self.gpr[ra] = ea_x;
            }
            662 => mem.write_u32(ea_x, s.swap_bytes())?,
            918 => mem.write_u16(ea_x, (s as u16).swap_bytes())?,
            150 => {
                mem.write_u32(ea_x, s)?;
                self.set_cr_field(0, 2 | self.so());
            }
            535 => self.fpr[rt] = f32::from_bits(mem.read_u32(ea_x)?) as f64,
            599 => self.fpr[rt] = f64::from_bits(mem.read_u64(ea_x)?),
            663 => mem.write_u32(ea_x, (self.fpr[rt] as f32).to_bits())?,
            727 => mem.write_u64(ea_x, self.fpr[rt].to_bits())?,
            983 => mem.write_u32(ea_x, self.fpr[rt].to_bits() as u32)?,
            1014 => {
                let base = ea_x & !31;
                for i in 0..8 {
                    mem.write_u32(base + i * 4, 0)?;
                }
            }
            // cache / sync hints
            54 | 86 | 246 | 278 | 470 | 598 | 854 | 982 | 566 | 342 => {}
            _ => {
                // arithmetic forms: OE bit (0x200) is ignored (OV is not modelled)
                match xo & 0x1ff {
                    266 => set_rt!(a.wrapping_add(b)),
                    40 => set_rt!(b.wrapping_sub(a)),
                    104 => set_rt!(a.wrapping_neg()),
                    235 => set_rt!((a as i32).wrapping_mul(b as i32) as u32),
                    75 => set_rt!((((a as i32 as i64) * (b as i32 as i64)) >> 32) as u32),
                    11 => set_rt!((((a as u64) * (b as u64)) >> 32) as u32),
                    491 => {
                        let r = if b == 0 || (a == 0x8000_0000 && b == 0xFFFF_FFFF) { 0 } else { ((a as i32) / (b as i32)) as u32 };
                        set_rt!(r)
                    }
                    459 => set_rt!(if b == 0 { 0 } else { a / b }),
                    10 => {
                        let (r, c) = addc3(a, b, 0);
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    138 => {
                        let (r, c) = addc3(a, b, self.ca());
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    8 => {
                        let (r, c) = addc3(!a, b, 1);
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    136 => {
                        let (r, c) = addc3(!a, b, self.ca());
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    202 => {
                        let (r, c) = addc3(a, 0, self.ca());
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    234 => {
                        let (r, c) = addc3(a, 0xFFFF_FFFF, self.ca());
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    200 => {
                        let (r, c) = addc3(!a, 0, self.ca());
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    232 => {
                        let (r, c) = addc3(!a, 0xFFFF_FFFF, self.ca());
                        self.set_ca(c);
                        set_rt!(r)
                    }
                    _ => return Ok(Some(Exit::Illegal(insn))),
                }
            }
        }
        Ok(None)
    }

    fn exec_fp(&mut self, op: u32, insn: u32) -> Option<Exit> {
        let frt = ((insn >> 21) & 31) as usize;
        let fra = ((insn >> 16) & 31) as usize;
        let frb = ((insn >> 11) & 31) as usize;
        let frc = ((insn >> 6) & 31) as usize;
        let xo10 = (insn >> 1) & 0x3ff;
        let xo5 = xo10 & 0x1f;
        let single = op == 59;
        let (a, b, c) = (self.fpr[fra], self.fpr[frb], self.fpr[frc]);
        let rnd = |v: f64| if single { v as f32 as f64 } else { v };

        let r = match xo5 {
            18 => Some(rnd(a / b)),
            20 => Some(rnd(a - b)),
            21 => Some(rnd(a + b)),
            22 => Some(rnd(b.sqrt())),
            24 if single => Some(rnd(1.0 / b)),
            23 if !single => Some(if a >= 0.0 { c } else { b }),
            25 => Some(rnd(a * c)),
            28 => Some(rnd(a * c - b)),
            29 => Some(rnd(a * c + b)),
            30 => Some(rnd(-(a * c - b))),
            31 => Some(rnd(-(a * c + b))),
            _ => None,
        };
        if let Some(v) = r {
            self.fpr[frt] = v;
            self.pc = self.pc.wrapping_add(4);
            return None;
        }
        if single {
            return Some(Exit::Illegal(insn));
        }
        match xo10 {
            0 | 32 => {
                let crf = (frt >> 2) as u32;
                let v = if a.is_nan() || b.is_nan() { 1 } else if a < b { 8 } else if a > b { 4 } else { 2 };
                self.set_cr_field(crf, v);
            }
            12 => self.fpr[frt] = b as f32 as f64,
            14 | 15 => {
                let i = if b.is_nan() { i32::MIN } else { b.trunc().clamp(i32::MIN as f64, i32::MAX as f64) as i32 };
                self.fpr[frt] = f64::from_bits(0xFFF8_0000_0000_0000 | (i as u32 as u64));
            }
            40 => self.fpr[frt] = -b,
            72 => self.fpr[frt] = b,
            136 => self.fpr[frt] = -b.abs(),
            264 => self.fpr[frt] = b.abs(),
            583 => self.fpr[frt] = f64::from_bits(0),
            711 | 134 | 70 | 38 => {}
            _ => return Some(Exit::Illegal(insn)),
        }
        self.pc = self.pc.wrapping_add(4);
        None
    }
}
