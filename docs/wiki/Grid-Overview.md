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
Floating windows smaller than the largest window are spared some of that shrinking, so that a small chat window doesn't end up too small to recognize next to big tiled windows.

Tiled windows form a block in the order of the scrolling layout, left to right, wrapped into rows like text.
The tabs of a tabbed column stay together in one row.
How wide the rows get is picked to show the windows as large as possible.

Floating windows each take the free spot closest to where they are on the screen, and keep to their side of it: a window in the top right corner shows in the top right of the grid, one along the top in the top half, and one near the middle of the screen anywhere.
For that, the grid rather shows everything smaller; only if keeping to their sides would show the windows less than 60% as large as they could be does a window in a corner keep to just the side it is further to, and failing that, any side.
That spot can be in the room that a short row of tiled windows leaves beside it.
When that shows everything larger, the block of tiled windows moves to one side of the grid, leaving the floating windows one large stretch of room instead of two narrow ones.
Among layouts that show the windows about as large, the grid prefers the block in the middle, then rows of even width over a last row with a single window, then the layout that keeps floating windows closest to where they are.
On a workspace with only floating windows, the grid works like the window overviews of other desktops.

While the grid stays open, it keeps what it can when windows come and go:

- Floating windows stay where the grid shows them. One that something else moves into keeps to its side of the screen, everything shrinking rather than it going to the other side.
- A floating window that opens takes the room that is left on its side, or if there is none, the room that is left anywhere; if there isn't enough, everything shrinks in place.
- Tiled columns keep their rows. A new column joins the row of the column before it if that row has room for it, and starts a row of its own after it otherwise. While the scale stays, the rows stay where they are, a row in the middle keeping its middle: a row that closing windows emptied leaves its room rather than everything moving over.
- Rows wrap anew when tiled windows came, went or moved and wrapping anew shows the windows noticeably larger. For floating windows alone coming and going, only if that also leaves the floating windows where they are.
- The block of tiled windows stays on its side of the grid.
- The scale doesn't grow while windows open, and grows noticeably when windows close; floating windows then move out along with the block.
- It never gets far worse than laying everything out anew, the way opening the grid would: as soon as that shows the windows 25% larger, or after windows closed, 15% larger, the grid moves everything there.

Opening the grid again with the same windows as when it closed shows them where it did then; a floating window that moved or changed size in the meantime takes the free spot closest to where it is now.
With windows opened or closed in the meantime, the grid lays everything out afresh.

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
