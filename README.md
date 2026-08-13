<h1 align="center"><img alt="niri" src="https://github.com/user-attachments/assets/07d05cd0-d5dc-4a28-9a35-51bae8f119a0"></h1>
<p align="center">A scrollable-tiling Wayland compositor.</p>
<p align="center">
    <a href="https://matrix.to/#/#niri:matrix.org"><img alt="Matrix" src="https://img.shields.io/badge/matrix-%23niri-blue?logo=matrix"></a>
    <a href="https://github.com/niri-wm/niri/blob/main/LICENSE"><img alt="GitHub License" src="https://img.shields.io/github/license/niri-wm/niri"></a>
    <a href="https://github.com/niri-wm/niri/releases"><img alt="GitHub Release" src="https://img.shields.io/github/v/release/niri-wm/niri?logo=github"></a>
</p>

<p align="center">
    <a href="https://niri-wm.github.io/niri/Getting-Started.html">Getting Started</a> | <a href="https://niri-wm.github.io/niri/Configuration%3A-Introduction.html">Configuration</a> | <a href="https://github.com/niri-wm/niri/discussions/325">Setup&nbsp;Showcase</a>
</p>

<img width="1280" height="720" alt="niri with a few windows open" src="https://github.com/user-attachments/assets/dea5909e-1859-4aaa-9d88-d37f9663e00b" />

## About

**niri-elite** is a fork of [niri](https://github.com/niri-wm/niri) that tracks upstream closely while adding extra features. Everything in mainline niri works here; on top of that, this branch adds:

| Feature | Description | Toggle / Config |
|---|---|---|
| **Grid overview** | An overview of all windows on the workspace in a grid, with keyboard navigation, tabs/columns/floating items, and per-monitor support. | `Mod+G` (`toggle-grid-overview`) — see [`grid-overview`](docs/wiki/Grid-Overview.md) |
| **Fullscreen magnifier** | Magnify any part of the screen, zoom with the scroll wheel. | `Mod+Shift+Z` (`toggle-magnifier`) |
| **Shake-to-enlarge** | Shake a window to enlarge it. | [`shake-to-enlarge`](resources/default-config.kdl) |
| **KDE blur protocol** | Implements the KDE blur protocol (`kde-blur`) for background blur on supported surfaces. | Automatic |
| **SVG cursor support** | Render SVG cursor themes (e.g. Bibata) natively. | Automatic |
| **Built-in xdg-desktop-portal backend** | ScreenCast/Screenshot portal interfaces are served by niri itself (`org.freedesktop.impl.portal.desktop.niri`) — no `xdg-desktop-portal-gnome` needed. | Automatic; ships `niri.portal` + `niri-portals.conf` |
| **Native screen cast picker** | A built-in picker UI for choosing what to screen-cast, with persisted permission tokens and configurable colors. | See [Screencasting](docs/wiki/Screencasting.md) |
| **SHM fallback for screencasts** | PipeWire screencasts fall back to SHM (memfd) when GPU is unavailable — works in VMs. | Automatic |
| **Software EGL renderer** | Software rendering support for VMs and GPUs without hardware acceleration. | Automatic |
| **Screenshot to file** | Save screenshots directly to a path via IPC, with async reply. | `niri msg action screenshot` |
| **Multi-GPU fixes** | Improved handling of multi-GPU setups (primary render node selection, display-only devices). | Automatic |

Mainline niri keeps a conservative stance on some of these (e.g. no grid overview, no built-in portal backend), so they are maintained here instead. Upstream changes are merged in regularly.

## Features

- Built from the ground up for scrollable tiling
- [Dynamic workspaces](https://niri-wm.github.io/niri/Workspaces.html) like in GNOME
- An [Overview](https://github.com/user-attachments/assets/379a5d1f-acdb-4c11-b36c-e85fd91f0995) that zooms out workspaces and windows
- Built-in screenshot UI
- Monitor and window screencasting through a built-in xdg-desktop-portal backend with a native picker
    - You can [block out](https://niri-wm.github.io/niri/Configuration%3A-Window-Rules.html#block-out-from) sensitive windows from screencasts
    - [Dynamic cast target](https://niri-wm.github.io/niri/Screencasting.html#dynamic-screencast-target) that can change what it shows on the go
- [Touchpad](https://github.com/niri-wm/niri/assets/1794388/946a910e-9bec-4cd1-a923-4a9421707515) and [mouse](https://github.com/niri-wm/niri/assets/1794388/8464e65d-4bf2-44fa-8c8e-5883355bd000) gestures
- Group windows into [tabs](https://niri-wm.github.io/niri/Tabs.html)
- Configurable layout: gaps, borders, struts, window sizes
- [Gradient borders](https://niri-wm.github.io/niri/Configuration%3A-Layout.html#gradients) with Oklab and Oklch support
- [Background blur](https://niri-wm.github.io/niri/Window-Effects.html) for windows and layer-shell surfaces
- [Animations](https://github.com/niri-wm/niri/assets/1794388/ce178da2-af9e-4c51-876f-8709c241d95e) with support for [custom shaders](https://github.com/niri-wm/niri/assets/1794388/27a238d6-0a22-4692-b794-30dc7a626fad)
- Live-reloading config
- Works with [screen readers](https://niri-wm.github.io/niri/Accessibility.html)

## Video Demo

https://github.com/niri-wm/niri/assets/1794388/bce834b0-f205-434e-a027-b373495f9729

Also check out these videos that showcase a lot of the niri functionality:

- [Niri Is My New Favorite Wayland Compositor](https://www.youtube.com/watch?v=DeYx2exm04M) by Brodie Robertson
- [How Is niri This Good? Live Demo + Config](https://www.youtube.com/watch?v=7XmD5UyyhZQ) by Nick Janetakis

## Status

Niri is stable for day-to-day use and does most things expected of a Wayland compositor.
Many people are daily-driving niri, and are happy to help in our [Matrix channel].

Give it a try!
Follow the instructions on the [Getting Started](https://niri-wm.github.io/niri/Getting-Started.html) page.
Grab a desktop shell like [DankMaterialShell] or [Noctalia] (or build a more traditional setup): niri by itself is not a complete desktop environment.
Also check out [awesome-niri], a list of niri-related links and projects.

Here are some points you may have questions about:

- **Multi-monitor**: yes, a core part of the design from the very start. Mixed DPI works.
- **Fractional scaling**: yes, plus all niri UI stays pixel-perfect.
- **NVIDIA**: seems to work fine.
- **Floating windows**: yes, starting from niri 25.01.
- **Input devices**: niri supports tablets, touchpads, and touchscreens.
You can map the tablet to a specific monitor, or use [OpenTabletDriver].
We have touchpad gestures, but no touchscreen gestures yet.
- **Wlr protocols**: yes, we have most of the important ones like layer-shell, gamma-control, screencopy.
You can check on [wayland.app](https://wayland.app) at the bottom of each protocol's page.
- **Performance**: while I run niri on beefy machines, I try to stay conscious of performance.
I've seen someone use it fine on an Eee PC 900 from 2008, of all things.
- **Xwayland**: [integrated](https://niri-wm.github.io/niri/Xwayland.html#using-xwayland-satellite) via xwayland-satellite starting from niri 25.08.

## Media

[niri: Making a Wayland compositor in Rust](https://youtu.be/Kmz8ODolnDg?list=PLRdS-n5seLRqrmWDQY4KDqtRMfIwU0U3T) · *December 2024*

My talk from the 2024 Moscow RustCon about niri, and how I do randomized property testing and profiling, and measure input latency.
The talk is in Russian, but I prepared full English subtitles that you can find in YouTube's subtitle language selector.

[An interview with Ivan, the developer behind Niri](https://www.trommelspeicher.de/podcast/special_the_developer_behind_niri) · *June 2025*

An interview by a German tech podcast Das Triumvirat (in English).
We talk about niri development and history, and my experience building and maintaining niri.

[A tour of the niri scrolling-tiling Wayland compositor](https://lwn.net/Articles/1025866/) · *July 2025*

An LWN article with a nice overview and introduction to niri.

## Contributing

If you'd like to help with niri, there are plenty of both coding- and non-coding-related ways to do so.
See [CONTRIBUTING.md](https://github.com/niri-wm/niri/blob/main/CONTRIBUTING.md) for an overview.

## Inspiration

Niri is heavily inspired by [PaperWM] which implements scrollable tiling on top of GNOME Shell.

One of the reasons that prompted me to try writing my own compositor is being able to properly separate the monitors.
Being a GNOME Shell extension, PaperWM has to work against Shell's global window coordinate space to prevent windows from overflowing.

## Tile Scrollably Elsewhere

Here are some other projects which implement a similar workflow:

- [PaperWM]: scrollable tiling on top of GNOME Shell.
- [karousel]: scrollable tiling on top of KDE.
- [scroll](https://github.com/dawsers/scroll) and [papersway]: scrollable tiling on top of sway/i3.
- Hyprland has a built-in [scrolling layout](https://wiki.hypr.land/Configuring/Layouts/Scrolling-Layout/).
- [Paneru] and [PaperWM.spoon]: scrollable tiling on top of macOS.

## Contact

Our main communication channel is a Matrix chat, feel free to join and ask a question: https://matrix.to/#/#niri:matrix.org

We also have a community Discord server: https://discord.gg/vT8Sfjy7sx

[PaperWM]: https://github.com/paperwm/PaperWM
[waybar]: https://github.com/Alexays/Waybar
[fuzzel]: https://codeberg.org/dnkl/fuzzel
[awesome-niri]: https://github.com/niri-wm/awesome-niri
[karousel]: https://github.com/peterfajdiga/karousel
[papersway]: https://spwhitton.name/tech/code/papersway/
[Paneru]: https://github.com/karinushka/paneru
[PaperWM.spoon]: https://github.com/mogenson/PaperWM.spoon
[Matrix channel]: https://matrix.to/#/#niri:matrix.org
[OpenTabletDriver]: https://opentabletdriver.net/
[DankMaterialShell]: https://danklinux.com/
[Noctalia]: https://noctalia.dev/
