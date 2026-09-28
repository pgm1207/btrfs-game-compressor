#!/bin/sh
#
# One-line installer:
#   curl -fsSL https://raw.githubusercontent.com/pablogonz12/btrfs-game-compressor/main/install.sh | sh
#
# Verifies and installs a tagged release archive into a directory on PATH.
# No build is needed; installation only writes the selected executable.

set -eu

PROG="btrfs-game-compressor"
REPO="${BTRFS_GAME_COMPRESSOR_REPO:-pablogonz12/btrfs-game-compressor}"
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

for dep in curl tar install sha256sum awk grep head chmod; do
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
ARCHIVE="$PROG-$VERSION.tar.gz"

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

# --- distribution detection ------------------------------------------------
# The packages that provide 'btrfs' and 'compsize' are named differently across
# distributions. This maps the running one to the right command; it is only ever
# offered, never run silently.

distro_fields() {
    [ -r /etc/os-release ] || { printf 'unknown '; return; }
    awk -F= '
        $1 == "ID"      { v = $2; gsub(/"/, "", v); id = v }
        $1 == "ID_LIKE" { v = $2; gsub(/"/, "", v); like = v }
        END { print (id ? id : "unknown") " " like }
    ' /etc/os-release 2>/dev/null
}

pkg_command() {
    # distro_fields prints "ID ID_LIKE"; splitting that into $1 and $2 is exactly
    # what is wanted here, so the word-splitting warning is silenced on purpose.
    # shellcheck disable=SC2046
    set -- $(distro_fields)
    # The compsize package is named 'compsize' on Arch, Fedora and openSUSE, but
    # 'btrfs-compsize' on Debian and Ubuntu. Getting this wrong makes the
    # offered install command fail, so the two are kept separate.
    case "${1:-unknown} ${2:-}" in
        *arch*|*cachyos*|*endeavouros*|*manjaro*|*steamos*|*garuda*)
            printf 'pacman -S btrfs-progs compsize' ;;
        *fedora*|*bazzite*|*nobara*|*rhel*|*centos*)
            printf 'dnf install btrfs-progs compsize' ;;
        *opensuse*|*suse*)
            printf 'zypper install btrfs-progs compsize' ;;
        *ubuntu*|*debian*|*pop*|*mint*|*elementary*|*zorin*)
            printf 'apt install btrfs-progs btrfs-compsize' ;;
        *) : ;;
    esac
}

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
install -m 0755 "$SCRIPT" "$BINDIR/$PROG"

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

# --- optional dependencies -------------------------------------------------

MISSING=""
have btrfs    || MISSING="$MISSING btrfs"
have compsize || MISSING="$MISSING compsize"
MISSING="${MISSING# }"

if [ -n "$MISSING" ]; then
    PKGS=$(pkg_command)
    say ""
    say "optional: $MISSING not found."
    if [ -n "$PKGS" ]; then
        say "          install with:  sudo $PKGS"
        if [ -t 0 ]; then
            if [ "$(id -u)" -eq 0 ] || have sudo; then
                printf 'Install them now? [y/N] '
                read -r reply
                case "$reply" in
                    [Yy]*)
                        if [ "$(id -u)" -eq 0 ]; then
                            # shellcheck disable=SC2086
                            $PKGS || say "install command failed; run it by hand."
                        else
                            # shellcheck disable=SC2086
                            sudo $PKGS || say "install command failed; run it by hand."
                        fi ;;
                    *) say "skipped - install them yourself when ready." ;;
                esac
            else
                say "          (sudo is not available; run it as root)"
            fi
        fi
    else
        say "          install them with your distribution's package manager."
    fi
fi

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
