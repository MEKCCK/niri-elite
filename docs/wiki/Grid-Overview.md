### Grid Overview

The Grid Overview is a grid-based view of your windows.
It shows every window as a tile in a grid, letting you quickly navigate between them.

Open it with the `toggle-grid-overview` bind, or by tapping the `Mod` key (pressing and releasing it without pressing any other key in between). The `Mod` key tap behavior is built in and works even without an explicit binding, controlled by the `default-mod-action` setting (enabled by default).

While in the grid overview, all keyboard shortcuts keep working.
Use the arrow keys to navigate between windows.
If a column contains multiple windows, use Up/Down to switch between them within the same cell.

Click a window to activate it and close the grid overview.
Focusing a window from elsewhere, like a taskbar or `niri msg action focus-window`, does the same.

### Minimized windows

The grid overview is where [minimized windows](./Configuration:-Key-Bindings.md#minimize-window) live: a minimized window keeps its place in the layout and only shows up here, as a cell marked with a colored frame.
Activating that cell restores the window, and so does closing the grid while that cell is selected.

Minimized cells count as normal cells for navigation *and* for moving:
moving a window with `move-column-left`/`move-column-right` steps onto the minimized cell instead of jumping over it, and moving a minimized cell itself reorders it without restoring it.
Outside the grid overview minimized windows are invisible, so moves there jump over them as before.

Pressing [`minimize-window`](./Configuration:-Key-Bindings.md#minimize-window) on a minimized cell restores the window in place: it goes back into the layout, loses its frame, and the grid stays open on it.
Minimizing works the same way round, so the grid focus stays on the cell you acted on either way, and the two are each other's undo.
Activating the cell (`Return`, or a click) restores it too, but also focuses it and closes the grid.

### Configuration

See the full documentation for the `grid-overview` section [here](./Configuration:-Miscellaneous.md#grid-overview).

You can set the gap between cells like this:

```kdl
grid-overview {
    gap 16
}
```

You can set the padding around the grid with one value for every side, or with separate values for each side:

```kdl
grid-overview {
    padding 100
}
```

```kdl
grid-overview {
    padding {
        left 64
        right 64
        top 48
        bottom 48
    }
}
```

To change the focused column scale, use the `focused-column-scale` setting:

```kdl
grid-overview {
    focused-column-scale 1.08
}
```

To change how minimized cells are marked, use the `minimized-highlight` setting:

```kdl
grid-overview {
    minimized-highlight {
        color "#999999b3"
        padding 6
        corner-radius 0
    }
}
```
