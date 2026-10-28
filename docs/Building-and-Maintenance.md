# Building and maintenance

Everything about working on this repository. For what the project is and what it
does, see the [README](../README.md).

## Building

Niri needs a Rust toolchain (1.87 or newer), the usual Wayland/DRM/pango
dependencies, and — for screencasting — the `libpipewire` headers plus
`libclang` for bindgen.

On Arch Linux the system libraries are usually already installed; the build
tools you may be missing are `rust` (or rustup) and `clang`:

```sh
sudo pacman -S --needed rust clang
```

bindgen does not always search `/usr/lib` for libclang, so point it there:

```sh
LIBCLANG_PATH=/usr/lib cargo build --release
```

Install:

```sh
sudo install -Dm755 target/release/niri /usr/local/bin/niri
```

`niri-session` resolves `niri` through `PATH`, so a binary in `/usr/local/bin`
wins over a distribution package in `/usr/bin` and survives package upgrades.
If you would rather replace the packaged binary (keeping a backup):

```sh
sudo cp -a /usr/bin/niri /usr/bin/niri.bak
sudo install -Dm755 target/release/niri /usr/bin/niri
```

The version string carries the `-elite` suffix (`niri 26.04-elite (...)`), so a
running fork build can never be mistaken for mainline niri.

## Installing from the AUR

The package files live in [`packaging/aur`](../packaging/aur) (`niri-elite-git`).
It builds from this repository's `main` branch and installs the same `niri`
binary and session files as the `niri` package, so it conflicts with `niri` and
`niri-git` and provides `niri` for anything that depends on it.

```sh
paru -S niri-elite-git     # or: yay -S niri-elite-git
```

To publish an update, copy `PKGBUILD` and the regenerated `.SRCINFO` into the
AUR repository and push:

```sh
cd packaging/aur
makepkg --printsrcinfo > .SRCINFO
git clone ssh://aur@aur.archlinux.org/niri-elite-git.git
cp PKGBUILD .SRCINFO niri-elite-git/ && cd niri-elite-git
git add PKGBUILD .SRCINFO && git commit -m "upgpkg: update to $(grep ^pkgver= PKGBUILD | cut -d= -f2)" && git push
```

## Syncing upstreams

```sh
git remote add upstream https://github.com/niri-wm/niri.git
git remote add shorin   https://github.com/SHORiN-KiWATA/niri.git
git remote add urayde   https://github.com/urayde/niri.git
git fetch upstream main
git fetch shorin main
git fetch urayde main
git merge upstream/main     # mainline first: it is the base for everything else
git merge shorin/main       # grid overview, magnifier, agent seat, portal
git merge urayde/main       # tearing, drm-syncobj, fifo, pointer warp
```

What to expect while merging:

- **binds and input** are the most volatile area. Upstream keeps reworking key
  handling while SHORiN's input layer lags behind it: take upstream's structure
  and re-express SHORiN's additions on top of it.
- **grid overview** comes from SHORiN — prefer their implementation over local
  patches.
- **screencasting and the portal backend** are fork-only; neither upstream has
  them, so those files resolve to this fork's side.
- **the default config** is a common conflict source: both upstreams edit it, and
  a duplicated bind makes `can_create_default_config` fail.
  `cargo test -p niri-config` catches that.

After every merge:

```sh
LIBCLANG_PATH=/usr/lib cargo test --workspace
```

## Smithay

`urayde/niri`'s protocol work needs DRM async flips from a smithay fork, while
the agent seat needs SHORiN's per-client seat filter, which only exists in
SHORiN's fork. Instead of stacking `[patch]` redirects, this build pins
[`MEKCCK/smithay`](https://github.com/MEKCCK/smithay): urayde's `6df9a643` with
SHORiN's single `seat: allow filtering which clients see a seat global` commit
on top.

When either side moves, rebase that fork and bump the two `rev` lines in
`Cargo.toml` (`[workspace.dependencies.smithay]` and `smithay-drm-extras`).

`zaroutt/Niri-glass` shares no history with this repository and cannot be
merged either: port `src/render_helpers/liquid_glass.rs`,
`src/render_helpers/shaders/clipped_surface.frag` and the `lg_*` uniforms in
`src/render_helpers/shaders/mod.rs` by hand when it changes.
