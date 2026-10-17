#!/bin/sh
# Rebases the fork's Smithay patch (SeatState::new_wl_seat_with_filter, for the agent seat) onto the
# Smithay rev that niri pins, pushes it to the Smithay fork, and points Cargo.toml's [patch] at it.
#
# Run it from the niri checkout after merging upstream. If the patch conflicts, the rebase stops in
# $SMITHAY_DIR; resolve it there, `git rebase --continue`, and run this again.
set -eu

SMITHAY_DIR=${SMITHAY_DIR:-$HOME/Documents/github/smithay-agent}
UPSTREAM=https://github.com/Smithay/smithay.git
FORK=https://github.com/SHORiN-KiWATA/smithay.git

root=$(git rev-parse --show-toplevel)
cd "$root"

# The rev upstream niri pins, and the patched commit we use now.
base=$(sed -n '/^\[workspace\.dependencies\.smithay\]/,/^\[/s/^rev = "\([0-9a-f]*\)"/\1/p' Cargo.toml | head -n 1)
current=$(sed -n 's|^smithay = { git = "'"$FORK"'", rev = "\([0-9a-f]*\)" }|\1|p' Cargo.toml)
if [ -z "$base" ] || [ -z "$current" ]; then
    echo "error: couldn't find the Smithay revs in Cargo.toml" >&2
    exit 1
fi

if [ ! -d "$SMITHAY_DIR" ]; then
    git clone --quiet "$UPSTREAM" "$SMITHAY_DIR"
    git -C "$SMITHAY_DIR" remote add fork "$FORK"
fi
cd "$SMITHAY_DIR"
git fetch --quiet origin
git fetch --quiet fork

if [ "$(git rev-parse "$current^")" = "$(git rev-parse "$base")" ]; then
    echo "Smithay patch $current is already on top of $base."
    exit 0
fi

# The patch is the single commit on top of whatever it was based on.
git checkout --quiet -B seat-global-filter "$current"
git rebase --onto "$base" "$current^" seat-global-filter
new=$(git rev-parse HEAD)

# One branch per base, so that the commits older niri revisions pin stay reachable.
branch="niri-$(printf %.8s "$base")"
git push --quiet fork "seat-global-filter:$branch"

cd "$root"
sed -i "s/$current/$new/g" Cargo.toml
cargo metadata --quiet --format-version 1 >/dev/null

echo "Smithay patch rebased: $current -> $new (on $base, branch $branch)."
