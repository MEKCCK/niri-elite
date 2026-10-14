### Grid Overview

The Grid Overview is a grid-based view of your windows.
It shows every window as a tile in a grid, letting you quickly navigate between them.

Open it with the `toggle-grid-overview` bind, or by tapping the `Mod` key (pressing and releasing it without pressing any other key in between). The `Mod` key tap behavior is built in and works even without an explicit binding, controlled by the `default-mod-action` setting (enabled by default).

While in the grid overview, all keyboard shortcuts keep working, and the focus actions (like `focus-column-left` or `focus-window-down`) move between cells.
On top of that, <kbd>H</kbd>/<kbd>J</kbd>/<kbd>K</kbd>/<kbd>L</kbd> without modifiers navigate between windows; bare arrow keys are not built in, they still go to the window.
If a column contains multiple windows, use <kbd>J</kbd>/<kbd>K</kbd> to switch between them within the same cell.
The tabs of a tabbed column get a cell each; a small tab indicator along the left edge of each of those cells, in the band where the focus ring goes, shows that they belong to one column and which tab each one is. On a focused cell it sits in a slot cut into the ring, so that the lit tab stands out from the ring. It follows the `tab-indicator` settings.
<kbd>Return</kbd> activates the selected window, and <kbd>Escape</kbd> closes the grid.

Click a window to activate it and close the grid overview.
Focusing a window from elsewhere, like a taskbar or `niri msg action focus-window`, does the same.

With `focus-follows-mouse` in the `grid-overview` section, moving the mouse also moves the focus to the cell under the pointer, so closing the grid activates that window.
The pointer only takes the focus over once it moved a few pixels (`min-distance`, 8 by default) from where it was when the grid opened or the keyboard last moved the focus, so nudging the mouse while navigating with the keyboard keeps the keyboard's choice.
Cells moving under a still pointer, for example as the grid opens, don't take the focus either.

```kdl
grid-overview {
    focus-follows-mouse min-distance=8
}
```

### Minimized windows

The grid overview is where [minimized windows](./Configuration:-Key-Bindings.md#minimize-window) live: a minimized window keeps its place in the layout and only shows up here, as a darkened cell marked with a colored frame.
Activating that cell restores the window, and so does closing the grid while that cell is selected.

Minimized cells count as normal cells for navigation *and* for moving:
moving a window with `move-column-left`/`move-column-right` steps onto the minimized cell instead of jumping over it, and moving a minimized cell itself reorders it without restoring it.
Outside the grid overview minimized windows are invisible, so moves there jump over them as before.

Pressing [`minimize-window`](./Configuration:-Key-Bindings.md#minimize-window) on a minimized cell restores the window in place: it goes back into the layout, loses its frame and its dark layer, and the grid stays open on it.
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

The focused cell is marked by the focus ring of its window.
In the grid, the ring keeps its on-screen width instead of shrinking with the cell, so it stays easy to see however many windows the grid shows.
To make it thicker in the grid, use the `focus-ring-width` setting:

```kdl
grid-overview {
    focus-ring-width 8
}
```

When the focus moves, the new cell's ring grows in while the old one shrinks back.
`focus-ring-transition` picks another way, `"slide"` or `"none"`, and the `focus-ring` animation sets the timing:

```kdl
grid-overview {
    focus-ring-transition "slide"
}
```

The focused column can also be scaled up with the `focused-column-scale` setting.
This is off by default, since a larger cell no longer shows the window's size next to the others:

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
