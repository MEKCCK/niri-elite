### Grid Overview

The Grid Overview is a grid-based view of your windows.
It shows every window as a tile in a grid, letting you quickly navigate between them.

Open it with the `toggle-grid-overview` bind, or by tapping the `Mod` key (pressing and releasing it without pressing any other key in between). The `Mod` key tap behavior is built in and works even without an explicit binding, controlled by the `default-mod-action` setting (enabled by default).

While in the grid overview, all keyboard shortcuts keep working.
Use the arrow keys to navigate between windows.
If a column contains multiple windows, use Up/Down to switch between them within the same cell.

Click a window to activate it and close the grid overview.

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
