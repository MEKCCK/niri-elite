#!/usr/bin/env bash
# Check this fork against upstream niri for config and IPC drift.
#
# Compares two things with upstream/main (or the given ref):
#   - resources/default-config.kdl: option tokens that upstream has and we do not
#   - niri-ipc/src/lib.rs: fields of the IPC structs, so a renamed or removed field shows up
#
# This is a heuristic text comparison, not a KDL or Rust parser: it is meant to run before merging
# upstream, as a reminder of what changed, not as a hard gate.
#
# Usage:
#   contrib/check-upstream-drift.sh              # compare against upstream/main
#   contrib/check-upstream-drift.sh origin/main  # compare against another ref
#
# Exit code: 0 when nothing that upstream has is missing here, 1 when drift was found.

set -euo pipefail

ref="${1:-upstream/main}"
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! git rev-parse --verify --quiet "$ref" >/dev/null; then
    echo "error: ref '$ref' not found; add the upstream remote and fetch it first:" >&2
    echo "  git remote add upstream https://github.com/niri-wm/niri.git && git fetch upstream main" >&2
    exit 2
fi

status=0

# --- config options ---------------------------------------------------------------------------

# Strip comments and braces, then collect bare word tokens. Structural keywords are filtered out;
# what remains is mostly option names.
extract_options() {
    sed -e 's|//.*$||' -e 's/[{}]//g' -e 's/[;"]//g' \
        | tr ' \t' '\n\n' \
        | grep -E '^[a-z][a-z0-9-]*$' \
        | grep -vxE 'include|optional|true|false|match|proportion|fixed|off|on|left|right|top|bottom|center|auto|none' \
        | sort -u
}

upstream_cfg="$(git show "$ref:resources/default-config.kdl" 2>/dev/null || true)"
if [ -z "$upstream_cfg" ]; then
    echo "error: cannot read resources/default-config.kdl from '$ref'" >&2
    exit 2
fi

printf '%s\n' "$upstream_cfg" | extract_options > /tmp/upstream-options.txt
extract_options < resources/default-config.kdl > /tmp/fork-options.txt

missing_cfg="$(comm -23 /tmp/upstream-options.txt /tmp/fork-options.txt)"
extra_cfg="$(comm -13 /tmp/upstream-options.txt /tmp/fork-options.txt)"

if [ -n "$missing_cfg" ]; then
    status=1
    echo "== config options upstream has, this fork's default config does not =="
    printf '  %s\n' $missing_cfg
    echo "   (check docs/wiki for these; our default config may simply not use them)"
    echo
fi

if [ -n "$extra_cfg" ]; then
    echo "== config options only in this fork's default config =="
    printf '  %s\n' $extra_cfg
    echo
fi

# --- IPC struct fields ------------------------------------------------------------------------

# Field names of the IPC structs we care about, with serde attributes ignored.
extract_ipc_fields() {
    awk '
        /^pub struct (Window|WindowLayout|Workspace|Output|KeyboardLayouts|Version) \{/ { struct=$3; in_struct=1; next }
        in_struct && /^\}/ { in_struct=0; struct=""; next }
        in_struct && /^    pub [a-z_]+:/ { name=$2; sub(":", "", name); print struct "." name }
    '
}

upstream_ipc="$(git show "$ref:niri-ipc/src/lib.rs" 2>/dev/null || true)"
if [ -z "$upstream_ipc" ]; then
    echo "error: cannot read niri-ipc/src/lib.rs from '$ref'" >&2
    exit 2
fi

printf '%s\n' "$upstream_ipc" | extract_ipc_fields | sort -u > /tmp/upstream-ipc.txt
extract_ipc_fields < niri-ipc/src/lib.rs | sort -u > /tmp/fork-ipc.txt

missing_ipc="$(comm -23 /tmp/upstream-ipc.txt /tmp/fork-ipc.txt)"
extra_ipc="$(comm -13 /tmp/upstream-ipc.txt /tmp/fork-ipc.txt)"

if [ -n "$missing_ipc" ]; then
    status=1
    echo "== IPC fields upstream has, this fork does not =="
    printf '  %s\n' $missing_ipc
    echo "   (clients like waybar read these; a missing field breaks them silently)"
    echo
fi

if [ -n "$extra_ipc" ]; then
    echo "== IPC fields only in this fork =="
    printf '  %s\n' $extra_ipc
    echo
fi

if [ "$status" -eq 0 ]; then
    echo "no drift: nothing upstream has is missing here"
fi

exit "$status"
