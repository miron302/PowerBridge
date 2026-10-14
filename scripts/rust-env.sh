# Sourced by scripts/bundle.sh. Makes sure the Rust compiler that will actually be used can build for the
# Mac architectures we need (Apple silicon and Intel).

# Use rustup's tools first. A Rust installed with Homebrew only includes the standard library for the Mac it was
# installed on, so it cannot build for both Apple silicon and Intel (error: "can't find crate for `std`").
if [ -x "$HOME/.cargo/bin/rustup" ]; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi

ensure_rust_target() {
  _t="$1"
  if command -v rustup >/dev/null 2>&1; then
    rustup target add "$_t" >/dev/null 2>&1 || true
  fi
  # Check against the compiler cargo will really run, not just what rustup says it installed.
  _sysroot="$(rustc --print sysroot 2>/dev/null)"
  if [ -z "$_sysroot" ] || [ ! -d "$_sysroot/lib/rustlib/$_t/lib" ]; then
    {
      echo "error: the Rust compiler in use ($(command -v rustc || echo 'none found')) cannot build for $_t."
      echo
      echo "This usually means Rust was installed with Homebrew and is being used instead of the rustup version."
      echo "Fix (pick one):"
      echo "  1. Install Rust with rustup (README, Step 2), then open a NEW Terminal window and try again."
      echo "  2. If you already use rustup:   export PATH=\"\$HOME/.cargo/bin:\$PATH\"   and run this script again."
      echo "  3. Remove the Homebrew copy:    brew uninstall rust   (then open a NEW Terminal window)"
      echo
      echo "To see which Rust is being used:  which -a cargo rustc"
    } >&2
    return 1
  fi
}
