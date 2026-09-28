PREFIX      ?= /usr/local
BINDIR      ?= $(PREFIX)/bin
MANDIR      ?= $(PREFIX)/share/man/man1
DATADIR     ?= $(PREFIX)/share/btrfs-game-compressor
DESTDIR     ?=

PROG        := btrfs-game-compressor
VERSION     := $(shell sed -n 's/^VERSION="\(.*\)"/\1/p' $(PROG))
MANPAGE     := $(PROG).1
TESTDIR     := test
SYSTEMDDIR  := systemd

# MANPAGE has to be defined above FILES: `:=` expands at assignment time, so
# referring to it earlier would silently yield an empty string and drop the
# manpage from the release tarball.
FILES       := $(PROG) $(MANPAGE) README.md LICENSE CHANGELOG.md CONTRIBUTING.md \
               install.sh Makefile GAMES.md ratios

.PHONY: all check test lint syntax install uninstall service service-off package \
        ratios-doc ratios-merge clean help

all: help

help:
	@echo "btrfs-game-compressor $(VERSION)"
	@echo
	@echo "  make check              syntax + shellcheck + tests (same as CI)"
	@echo "  make test               behavioural tests only"
	@echo "  make lint               shellcheck only"
	@echo "  make install            install to $(BINDIR)"
	@echo "  make install PREFIX=~   install to ~/.local"
	@echo "  make service            enable the watch-only systemd user timer"
	@echo "  make uninstall          remove the installed files"
	@echo "  make ratios-doc         regenerate GAMES.md from ratios/games.json"
	@echo "  make package            build a release tarball in dist/"
	@echo "  make clean              remove build artifacts"

check: syntax lint test

syntax:
	@bash -n $(PROG) && echo "syntax: ok"

# Show every ShellCheck finding, but only fail on real errors. The script has a
# number of deliberate informational/style notes (literal backticks in printf,
# colour escapes in a format string, an intentionally split word list) that CI
# already treats as non-fatal, and this target matches that policy.
lint:
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -s bash -e SC1091 $(PROG) || true; \
		if shellcheck -s bash -e SC1091 -S error $(PROG); then \
			echo "shellcheck: ok (no errors)"; \
		else \
			echo "shellcheck: errors found" >&2; \
			exit 1; \
		fi; \
	else \
		echo "shellcheck: not installed, skipping"; \
	fi

test:
	@./test/smoke.sh

# Regenerate the browsable Markdown table from ratios/games.json. The renderer is
# the script itself, so this needs no extra tooling and no network access.
ratios-doc:
	@tmp=GAMES.md.tmp; \
		if ./$(PROG) --render-ratios > "$$tmp"; then \
			mv "$$tmp" GAMES.md; \
		else \
			rm -f "$$tmp"; \
			echo "ratios-doc: could not render the table; GAMES.md left untouched" >&2; \
			exit 1; \
		fi
	@# Refresh the full game list embedded in README.md between its markers, from
	@# the GAMES.md we just wrote, so the README cannot drift from the JSON.
	@awk '/<!-- GAME-LIST-START -->/ { print; while ((getline l < "GAMES.md") > 0) if (l ~ /^\|/) print l; skip=1; next } /<!-- GAME-LIST-END -->/ { skip=0 } !skip { print }' README.md > README.md.tmp && mv README.md.tmp README.md
	@echo "GAMES.md and the README game list regenerated from ratios/games.json"

# Fold a `--export-ratios` document into the community table and refresh the docs.
#   btrfs-game-compressor --export-ratios > my-ratios.json
#   make ratios-merge FILE=my-ratios.json
# Averages with the existing samples and widens min/max; adds new games.
ratios-merge:
	@test -n "$(FILE)" || { echo "usage: make ratios-merge FILE=my-ratios.json [DRY_RUN=1]" >&2; exit 2; }
	@if [ "$(DRY_RUN)" = "1" ]; then \
		python3 ratios/merge.py ratios/games.json "$(FILE)" --dry-run >/dev/null; \
		echo "(dry run: ratios/games.json not written)"; \
	else \
		python3 ratios/merge.py ratios/games.json "$(FILE)"; \
		$(MAKE) --no-print-directory ratios-doc; \
	fi

install:
	@install -d $(DESTDIR)$(BINDIR)
	@install -m 0755 $(PROG) $(DESTDIR)$(BINDIR)/$(PROG)
	@if [ -f $(MANPAGE) ]; then \
		install -d $(DESTDIR)$(MANDIR); \
		install -m 0644 $(MANPAGE) $(DESTDIR)$(MANDIR)/$(MANPAGE); \
	fi
	# The community ratio table is installed alongside the script so --ratios
	# works offline on a machine that never had a git checkout.
	@install -d $(DESTDIR)$(DATADIR)/ratios
	@install -m 0644 ratios/games.json $(DESTDIR)$(DATADIR)/ratios/games.json
	@install -m 0644 GAMES.md $(DESTDIR)$(DATADIR)/GAMES.md
	@echo "installed $(DESTDIR)$(BINDIR)/$(PROG)"

uninstall:
	@rm -f $(DESTDIR)$(BINDIR)/$(PROG)
	@rm -f $(DESTDIR)$(MANDIR)/$(MANPAGE)
	@rm -f $(DESTDIR)$(DATADIR)/ratios/games.json $(DESTDIR)$(DATADIR)/GAMES.md
	@rmdir $(DESTDIR)$(DATADIR)/ratios $(DESTDIR)$(DATADIR) 2>/dev/null || true
	@echo "removed $(DESTDIR)$(BINDIR)/$(PROG)"
	@echo "config and state left in place: ~/.config/btrfs-game-compressor, ~/.local/state/btrfs-game-compressor"

# Optional, watch-only. Installs a systemd user timer that runs '--notify'
# twice a day and never compresses anything. Removed with 'make service-off'.
service:
	@install -d $(DESTDIR)$(HOME)/.config/systemd/user
	@install -m 0644 $(SYSTEMDDIR)/btrfs-game-compressor-notify.service \
			$(DESTDIR)$(HOME)/.config/systemd/user/
	@install -m 0644 $(SYSTEMDDIR)/btrfs-game-compressor-notify.timer \
			$(DESTDIR)$(HOME)/.config/systemd/user/
	@systemctl --user daemon-reload
	@systemctl --user enable --now btrfs-game-compressor-notify.timer
	@echo "notify timer enabled (watch-only; it never compresses):"
	@systemctl --user list-timers btrfs-game-compressor-notify.timer --no-pager

service-off:
	@systemctl --user disable --now btrfs-game-compressor-notify.timer 2>/dev/null || true
	@rm -f $(HOME)/.config/systemd/user/btrfs-game-compressor-notify.service \
	       $(HOME)/.config/systemd/user/btrfs-game-compressor-notify.timer
	@systemctl --user daemon-reload 2>/dev/null || true
	@echo "notify timer removed"

package:
	@rm -rf dist/$(PROG)-$(VERSION)
	@mkdir -p dist/$(PROG)-$(VERSION)
	@cp -R $(FILES) $(TESTDIR) $(SYSTEMDDIR) dist/$(PROG)-$(VERSION)/
	@tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
		-cf - -C dist $(PROG)-$(VERSION) | gzip -n > dist/$(PROG)-$(VERSION).tar.gz
	@rm -rf dist/$(PROG)-$(VERSION)
	@echo "dist/$(PROG)-$(VERSION).tar.gz"
	@echo "release archive sha256:"
	@sha256sum dist/$(PROG)-$(VERSION).tar.gz | cut -d' ' -f1

clean:
	@rm -rf dist
	@echo "clean"
