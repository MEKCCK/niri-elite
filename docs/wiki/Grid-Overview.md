### Grid Overview

The Grid Overview is a grid-based view of your windows.
It shows every window as a tile in a grid, letting you quickly navigate between them.

Open it with the `toggle-grid-overview` bind, or by tapping the `Mod` key (pressing and releasing it without pressing any other key in between). The `Mod` key tap behavior is built in and works even without an explicit binding, controlled by the `default-mod-action` setting (enabled by default).

While in the grid overview, all keyboard shortcuts keep working, and the focus actions (like `focus-column-left` or `focus-window-down`) move between cells.
On top of that, <kbd>H</kbd>/<kbd>J</kbd>/<kbd>K</kbd>/<kbd>L</kbd> without modifiers navigate between windows; bare arrow keys are not built in, they still go to the window.
Navigation goes by where the cells are on screen: to the next cell in that direction, preferring one that lines up with the current cell, so it reaches floating windows as well as tiled ones.
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

### Layout

The grid shows windows at one scale, the largest at which they all fit, so windows keep their sizes relative to each other: a small dialog stays smaller than a big editor.

Tiled windows form a block in the middle of the grid, in the order of the scrolling layout, left to right, wrapped into rows like text.
The tabs of a tabbed column stay together in one row.
How wide the rows get is picked to show the windows as large as possible.

Floating windows start out where they are on the screen and push each other and the block of tiled windows apart, so they end up around the block, on the side of the screen they are on.
A floating window that a gap in the block, like beside a short row, has room for closer to where it is than where it got pushed to takes the gap.
On a workspace with only floating windows, the grid works like the window overviews of other desktops.

When windows come and go, the grid keeps the width the rows wrap at as long as it shows the windows at least 92% as large as another one would, and keeps its scale unless the windows could grow by more than about 7%.
Opening the grid again starts from what it showed when it closed, the same way, so with nothing changed it shows the same.

Dragging a tiled window in the grid moves it in the scrolling layout: drop it on a tiled cell to put it into that column, at the edge of one to put it beside it, or anywhere else, floating cells included, to put it beside the nearest tiled cell.
Dragging a floating window moves it on the screen: its middle goes where you let go of it, and the grid shows it there, or in the closest free spot, moving it on from where you let go.
As outside the grid, clicking the right mouse button during the drag switches the window between floating and tiling.
[`toggle-window-floating`](./Configuration:-Key-Bindings.md) switches the window in the focused cell.

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
