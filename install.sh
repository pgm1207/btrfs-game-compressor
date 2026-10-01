#!/bin/sh
#
# One-line installer:
#   curl -fsSL https://raw.githubusercontent.com/pgm1207/btrfs-game-compressor/main/install.sh | sh
#
# Verifies and installs a tagged release archive into a directory on PATH.
# Release archives include the Bash interface and a static native backend.

set -eu

PROG="btrfs-game-compressor"
REPO="${BTRFS_GAME_COMPRESSOR_REPO:-pgm1207/btrfs-game-compressor}"
# Empty means "follow the latest release"; see the resolution below.
VERSION="${BTRFS_GAME_COMPRESSOR_VERSION:-}"

PREFIX="${PREFIX:-$HOME/.local}"
case "$PREFIX" in
    /*) ;;
    *)  PREFIX="$HOME/$PREFIX" ;;
esac
BINDIR="$PREFIX/bin"

TMPDIR_="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_"' EXIT INT TERM

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

for dep in curl tar install sha256sum awk grep head chmod mv uname; do
    have "$dep" || die "required command not found: $dep"
done

# Follow the latest published release when no version is pinned, so that
# re-running this installer is an update, not a reinstall of an old copy. The
# tag comes from the GitHub API; awk pulls it out without needing jq.
if [ -z "$VERSION" ]; then
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
        | awk -F'"' '/"tag_name"[[:space:]]*:/ { print $4; exit }')
    VERSION="${TAG#v}"
fi
[ -n "$VERSION" ] || die "could not determine the latest release; pin one with BTRFS_GAME_COMPRESSOR_VERSION=x.y.z"
ARCH=$(uname -m)
case "$ARCH" in x86_64|aarch64) ;; *) die "unsupported architecture: $ARCH" ;; esac
[ "$(uname -s)" = Linux ] || die "Linux is required"
ARCHIVE="$PROG-$VERSION-linux-$ARCH.tar.gz"

RELEASE_URL="https://github.com/$REPO/releases/download/v$VERSION"

say "fetching $PROG $VERSION from $RELEASE_URL"
curl -fsSL "$RELEASE_URL/$ARCHIVE" -o "$TMPDIR_/$ARCHIVE" || die "release archive download failed"
curl -fsSL "$RELEASE_URL/SHA256SUMS" -o "$TMPDIR_/SHA256SUMS" || die "release checksum manifest download failed"

EXPECTED=$(awk -v file="$ARCHIVE" '$2 == file && length($1) == 64 && $1 ~ /^[[:xdigit:]]+$/ { print $1 }' "$TMPDIR_/SHA256SUMS")
[ -n "$EXPECTED" ] || die "no valid checksum for $ARCHIVE in SHA256SUMS"
ACTUAL=$(sha256sum "$TMPDIR_/$ARCHIVE" | awk '{print $1}')
[ "$ACTUAL" = "$EXPECTED" ] || die "checksum verification failed for $ARCHIVE"

tar -xzf "$TMPDIR_/$ARCHIVE" -C "$TMPDIR_" || die "could not unpack release archive"
SRCDIR="$TMPDIR_/$PROG-$VERSION"
SCRIPT="$SRCDIR/$PROG"
[ -s "$SCRIPT" ] || die "release archive does not contain $PROG"
head -n1 "$SCRIPT" | grep -q '^#!' || die "release executable is not a script"

chmod +x "$SCRIPT"

BACKEND="$SRCDIR/bgc-native"
[ -s "$BACKEND" ] || die "release archive does not contain bgc-native"
chmod +x "$BACKEND"
[ "$("$BACKEND" --protocol-version)" = 1 ] || die "incompatible native backend"

# --- install ---------------------------------------------------------------

if [ -t 0 ]; then
    say ""
    say "About to install to: $BINDIR/$PROG"
    printf 'Continue? [y/N] '
    read -r reply
    case "$reply" in
        [Yy]*) ;;
        *) say "aborted."; exit 0 ;;
    esac
fi

mkdir -p "$BINDIR"
install -m 0755 "$BACKEND" "$BINDIR/.bgc-native.new"
mv -f "$BINDIR/.bgc-native.new" "$BINDIR/bgc-native"
install -m 0755 "$SCRIPT" "$BINDIR/.$PROG.new"
mv -f "$BINDIR/.$PROG.new" "$BINDIR/$PROG"

# The manpage and the community ratio table go where the script looks for them
# at run time: <prefix>/share/.... The table has to be installed, not just
# shipped in the archive, or --ratios cannot work offline on an installed copy.
if [ -f "$SRCDIR/$PROG.1" ]; then
    install -d "$PREFIX/share/man/man1"
    install -m 0644 "$SRCDIR/$PROG.1" "$PREFIX/share/man/man1/$PROG.1"
fi
if [ -f "$SRCDIR/ratios/games.json" ]; then
    install -d "$PREFIX/share/$PROG/ratios"
    install -m 0644 "$SRCDIR/ratios/games.json" "$PREFIX/share/$PROG/ratios/games.json"
    [ -f "$SRCDIR/GAMES.md" ] && install -m 0644 "$SRCDIR/GAMES.md" "$PREFIX/share/$PROG/GAMES.md"
fi

say ""
say "installed $BINDIR/$PROG"

# --- PATH ------------------------------------------------------------------

case ":$PATH:" in
    *":$BINDIR:"*) ;;
    *)
        PATH_LINE="export PATH=\"$BINDIR:\$PATH\""
        say ""
        say "warning: $BINDIR is not in your PATH."
        case "${SHELL:-}" in
            */fish)
                say "add this to your fish configuration:"
                say "  set -gx PATH $BINDIR \$PATH" ;;
            *)
                case "${SHELL:-}" in
                    */zsh)  RC_FILE="$HOME/.zshrc" ;;
                    */bash) RC_FILE="$HOME/.bashrc" ;;
                    *)      RC_FILE="$HOME/.profile" ;;
                esac
                if [ -f "$RC_FILE" ] && grep -qF "$PATH_LINE" "$RC_FILE" 2>/dev/null; then
                    say "$RC_FILE already adds it; open a new terminal."
                elif [ -t 0 ]; then
                    printf 'Add %s to %s now? [y/N] ' "$BINDIR" "$RC_FILE"
                    read -r reply
                    case "$reply" in
                        [Yy]*)
                            printf '\n# added by %s installer\n%s\n' "$PROG" "$PATH_LINE" >> "$RC_FILE"
                            say "added to $RC_FILE (open a new terminal, or: . $RC_FILE)" ;;
                        *)
                            say "add this line yourself:"
                            say "  $PATH_LINE" ;;
                    esac
                else
                    say "add this line yourself:"
                    say "  $PATH_LINE"
                fi ;;
        esac
        ;;
esac

say ""
say "next:  $PROG --status      check your library"
say "       $PROG --self-update update later (or re-run this installer)"
say "       $PROG --uninstall   remove it again"
say "       $PROG --help        everything else"
