//! Small dependency-free DEFLATE decoder (RFC 1951) with gzip (RFC 1952) and zlib (RFC 1950) wrappers.

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl<'a> Bits<'a> {
    fn bits(&mut self, n: u32) -> Result<u32, String> {
        while self.cnt < n {
            let b = *self.data.get(self.pos).ok_or("unexpected end of compressed data")?;
            self.pos += 1;
            self.buf |= (b as u32) << self.cnt;
            self.cnt += 8;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.cnt -= n;
        Ok(v)
    }
}

struct Huff {
    count: [u16; 16],
    symbol: Vec<u16>,
}

fn build(lengths: &[u8]) -> Result<Huff, String> {
    let mut count = [0u16; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    let mut left: i32 = 1;
    for len in 1..16 {
        left <<= 1;
        left -= count[len] as i32;
        if left < 0 {
            return Err("corrupt compressed data (over-subscribed code)".into());
        }
    }
    let mut offs = [0u16; 16];
    for len in 1..15 {
        offs[len + 1] = offs[len] + count[len];
    }
    let mut symbol = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l != 0 {
            symbol[offs[l as usize] as usize] = sym as u16;
            offs[l as usize] += 1;
        }
    }
    Ok(Huff { count, symbol })
}

fn decode(b: &mut Bits, h: &Huff) -> Result<u16, String> {
    let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
    for len in 1..16 {
        code |= b.bits(1)? as i32;
        let count = h.count[len] as i32;
        if code - count < first {
            return h
                .symbol
                .get((index + (code - first)) as usize)
                .copied()
                .ok_or_else(|| "corrupt compressed data".to_string());
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    Err("corrupt compressed data (bad code)".into())
}

const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

fn codes(b: &mut Bits, out: &mut Vec<u8>, lit: &Huff, dist: &Huff, limit: usize) -> Result<(), String> {
    loop {
        let sym = decode(b, lit)? as usize;
        if sym < 256 {
            out.push(sym as u8);
            if out.len() > limit {
                return Err("decompressed data is too large".into());
            }
        } else if sym == 256 {
            return Ok(());
        } else {
            let s = sym - 257;
            if s >= 29 {
                return Err("corrupt compressed data (bad length)".into());
            }
            let len = LBASE[s] as usize + b.bits(LEXT[s] as u32)? as usize;
            let ds = decode(b, dist)? as usize;
            if ds >= 30 {
                return Err("corrupt compressed data (bad distance)".into());
            }
            let d = DBASE[ds] as usize + b.bits(DEXT[ds] as u32)? as usize;
            if d > out.len() {
                return Err("corrupt compressed data (distance too far back)".into());
            }
            if out.len() + len > limit {
                return Err("decompressed data is too large".into());
            }
            let start = out.len() - d;
            for i in 0..len {
                let v = out[start + i];
                out.push(v);
            }
        }
    }
}

/// Decode a raw DEFLATE stream. `limit` caps the output size.
pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut b = Bits { data, pos: 0, buf: 0, cnt: 0 };
    let mut out: Vec<u8> = Vec::with_capacity(data.len().saturating_mul(3).min(limit).min(1 << 28));
    loop {
        let last = b.bits(1)?;
        match b.bits(2)? {
            0 => {
                b.buf = 0;
                b.cnt = 0;
                let p = b.pos;
                let hdr = data.get(p..p + 4).ok_or("unexpected end of compressed data")?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
                let nlen = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
                if len != (!nlen & 0xffff) {
                    return Err("corrupt compressed data (stored block)".into());
                }
                let body = data.get(p + 4..p + 4 + len).ok_or("unexpected end of compressed data")?;
                if out.len() + len > limit {
                    return Err("decompressed data is too large".into());
                }
                out.extend_from_slice(body);
                b.pos = p + 4 + len;
            }
            1 => {
                let mut l = [0u8; 288];
                for (i, v) in l.iter_mut().enumerate() {
                    *v = match i {
                        0..=143 => 8,
                        144..=255 => 9,
                        256..=279 => 7,
                        _ => 8,
                    };
                }
                let lit = build(&l)?;
                let dist = build(&[5u8; 30])?;
                codes(&mut b, &mut out, &lit, &dist, limit)?;
            }
            2 => {
                let nlen = b.bits(5)? as usize + 257;
                let ndist = b.bits(5)? as usize + 1;
                let ncode = b.bits(4)? as usize + 4;
                if nlen > 286 || ndist > 30 {
                    return Err("corrupt compressed data (too many codes)".into());
                }
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut lengths = [0u8; 320];
                for &o in ORDER.iter().take(ncode) {
                    lengths[o] = b.bits(3)? as u8;
                }
                let lencode = build(&lengths[..19])?;
                let mut idx = 0;
                let mut ll = [0u8; 320];
                while idx < nlen + ndist {
                    let sym = decode(&mut b, &lencode)?;
                    if sym < 16 {
                        ll[idx] = sym as u8;
                        idx += 1;
                    } else {
                        let (val, rep) = match sym {
                            16 => {
                                if idx == 0 {
                                    return Err("corrupt compressed data (repeat with no previous length)".into());
                                }
                                (ll[idx - 1], 3 + b.bits(2)? as usize)
                            }
                            17 => (0, 3 + b.bits(3)? as usize),
                            _ => (0, 11 + b.bits(7)? as usize),
                        };
                        if idx + rep > nlen + ndist {
                            return Err("corrupt compressed data (too many lengths)".into());
                        }
                        for _ in 0..rep {
                            ll[idx] = val;
                            idx += 1;
                        }
                    }
                }
                let lit = build(&ll[..nlen])?;
                let dist = build(&ll[nlen..nlen + ndist])?;
                codes(&mut b, &mut out, &lit, &dist, limit)?;
            }
            _ => return Err("corrupt compressed data (bad block type)".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

pub fn gunzip(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b || data[2] != 8 {
        return Err("not a gzip stream".into());
    }
    let flags = data[3];
    let mut p = 10usize;
    if flags & 4 != 0 {
        let xlen = u16::from_le_bytes([*data.get(p).ok_or("truncated gzip header")?, *data.get(p + 1).ok_or("truncated gzip header")?]) as usize;
        p += 2 + xlen;
    }
    for bit in [8u8, 16u8] {
        if flags & bit != 0 {
            while *data.get(p).ok_or("truncated gzip header")? != 0 {
                p += 1;
            }
            p += 1;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    inflate(data.get(p..).ok_or("truncated gzip header")?, limit)
}

pub fn zlib_decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 6 || data[0] & 0x0f != 8 || ((data[0] as u16) << 8 | data[1] as u16) % 31 != 0 || data[1] & 0x20 != 0 {
        return Err("not a zlib stream".into());
    }
    inflate(&data[2..], limit)
}
