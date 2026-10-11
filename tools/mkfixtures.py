#!/usr/bin/env python3
"""Generate tiny 32-bit PowerPC Mach-O test executables (no PPC toolchain needed).

Produces tests/fixtures/{hello_sc,calc_printf,calc_printf_fat,ops}.bin
"""
import struct, os, sys

OUT = os.path.join(os.path.dirname(__file__), "..", "tests", "fixtures")

# ---------------------------------------------------------------- mini assembler
def u32(x): return x & 0xFFFFFFFF
def D(op, rt, ra, imm): return (op << 26) | (rt << 21) | (ra << 16) | (imm & 0xFFFF)
def addi(rt, ra, si): return D(14, rt, ra, si)
def addis(rt, ra, si): return D(15, rt, ra, si)
def ori(ra, rs, ui): return D(24, rs, ra, ui)
def li(rt, v): return addi(rt, 0, v)
def lwz(rt, d, ra): return D(32, rt, ra, d)
def stw(rs, d, ra): return D(36, rs, ra, d)
def stwu(rs, d, ra): return D(37, rs, ra, d)
def lwzu(rt, d, ra): return D(33, rt, ra, d)
def cmpwi(ra, si): return D(11, 0, ra, si)
def X(rt, ra, rb, xo, rc=0): return (31 << 26) | (rt << 21) | (ra << 16) | (rb << 11) | (xo << 1) | rc
def add(rt, ra, rb): return X(rt, ra, rb, 266)
def mullw(rt, ra, rb): return X(rt, ra, rb, 235)
def divw(rt, ra, rb): return X(rt, ra, rb, 491)
def mulhw(rt, ra, rb): return X(rt, ra, rb, 75)
def subf(rt, ra, rb): return X(rt, ra, rb, 40)
def neg(rt, ra): return X(rt, ra, 0, 104)
def or_(ra, rs, rb): return X(rs, ra, rb, 444)
def mr(ra, rs): return or_(ra, rs, rs)
def cntlzw(ra, rs): return X(rs, ra, 0, 26)
def extsb(ra, rs): return X(rs, ra, 0, 954)
def srawi(ra, rs, sh): return X(rs, ra, sh, 824)
def lwbrx(rt, ra, rb): return X(rt, ra, rb, 534)
def mflr(rt): return X(rt, 8, 0, 339)
def mtlr(rs): return X(rs, 8, 0, 467)
def mtctr(rs): return X(rs, 9, 0, 467)
def rlwinm(ra, rs, sh, mb, me): return (21 << 26) | (rs << 21) | (ra << 16) | (sh << 11) | (mb << 6) | (me << 1)
SC = 0x44000002
BCTR = 0x4E800420
BLR = 0x4E800020
NOP = ori(0, 0, 0)
BCL_NEXT = 0x429F0005  # bcl 20,31,$+4

def li32(rt, v):
    v = u32(v)
    return [addis(rt, 0, (v >> 16) & 0xFFFF), ori(rt, rt, v & 0xFFFF)]

class Asm:
    def __init__(s, base): s.base = base; s.w = []; s.labels = {}; s.fix = []
    def e(s, *ws):
        for x in ws: s.w.append(u32(x))
    def pc(s): return s.base + 4 * len(s.w)
    def label(s, n): s.labels[n] = len(s.w)
    def bl(s, target): s.e(0x48000001 | ((target - s.pc()) & 0x03FFFFFC))
    def bc(s, bo, bi, label):
        s.fix.append((len(s.w), bo, bi, label)); s.e(0)
    def done(s):
        for idx, bo, bi, label in s.fix:
            disp = (s.labels[label] - idx) * 4
            s.w[idx] = u32((16 << 26) | (bo << 21) | (bi << 16) | (disp & 0xFFFC))
        return s.w

# ---------------------------------------------------------------- Mach-O writer
TEXT_VM, DATA_VM, LINK_VM = 0x1000, 0x2000, 0x3000
STUB_SIZE = 32

def pad(b, n): return b + b"\0" * (n - len(b))
def name16(s): return pad(s.encode(), 16)

def section(sect, seg, addr, size, off, flags=0, r1=0, r2=0, align=2):
    return name16(sect) + name16(seg) + struct.pack(">IIIIIIIII", addr, size, off, align, 0, 0, flags, r1, r2)

def segment(name, vm, vmsize, fo, fsize, prot, sects=b"", nsects=0):
    body = name16(name) + struct.pack(">IIIIIIII", vm, vmsize, fo, fsize, 7, prot, nsects, 0)
    return struct.pack(">II", 1, 8 + len(body) + len(sects)) + body + sects

def stub_words(stub_addr, lp_addr):
    L = stub_addr + 8
    d = lp_addr - L
    ha = ((d + 0x8000) >> 16) & 0xFFFF
    lo = d & 0xFFFF
    return [mflr(0), BCL_NEXT, mflr(11), addis(11, 11, ha), mtlr(0), lwzu(12, lo, 11), mtctr(12), BCTR]

def build_macho(prog, strings, imports, dylibs=()):
    """prog(asm, A) fills the assembler. A maps 'stub:<name>' / 'str:<name>' to addresses."""
    n = len(imports)
    def layout(code_len):
        text_addr = TEXT_VM + 0x600
        stubs_addr = text_addr + code_len
        cstr_addr = stubs_addr + n * STUB_SIZE
        A = {}
        for i, nm in enumerate(imports): A["stub:" + nm] = stubs_addr + i * STUB_SIZE
        cur = cstr_addr
        for k, v in strings.items():
            A["str:" + k] = cur; cur += len(v) + 1
        return text_addr, stubs_addr, cstr_addr, cur, A
    # pass 1 to get size
    a = Asm(TEXT_VM + 0x600); prog(a, layout(0)[4]); code_len = 4 * len(a.done())
    text_addr, stubs_addr, cstr_addr, end, A = layout(code_len)
    a = Asm(text_addr); prog(a, A); code = a.done()
    assert 4 * len(code) == code_len
    assert end < TEXT_VM + 0x1000, "text segment overflow"
    lp_addr = DATA_VM
    text = b"".join(struct.pack(">I", w) for w in code)
    stubs = b"".join(struct.pack(">I", w) for i, nm in enumerate(imports) for w in stub_words(stubs_addr + i * STUB_SIZE, lp_addr + 4 * i))
    cstr = b"".join(v + b"\0" for v in strings.values())
    seg_text_data = pad(b"\0" * 0x600 + text + stubs + cstr, 0x1000)  # header gets patched in below
    # symbols
    strtab = b"\0"; nlists = b""
    for nm in imports:
        nlists += struct.pack(">IBBHI", len(strtab), 0x01, 0, 0, 0)
        strtab += ("_" + nm).encode() + b"\0"
    indirect = b"".join(struct.pack(">I", i) for i in range(n)) * 2
    link = nlists + strtab + pad(b"", (-len(strtab)) % 4) + indirect
    symoff = 0x2000
    stroff = symoff + len(nlists)
    indoff = stroff + len(strtab) + ((-len(strtab)) % 4)

    sects_text = (section("__text", "__TEXT", text_addr, len(text), 0x600, 0x80000400) +
                  section("__picsymbolstub1", "__TEXT", stubs_addr, n * STUB_SIZE, 0x600 + len(text), 0x80000008, 0, STUB_SIZE) +
                  section("__cstring", "__TEXT", cstr_addr, len(cstr), 0x600 + len(text) + n * STUB_SIZE, 2, align=0))
    sects_data = section("__la_symbol_ptr", "__DATA", lp_addr, 4 * n, 0x1000, 7, n)
    cmds = []
    cmds.append(segment("__PAGEZERO", 0, 0x1000, 0, 0, 0))
    cmds.append(segment("__TEXT", TEXT_VM, 0x1000, 0, 0x1000, 5, sects_text, 3))
    cmds.append(segment("__DATA", DATA_VM, 0x1000, 0x1000, 0x1000, 3, sects_data, 1))
    cmds.append(segment("__LINKEDIT", LINK_VM, 0x1000, 0x2000, len(link), 1))
    cmds.append(struct.pack(">IIIIII", 2, 24, symoff, n, stroff, len(strtab)))
    cmds.append(struct.pack(">II", 0xB, 80) + struct.pack(">18I", 0, 0, 0, 0, 0, n, 0, 0, 0, 0, 0, 0, indoff, 2 * n, 0, 0, 0, 0))
    state = [text_addr, 0] + [0] * 32 + [0] * 6
    cmds.append(struct.pack(">IIII", 5, 176, 1, 40) + struct.pack(">40I", *state))
    for d in dylibs:
        nm = d.encode() + b"\0"
        nm = nm + b"\0" * ((-len(nm)) % 4)
        cmds.append(struct.pack(">IIIIII", 0xC, 24 + len(nm), 24, 2, 0x10000, 0x10000) + nm)
    cmdbytes = b"".join(cmds)
    header = struct.pack(">IIIIIII", 0xFEEDFACE, 18, 0, 2, len(cmds), len(cmdbytes), 1)
    head = header + cmdbytes
    assert len(head) <= 0x600
    seg_text = head + seg_text_data[len(head):]
    data_seg = b"\0" * 0x1000
    return seg_text + data_seg + link

def fat_wrap(thin):
    junk = b"\xCE\xFA\xED\xFE" + b"\0" * 252
    hdr = struct.pack(">II", 0xCAFEBABE, 2)
    hdr += struct.pack(">IIIII", 7, 3, 0x1000, len(junk), 12)
    hdr += struct.pack(">IIIII", 18, 0, 0x2000, len(thin), 12)
    return pad(hdr, 0x1000) + pad(junk, 0x1000) + thin

# ---------------------------------------------------------------- programs
def prog_hello(a, A):
    msg = A["str:msg"]
    a.e(li(0, 4), li(3, 1), *li32(4, msg), li(5, 20), SC, NOP)       # write(1, msg, 20)
    a.e(li(0, 1), li(3, 0), SC, NOP)                                   # exit(0)

def prog_calc(a, A):
    a.e(stwu(1, -80, 1))
    a.e(li(4, 0), li(5, 1)); a.label("l1")
    a.e(add(4, 4, 5), addi(5, 5, 1), cmpwi(5, 11)); a.bc(4, 2, "l1")   # sum 1..10
    a.e(li(6, 1), li(7, 1)); a.label("l2")
    a.e(mullw(6, 6, 7), addi(7, 7, 1), cmpwi(7, 11)); a.bc(4, 2, "l2") # 10!
    a.e(mr(5, 6), li(6, 255), *li32(7, A["str:name"]), *li32(3, A["str:fmt"]))
    a.bl(A["stub:printf"])
    a.e(li(3, 7))
    a.bl(A["stub:exit"])

def prog_ops(a, A):
    a.e(stwu(1, -64, 1))
    a.e(*li32(3, 0x12345678), rlwinm(14, 3, 8, 24, 31))
    a.e(li(3, -16), srawi(15, 3, 2))
    a.e(li(3, 100), li(4, 7), divw(16, 3, 4))
    a.e(*li32(3, 0x10000000), li(4, 0x10), mulhw(17, 3, 4))
    a.e(li(3, 5), neg(18, 3))
    a.e(*li32(3, 0x10000), cntlzw(19, 3))
    a.e(li(3, 0x80), extsb(20, 3))
    a.e(li(3, 3), li(4, 10), subf(21, 3, 4))
    a.e(*li32(3, 0x11223344), stw(3, 8, 1), addi(5, 1, 8), lwbrx(22, 0, 5))
    a.e(*li32(3, A["str:fmt1"]), mr(4, 14), mr(5, 15), mr(6, 16), mr(7, 17), mr(8, 18), mr(9, 19), mr(10, 20))
    a.bl(A["stub:printf"])
    a.e(*li32(3, A["str:fmt2"]), mr(4, 21), mr(5, 22))
    a.bl(A["stub:printf"])
    a.e(li(3, 0))
    a.bl(A["stub:exit"])

PPC_EXE_HOLDER = []

def prog_cocoa(a, A):
    a.bl(A["stub:NSApplicationMain"])
    a.e(li(3, 0))
    a.bl(A["stub:exit"])

LIBSYS = "/usr/lib/libSystem.B.dylib"
COCOA = "/System/Library/Frameworks/Cocoa.framework/Versions/A/Cocoa"
FOUNDATION = "/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation"

def make_app(name, exe, binary, plist_dict, icon=True, binary_plist=False):
    import plistlib, shutil
    root = os.path.join(OUT, "apps", name + ".app")
    shutil.rmtree(root, ignore_errors=True)
    os.makedirs(os.path.join(root, "Contents", "MacOS"))
    os.makedirs(os.path.join(root, "Contents", "Resources"))
    with open(os.path.join(root, "Contents", "MacOS", exe), "wb") as f: f.write(binary)
    os.chmod(os.path.join(root, "Contents", "MacOS", exe), 0o755)
    d = dict(plist_dict); d["CFBundleExecutable"] = exe; d["CFBundlePackageType"] = "APPL"
    with open(os.path.join(root, "Contents", "Info.plist"), "wb") as f:
        f.write(plistlib.dumps(d, fmt=plistlib.FMT_BINARY if binary_plist else plistlib.FMT_XML))
    if icon:
        with open(os.path.join(root, "Contents", "Resources", "app.icns"), "wb") as f: f.write(b"icns\0\0\0\x08")


# ---------------------------------------------------------------- installer packages (.pkg)
import gzip, zlib, hashlib, shutil, random

def cpio_odc(entries):
    out = b""
    for ino, (name, mode, data) in enumerate(entries, 1):
        nm = name.encode() + b"\0"
        out += (b"070707" + b"%06o" % 0 + b"%06o" % ino + b"%06o" % mode + b"%06o" % 0 + b"%06o" % 0 +
                b"%06o" % 1 + b"%06o" % 0 + b"%011o" % 0 + b"%06o" % len(nm) + b"%011o" % len(data)) + nm + data
    nm = b"TRAILER!!!\0"
    return out + b"070707" + b"%06o" * 6 % (0, 0, 0, 0, 0, 1) + b"%06o" % 0 + b"%011o" % 0 + b"%06o" % len(nm) + b"%011o" % 0 + nm

def cpio_newc(entries):
    def hdr(ino, mode, size, nlen):
        f = [ino, mode, 0, 0, 1, 0, size, 0, 0, 0, 0, nlen, 0]
        return b"070701" + b"".join(b"%08X" % v for v in f)
    def pad(b): return b + b"\0" * ((-len(b)) % 4)
    out = b""
    for ino, (name, mode, data) in enumerate(entries, 1):
        nm = name.encode() + b"\0"
        out += pad(hdr(ino, mode, len(data), len(nm)) + nm) + pad(data)
    nm = b"TRAILER!!!\0"
    return out + pad(hdr(0, 0, 0, len(nm)) + nm)

def tree_entries(app_dir, prefix):
    """cpio entries (dirs + files) for an app folder, as installed under `prefix`."""
    ents = []
    base = os.path.dirname(app_dir)
    seen = set()
    for d, dirs, fs_ in os.walk(app_dir):
        dirs.sort()
        rel = os.path.relpath(d, base)
        ents.append((prefix + "/" + rel, 0o040755, b""))
        for f in sorted(fs_):
            with open(os.path.join(d, f), "rb") as fh:
                ents.append((prefix + "/" + os.path.join(rel, f), 0o100755, fh.read()))
    return ents

def xar(entries):
    """entries: [(path, bytes, 'raw'|'zlib')] -> flat-package (xar) bytes."""
    heap = bytearray(b"\0" * 20)
    tree = {}
    for path, data, style in entries:
        parts = path.split("/")
        t = tree
        for part in parts[:-1]:
            t = t.setdefault(part, {})
        t[parts[-1]] = (data, style)
    ids = [0]
    def esc(x): return x.replace("&", "&amp;").replace("<", "&lt;")
    def render(name, t):
        ids[0] += 1
        i = ids[0]
        if isinstance(t, dict):
            return '<file id="%d"><name>%s</name><type>directory</type>%s</file>' % (i, esc(name), "".join(render(n, c) for n, c in t.items()))
        data, style = t
        stored = zlib.compress(data) if style == "zlib" else data
        enc = "application/x-gzip" if style == "zlib" else "application/octet-stream"
        off = len(heap)
        heap.extend(stored)
        return ('<file id="%d"><name>%s</name><type>file</type><data><length>%d</length><encoding style="%s"/>'
                '<offset>%d</offset><size>%d</size></data></file>') % (i, esc(name), len(stored), enc, off, len(data))
    files = "".join(render(n, c) for n, c in tree.items())
    toc = ('<?xml version="1.0" encoding="UTF-8"?>\n<xar><toc><creation-time>2008-01-01T00:00:00</creation-time>'
           '<checksum style="sha1"><offset>0</offset><size>20</size></checksum>%s</toc></xar>' % files).encode()
    toc_c = zlib.compress(toc)
    heap[0:20] = hashlib.sha1(toc_c).digest()
    return struct.pack(">4sHHQQI", b"xar!", 28, 1, len(toc_c), len(toc), 1) + toc_c + bytes(heap)

def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f: f.write(data)

def make_pkgs(ppc_exe):
    root = os.path.join(OUT, "pkgs")
    shutil.rmtree(root, ignore_errors=True)
    apps = os.path.join(OUT, "apps")
    hello, bintool, cocoa = (os.path.join(apps, n + ".app") for n in ("Hello", "Bin Tool", "Cocoa"))

    # 1) old-style bundle package (Mac OS X 10.4): Contents/Archive.pax.gz = gzip'd cpio (odc)
    ents = [("./Applications", 0o040755, b"")] + tree_entries(hello, "./Applications")
    write(os.path.join(root, "Bundle.pkg/Contents/Archive.pax.gz"), gzip.compress(cpio_odc(ents), 9, mtime=0))
    write(os.path.join(root, "Bundle.pkg/Contents/Info.plist"), b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict/></plist>")
    write(os.path.join(root, "Bundle.pkg/Contents/Archive.bom"), b"BOMStore")

    # 2) old-style metapackage: Suite.mpkg/Contents/Packages/Inner.pkg/... (an app + a command-line tool)
    ents = ([("./Applications", 0o040755, b"")] + tree_entries(bintool, "./Applications") +
            [("./usr/local/bin", 0o040755, b""), ("./usr/local/bin/ppctool", 0o100755, ppc_exe),
             ("./usr/local/bin/notes.txt", 0o100644, b"not an executable")])
    write(os.path.join(root, "Suite.mpkg/Contents/Packages/Inner.pkg/Contents/Archive.pax.gz"), gzip.compress(cpio_odc(ents), 9, mtime=0))
    write(os.path.join(root, "Suite.mpkg/Contents/Info.plist"), b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict/></plist>")

    # 3) flat package (10.5+), single component: root-level Payload = gzip'd cpio (newc)
    ents = [("./Applications", 0o040755, b"")] + tree_entries(hello, "./Applications")
    write(os.path.join(root, "flat_simple.pkg"), xar([
        ("PackageInfo", b"<pkg-info/>", "zlib"),
        ("Bom", b"BOMStore", "raw"),
        ("Payload", gzip.compress(cpio_newc(ents), 6, mtime=0), "raw")]))

    # 4) flat product package: Distribution + Inner.pkg/Payload (uncompressed odc cpio, zlib at xar level)
    ents = [("./Applications", 0o040755, b"")] + tree_entries(cocoa, "./Applications") + tree_entries(hello, "./Applications")
    write(os.path.join(root, "flat_product.pkg"), xar([
        ("Distribution", b"<installer-gui-script/>", "zlib"),
        ("Inner.pkg/PackageInfo", b"<pkg-info/>", "zlib"),
        ("Inner.pkg/Payload", cpio_odc(ents), "zlib"),
        ("Inner.pkg/Scripts", b"not run by PowerBridge", "raw")]))

    # 5) hostile package: path traversal, absolute names, write-through-symlink attempt
    ents = [("./safe", 0o040755, b""), ("./safe/ok.txt", 0o100644, b"fine"),
            ("../escape1.txt", 0o100644, b"bad"), ("./a/../../escape2.txt", 0o100644, b"bad"),
            ("/abs_name.txt", 0o100644, b"stays inside"),
            ("./evil_abs", 0o120777, b"/etc"), ("./link", 0o120777, b"../.."),
            ("./link/pwned.txt", 0o100644, b"bad")]
    write(os.path.join(root, "evil.pkg"), xar([("Payload", gzip.compress(cpio_odc(ents), 6, mtime=0), "raw")]))

    # 6) unsupported compression
    write(os.path.join(root, "bz.pkg"), xar([("Payload", b"BZh91AY&SY" + b"\0" * 32, "raw")]))
    write(os.path.join(root, "notapkg.pkg"), b"hello, I am not an installer")

    # decompression test vectors
    rnd = random.Random(7)
    text = (" ".join(rnd.choice(["bridge", "power", "mac", "classic", "pixel", "retro", "cable", "tower", "deck", "water"]) for _ in range(40000))).encode()
    rawr = bytes(rnd.getrandbits(8) for _ in range(70000))
    mixed = b"".join(text[i:i + 500] + rawr[i:i + 300] + b"A" * (i % 700) for i in range(0, 60000, 500))
    vec = os.path.join(root, "inflate")
    for name, data, lvl in [("hello", b"hello hello hello hello", 9), ("text", text, 6), ("random_stored", rawr, 0), ("mixed", mixed, 9), ("empty", b"", 6)]:
        write(os.path.join(vec, name + ".raw"), data)
        write(os.path.join(vec, name + ".gz"), gzip.compress(data, lvl, mtime=0))
        write(os.path.join(vec, name + ".zlib"), zlib.compress(data, lvl))

def main():
    os.makedirs(OUT, exist_ok=True)
    files = {
        "hello_sc": build_macho(prog_hello, {"msg": b"Hello from PowerPC!\n"}, []),
        "calc_printf": build_macho(prog_calc, {"fmt": b"sum=%d fact=%d hex=0x%x str=%s\n", "name": b"PowerPC"}, ["printf", "exit"]),
        "ops": build_macho(prog_ops, {"fmt1": b"%d %d %d %d %d %d %d\n", "fmt2": b"%d %x\n"}, ["printf", "exit"]),
    }
    files["calc_printf_fat"] = fat_wrap(files["calc_printf"])
    strs = {"fmt": b"sum=%d fact=%d hex=0x%x str=%s\n", "name": b"PowerPC"}
    hello_app = build_macho(prog_calc, strs, ["printf", "exit"], [LIBSYS])
    make_app("Hello", "hello", hello_app, {"CFBundleName": "Hello", "CFBundleIdentifier": "test.ppcx.hello",
             "CFBundleShortVersionString": "1.2", "CFBundleIconFile": "app", "LSMinimumSystemVersion": "10.4"})
    make_app("Bin Tool", "bintool", build_macho(prog_ops, {"fmt1": b"%d %d %d %d %d %d %d\n", "fmt2": b"%d %x\n"}, ["printf", "exit"], [LIBSYS]),
             {"CFBundleName": "Bin Tool", "CFBundleIdentifier": "test.ppcx.bintool"}, icon=False, binary_plist=True)
    make_app("Cocoa", "cocoa", build_macho(prog_cocoa, {}, ["NSApplicationMain", "exit"], [LIBSYS, COCOA, FOUNDATION]),
             {"CFBundleName": "Cocoa", "CFBundleIdentifier": "test.ppcx.cocoa"}, icon=False)
    PPC_EXE_HOLDER.append(files["calc_printf"])
    make_app("Partial", "partial", build_macho(prog_calc, strs, ["printf", "exit", "mystery_fn"], [LIBSYS]),
             {"CFBundleName": "Partial", "CFBundleIdentifier": "test.ppcx.partial"}, icon=False)
    for k, v in files.items():
        with open(os.path.join(OUT, k + ".bin"), "wb") as f: f.write(v)
        print("wrote", k, len(v))
    make_pkgs(PPC_EXE_HOLDER[0])
    print("wrote pkgs")

if __name__ == "__main__":
    main()
