#!/usr/bin/env bash
#
# Behavioural tests for btrfs-game-compressor.
#
# Runs against a throwaway HOME so it never touches your real config or state,
# and never invokes btrfs/compsize. Safe to run anywhere, including CI.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROG="$REPO_ROOT/btrfs-game-compressor"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0

export HOME="$WORK/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME"

# Neutralise anything host-specific that would change results.
unset NO_COLOR
export LC_ALL=C
# The suite must never reach GitHub: the automatic update check is disabled
# globally, and exercised separately with a fake curl.
export BTRFS_GAME_COMPRESSOR_NO_SELF_UPDATE=1

ok()   { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; [ $# -gt 1 ] && printf '       %s\n' "$2"; }
group(){ printf '\n%s\n' "$1"; }

check_rc() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" -eq "$expected" ]; then ok "$desc"; else bad "$desc" "expected rc=$expected, got rc=$actual"; fi
}

check_contains() {
    local desc="$1" needle="$2" hay="$3"
    case "$hay" in
        *"$needle"*) ok "$desc" ;;
        *) bad "$desc" "output did not contain '$needle'" ;;
    esac
}

check_not_contains() {
    local desc="$1" needle="$2" hay="$3"
    case "$hay" in
        *"$needle"*) bad "$desc" "output unexpectedly contained '$needle'" ;;
        *) ok "$desc" ;;
    esac
}

# Run the program with the throwaway HOME.
run() { "$PROG" "$@" 2>&1; }

# ---------------------------------------------------------------------------
group "Platform detection"

# Helper: run --status with a fake os-release to test platform detection.
run_with_os_release() {
    local content="$1"
    local os_release_file="$WORK/os-release"
    printf '%s\n' "$content" > "$os_release_file"
    BTRFS_GAME_COMPRESSOR_OS_RELEASE="$os_release_file" "$PROG" --status --no-color 2>&1
}

# CachyOS (this device)
out=$(run_with_os_release 'ID=cachyos
ID_LIKE=arch')
check_contains "CachyOS is detected" "CachyOS" "$out"

# SteamOS
out=$(run_with_os_release 'ID=steamos')
check_contains "SteamOS is detected" "SteamOS" "$out"

# SteamOS with Steam Deck variant
out=$(run_with_os_release 'ID=steamos
VARIANT_ID=steamdeck')
check_contains "Steam Deck variant is detected" "Steam Deck" "$out"

# Bazzite
out=$(run_with_os_release 'ID=bazzite
ID_LIKE=fedora')
check_contains "Bazzite is detected" "Bazzite" "$out"

# Fedora
out=$(run_with_os_release 'ID=fedora')
check_contains "Fedora is detected" "Fedora" "$out"

# Fedora (Silverblue variant)
out=$(run_with_os_release 'ID=fedora
VARIANT_ID=silverblue')
check_contains "Fedora Silverblue is detected" "Fedora" "$out"

# Arch
out=$(run_with_os_release 'ID=arch')
check_contains "Arch Linux is detected" "Arch Linux" "$out"

# Ubuntu
out=$(run_with_os_release 'ID=ubuntu')
check_contains "Ubuntu is detected" "Ubuntu" "$out"

# Debian
out=$(run_with_os_release 'ID=debian')
check_contains "Debian is detected" "Debian" "$out"

# openSUSE
out=$(run_with_os_release 'ID=opensuse-leap')
check_contains "openSUSE is detected" "openSUSE" "$out"

# ID_LIKE fallback: Pop!_OS -> ubuntu
out=$(run_with_os_release 'ID=pop
ID_LIKE="ubuntu debian"')
check_contains "Pop!_OS falls back to ubuntu" "Ubuntu" "$out"

# ID_LIKE fallback: EndeavourOS -> arch
out=$(run_with_os_release 'ID=endeavouros
ID_LIKE="arch"')
check_contains "EndeavourOS falls back to arch" "Arch Linux" "$out"

# ID_LIKE fallback: Nobara -> fedora
out=$(run_with_os_release 'ID=nobara
ID_LIKE="fedora"')
check_contains "Nobara falls back to Fedora" "Fedora" "$out"

# Unknown distro -> "other"
out=$(run_with_os_release 'ID=gentoo')
check_contains "Gentoo falls back to other" "other" "$out"

# No os-release at all -> no crash
out=$(HOME="$WORK/empty" "$PROG" --status --no-color 2>&1)
check_rc "missing os-release does not crash" 0 $?

# ---------------------------------------------------------------------------
group "CLI surface"

out=$(run --version); rc=$?
check_rc "--version exits 0" 0 $rc
check_contains "--version prints name" "btrfs-game-compressor" "$out"
check_contains "--version prints a version" "1." "$out"
check_not_contains "--version emits no ANSI escapes" $'\033' "$out"

out=$(run --help); rc=$?
check_rc "--help exits 0" 0 $rc
check_contains "--help shows usage" "USAGE" "$out"
check_contains "--help documents --status" "--status" "$out"
check_contains "--help documents --dry-run" "--dry-run" "$out"
check_contains "--help documents --library" "--library" "$out"
check_contains "--help documents --balance" "--balance" "$out"
check_contains "--help documents JSON status" "--json" "$out"
check_contains "--help documents notifications" "--notify" "$out"
check_contains "--help documents NO_COLOR" "NO_COLOR" "$out"
check_not_contains "--help no longer advertises --all-libraries" "--all-libraries" "$out"

# --all-libraries was accepted and then ignored, which is a lie about what the
# flag does. A flag that silently does nothing is worse than no flag.
out=$(run --all-libraries); rc=$?
check_rc "the unimplemented --all-libraries is rejected" 2 $rc
check_contains "the rejected flag explains itself" "unknown option" "$out"

out=$(run --bogus); rc=$?
check_rc "unknown flag exits 2" 2 $rc
check_contains "unknown flag explains itself" "unknown option" "$out"

out=$(run -V); rc=$?
check_rc "-V short flag exits 0" 0 $rc

out=$(run -h); rc=$?
check_rc "-h short flag exits 0" 0 $rc

# ---------------------------------------------------------------------------
group "Non-interactive modes"

out=$(run --status --no-color); rc=$?
check_rc "--status exits 0 with no libraries" 0 $rc
check_contains "--status prints a header" "GAME" "$out"
check_contains "--status prints a summary" "libraries:" "$out"
check_not_contains "--status emits no ANSI when --no-color" $'\033' "$out"

out=$(run --status); rc=$?
check_rc "--status exits 0 with colors allowed" 0 $rc

out=$(run --dry-run --no-color); rc=$?
check_rc "--dry-run exits 0" 0 $rc
check_contains "--dry-run reports nothing to do" "nothing to do" "$out"

out=$(run --status --pending --no-color); rc=$?
check_rc "--status --pending exits 0" 0 $rc

out=$(run --status --json); rc=$?
check_rc "--status --json exits 0" 0 $rc
check_contains "--status --json emits a JSON object" '{"version":' "$out"
check_contains "--status --json includes a summary" '"summary":' "$out"
out=$(run --json); rc=$?
check_rc "--json without --status is rejected" 2 $rc

out=$(run --help); rc=$?
check_contains "--help documents --history" "--history" "$out"
check_contains "--help documents --stats" "--stats" "$out"
check_contains "--help documents --benchmark" "--benchmark" "$out"

# --history and --stats read the state database. With an empty database they
# must still exit cleanly rather than erroring on a missing file.
out=$(run --history --no-color); rc=$?
check_rc "--history exits 0 with no history" 0 $rc
check_contains "--history explains it has nothing yet" "No compression history" "$out"

out=$(run --stats --no-color); rc=$?
check_rc "--stats exits 0 with no history" 0 $rc
check_contains "--stats explains it has nothing yet" "No compressed games recorded" "$out"

out=$(run --benchmark --no-color); rc=$?
check_rc "--benchmark exits 0 with no data" 0 $rc
check_contains "--benchmark explains it has nothing yet" "No measured games yet" "$out"

# They are self-contained one-shot modes; combining one with another mode would
# silently ignore the other, so the combination is rejected.
out=$(run --history --status); rc=$?
check_rc "--history cannot be combined with --status" 2 $rc
check_contains "the rejected --history combo explains itself" "cannot be combined" "$out"
out=$(run --stats --dry-run); rc=$?
check_rc "--stats cannot be combined with --dry-run" 2 $rc
out=$(run --stats --json); rc=$?
check_rc "--stats cannot be combined with --json" 2 $rc
out=$(run --benchmark --status); rc=$?
check_rc "--benchmark cannot be combined with --status" 2 $rc
out=$(run --benchmark --dry-run); rc=$?
check_rc "--benchmark cannot be combined with --dry-run" 2 $rc
out=$(run --json); rc=$?
check_rc "--json without --status or --benchmark is rejected" 2 $rc

# --balance rewrites a whole filesystem, so it must never be silently combined
# with a reporting mode and then fall through to the confirmation prompt.
out=$(run --status --balance); rc=$?
check_rc "--balance cannot be combined with --status" 2 $rc
check_contains "the rejected --balance combo explains itself" "cannot be combined" "$out"

# The ratio-table modes obey the same one-mode-at-a-time rule.
out=$(run --ratios --status); rc=$?
check_rc "--ratios cannot be combined with --status" 2 $rc
out=$(run --export-ratios --benchmark); rc=$?
check_rc "--export-ratios cannot be combined with --benchmark" 2 $rc
out=$(run --import-ratios /tmp/nope.json --status); rc=$?
check_rc "--import-ratios cannot be combined with --status" 2 $rc
out=$(run --json); rc=$?
check_rc "--json alone is rejected" 2 $rc

# Interactive mode must refuse to run without a terminal, and must say why.
out=$("$PROG" < /dev/null 2>&1); rc=$?
check_rc "no TTY and no subcommand exits nonzero" 1 $rc
check_contains "no TTY explains the alternative" "--status" "$out"

# ---------------------------------------------------------------------------
group "Read-only modes never mutate state"

STATE_DIR="$HOME/.local/state/btrfs-game-compressor"
STATE_FILE="$STATE_DIR/compressed_games.db"
rm -f "$STATE_FILE"
run --status >/dev/null
run --dry-run >/dev/null
if [ -s "$STATE_FILE" ]; then
    bad "state file stays empty" "it has content: $(cat "$STATE_FILE")"
else
    ok "state file stays empty"
fi

# ---------------------------------------------------------------------------
group "--library"

mkdir -p "$WORK/lib1/steamapps/common"
out=$(run --library "$WORK/lib1"); rc=$?
check_rc "--library accepts a library root" 0 $rc
check_contains "--library confirms registration" "Registered" "$out"
if grep -qxF "$WORK/lib1/steamapps/common" "$HOME/.config/btrfs-game-compressor/custom_libraries.txt" 2>/dev/null; then
    ok "--library normalises to steamapps/common"
else
    bad "--library normalises to steamapps/common" "not found in custom_libraries.txt"
fi

out=$(run --library "$WORK/lib1"); rc=$?
check_rc "--library is idempotent" 0 $rc
check_contains "--library reports duplicates" "Already registered" "$out"
count=$(grep -cxF "$WORK/lib1/steamapps/common" "$HOME/.config/btrfs-game-compressor/custom_libraries.txt" 2>/dev/null || echo 0)
if [ "$count" -eq 1 ]; then
    ok "--library did not duplicate the entry"
else
    bad "--library did not duplicate the entry" "found $count copies"
fi

out=$(run --library "$WORK/does-not-exist"); rc=$?
check_rc "--library rejects a missing directory" 1 $rc
check_contains "--library explains a missing directory" "not a directory" "$out"

out=$(run --library); rc=$?
check_rc "--library without an argument exits 2" 2 $rc

# ---------------------------------------------------------------------------
group "Config and state layout"

if [ -d "$HOME/.config/btrfs-game-compressor" ]; then
    ok "config dir created under XDG_CONFIG_HOME"
else
    bad "config dir created under XDG_CONFIG_HOME"
fi
if [ -f "$HOME/.config/btrfs-game-compressor/throttle.conf" ]; then
    ok "throttle.conf created"
    val=$(cat "$HOME/.config/btrfs-game-compressor/throttle.conf")
    if [ "$val" = "0" ]; then ok "throttle defaults to full speed"; else bad "throttle defaults to full speed" "got '$val'"; fi
else
    bad "throttle.conf created"
fi

if [ -f "$HOME/.config/btrfs-game-compressor/min_size_mib" ]; then
    ok "min_size_mib created"
    val=$(cat "$HOME/.config/btrfs-game-compressor/min_size_mib")
    if [ "$val" = "1" ]; then ok "min_size_mib defaults to 1"; else bad "min_size_mib defaults to 1" "got '$val'"; fi
else
    bad "min_size_mib created"
fi

# ---------------------------------------------------------------------------
group "Internal unit checks"
# Functions are sourced by truncating the script before its entry point, so
# these can be tested without running the TUI.

LIB="$WORK/lib.sh"
sed '/^# --- Entry Point ---/,$d' "$PROG" > "$LIB"
# shellcheck source=/dev/null
. "$LIB" 2>/dev/null

if declare -f scan_game_dir >/dev/null 2>&1; then
    d="$WORK/deep"; rm -rf "$d"; mkdir -p "$d/a/b/c"
    touch "$d/a/b/c/payload" "$d/top"
    sleep 1
    ref=$(date +%s)

    read -r bytes files newer human <<< "$(scan_game_dir "$d" "$ref")"
    if [ "$newer" -eq 0 ]; then
        ok "scan_game_dir: untouched tree is not newer"
    else
        bad "scan_game_dir: untouched tree is not newer" "newer=$newer"
    fi
    if [ "$files" -eq 2 ] && [ "$bytes" -eq 0 ]; then
        ok "scan_game_dir: counts files and bytes"
    else
        bad "scan_game_dir: counts files and bytes" "files=$files bytes=$bytes"
    fi

    touch "$d/a/b/c/payload"
    read -r bytes files newer human <<< "$(scan_game_dir "$d" "$ref")"
    if [ "$newer" -eq 1 ]; then
        ok "scan_game_dir: deep change is detected"
    else
        bad "scan_game_dir: deep change is detected" "a depth-5 change was missed"
    fi

    # A directory with no files at all must report zero, not fail.
    e="$WORK/empty"; rm -rf "$e"; mkdir -p "$e"
    read -r bytes files newer human <<< "$(scan_game_dir "$e" "$ref")"
    if [ "$bytes" -eq 0 ] && [ "$files" -eq 0 ]; then
        ok "scan_game_dir: empty directory reports 0 0"
    else
        bad "scan_game_dir: empty directory reports 0 0" "bytes=$bytes files=$files"
    fi

    # A missing directory must not hang or emit garbage.
    read -r bytes files newer human <<< "$(scan_game_dir "$WORK/nope" "$ref")"
    if [ "$bytes" -eq 0 ] && [ "$files" -eq 0 ]; then
        ok "scan_game_dir: missing directory degrades safely"
    else
        bad "scan_game_dir: missing directory degrades safely" "bytes=$bytes files=$files"
    fi
else
    bad "scan_game_dir is defined"
fi

if declare -f load_config >/dev/null 2>&1; then
    CONFIG_DIR="$WORK/cfg"; mkdir -p "$CONFIG_DIR"
    MIN_SIZE_FILE="$CONFIG_DIR/min_size_mib"

    echo 1 > "$MIN_SIZE_FILE"; load_config
    [ "$MIN_SIZE_MIB" = "1" ] && ok "load_config: default threshold" || bad "load_config: default threshold" "got $MIN_SIZE_MIB"

    echo 512 > "$MIN_SIZE_FILE"; load_config
    [ "$MIN_SIZE_MIB" = "512" ] && ok "load_config: custom threshold" || bad "load_config: custom threshold" "got $MIN_SIZE_MIB"

    echo 0 > "$MIN_SIZE_FILE"; load_config
    [ "$MIN_SIZE_MIB" = "0" ] && ok "load_config: 0 disables the filter" || bad "load_config: 0 disables the filter" "got $MIN_SIZE_MIB"

    echo "not-a-number" > "$MIN_SIZE_FILE"
    load_config 2>/dev/null
    [ "$MIN_SIZE_MIB" = "1" ] && ok "load_config: invalid value falls back to 1" || bad "load_config: invalid value falls back" "got $MIN_SIZE_MIB"
else
    bad "load_config is defined"
fi

if declare -f state_upsert >/dev/null 2>&1; then
    STATE_DIR="$WORK/state"; mkdir -p "$STATE_DIR"
    STATE_FILE="$STATE_DIR/games.db"; : > "$STATE_FILE"
    for n in 'Celeste' 'Warframe [DLC]' 'AC/DC' 'Re|Name'; do
        state_upsert "/games/$n" "/games/$n|$n|100|10.00GiB|5.00GiB|5 GB|5120|10240|5120"
    done
    lines=$(grep -vc '^#' "$STATE_FILE")
    if [ "$lines" -eq 4 ]; then
        ok "state_upsert: names with metacharacters are stored"
    else
        bad "state_upsert: names with metacharacters are stored" "got $lines records, want 4"
    fi
    # The version header must be written, but exactly once, no matter how many
    # records are added.
    headers=$(grep -c '^# btrfs-game-compressor state v' "$STATE_FILE")
    if [ "$headers" -eq 1 ]; then
        ok "state_upsert: the version header is written exactly once"
    else
        bad "state_upsert: the version header is written exactly once" "found $headers header(s)"
    fi
    state_upsert '/games/Warframe [DLC]' '/games/Warframe [DLC]|Warframe [DLC]|200|10.00GiB|4.00GiB|6 GB|6144|10240|4096'
    lines=$(grep -vc '^#' "$STATE_FILE")
    ts=$(awk -F'|' '$1=="/games/Warframe [DLC]" {print $3}' "$STATE_FILE")
    if [ "$lines" -eq 4 ] && [ "$ts" = "200" ]; then
        ok "state_upsert: re-upsert replaces, never duplicates"
    else
        bad "state_upsert: re-upsert replaces, never duplicates" "records=$lines ts=$ts"
    fi
    encoded=$(encode_field $'/games/pipe|percent%/line\nnext')
    decode_field "$encoded"
    if [ "$DECODED_FIELD" = $'/games/pipe|percent%/line\nnext' ]; then
        ok "state fields: unusual path characters round-trip"
    else
        bad "state fields: unusual path characters round-trip" "got '$DECODED_FIELD'"
    fi
    quoted=$(json_quote $'quote" slash\\ line\nnext')
    if [ "$quoted" = '"quote\" slash\\ line\u000anext"' ]; then
        ok "JSON quoting: quotes, slashes and control characters are escaped"
    else
        bad "JSON quoting: quotes, slashes and control characters are escaped" "$quoted"
    fi

    # Migration from the old name-keyed formats. This is the regression that cost
    # a whole library its history: records keyed by name were silently discarded,
    # so every game looked uncompressed again. They are now matched to a path.
    STATE_DIR="$WORK/legacy-state"; mkdir -p "$STATE_DIR"
    STATE_FILE="$STATE_DIR/games.db"
    {
        echo "# btrfs-game-compressor state v2"
        echo 'Solo|1000000000|100M|40M|60M|60|100|40'
        echo 'TimestampOnly|1000000000'
        echo 'Orphan|1000000000|10M|9M|1M|1|10|9'
    } > "$STATE_FILE"
    load_state
    if [ "${LEGACY_REC[Solo]:-}" = "100M|40M|60M|60|100|40" ]; then
        ok "load_state: legacy 8-field records are captured by name"
    else
        bad "load_state: legacy 8-field records are captured by name" \
            "got '${LEGACY_REC[Solo]:-}'"
    fi
    if [ "${LEGACY_MAP[TimestampOnly]:-}" = "1000000000" ]; then
        ok "load_state: legacy 2-field records keep their timestamp"
    else
        bad "load_state: legacy 2-field records keep their timestamp" \
            "got '${LEGACY_MAP[TimestampOnly]:-}'"
    fi
    # Two installs of the same name must not inherit one history. Only a name
    # used by exactly one install is migrated.
    persist_legacy_migration "Solo"$'\t'"/libs/a/Solo" "TimestampOnly"$'\t'"/libs/a/TimestampOnly"
    if awk -F'|' '$1=="/libs/a/Solo" && $9=="40" {found=1} END{exit !found}' "$STATE_FILE"; then
        ok "migration: a matched legacy record becomes path-keyed"
    else
        bad "migration: a matched legacy record becomes path-keyed" "$(cat "$STATE_FILE")"
    fi
    if grep -qx 'Solo|1000000000|100M|40M|60M|60|100|40' "$STATE_FILE"; then
        bad "migration: the migrated legacy line is removed"
    else
        ok "migration: the migrated legacy line is removed"
    fi
    if grep -qx 'Orphan|1000000000|10M|9M|1M|1|10|9' "$STATE_FILE"; then
        ok "migration: an unmatched legacy record is preserved"
    else
        bad "migration: an unmatched legacy record is preserved" "$(cat "$STATE_FILE")"
    fi
    if [ -s "$STATE_DIR/games.db.bak" ]; then
        ok "migration: a backup is written before the state is replaced"
    else
        bad "migration: a backup is written before the state is replaced"
    fi

    mkdir -p "$WORK/fail-bin" "$WORK/fail-state"
    cat > "$WORK/fail-bin/btrfs" <<'SHIM'
#!/bin/sh
exit 1
SHIM
    chmod +x "$WORK/fail-bin/btrfs"
    : > "$WORK/fail-state/games.db"
    printf '0\n' > "$WORK/fail-state/throttle.conf"
    STATE_DIR="$WORK/fail-state"; STATE_FILE="$STATE_DIR/games.db"
    THROTTLE_FILE="$STATE_DIR/throttle.conf"; COMPSIZE_OK=0
    saved_path="$PATH"; PATH="$WORK/fail-bin:$PATH"
    if compress_and_record "FailingGame" "$WORK/failing-game" >/dev/null 2>&1; then
        bad "failed defragmentation is returned as failure"
    elif [ ! -s "$STATE_FILE" ]; then
        ok "failed defragmentation does not write compression history"
    else
        bad "failed defragmentation does not write compression history" "$(cat "$STATE_FILE")"
    fi
    PATH="$saved_path"

    printf '%s\n' \
        '/games/one|SameGame|100|10M|5M|5M|5|10|5' \
        '/games/two|SameGame|200|10M|9M|1M|1|10|9' > "$STATE_FILE"
    load_state
    if [ "${STATE_MAP[/games/one]:-}" = "100" ] && [ "${STATE_MAP[/games/two]:-}" = "200" ] && \
       [ "${STATE_RATIO[/games/one]:-}" != "${STATE_RATIO[/games/two]:-}" ]; then
        ok "state history: duplicate game names have independent path records"
    else
        bad "state history: duplicate game names have independent path records"
    fi
else
    bad "state_upsert is defined"
fi

if declare -f parse_vdf_libraries >/dev/null 2>&1; then
    VDF="$WORK/libraryfolders.vdf"
    cat > "$VDF" <<'VDF'
"libraryfolders"
{
  "0" { "path" "/games/one" }
  "1" { "path" "/games/with \"quoted\" name" }
  "2" { "path" "/games/back\\slash" }
}
VDF
    mapfile -t parsed < <(parse_vdf_libraries "$VDF")
    if [ "${#parsed[@]}" -eq 3 ] && [ "${parsed[0]}" = "/games/one" ] && \
       [ "${parsed[1]}" = '/games/with "quoted" name' ] && \
       [ "${parsed[2]}" = '/games/back\slash' ]; then
        ok "VDF parser: spaces, escaped quotes and backslashes are handled"
    else
        bad "VDF parser: spaces, escaped quotes and backslashes are handled" "${parsed[*]}"
    fi
fi

if declare -f to_mib >/dev/null 2>&1; then
    got=$(to_mib "10.00GiB");   want=10240
    [ "$got" = "$want" ] && ok "to_mib: GiB" || bad "to_mib: GiB" "got $got want $want"
    got=$(to_mib "512.00MiB");  want=512
    [ "$got" = "$want" ] && ok "to_mib: MiB" || bad "to_mib: MiB" "got $got want $want"
    got=$(to_mib "1.5 GiB");    want=1536
    [ "$got" = "$want" ] && ok "to_mib: space-separated unit" || bad "to_mib: space-separated unit" "got $got want $want"
    got=$(to_mib "1048576");    want=1
    [ "$got" = "$want" ] && ok "to_mib: bare bytes" || bad "to_mib: bare bytes" "got $got want $want"
else
    bad "to_mib is defined"
fi

if declare -f game_index_by_path >/dev/null 2>&1; then
    GAMES_PATH=("/a/one/" "/a/two/" "/a/three/")
    got=$(game_index_by_path "/a/two/")
    [ "$got" = "1" ] && ok "game_index_by_path: finds the right index" || bad "game_index_by_path" "got '$got' want 1"
    game_index_by_path "/nope/" >/dev/null 2>&1
    if [ $? -ne 0 ]; then ok "game_index_by_path: fails cleanly on unknown path"; else bad "game_index_by_path: fails cleanly"; fi
else
    bad "game_index_by_path is defined"
fi

# ---------------------------------------------------------------------------
group "Unmeasured compression is remembered"

# With compsize absent or failing, a successful defrag must still be recorded, or
# the game is re-compressed on every single run. It is stored as a zeroed marker
# that the statistics readers ignore.
if declare -f save_summary_record >/dev/null 2>&1 && declare -f load_state >/dev/null 2>&1; then
    SAVE_STATE_DIR="$STATE_DIR"
    SAVE_STATE_FILE="$STATE_FILE"
    STATE_DIR="$WORK"
    STATE_FILE="$WORK/unmeasured.db"
    : > "$STATE_FILE"
    COMPSIZE_OK=0
    save_summary_record "NoMeasure" "/libs/NoMeasure" >/dev/null 2>&1

    if grep -q 'NoMeasure' "$STATE_FILE"; then
        ok "an unmeasured compression is recorded"
    else
        bad "an unmeasured compression is recorded" "$(cat "$STATE_FILE" 2>/dev/null)"
    fi
    got=$(awk -F'|' '$2=="NoMeasure" {print $8"|"$9}' "$STATE_FILE")
    [ "$got" = "0|0" ] && ok "the marker carries zero measurements" \
        || bad "the marker carries zero measurements" "got '$got'"

    STATE_MAP=()
    load_state
    if [ -n "${STATE_MAP["/libs/NoMeasure"]:-}" ]; then
        ok "the marker makes the game read as compressed"
    else
        bad "the marker makes the game read as compressed"
    fi

    rows=$(state_summary | cut -f1)
    [ "$rows" = "0" ] && ok "the marker is excluded from statistics" \
        || bad "the marker is excluded from statistics" "rows=$rows"
    hist=$(history_rows 0)
    [ -z "$hist" ] && ok "the marker is excluded from history" \
        || bad "the marker is excluded from history" "$hist"

    STATE_DIR="$SAVE_STATE_DIR"
    STATE_FILE="$SAVE_STATE_FILE"
else
    bad "save_summary_record and load_state are defined"
fi

# ---------------------------------------------------------------------------
group "Zstd mount option parsing"

# Real-world mount option strings. The SteamOS cases come from
# popsUlfr/steamos-btrfs, which is what Valve-adjacent Btrfs setups ship.
if declare -f parse_zstd_level >/dev/null 2>&1; then
    _lvl() { parse_zstd_level "$1" 2>/dev/null; }

    check_level() {
        _got=$(_lvl "$2")
        if [ "$_got" = "$1" ]; then
            ok "parse_zstd_level: $3"
        else
            bad "parse_zstd_level: $3" "got '$_got' want '$1'"
        fi
    }

    # Regression: SteamOS mounts with compress-force=, which the previous
    # `case` statement did not recognise, so it warned that an already
    # zstd-compressed library had no compress= option.
    check_level "3"  "rw,noatime,compress-force=zstd,lazytime" \
        "SteamOS default compress-force=zstd is recognised"
    check_level "6"  "rw,noatime,lazytime,compress-force=zstd:6,space_cache=v2,autodefrag,nodiscard" \
        "SteamOS current compress-force=zstd:6 is recognised"
    check_level "15" "rw,compress-force=zstd:15" \
        "older Deck zstd:15 is recognised"
    check_level "1"  "rw,compress=zstd:1" \
        "compress=zstd:1 is recognised"

    check_level "" "rw,noatime,compress=lzo"  "lzo yields no zstd level"
    check_level "" "rw,noatime"                "no compression yields no level"
    check_level "" ""                         "empty options yield no level"

    # compress-force must win over compress when both are present.
    check_level "6" "compress=zstd:1,compress-force=zstd:6" \
        "compress-force takes precedence over compress"

    # Guard against the old typo (compr_force, underscore) ever coming back.
    check_level "" "rw,compr_force=zstd:6" \
        "misspelled compr_force is not accepted as zstd"

    parse_zstd_level "rw,compress=zstd:abc" >/dev/null 2>&1
    if [ $? -ne 0 ]; then
        ok "parse_zstd_level: rejects a non-numeric level"
    else
        bad "parse_zstd_level: rejects a non-numeric level"
    fi
else
    bad "parse_zstd_level is defined"
fi

if declare -f effective_level >/dev/null 2>&1; then
    COMPRESS_LEVEL=6
    [ "$(effective_level /nonexistent)" = "6" ] \
        && ok "effective_level: configured level overrides the mount" \
        || bad "effective_level: configured level overrides the mount"

    COMPRESS_LEVEL=auto
    [ "$(effective_level /nonexistent)" = "3" ] \
        && ok "effective_level: auto falls back to 3 with no zstd mount" \
        || bad "effective_level: auto falls back to 3 with no zstd mount"

    COMPRESS_LEVEL=bogus
    [ "$(effective_level /nonexistent)" = "bogus" ] \
        && ok "effective_level: 'auto' is the only non-numeric value honoured" \
        || bad "effective_level: unexpected passthrough"
    COMPRESS_LEVEL=auto
else
    bad "effective_level is defined"
fi

# ---------------------------------------------------------------------------
group "Size formatting"

# Regression guard: an earlier version indexed the unit table off by one and
# reported a 2 GiB game as "2.0T". These are exact binary boundaries, so any
# error is unambiguous.
if declare -f bytes_human >/dev/null 2>&1; then
    check_h() {
        _h=$(bytes_human "$1")
        if [ "$_h" = "$2" ]; then
            ok "bytes_human: $1 -> $2"
        else
            bad "bytes_human: $1" "got '$_h' want '$2'"
        fi
    }
    check_h 0             "0B"
    check_h 512           "512B"
    check_h 1023          "1023B"
    check_h 1024          "1.0K"
    check_h 1536          "1.5K"
    check_h 1048575       "1024.0K"
    check_h 1048576       "1.0M"
    check_h 2147483648    "2.0G"
    check_h 1099511627776 "1.0T"
else
    bad "bytes_human is defined"
fi

# ---------------------------------------------------------------------------
group "Low-yield prediction"

# End-to-end: a library with two games that both have compression history, one
# that used to compress well and one that did not, and a state file that marks
# both as needing work. Only the poor investment should be held back.
L="$WORK/yield library/steamapps/common"
mkdir -p "$L/GoodGame" "$L/BadGame"
# 50 MiB of real bytes each, so they clear the empty-install filter
dd if=/dev/zero of="$L/GoodGame/data" bs=1M count=50 2>/dev/null
dd if=/dev/zero of="$L/BadGame/data"  bs=1M count=50 2>/dev/null
mkdir -p "$HOME/.config/btrfs-game-compressor"
echo "$L" > "$HOME/.config/btrfs-game-compressor/custom_libraries.txt"

S="$HOME/.local/state/btrfs-game-compressor/compressed_games.db"
# 50 MiB each. GoodGame: 50->25, a 50% ratio. BadGame: 50->49, a 1% ratio.
# The timestamps are in the past so both read as updated rather than compressed.
{
    echo "$L/GoodGame|GoodGame|1000000000|50M|25M|25M|25|50|25"
    echo "$L/BadGame|BadGame|1000000000|50M|49M|1M|1|50|49"
} > "$S"

# The library must look like btrfs, which the test filesystem is not. Only
# intercept the `stat -f -c %T` probe and let everything else through.
mkdir -p "$WORK/shim"
cat > "$WORK/shim/stat" <<'SHIM'
#!/bin/sh
if [ "$1" = "-f" ] && [ "$3" = "%T" ]; then echo btrfs; else exec /usr/bin/stat "$@"; fi
SHIM
chmod +x "$WORK/shim/stat"
OLD_PATH="$PATH"
PATH="$WORK/shim:$PATH"

PC="$HOME/.config/btrfs-game-compressor/min_gain_pct"
MC="$HOME/.config/btrfs-game-compressor/min_gain_mib"
# A 5% ratio floor and a 32 MiB absolute floor. GoodGame's 25 MiB predicted
# gain clears 32; BadGame's 1% ratio does not clear 5.
echo "5"  > "$PC"
echo "20" > "$MC"

# The ratio is the primary test: a 1% game is not worth a pass no matter its
# size, and a 50% game is worth it no matter how small.
out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^GoodGame .*LOW YIELD"; then
    bad "a 50%-ratio game is not held back"
else
    ok "a 50%-ratio game is not held back"
fi
if printf '%s' "$out" | grep -q "^BadGame .*LOW YIELD"; then
    ok "a 1%-ratio game is held back as low yield"
else
    bad "a 1%-ratio game is held back as low yield" "got: $(printf '%s' "$out" | grep '^BadGame' || echo none)"
fi

# The absolute floor is a separate test: it must hold back a great 50% game
# when the saving is too small to be worth the effort, even though the ratio
# passes. This is the "not worth it for 3 MB" case.
echo "64" > "$MC"
out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^GoodGame .*LOW YIELD"; then
    ok "a small absolute gain is held back despite a good ratio"
else
    bad "a small absolute gain is held back despite a good ratio"
fi
# ...and the ratio test must still apply with the floor relaxed.
echo "0" > "$MC"
# Capture first, then grep: piping the program straight into `grep -q` makes
# grep exit on the first match, and `pipefail` then reports the program's
# SIGPIPE as a failure.
out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^BadGame .*LOW YIELD"; then
    ok "the ratio test still applies when the absolute floor is off"
else
    bad "the ratio test still applies when the absolute floor is off" "$($PROG --status --no-color 2>&1 | grep -E 'Good|Bad')"
fi

# Raising the ratio floor must catch a game that passed at 5%.
echo "5" > "$MC"
echo "80" > "$PC"
out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^GoodGame .*LOW YIELD"; then
    ok "min_gain_pct is honoured"
else
    bad "min_gain_pct is honoured" "$($PROG --status --no-color 2>&1 | grep -E 'Good|Bad')"
fi

# Switching both off must return everything to the normal flow.
echo "0" > "$PC"
echo "0" > "$MC"
out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "LOW YIELD"; then
    bad "zero thresholds disable the low-yield filter"
else
    ok "zero thresholds disable the low-yield filter"
fi
echo "5" > "$PC"
echo "20" > "$MC"

# And dry-run must exclude a low-yield game while saying why.
out=$("$PROG" --dry-run --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^would compress.*BadGame"; then
    bad "dry-run excludes low-yield games"
else
    ok "dry-run excludes low-yield games"
fi
if printf '%s' "$out" | grep -q "skipped as low yield"; then
    ok "dry-run explains why it skipped them"
else
    bad "dry-run explains why it skipped them"
fi

json_out=$("$PROG" --status --json --no-color 2>/dev/null)
check_contains "JSON status contains full game names" '"name":"GoodGame"' "$json_out"
check_contains "JSON status preserves library paths with spaces" "\"path\":\"$L/GoodGame\"" "$json_out"
check_contains "JSON status includes machine-sized byte counts" '"size_bytes":' "$json_out"
if command -v python3 >/dev/null 2>&1; then
    if printf '%s\n' "$json_out" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        ok "JSON status parses as valid JSON"
    else
        bad "JSON status parses as valid JSON" "$json_out"
    fi
fi

# --history and --stats read the same database, so a record seeded above must be
# visible in both. This is what makes the numbers trustworthy: they come from the
# file, not from a fresh guess.
hist_out=$("$PROG" --history --no-color 2>/dev/null)
check_contains "--history lists a recorded game" "GoodGame" "$hist_out"
check_contains "--history shows the saving" "25M" "$hist_out"
check_contains "--history reports a record count" "record(s)" "$hist_out"
stats_out=$("$PROG" --stats --no-color 2>/dev/null)
check_contains "--stats counts the games" "Games compressed:" "$stats_out"
# 50 MiB raw -> 25 MiB saved from GoodGame, plus 1 MiB from BadGame = 26 MiB.
check_contains "--stats sums the space saved" "26" "$stats_out"
check_not_contains "--history is read-only and writes no ANSI when asked" $'\033' "$hist_out"

# --benchmark reports the same records, sorted best-first, and its summary must
# be arithmetic on that data rather than a guess. GoodGame saves 50%, BadGame 1%.
bench_out=$("$PROG" --benchmark --no-color 2>/dev/null)
check_contains "--benchmark lists a game" "GoodGame" "$bench_out"
check_contains "--benchmark reports the best game" "GoodGame" "$bench_out"
check_contains "--benchmark names the best ratio" "50.0%" "$bench_out"
check_contains "--benchmark counts games over 50%" "Games saving over 50%:" "$bench_out"
# GoodGame sorts above BadGame because 50% > 1%.
gg_line=$(printf '%s\n' "$bench_out" | grep -n '^GoodGame' | cut -d: -f1)
bg_line=$(printf '%s\n' "$bench_out" | grep -n '^BadGame' | cut -d: -f1)
if [ -n "$gg_line" ] && [ -n "$bg_line" ] && [ "$gg_line" -lt "$bg_line" ]; then
    ok "--benchmark sorts the best game first"
else
    bad "--benchmark sorts the best game first" "$bench_out"
fi

bench_json=$("$PROG" --benchmark --json --no-color 2>/dev/null)
check_contains "--benchmark --json emits a summary" '"summary":' "$bench_json"
check_contains "--benchmark --json includes per-game ratios" '"ratio_pct":' "$bench_json"
if command -v python3 >/dev/null 2>&1; then
    if printf '%s\n' "$bench_json" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        ok "benchmark JSON parses as valid JSON"
    else
        bad "benchmark JSON parses as valid JSON" "$bench_json"
    fi
fi

cat > "$WORK/shim/notify-send" <<'SHIM'
#!/bin/sh
printf '<%s>\n' "$@" > "$NOTIFY_LOG"
SHIM
chmod +x "$WORK/shim/notify-send"
NOTIFY_LOG="$WORK/notification.txt" \
    "$PROG" --notify >/dev/null 2>&1
if grep -q '1 game(s) changed' "$WORK/notification.txt" && \
   grep -q 'GoodGame' "$WORK/notification.txt" && \
   ! grep -q 'BadGame' "$WORK/notification.txt"; then
    ok "--notify reports actionable games and omits low-yield games"
else
    bad "--notify reports actionable games and omits low-yield games" \
        "$(cat "$WORK/notification.txt" 2>/dev/null || echo no notification)"
fi

# A game with no history has no prediction and must never be held back.
printf '%s|NewGame|1000000000|50M|49M|1M|1|50|49\n' "$L/NewGame" > "$S"
mkdir -p "$L/NewGame"
dd if=/dev/zero of="$L/NewGame/data" bs=1M count=50 2>/dev/null
rm -f "$S"
out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^NewGame .*LOW YIELD"; then
    bad "a game with no history is not judged"
else
    ok "a game with no history is not judged"
fi
rm -rf "$L/NewGame"

# ---------------------------------------------------------------------------
group "Name truncation"

# LC_ALL=C is forced, so a naive cut can land inside a multi-byte character and
# leave half a glyph on screen. These are the exact strings that broke before.
if declare -f fit_name >/dev/null 2>&1; then
    check_fit() {
        fit_name "$2" "$3"
        if [ "$NAME_FIT" = "$1" ]; then
            ok "fit_name: $4"
        else
            bad "fit_name: $4" "got '$NAME_FIT' want '$1'"
        fi
    }

    check_fit "Short" "Short" 40 "a short name is left alone"
    check_fit "123456789" "123456789" 10 "a name that fits is left alone"
    check_fit "1234567890" "1234567890" 10 "an exactly-fitting name is left alone"
    check_fit "123456789…" "12345678901" 10 "one character over is clamped"
    check_fit "123456789…" "123456789012345678901234567890" 10 "a long name is clamped"

    # Multi-byte: 'é' is 2 bytes, 'Ō' 2, each CJK glyph 3. A byte-indexed cut
    # here produces invalid UTF-8, which the iconv check below would catch.
    check_fit "CaféSupe…" "$(printf 'Caf\303\251SuperLongGameName')" 10 \
        "a 2-byte character is not split"
    check_fit "ŌkamiSup…" "$(printf '\305\214kamiSuperLongGameName')" 10 \
        "a leading 2-byte character is not split"
    check_fit "日本語…" "$(printf '\346\227\245\346\234\254\350\252\236\343\202\262')" 10 \
        "3-byte characters are not split"
    check_fit "日本語" "$(printf '\346\227\245\346\234\254\350\252\236')" 10 \
        "a name that already fits is not marked as truncated"

    # Whatever it does, the result must still be valid UTF-8 and must fit.
    # Under LC_ALL=C ${#s} is bytes, so count characters properly: every UTF-8
    # character has exactly one byte that is not a continuation byte (0x80-0xBF).
    disp_width() {
        printf '%s' "$1" | awk '{
            for (i = 1; i <= length($0); i++) {
                c = substr($0, i, 1)
                if (c < "\200" || c > "\277") n++
            }
        }
        END { print n + 0 }'
    }
    for probe in "$(printf '\346\227\245\346\234\254\350\252\236\343\202\274\343\203\274\343\203\240LongTitle')" \
                 "$(printf 'Caf\303\251')" "plain" "" ; do
        for w in 1 2 3 5 10 40 ; do
            fit_name "$probe" "$w"
            if ! printf '%s' "$NAME_FIT" | iconv -f UTF-8 -t UTF-8 >/dev/null 2>&1; then
                bad "fit_name keeps output valid UTF-8" "broke on width $w: '$NAME_FIT'"
                continue
            fi
            got=$(disp_width "$NAME_FIT")
            if [ "$got" -gt "$w" ]; then
                bad "fit_name respects the width budget" \
                    "budget $w, produced $got columns: '$NAME_FIT'"
            fi
        done
    done
    ok "fit_name never splits a character and never overflows the budget"
else
    bad "fit_name is defined"
fi

# ---------------------------------------------------------------------------
group "Running games are never defragmented"

# Detection asks the kernel whether any live process has a file from the game
# directory mapped in. So the test puts a real long-lived process in there:
# a copy of sleep, executing. Steam's appmanifest StateFlags is deliberately
# not consulted, because the published references disagree on which bit means
# "running" and guessing wrong defragments a live game.
cp "$(command -v sleep)" "$L/GoodGame/runner" 2>/dev/null
"$L/GoodGame/runner" 30 >/dev/null 2>&1 &
RUN_PID=$!
escaped_L=${L// /\\040}
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
    sleep 0.2
    grep -Fq "$escaped_L/GoodGame/" "/proc/$RUN_PID/maps" 2>/dev/null && break
done

out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "^GoodGame .*RUNNING"; then
    ok "a game with a live process is reported as RUNNING"
else
    bad "a game with a live process is reported as RUNNING" \
        "got: $(printf '%s' "$out" | grep '^GoodGame' || echo none)"
fi
if printf '%s' "$out" | grep -q "^BadGame .*RUNNING"; then
    bad "a game with no live process is not reported as RUNNING"
else
    ok "a game with no live process is not reported as RUNNING"
fi
check_contains "--status counts the running games" "running: 1" "$out"

# The footer has to account for every discovered game exactly once. This is the
# arithmetic that catches a state being tallied twice or not at all, which is
# what made the TUI header and the --status footer disagree about the same word.
footer=$(printf '%s' "$out" | sed -n 's/^libraries: .*games: \([0-9]*\).*/\1/p')
n_games=$(printf '%s' "$out" | grep -c '^GoodGame\|^BadGame')
pending=$(printf '%s' "$out" | sed -n 's/.*pending: \([0-9]*\).*/\1/p')
comp=$(printf '%s' "$out" | sed -n 's/.*compressed: \([0-9]*\).*/\1/p')
low=$(printf '%s' "$out" | sed -n 's/.*low-yield: \([0-9]*\).*/\1/p')
run=$(printf '%s' "$out" | sed -n 's/.*running: \([0-9]*\).*/\1/p')
n_sum=$(( pending + comp + low + run ))
if [ "$footer" = "$n_sum" ] && [ "$n_sum" = "$n_games" ]; then
    ok "the status footer accounts for every game exactly once"
else
    bad "the status footer accounts for every game exactly once" \
        "games=$n_games footer=$footer sum=$n_sum (pending=$pending compressed=$comp low-yield=$low running=$run)"
fi

out=$("$PROG" --dry-run --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "would compress.*GoodGame"; then
    bad "dry-run refuses to queue a running game"
else
    ok "dry-run refuses to queue a running game"
fi
check_contains "dry-run says why it skipped the running game" "game is running" "$out"
check_contains "dry-run summarises the running skips" "skipped because they are running" "$out"

# The unit behind the end-to-end check above.
if declare -f game_is_running >/dev/null 2>&1; then
    LIBS=("$L")
    discover_running_games
    if game_is_running "$L/GoodGame"; then
        ok "game_is_running: positive"
    else
        bad "game_is_running: positive" "the runner is still alive at this point"
    fi
    if game_is_running "$L/NoSuchGameAnywhere"; then
        bad "game_is_running: negative"
    else
        ok "game_is_running: negative"
    fi
    if game_is_running ""; then
        bad "game_is_running: empty name is not running"
    else
        ok "game_is_running: empty name is not running"
    fi
else
    bad "game_is_running is defined"
fi

kill "$RUN_PID" 2>/dev/null
wait "$RUN_PID" 2>/dev/null
rm -f "$L/GoodGame/runner"

out=$("$PROG" --status --no-color 2>/dev/null)
if printf '%s' "$out" | grep -q "RUNNING"; then
    bad "the game returns to normal once the process exits" \
        "$(printf '%s' "$out" | grep RUNNING || true)"
else
    ok "the game returns to normal once the process exits"
fi

# ---------------------------------------------------------------------------
group "--balance"

# btrfs is not needed to get this far, but require_btrfs runs before the
# confirmation, so give it something to find.
cat > "$WORK/shim/btrfs" <<'SHIM'
#!/bin/sh
echo "SHIM btrfs $*" >&2
exit 0
SHIM
chmod +x "$WORK/shim/btrfs"

# stdin from /dev/null so the TTY test does not depend on how the suite itself
# was invoked.
out=$("$PROG" --balance < /dev/null 2>&1); rc=$?
check_rc "--balance refuses to run without a terminal" 1 $rc
check_contains "--balance explains why it stopped" "needs a terminal" "$out"
check_contains "--balance still prints the exact command" "btrfs balance start -m" "$out"
check_not_contains "--balance ran nothing unattended" "SHIM btrfs" "$out"

PATH="$OLD_PATH"

# ---------------------------------------------------------------------------
group "Community ratio table"

# The parser has to work on the exact shape the repository stores and on the
# exact shape --export-ratios produces, because those must round-trip.
RATIOS_FIXTURE="$WORK/games.json"
cat > "$RATIOS_FIXTURE" <<'JSON'
{
  "schema": 1,
  "games": {
    "Big Gains": {
      "zstd1": { "pct": 72.5, "samples": 3, "min": 70.0, "max": 74.0 },
      "zstd6": { "pct": 61.0, "samples": 1, "min": 61.0, "max": 61.0 }
    },
    "No Gains": {
      "zstd1": { "pct": 2.0, "samples": 1, "min": 2.0, "max": 2.0 }
    }
  }
}
JSON

parsed=$(ratios_parse "$RATIOS_FIXTURE" 2>/dev/null)
check_contains "ratios_parse: reads a game and level" "Big Gains	zstd1	72.5	3" "$parsed"
check_contains "ratios_parse: reads a second level of the same game" "Big Gains	zstd6	61.0	1" "$parsed"
check_contains "ratios_parse: reads a second game" "No Gains	zstd1	2.0	1" "$parsed"
count=$(printf '%s\n' "$parsed" | grep -c .)
if [ "$count" -eq 3 ]; then
    ok "ratios_parse: exactly one row per game/level pair"
else
    bad "ratios_parse: exactly one row per game/level pair" "got $count rows"
fi

# A level-1 mount must never be told what a level-6 measurement saw.
RATIOS_CACHE="$RATIOS_FIXTURE"
HINT_RATIO=()
load_ratios_hint 1 >/dev/null 2>&1
if [ "${HINT_RATIO[Big Gains]:-}" = "72.5" ]; then
    ok "ratios hint: a matching level is used"
else
    bad "ratios hint: a matching level is used" "got '${HINT_RATIO[Big Gains]:-}'"
fi
HINT_RATIO=()
load_ratios_hint 3 >/dev/null 2>&1
if [ -z "${HINT_RATIO[Big Gains]:-}" ]; then
    ok "ratios hint: a level with no data yields no hint"
else
    bad "ratios hint: a level with no data yields no hint" "got '${HINT_RATIO[Big Gains]:-}'"
fi
RATIOS_CACHE="$HOME/.config/btrfs-game-compressor/ratios_hint.json"

# Malformed input must produce no rows, so callers treat it as "no table".
printf 'this is not json at all\n' > "$WORK/bad.json"
if [ -z "$(ratios_parse "$WORK/bad.json" 2>/dev/null)" ]; then
    ok "ratios_parse: malformed input yields no rows"
else
    bad "ratios_parse: malformed input yields no rows"
fi

# --export-ratios must emit something the parser and any JSON reader accept.
# The program runs as a child, so it reads its state from $HOME, not from this
# shell's STATE_FILE. Seed that path directly.
STATE_FILE="$HOME/.local/state/btrfs-game-compressor/compressed_games.db"
mkdir -p "$(dirname "$STATE_FILE")"
printf '# hdr\n/libs/Factorio|Factorio|1|2.10GiB|1.60GiB|512 MB|512|2150|1638\n/libs/Celeste|Celeste|1|1.00GiB|200M|824 MB|824|1024|200\n' > "$STATE_FILE"
exported=$("$PROG" --export-ratios 2>/dev/null)
check_contains "--export-ratios emits a games object" '"games": {' "$exported"
check_contains "--export-ratios names a measured game" 'Factorio' "$exported"
if command -v python3 >/dev/null 2>&1; then
    if printf '%s\n' "$exported" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        ok "--export-ratios output is valid JSON"
    else
        bad "--export-ratios output is valid JSON" "$exported"
    fi
fi
# What it emits must parse back to the same values, or the round trip that
# contributors rely on is broken.
printf '%s\n' "$exported" > "$WORK/exported.json"
round=$(ratios_parse "$WORK/exported.json" 2>/dev/null)
check_contains "--export-ratios round-trips through the parser" "Factorio" "$round"

# A game name with a quote must survive export, because jesc/unesc have to pair.
printf '# hdr\n/a/Q|Quote\\"Game|1|10M|5M|5M|5|10|5\n' > "$STATE_FILE"
quoted_export=$("$PROG" --export-ratios 2>/dev/null)
if command -v python3 >/dev/null 2>&1; then
    if printf '%s\n' "$quoted_export" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        ok "--export-ratios escapes a quote in a game name"
    else
        bad "--export-ratios escapes a quote in a game name" "$quoted_export"
    fi
fi
: > "$STATE_FILE"

# Import must refuse a file that yields no entries, and must never cache garbage.
out=$("$PROG" --import-ratios "$WORK/bad.json" 2>&1); rc=$?
check_rc "--import-ratios rejects a file with no entries" 1 $rc
check_contains "--import-ratios explains the rejection" "not a usable ratios file" "$out"
if [ ! -e "$HOME/.config/btrfs-game-compressor/ratios_hint.json" ]; then
    ok "--import-ratios caches nothing when the file is rejected"
else
    bad "--import-ratios caches nothing when the file is rejected"
fi

out=$("$PROG" --import-ratios "$RATIOS_FIXTURE" 2>&1); rc=$?
check_rc "--import-ratios accepts a valid file" 0 $rc
check_contains "--import-ratios reports the import" "Imported 3 entries" "$out"

out=$("$PROG" --ratios --no-color 2>/dev/null)
check_contains "--ratios lists the imported table" "Big Gains" "$out"
out=$("$PROG" --ratios "Big Gains" --no-color 2>/dev/null)
check_contains "--ratios for one game shows its level" "zstd1" "$out"
out=$("$PROG" --ratios "Absent Game" --no-color 2>/dev/null)
check_contains "--ratios for an unknown game says so" "No entry for" "$out"

out=$("$PROG" --ratios --json 2>/dev/null)
check_contains "--ratios --json emits ratios" '"ratios":[' "$out"
if command -v python3 >/dev/null 2>&1; then
    if printf '%s\n' "$out" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        ok "--ratios --json parses as valid JSON"
    else
        bad "--ratios --json parses as valid JSON" "$out"
    fi
fi
# The hint cache must not outlive the test into a later group's expectations.
rm -f "$HOME/.config/btrfs-game-compressor/ratios_hint.json"

# ---------------------------------------------------------------------------
group "Community hints in status and notify"

# A hint is shown only for a game this machine has not measured, and only at the
# library's own zstd level. Pin the level to 1 so the test does not depend on how
# the host happens to mount its root filesystem.
HL="$WORK/hint library/steamapps/common"
mkdir -p "$HL/Hinted" "$HL/OnlySix" "$HL/Tiny"
dd if=/dev/zero of="$HL/Hinted/data"  bs=1M count=50 2>/dev/null
dd if=/dev/zero of="$HL/OnlySix/data" bs=1M count=50 2>/dev/null
dd if=/dev/zero of="$HL/Tiny/data"    bs=1M count=50 2>/dev/null
echo "$HL" > "$HOME/.config/btrfs-game-compressor/custom_libraries.txt"
printf '1\n' > "$HOME/.config/btrfs-game-compressor/compress_level"
: > "$HOME/.local/state/btrfs-game-compressor/compressed_games.db"

cat > "$WORK/hint.json" <<'JSON'
{
  "schema": 1,
  "games": {
    "Hinted":  { "zstd1": { "pct": 72.5, "samples": 2, "min": 70.0, "max": 75.0 } },
    "OnlySix": { "zstd6": { "pct": 61.0, "samples": 1, "min": 61.0, "max": 61.0 } },
    "Tiny":    { "zstd1": { "pct": 2.0,  "samples": 1, "min": 2.0,  "max": 2.0 } }
  }
}
JSON
"$PROG" --import-ratios "$WORK/hint.json" >/dev/null 2>&1

PATH="$WORK/shim:$PATH"
out=$("$PROG" --status --no-color 2>/dev/null)
check_contains "status shows a community hint at the matching level" "table says ~72.5% (zstd1)" "$out"
check_not_contains "status hides a hint measured at a different level" "61.0" "$out"

json_hint=$("$PROG" --status --json --no-color 2>/dev/null)
check_contains "JSON status carries the community hint" '"community_hint_pct":72.5' "$json_hint"
check_contains "JSON status names the hint level" '"community_hint_level":"zstd1"' "$json_hint"
check_not_contains "JSON status omits a mismatched hint" 'community_hint_pct":61' "$json_hint"
if command -v python3 >/dev/null 2>&1; then
    if printf '%s\n' "$json_hint" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        ok "JSON status with a hint parses as valid JSON"
    else
        bad "JSON status with a hint parses as valid JSON" "$json_hint"
    fi
fi

check_contains "status flags a table-predicted low-yield game" \
    "table says ~2.0% (zstd1) - likely low yield" "$out"
check_contains "JSON flags a table-predicted low-yield game" \
    '"community_hint_low_yield":true' "$json_hint"
check_contains "JSON marks a good hint as not low yield" \
    '"community_hint_low_yield":false' "$json_hint"

# The advisory must not turn into a block: the game is still listed as one that
# would be compressed, with the warning attached.
dry_hint=$("$PROG" --dry-run --no-color 2>/dev/null)
check_contains "dry-run advises a table-predicted low-yield game" \
    "table: ~2.0% - likely low yield" "$dry_hint"
check_contains "dry-run explains the community advisory" \
    "community table expects under 5%" "$dry_hint"
if printf '%s' "$dry_hint" | grep -q '^would compress.*Tiny'; then
    ok "an advisory does not hold a game back"
else
    bad "an advisory does not hold a game back" "$dry_hint"
fi

NOTIFY_LOG="$WORK/hint-notification.txt" "$PROG" --notify >/dev/null 2>&1
if grep -q 'Hinted (~72.5% at zstd1)' "$WORK/hint-notification.txt"; then
    ok "notify includes the hint for a matching level"
else
    bad "notify includes the hint for a matching level" \
        "$(cat "$WORK/hint-notification.txt" 2>/dev/null || echo no notification)"
fi
if grep -q 'OnlySix (~61' "$WORK/hint-notification.txt"; then
    bad "notify omits a hint measured at a different level"
else
    ok "notify omits a hint measured at a different level"
fi
PATH="$OLD_PATH"

rm -f "$HOME/.config/btrfs-game-compressor/ratios_hint.json" \
      "$HOME/.config/btrfs-game-compressor/compress_level"

# ---------------------------------------------------------------------------
group "Read-only modes skip library discovery"

# --ratios, --export-ratios and --render-ratios read the ratio table and the
# state file only. Walking a 250k-file library for them is pure waste, so a
# find(1) that fails loudly proves the walk never starts.
mkdir -p "$WORK/shim-find"
cat > "$WORK/shim-find/find" <<'SHIM'
#!/bin/sh
echo "FIND CALLED" >&2
exit 1
SHIM
chmod +x "$WORK/shim-find/find"

out=$(PATH="$WORK/shim-find:$PATH" "$PROG" --render-ratios 2>&1); rc=$?
check_rc "--render-ratios does not require discovery" 0 $rc
check_not_contains "--render-ratios does not walk a library" "FIND CALLED" "$out"
check_contains "--render-ratios still renders the table" "| Game | Level | Saving | Samples |" "$out"

out=$(PATH="$WORK/shim-find:$PATH" "$PROG" --ratios --no-color 2>&1); rc=$?
check_rc "--ratios does not require discovery" 0 $rc
check_not_contains "--ratios does not walk a library" "FIND CALLED" "$out"

: > "$HOME/.local/state/btrfs-game-compressor/compressed_games.db"
out=$(PATH="$WORK/shim-find:$PATH" "$PROG" --export-ratios 2>&1); rc=$?
check_rc "--export-ratios does not require discovery" 0 $rc
check_not_contains "--export-ratios does not walk a library" "FIND CALLED" "$out"

# The table must be found in the layout `make install` uses
# (bin/../share/btrfs-game-compressor), not only in a git checkout.
INST="$WORK/fakeprefix"
mkdir -p "$INST/bin" "$INST/share/btrfs-game-compressor/ratios"
cp "$PROG" "$INST/bin/btrfs-game-compressor"
chmod +x "$INST/bin/btrfs-game-compressor"
printf '{"schema":1,"games":{"Installed Game":{"zstd1":{"pct":33.3,"samples":1,"min":33.3,"max":33.3}}}}\n' \
    > "$INST/share/btrfs-game-compressor/ratios/games.json"
out=$(HOME="$WORK/installed-home" "$INST/bin/btrfs-game-compressor" --ratios --no-color 2>&1); rc=$?
check_rc "an installed layout finds the shipped table" 0 $rc
check_contains "the installed table is the one that is read" "Installed Game" "$out"

# A package manager puts a symlink on PATH pointing into the real install
# directory; the table ships next to the real file, not the symlink.
SYM="$WORK/symroot"
mkdir -p "$SYM/real/bin" "$SYM/real/share/btrfs-game-compressor/ratios" "$SYM/bin"
cp "$PROG" "$SYM/real/bin/btrfs-game-compressor"
chmod +x "$SYM/real/bin/btrfs-game-compressor"
printf '{"schema":1,"games":{"Symlinked Game":{"zstd1":{"pct":44.4,"samples":1,"min":44.4,"max":44.4}}}}\n' \
    > "$SYM/real/share/btrfs-game-compressor/ratios/games.json"
ln -s ../real/bin/btrfs-game-compressor "$SYM/bin/btrfs-game-compressor"
out=$(HOME="$WORK/symlink-home" "$SYM/bin/btrfs-game-compressor" --ratios --no-color 2>&1); rc=$?
check_rc "a symlinked install finds the shipped table" 0 $rc
check_contains "the symlink target's table is the one read" "Symlinked Game" "$out"

# A maintainer's personal import must not leak into the committed table: the
# renderer has to read ratios/games.json, never the imported cache.
cat > "$WORK/imported-only.json" <<'JSON'
{"schema":1,"games":{"Imported Only":{"zstd1":{"pct":9.9,"samples":1,"min":9.9,"max":9.9}}}}
JSON
"$PROG" --import-ratios "$WORK/imported-only.json" >/dev/null 2>&1
out=$("$PROG" --render-ratios 2>&1); rc=$?
check_rc "--render-ratios ignores the imported cache" 0 $rc
check_contains "--render-ratios renders the committed table" "9 Kings" "$out"
check_not_contains "--render-ratios does not render a personal import" "Imported Only" "$out"
rm -f "$HOME/.config/btrfs-game-compressor/ratios_hint.json"

# And with no table anywhere it must fail without leaving a half-written file.
mkdir -p "$WORK/noprefix/bin"
cp "$PROG" "$WORK/noprefix/bin/btrfs-game-compressor"
chmod +x "$WORK/noprefix/bin/btrfs-game-compressor"
out=$(HOME="$WORK/noprefix-home" "$WORK/noprefix/bin/btrfs-game-compressor" --render-ratios 2>&1); rc=$?
check_rc "--render-ratios fails cleanly with no table" 1 $rc
check_contains "--render-ratios reports a missing table" "No ratio table found." "$out"
check_not_contains "--render-ratios prints a real newline" 'found.\n' "$out"

# ---------------------------------------------------------------------------
group "Self-update"

# A package-managed copy must never be rewritten in place.
if declare -f package_managed_path >/dev/null 2>&1; then
    for p in /usr/bin/btrfs-game-compressor \
             /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bgc/bin/btrfs-game-compressor \
             /home/linuxbrew/.linuxbrew/Cellar/btrfs-game-compressor/0.1.0/bin/btrfs-game-compressor; do
        if package_managed_path "$p"; then ok "package_managed_path: refuses $p"
        else bad "package_managed_path: refuses $p"; fi
    done
    if package_managed_path "$WORK/selfup/bin/btrfs-game-compressor"; then
        bad "package_managed_path: allows a user install" "$WORK/selfup/bin/btrfs-game-compressor"
    else
        ok "package_managed_path: allows a user install"
    fi
else
    bad "package_managed_path is defined"
fi

# A fake curl serves the GitHub release API plus a newer archive and its
# SHA256SUMS, so --check-update and --self-update can be tested without network.
UPDF="$WORK/update-fixture"
NEWVER=9.9.9
mkdir -p "$UPDF/serve" "$UPDF/build/btrfs-game-compressor-$NEWVER" "$WORK/update-bin"
cat > "$UPDF/build/btrfs-game-compressor-$NEWVER/btrfs-game-compressor" <<'SCR'
#!/bin/sh
echo "new-version-fixture"
SCR
tar -czf "$UPDF/serve/btrfs-game-compressor-$NEWVER.tar.gz" -C "$UPDF/build" "btrfs-game-compressor-$NEWVER"
(cd "$UPDF/serve" && sha256sum "btrfs-game-compressor-$NEWVER.tar.gz" > SHA256SUMS)
cat > "$WORK/update-bin/curl" <<'CURL'
#!/bin/sh
out=""; url=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; out="$1" ;;
        --max-time) shift ;;
        -*) ;;
        *) url="$1" ;;
    esac
    shift
done
case "$url" in
    *api.github.com*)
        body='{"tag_name":"v'"${FAKE_TAG:-9.9.9}"'"}'
        if [ -n "$out" ]; then printf '%s\n' "$body" > "$out"; else printf '%s\n' "$body"; fi
        exit 0 ;;
esac
file=$(basename "$url")
if [ -f "$UPDATE_FIXTURE/serve/$file" ]; then cp "$UPDATE_FIXTURE/serve/$file" "$out"; exit 0; fi
exit 1
CURL
chmod +x "$WORK/update-bin/curl"

mkdir -p "$WORK/selfup/bin"
cp "$PROG" "$WORK/selfup/bin/btrfs-game-compressor"
chmod +x "$WORK/selfup/bin/btrfs-game-compressor"
SUHOME="$WORK/selfup-home"

out=$(PATH="$WORK/update-bin:$PATH" UPDATE_FIXTURE="$UPDF" FAKE_TAG="$NEWVER" HOME="$SUHOME" \
    "$WORK/selfup/bin/btrfs-game-compressor" --check-update 2>&1); rc=$?
check_rc "--check-update succeeds" 0 "$rc"
check_contains "--check-update reports the newer release" "$NEWVER is available" "$out"
if grep -q 'new-version-fixture' "$WORK/selfup/bin/btrfs-game-compressor"; then
    bad "--check-update changes nothing" "the script was rewritten"
else
    ok "--check-update changes nothing"
fi

out=$(PATH="$WORK/update-bin:$PATH" UPDATE_FIXTURE="$UPDF" FAKE_TAG="$NEWVER" HOME="$SUHOME" \
    "$WORK/selfup/bin/btrfs-game-compressor" --self-update 2>&1); rc=$?
check_rc "--self-update succeeds against a verified release" 0 "$rc"
check_contains "--self-update reports what it did" "Updated btrfs-game-compressor 0.1.0 -> $NEWVER" "$out"
if grep -q 'new-version-fixture' "$WORK/selfup/bin/btrfs-game-compressor"; then
    ok "--self-update replaced the file"
else
    bad "--self-update replaced the file" "$(head -n1 "$WORK/selfup/bin/btrfs-game-compressor")"
fi

# A tampered archive must be refused and the running copy left alone.
printf 'tamper' >> "$UPDF/serve/btrfs-game-compressor-$NEWVER.tar.gz"
cp "$PROG" "$WORK/selfup/bin/btrfs-game-compressor"
out=$(PATH="$WORK/update-bin:$PATH" UPDATE_FIXTURE="$UPDF" FAKE_TAG="$NEWVER" HOME="$SUHOME" \
    "$WORK/selfup/bin/btrfs-game-compressor" --self-update 2>&1); rc=$?
check_rc "--self-update rejects a tampered archive" 1 "$rc"
check_contains "--self-update explains the checksum failure" "checksum mismatch" "$out"
if grep -q 'new-version-fixture' "$WORK/selfup/bin/btrfs-game-compressor"; then
    bad "--self-update leaves a tampered install untouched" "the fixture script was written"
else
    ok "--self-update leaves a tampered install untouched"
fi

# ---------------------------------------------------------------------------
group "Uninstall"

# --yes skips the confirmation so the removal can be tested; the plain flag must
# still refuse when there is no terminal to confirm at.
UN="$WORK/uninst"
mkdir -p "$UN/bin" "$UN/share/man/man1" \
         "$UN/share/btrfs-game-compressor/ratios"
cp "$PROG" "$UN/bin/btrfs-game-compressor"
chmod +x "$UN/bin/btrfs-game-compressor"
printf '.TH TEST 1\n' > "$UN/share/man/man1/btrfs-game-compressor.1"
printf '{"schema":1,"games":{}}\n' > "$UN/share/btrfs-game-compressor/ratios/games.json"
printf '# Community compression ratios\n' > "$UN/share/btrfs-game-compressor/GAMES.md"
UNHOME="$WORK/uninst-home"

out=$("$UN/bin/btrfs-game-compressor" --uninstall < /dev/null 2>&1); rc=$?
check_rc "--uninstall refuses without a terminal" 1 "$rc"
check_contains "--uninstall says why it stopped" "needs a terminal" "$out"
[ -f "$UN/bin/btrfs-game-compressor" ] &&
    ok "--uninstall changed nothing when it refused" ||
    bad "--uninstall changed nothing when it refused"

out=$(HOME="$UNHOME" "$UN/bin/btrfs-game-compressor" --uninstall --yes 2>&1); rc=$?
check_rc "--uninstall --yes succeeds" 0 "$rc"
check_contains "--uninstall reports what it did" "Removed btrfs-game-compressor" "$out"
[ ! -e "$UN/bin/btrfs-game-compressor" ] &&
    ok "--uninstall removes the script" || bad "--uninstall removes the script"
[ ! -e "$UN/share/man/man1/btrfs-game-compressor.1" ] &&
    ok "--uninstall removes the manpage" || bad "--uninstall removes the manpage"
[ ! -e "$UN/share/btrfs-game-compressor/ratios/games.json" ] &&
    ok "--uninstall removes the ratio table" || bad "--uninstall removes the ratio table"
[ -d "$UNHOME/.config/btrfs-game-compressor" ] &&
    ok "--uninstall keeps the configuration" || bad "--uninstall keeps the configuration"

out=$("$PROG" --yes 2>&1); rc=$?
check_rc "--yes without --uninstall is rejected" 2 "$rc"
check_contains "--yes rejection explains the rule" "only applies to --uninstall" "$out"

# ---------------------------------------------------------------------------
group "Ratio merge tool"

# ratios/merge.py folds an --export-ratios document into the community table.
if command -v python3 >/dev/null 2>&1 && [ -f "$REPO_ROOT/ratios/merge.py" ]; then
    cp "$REPO_ROOT/ratios/games.json" "$WORK/merge-target.json"
    cat > "$WORK/merge-in.json" <<'JSON'
{"schema":1,"games":{
  "MechHavoc":{"zstd1":{"pct":80.0,"samples":2,"min":79.0,"max":81.0}},
  "Brand New Game":{"zstd1":{"pct":50.0,"samples":3,"min":45.0,"max":55.0}}
}}
JSON
    out=$(python3 "$REPO_ROOT/ratios/merge.py" "$WORK/merge-target.json" "$WORK/merge-in.json" 2>&1); rc=$?
    check_rc "ratio merge succeeds" 0 $rc
    check_contains "ratio merge reports what it did" "1 new, 1 updated" "$out"
    got=$(python3 - "$WORK/merge-target.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
m=d["games"]["MechHavoc"]["zstd1"]
print(f'{m["pct"]} {m["samples"]} {m["min"]} {m["max"]} {"Brand New Game" in d["games"]}')
PY
)
    [ "$got" = "85.0 4 79.0 90.0 True" ] &&
        ok "ratio merge averages samples and widens the range" ||
        bad "ratio merge averages samples and widens the range" "got '$got'"

    printf 'not json' > "$WORK/merge-bad.json"
    out=$(python3 "$REPO_ROOT/ratios/merge.py" "$WORK/merge-target.json" "$WORK/merge-bad.json" 2>&1); rc=$?
    check_rc "ratio merge rejects invalid JSON" 1 "$rc"
else
    ok "ratio merge tool skipped (python3 not available)"
fi

# ---------------------------------------------------------------------------
group "TUI source hygiene"

# The main loop runs at top level, not inside a function, so a stray `local`
# there prints "can only be used in a function" on every redraw and writes the
# message over the interface. Guard against one creeping back in.
stray_local=$(sed -n '/^# --- Main Program Loop ---/,$p' "$PROG" | grep -cE '^[[:space:]]*local[[:space:]]')
if [ "$stray_local" -eq 0 ]; then
    ok "the TUI main loop has no top-level local declarations"
else
    bad "the TUI main loop has no top-level local declarations" "$stray_local found"
fi

# ---------------------------------------------------------------------------
group "Verified release installer"

REL="$WORK/release-fixture"
VERSION=0.1.0
ARCHIVE="btrfs-game-compressor-$VERSION.tar.gz"
mkdir -p "$REL/btrfs-game-compressor-$VERSION/ratios" "$WORK/install-bin"
cat > "$REL/btrfs-game-compressor-$VERSION/btrfs-game-compressor" <<'SCRIPT'
#!/bin/sh
echo release-fixture
SCRIPT
printf '.TH TEST 1\n' > "$REL/btrfs-game-compressor-$VERSION/btrfs-game-compressor.1"
printf '{"schema":1,"games":{}}\n' > "$REL/btrfs-game-compressor-$VERSION/ratios/games.json"
printf '# Community compression ratios\n' > "$REL/btrfs-game-compressor-$VERSION/GAMES.md"
tar -czf "$REL/$ARCHIVE" -C "$REL" "btrfs-game-compressor-$VERSION"
(cd "$REL" && sha256sum "$ARCHIVE" > SHA256SUMS)
cat > "$WORK/install-bin/curl" <<'CURL'
#!/bin/sh
out=""; url=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; out="$1" ;;
        -*) ;;
        *) url="$1" ;;
    esac
    shift
done
case "$url" in
    *api.github.com*)
        # The installer asks for the latest tag when no version is pinned.
        body=$(printf '{\n  "tag_name": "v%s"\n}\n' "${RELEASE_VERSION:-0.0.0}")
        if [ -n "$out" ]; then printf '%s' "$body" > "$out"; else printf '%s' "$body"; fi
        exit 0 ;;
esac
file=$(basename "$url")
cp "$RELEASE_FIXTURE/$file" "$out"
CURL
chmod +x "$WORK/install-bin/curl"
INSTALL_HOME="$WORK/install-home"
out=$(RELEASE_FIXTURE="$REL" HOME="$INSTALL_HOME" PREFIX="$INSTALL_HOME/.local" \
    BTRFS_GAME_COMPRESSOR_REPO=example/project BTRFS_GAME_COMPRESSOR_VERSION="$VERSION" \
    PATH="$WORK/install-bin:$PATH" sh "$REPO_ROOT/install.sh" 2>&1); rc=$?
check_rc "installer accepts a valid release checksum" 0 "$rc"
check_contains "installer installs the release executable" "installed $INSTALL_HOME/.local/bin/btrfs-game-compressor" "$out"
[ -f "$INSTALL_HOME/.local/share/man/man1/btrfs-game-compressor.1" ] &&
    ok "installer installs the manpage" || bad "installer installs the manpage"
[ -f "$INSTALL_HOME/.local/share/btrfs-game-compressor/ratios/games.json" ] &&
    ok "installer installs the ratio table" || bad "installer installs the ratio table"

# Re-running with no version pinned must resolve the latest release: that is what
# makes "run the one-liner again" an update instead of a reinstall of an old copy.
LATEST_HOME="$WORK/latest-install-home"
out=$(RELEASE_FIXTURE="$REL" RELEASE_VERSION="$VERSION" HOME="$LATEST_HOME" \
    PREFIX="$LATEST_HOME/.local" BTRFS_GAME_COMPRESSOR_REPO=example/project \
    PATH="$WORK/install-bin:$PATH" sh "$REPO_ROOT/install.sh" 2>&1); rc=$?
check_rc "installer resolves the latest release when none is pinned" 0 "$rc"
check_contains "installer reports the resolved version" "fetching btrfs-game-compressor $VERSION" "$out"

printf 'tamper' >> "$REL/$ARCHIVE"
BAD_HOME="$WORK/bad-install-home"
out=$(RELEASE_FIXTURE="$REL" HOME="$BAD_HOME" PREFIX="$BAD_HOME/.local" \
    BTRFS_GAME_COMPRESSOR_REPO=example/project BTRFS_GAME_COMPRESSOR_VERSION="$VERSION" \
    PATH="$WORK/install-bin:$PATH" sh "$REPO_ROOT/install.sh" 2>&1); rc=$?
check_rc "installer rejects a modified release archive" 1 "$rc"
check_contains "installer explains checksum failure" "checksum verification failed" "$out"
[ ! -e "$BAD_HOME/.local/bin/btrfs-game-compressor" ] &&
    ok "checksum failure installs nothing" || bad "checksum failure installs nothing"

# ---------------------------------------------------------------------------
printf '\n----------------------------------------\n'
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
