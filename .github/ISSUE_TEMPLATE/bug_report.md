name: Bug report
description: Something behaves incorrectly
labels: ["bug"]
body:
  - type: textarea
    id: what-happened
    attributes:
      label: What happened
      description: What did you do, what did you expect, and what happened instead?
    validations:
      required: true

  - type: textarea
    id: reproduce
    attributes:
      label: Steps to reproduce
      description: Exact commands or keystrokes, starting from a clean state if possible.
    validations:
      required: true

  - type: textarea
    id: output
    attributes:
      label: Output
      description: >-
        Paste the output of the commands below. This usually identifies the
        problem immediately, so please include it.
      render: shell
    value: |
      $ btrfs-game-compressor --version
      $ btrfs-game-compressor --status --no-color
      $ findmnt -no OPTIONS --target /path/to/steamapps/common
    validations:
      required: true

  - type: input
    id: os
    attributes:
      label: Distribution and versions
      placeholder: "Arch, btrfs-progs 6.3.2, btrfs-compsize 2.1"
    validations:
      required: true

  - type: input
    id: shell
    attributes:
      label: bash version
      description: Output of `bash --version | head -1`
      placeholder: "GNU bash, version 5.2.21(1)-release"

  - type: dropdown
    id: entry
    attributes:
      label: How did you run it?
      options:
        - Interactive TUI
        - --status
        - --dry-run
        - --library
        - make install / install.sh
    validations:
      required: true
