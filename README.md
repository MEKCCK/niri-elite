<h1 align="center">niri-elite</h1>
<p align="center">A scrollable-tiling Wayland compositor — a feature-packed fork of <a href="https://github.com/niri-wm/niri">niri</a>.</p>
<p align="center">
    <a href="https://github.com/MEKCCK/niri-elite/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/github/license/MEKCCK/niri-elite"></a>
    <a href="https://github.com/niri-wm/niri"><img alt="Upstream" src="https://img.shields.io/badge/upstream-niri--wm%2Fniri-blue"></a>
    <a href="https://github.com/SHORiN-KiWATA/niri"><img alt="Upstream" src="https://img.shields.io/badge/upstream-SHORiN--KiWATA%2Fniri-blue"></a>
    <a href="https://github.com/urayde/niri"><img alt="Upstream" src="https://img.shields.io/badge/upstream-urayde%2Fniri-blue"></a>
    <a href="https://github.com/zaroutt/Niri-glass"><img alt="Glass" src="https://img.shields.io/badge/glass-zaroutt%2FNiri--glass-purple"></a>
</p>

<p align="center">
    <a href="https://niri-wm.github.io/niri/Getting-Started.html">Getting Started (upstream)</a> |
    <a href="https://niri-wm.github.io/niri/Configuration%3A-Introduction.html">Configuration (upstream)</a> |
    <a href="docs/wiki/Grid-Overview.md">Grid Overview</a> |
    <a href="docs/wiki/Screencasting.md">Screencasting</a>
</p>

## What this is

`niri-elite` is my personal build of niri — an experimental playground rather
than a project with a roadmap. It tracks mainline niri closely, and on top of
that it collects whatever I find worth living with: features from other people's
forks, work that has not landed upstream (yet, or ever), and a few things I
ported by hand from projects that share no git history with this one. Upstream
behavior is always the default; anything added here is a layer you can ignore,
disable or delete.

None of this is upstream policy, and nothing here is a promise that mainline
niri will ever want it. It is a working daily driver first and a showcase
second.

The extra code comes from these sources:

| Upstream | What is taken from it |
|---|---|
| [`niri-wm/niri`](https://github.com/niri-wm/niri) | The compositor itself. Merged regularly; all upstream behavior and fixes are kept. |
| [`SHORiN-KiWATA/niri`](https://github.com/SHORiN-KiWATA/niri) | Grid overview, fullscreen magnifier, window minimization, the agent seat, and most of the grid/input polish built on top of them. |
| [`urayde/niri`](https://github.com/urayde/niri) | Protocol work that is not upstream: tearing control, `linux-drm-syncobj-v1`, `wp-fifo-v1`, pointer warp, and the DRM async-flip support they need. |
| [`zaroutt/Niri-glass`](https://github.com/zaroutt/Niri-glass) | The liquid-glass background effect. That repository shares no git history with this one, so its code is ported by hand. |

Mainline niri deliberately stays conservative on several of these (no grid
overview, no built-in portal backend, no minimization), so they are maintained
here instead. Building, packaging and the upstream-sync workflow live in
[`docs/Building-and-Maintenance.md`](docs/Building-and-Maintenance.md).

## What it adds over mainline niri

### Windows and layout

| Feature | Notes |
|---|---|
| **Grid overview** | Every window of the workspace laid out in a grid, with <kbd>H</kbd>/<kbd>J</kbd>/<kbd>K</kbd>/<kbd>L</kbd> and the regular focus actions moving between cells, tabbed columns as separate cells, per-monitor support and configurable gap/padding/scale. `Mod+G`, or a bare `Mod` tap. See [`Grid-Overview`](docs/wiki/Grid-Overview.md). |
| **Window minimization** | A real minimized state: `minimize-window`, `unminimize-window`, `toggle-window-minimized` (and the `-by-id` variants), the `open-minimized` window rule, a placeholder cell in the grid and fade animations. |
| **Hot-corner actions** | Any corner can run `overview` or `grid-overview`, globally or per output: `gestures { hot-corners { bottom-left { grid-overview; } } }`. |
| **Grid-aware focus and moves** | Focus and move actions act on the focused cell while the grid is open, and workspace moves animate through the grid. |

### Input and binds

| Feature | Notes |
|---|---|
| **Press and release actions** | `key { press { ...; } release { ...; } }`, with the guarantee that a press action which ran also runs its release. |
| **Bare modifier triggers** | Bind `Mod`, `Super+Shift`, `Alt_L` and friends directly as bind triggers. |
| **Raw and ASCII keysym fallbacks** | Binds keep working on non-ASCII layouts: the raw keysym of the current layout and the keysym of an ASCII layout are both tried. |
| **`default-mod-action`** | Tapping the mod key alone toggles the grid overview, unless a bind on the mod key exists. Disable with `grid-overview { default-mod-action false; }`. |
| **Magnifier movement and binds** | `move-magnifier-left/right/up/down`, `magnifier.move-step`, and a `magnifier { binds { ... } }` section whose binds only take over while the magnifier is on. |
| **`confine-mouse-to-fullscreen`** | Keep the pointer inside a fullscreen window. |

### Rendering and effects

| Feature | Notes |
|---|---|
| **Liquid glass** | Frosted, refractive background effect with lens distortion, glow edges, fringing, dilution, adaptive dim/boost and edge thickness controls — `background-effect { liquid-glass { ... } }`. |
| **KDE blur protocol** | Implements `org_kde_kwin_blur` for background blur on supporting surfaces. |
| **SVG cursors** | SVG cursor themes (Bibata and friends) rendered natively. |
| **Shake-to-enlarge cursor** | Shake the mouse to enlarge the cursor — zoom factor, threshold, hold duration and optional gradual growth via `cursor { shake-to-enlarge { ... } }`. |
| **Software EGL, multi-GPU** | Software rendering for VMs, better primary render-node selection and display-only GPU handling. |

### Protocols and presentation

| Feature | Notes |
|---|---|
| **Tearing control** | Implements `wp-tearing-control-v1`, so games and video players can ask for immediate presentation. Tearing only happens where a window rule allows it: `allow-tearing true`/`false` per window, or `allow-tearing-content-type "game" "video"` to let windows that declare their content tear without being named individually. `debug { force-tearing; }` tears regardless of the rules. |
| **Content type** | Implements `wp_content_type_v1`, so players and games can tell the compositor whether they show game, video or photo content; `allow-tearing-content-type` consumes it. |
| **Explicit sync** | Implements `linux-drm-syncobj-v1`, so clients can attach DRM syncobj timelines to their buffers instead of relying on implicit fences. |
| **`wp-fifo-v1`** | FIFO-style buffer release, so a client that falls behind is throttled instead of queueing frames. |
| **Pointer warp** | Implements `wp_pointer_warp_v1`, letting pointer-lock clients warp the cursor without hiding and re-showing it. |
| **Async page flips** | DRM async flips in the TTY backend, including per-plane async format probing and a fallback to a blocking commit when the driver cannot do it. |

These come from [`urayde/niri`](https://github.com/urayde/niri) and need smithay
work that is not upstream, so this build uses
[`MEKCCK/smithay`](https://github.com/MEKCCK/smithay) — urayde's async flip and
tearing branches with the per-client seat filter that the agent seat needs
folded in.

### Screencasting and screenshots

| Feature | Notes |
|---|---|
| **Built-in xdg-desktop-portal backend** | ScreenCast and Screenshot are served by niri itself (`org.freedesktop.impl.portal.desktop.niri`); `xdg-desktop-portal-gnome` is not needed. Ships `niri.portal` and `niri-portals.conf`. |
| **Native screen cast picker** | Built-in picker for choosing outputs or windows to cast, with persisted permission tokens and configurable colors. |
| **SHM fallback** | PipeWire screencasts fall back to shared memory when dmabuf is unavailable, so casting works in VMs and without a usable GPU device. |
| **Screenshot to a path** | `niri msg action screenshot --path ...` writes the PNG and replies with the path over IPC. |
| **Pick-color UI** | Freeze the screen, show a magnifier, pick a pixel color. |

### Automation

| Feature | Notes |
|---|---|
| **Agent seat** (cargo feature `agent`, on by default) | A second input seat for automation over IPC, so scripts can send keys and clicks without stealing the user's seat. Configure with `agent { off; hide-seat-from "app-id"; }`. |

## Keys you get out of the box

The bundled [default config](resources/default-config.kdl) binds, among others:

```kdl
Mod+O                  { toggle-overview; }
Mod                    { release { toggle-grid-overview; } }   // tap Mod for the grid
Mod+Shift+Z            { toggle-magnifier; }
Mod+N                  { minimize-window; }
Mod+Alt+WheelScrollUp   { adjust-magnifier-zoom 0.2; }
Mod+Alt+WheelScrollDown { adjust-magnifier-zoom -0.2; }
```

Delete the bare `Mod` bind if you would rather let the built-in
`default-mod-action` handle the tap.

## Documentation

Configuration lives in `~/.config/niri/config.kdl`, with the usual `include`
support. The wiki under [`docs/wiki`](docs/wiki) is upstream's documentation plus
fork pages such as [`Grid-Overview`](docs/wiki/Grid-Overview.md) and
[`Screencasting`](docs/wiki/Screencasting.md); upstream docs apply unless a fork
page says otherwise.

## Credits

- [niri](https://github.com/niri-wm/niri) by Ivan Molodetskikh and contributors —
  the compositor, and the vast majority of this code.
- [SHORiN-KiWATA/niri](https://github.com/SHORiN-KiWATA/niri) — grid overview,
  magnifier, minimization, agent seat.
- [urayde/niri](https://github.com/urayde/niri) — tearing control, explicit sync,
  `wp-fifo-v1`, pointer warp and the async-flip support behind them, plus the
  smithay fork they build on.
- [zaroutt/Niri-glass](https://github.com/zaroutt/Niri-glass) — the liquid-glass
  effect and its shaders.
- [PaperWM](https://github.com/paperwm/PaperWM) — the scrollable-tiling idea niri
  itself was inspired by.

## License

GPL-3.0-or-later, same as niri. See [LICENSE](LICENSE).
