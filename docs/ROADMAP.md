# Roadmap: heavier PowerPC apps (Microsoft Word, games, …)

**Short version:** PowerBridge 1.0.1 runs programs that only use basic system calls and a subset of the C library. It
does **not** run apps like Microsoft Office or most PowerPC games, and no small update can change that. This page
explains why and what a real path would take.

## Why Word and games don't run

1. **They are built on Apple's PowerPC frameworks.** Word (Office 2004/v.X) and almost every Mac game use Carbon and/or
   Cocoa, plus QuickTime, OpenGL, Sound Manager, ATSUI text, and so on. Those PowerPC libraries are not part of
   PowerBridge, and today's macOS contains no PowerPC copies. A PowerPC app can't start without them.
2. **They talk to the window system.** Windows, menus, events, fonts, the clipboard and the screen all go through
   operating-system services (Mach IPC). None of that exists for a PowerPC program inside PowerBridge.
3. **Speed.** The interpreter runs roughly 100 million PowerPC instructions per second (measured in a Linux VM; a modern
   Mac should be faster). A PowerPC G4/G5 does around ten times that. Productivity apps could cope; real-time games
   would not, without a JIT compiler.
4. **CPU features.** Many games and Photoshop-class apps use AltiVec (vector) instructions, which are not implemented.

(Classic Mac OS 9 apps are a different file format entirely and are out of scope.)

## What 1.0.1 *did* improve for heavier command-line programs

- Real `fopen`/`fread`/`fgets`/`fseek`/… file I/O, `strtol`/`strtod`, `strstr`, case-insensitive compares
- The math library (`sqrt`, `sin`, `pow`, …) with proper floating-point results
- Recognition of the decorated symbol names real Leopard-era binaries use (`_fwrite$UNIX2003`, `_printf$LDBL128`, …)
- Universal build (Apple silicon + Intel)

## Realistic paths forward (each is large)

| Path | What it means | Rough scale |
| --- | --- | --- |
| **A. Run a real PowerPC system** | The user supplies PowerPC system files (e.g. from a Mac OS X 10.4/10.5 install they own). PowerBridge runs Apple's own dyld and frameworks inside the emulator and emulates the kernel/Mach services, WindowServer and CoreGraphics, bridging output to a real window. This is the approach maclator uses for arm64. | Many months; the graphics/IPC layer is the hard part |
| **B. Rosetta-style bridging** | Translate only the app's PowerPC code and forward its framework calls to the Mac's native frameworks. | Not practical for 32-bit Carbon on today's 64-bit-only macOS |
| **C. One target at a time** | Pick a specific game or app and emulate exactly the handful of frameworks it uses (e.g. SDL + OpenGL for a particular port). | Weeks per target, but achievable and testable |

Whichever path is chosen, these supporting pieces are needed: a JIT or much faster interpreter, AltiVec, threads,
signals, and a lot more of the C library and Mach interfaces.

**Recommendation:** pick one specific app or game you care about (name and version) and attempt path C for that title.
That gives a concrete, testable goal instead of an open-ended "support everything".
