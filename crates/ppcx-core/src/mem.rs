//! Sparse 32-bit big-endian guest memory (4 KiB pages, flat page table).

#[derive(Debug, Clone, Copy)]
pub struct Fault {
    pub addr: u32,
    pub write: bool,
}

type Page = Box<[u8; 4096]>;

pub struct Memory {
    pages: Vec<Option<Page>>,
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

impl Memory {
    pub fn new() -> Self {
        let mut pages = Vec::with_capacity(1 << 20);
        pages.resize_with(1 << 20, || None);
        Memory { pages }
    }

    /// Map (zero-filled) every page touched by [addr, addr+len).
    pub fn map(&mut self, addr: u32, len: u32) {
        if len == 0 {
            return;
        }
        let start = (addr >> 12) as u64;
        let end = ((addr as u64 + len as u64 + 0xfff) >> 12).min(1 << 20);
        for p in start..end {
            let slot = &mut self.pages[p as usize];
            if slot.is_none() {
                *slot = Some(Box::new([0u8; 4096]));
            }
        }
    }

    pub fn is_mapped(&self, addr: u32) -> bool {
        self.pages[(addr >> 12) as usize].is_some()
    }

    #[inline]
    fn pg(&self, addr: u32, write: bool) -> Result<&[u8; 4096], Fault> {
        match &self.pages[(addr >> 12) as usize] {
            Some(p) => Ok(p),
            None => Err(Fault { addr, write }),
        }
    }

    #[inline]
    fn pg_mut(&mut self, addr: u32) -> Result<&mut [u8; 4096], Fault> {
        match &mut self.pages[(addr >> 12) as usize] {
            Some(p) => Ok(p),
            None => Err(Fault { addr, write: true }),
        }
    }

    #[inline]
    pub fn read_u8(&self, a: u32) -> Result<u8, Fault> {
        Ok(self.pg(a, false)?[(a & 0xfff) as usize])
    }

    #[inline]
    pub fn read_u16(&self, a: u32) -> Result<u16, Fault> {
        let off = (a & 0xfff) as usize;
        if off <= 4094 {
            let p = self.pg(a, false)?;
            Ok(u16::from_be_bytes([p[off], p[off + 1]]))
        } else {
            Ok(((self.read_u8(a)? as u16) << 8) | self.read_u8(a.wrapping_add(1))? as u16)
        }
    }

    #[inline]
    pub fn read_u32(&self, a: u32) -> Result<u32, Fault> {
        let off = (a & 0xfff) as usize;
        if off <= 4092 {
            let p = self.pg(a, false)?;
            Ok(u32::from_be_bytes([p[off], p[off + 1], p[off + 2], p[off + 3]]))
        } else {
            let mut v = 0u32;
            for i in 0..4 {
                v = (v << 8) | self.read_u8(a.wrapping_add(i))? as u32;
            }
            Ok(v)
        }
    }

    pub fn read_u64(&self, a: u32) -> Result<u64, Fault> {
        let hi = self.read_u32(a)? as u64;
        let lo = self.read_u32(a.wrapping_add(4))? as u64;
        Ok((hi << 32) | lo)
    }

    #[inline]
    pub fn write_u8(&mut self, a: u32, v: u8) -> Result<(), Fault> {
        self.pg_mut(a)?[(a & 0xfff) as usize] = v;
        Ok(())
    }

    #[inline]
    pub fn write_u16(&mut self, a: u32, v: u16) -> Result<(), Fault> {
        let off = (a & 0xfff) as usize;
        let b = v.to_be_bytes();
        if off <= 4094 {
            let p = self.pg_mut(a)?;
            p[off] = b[0];
            p[off + 1] = b[1];
            Ok(())
        } else {
            self.write_u8(a, b[0])?;
            self.write_u8(a.wrapping_add(1), b[1])
        }
    }

    #[inline]
    pub fn write_u32(&mut self, a: u32, v: u32) -> Result<(), Fault> {
        let off = (a & 0xfff) as usize;
        let b = v.to_be_bytes();
        if off <= 4092 {
            let p = self.pg_mut(a)?;
            p[off..off + 4].copy_from_slice(&b);
            Ok(())
        } else {
            for (i, byte) in b.iter().enumerate() {
                self.write_u8(a.wrapping_add(i as u32), *byte)?;
            }
            Ok(())
        }
    }

    pub fn write_u64(&mut self, a: u32, v: u64) -> Result<(), Fault> {
        self.write_u32(a, (v >> 32) as u32)?;
        self.write_u32(a.wrapping_add(4), v as u32)
    }

    pub fn read_bytes(&self, addr: u32, len: u32) -> Result<Vec<u8>, Fault> {
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            out.push(self.read_u8(addr.wrapping_add(i))?);
        }
        Ok(out)
    }

    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), Fault> {
        for (i, b) in data.iter().enumerate() {
            self.write_u8(addr.wrapping_add(i as u32), *b)?;
        }
        Ok(())
    }

    /// Read a NUL-terminated string (without the terminator).
    pub fn read_cstr(&self, addr: u32, max: usize) -> Result<Vec<u8>, Fault> {
        let mut out = Vec::new();
        let mut a = addr;
        while out.len() < max {
            let b = self.read_u8(a)?;
            if b == 0 {
                break;
            }
            out.push(b);
            a = a.wrapping_add(1);
        }
        Ok(out)
    }
}
