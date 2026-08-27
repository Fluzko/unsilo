<div align="center" style="text-align: center;">
  <img src="assets/banner.svg" alt="unsilo" width="500">
</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/unsilo?logo=rust)](https://crates.io/crates/unsilo)
[![docs](https://img.shields.io/docsrs/unsilo?logo=docsdotrs)](https://docs.rs/unsilo)
[![ci](https://img.shields.io/github/actions/workflow/status/Fluzko/unsilo/ci.yml?branch=main&label=ci)](https://github.com/Fluzko/unsilo/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/unsilo)](LICENSE)

</div>

---

Claude Desktop hides your own history from you. Sign in with a second account
and every transcript from the first one vanishes from the list, even though
the files never moved. unsilo hard-links the ones the active account can't
see into its session store, safely and reversibly, so `claude --resume` finds
all of your conversations, not whatever slice the signed-in account happens
to own. Nothing is copied, nothing is deleted, and `unsilo off` puts it back
exactly as it was.

---

Claude Desktop indexes its sessions by account and organization:

```
<userData>/claude-code-sessions/<accountUuid>/<organizationUuid>/local_<id>.json
```

and the session list reads **only** the directory of the account that is
signed in.

```
$ unsilo doctor
  2 desktop sessions NOT visible under the active account

$ unsilo apply
  desktop
    + 4a4c4b0e  Resuming conversations in folders  (from 1e3fc9c4/9410ab45)
    + 6709c064  Code comments review               (from 1e3fc9c4/9410ab45)

$ unsilo off
  store untouched: 132 transcripts. unsilo apply to turn it back on
```

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Fluzko/unsilo/main/install.sh | sh
```

Picks the build for your platform, checks it against the published sha256 before
unpacking it, and puts the binary in `~/.local/bin`. `UNSILO_INSTALL_DIR` and
`UNSILO_VERSION` override where and which.

Then, before anything else:

```bash
unsilo doctor
```

It writes nothing and tells you what it found, including whether writing would be
safe on your machine.

<details>
<summary><b>macOS</b> — and the Gatekeeper catch</summary>

The one-liner above works as it is. Worth knowing why: a **browser** marks what it
downloads with `com.apple.quarantine`, and Gatekeeper then kills the binary
because it is not signed by a paid Apple developer account. `curl` sets no such
attribute, so anything installed this way is unaffected.

If you did download through a browser:

```bash
xattr -d com.apple.quarantine unsilo
```

Taking the archive by hand, Apple Silicon:

```bash
tar -xzf unsilo-aarch64-apple-darwin.tar.gz
install -m 755 unsilo-aarch64-apple-darwin/unsilo ~/.local/bin/unsilo
```

Intel: `unsilo-x86_64-apple-darwin.tar.gz`.

</details>

<details>
<summary><b>Linux</b></summary>

The one-liner above works as it is. The x86_64 build is static, so it does not
care which libc you have.

Taking the archive by hand:

```bash
tar -xzf unsilo-x86_64-unknown-linux-musl.tar.gz
install -m 755 unsilo-x86_64-unknown-linux-musl/unsilo ~/.local/bin/unsilo
```

On ARM: `unsilo-aarch64-unknown-linux-gnu.tar.gz`.

</details>

<details>
<summary><b>Windows</b></summary>

The script does not cover Windows. Take
`unsilo-x86_64-pc-windows-msvc.tar.gz` from the
[latest release](https://github.com/Fluzko/unsilo/releases/latest), extract it,
and put `unsilo.exe` somewhere on your `PATH`.

SmartScreen may warn about an unsigned executable, for the same reason
Gatekeeper does on macOS.

</details>

<details>
<summary><b>From source</b></summary>

Needs nothing but a Rust toolchain, because SQLite is compiled in:

```bash
cargo install --git https://github.com/Fluzko/unsilo unsilo
```

</details>

<details>
<summary>Reading the install script first</summary>

Piping a script into a shell means running whatever that URL serves. If you would
rather look at it, which is reasonable:

```bash
curl -fsSL -O https://raw.githubusercontent.com/Fluzko/unsilo/main/install.sh
less install.sh && sh install.sh
```

Every archive on the [release page](https://github.com/Fluzko/unsilo/releases/latest)
also ships a `.sha256` beside it, which is what the script checks.

</details>

## Commands

|                   |                                                                               |
| ----------------- | ----------------------------------------------------------------------------- |
| `unsilo doctor`   | What exists, what cannot be seen, and whether writing is safe. Writes nothing |
| `unsilo find`     | Search and list. `--format resume` prints a ready to run `claude --resume`    |
| `unsilo apply`    | Make the selected conversations visible under the active account              |
| `unsilo off`      | Remove what it projected. The store is left untouched                         |
| `unsilo snapshot` | Capture Claude's state, or unsilo's own                                       |
| `unsilo restore`  | Put back what a snapshot captured                                             |

Full reference in [CLI.md](CLI.md).

## Usage

### First run

```bash
unsilo doctor                              # writes nothing; read this first
unsilo snapshot claude --name pre-unsilo   # a picture of Claude untouched
unsilo apply --dry-run                     # what would change
unsilo apply                               # captures a baseline, then applies
```

`doctor` leads with the number that matters:

```
┌──────────────────────────────────────────────────────────┐
│ 2 desktop sessions NOT visible under the active account  │
└──────────────────────────────────────────────────────────┘
```

`--dry-run` prints the plan and exits 4 when there is something to do, so
`unsilo apply --dry-run || unsilo apply` works in a script.

### After switching accounts

```bash
unsilo apply
```

```
── apply ─────────────────────────────────────────────────────────────────
  active account  1e3fc9c4 / 9410ab45  (work@example.com)
  selected        131

  desktop
    + 4a4c4b0e  Resuming conversations in folders  (from 81774974/06f92962, 4.1 KB of mcp dropped)
    + 6709c064  Code comments review               (from 81774974/06f92962, 4.1 KB of mcp dropped)
    = 5 already visible

  2 changes
```

Idempotent: run it again and it reports `0 changes` without writing.

### Bringing terminal conversations into the desktop

A conversation started with `claude` in a terminal has no desktop entry, so the
desktop has never listed it under any account. `apply` says how many:

```
  126 conversation(s) the desktop has never known about
  --adopt-cli-sessions would give them an entry so it lists them
```

Look before you leap, then do it:

```bash
unsilo apply --dry-run --adopt-cli-sessions
unsilo apply --adopt-cli-sessions
```

The entry is built from the transcript and marked as terminal-born, so
`--origin cli` still tells the two apart afterwards. The other direction needs
nothing: a desktop session already writes its transcript where the CLI looks, so
`claude --resume` finds it.

To adopt one rather than all of them, keep it additive:

```bash
unsilo apply --adopt-cli-sessions --id 7494e26c --no-prune
```

Without `--no-prune` that is not an addition. `apply` is declarative: the filter
describes the whole visible set, so a narrow filter also removes what falls
outside it.

### Finding a conversation and going back to it

```bash
unsilo find "the timeout bug"
unsilo find --project my-repo --since 30d
unsilo find --email work@example.com --branch main
```

```
── conversations ─────────────────────────────────────────────────────────
ID        DATE        PROJECT                      SIZE      ACCOUNT                TITLE
7494e26c  2026-08-26  ~/code/projects              564.2 KB  work@example.com?      README and logo
54e36768  2026-08-24  ~/code/projects              68.1 KB   (cli only)             Conversations do not refresh
```

A trailing `?` on the account means it was inferred from when the conversation
started, not stated by a desktop entry. `(cli only)` means nothing knows.
`--confirmed-only` drops the inferred ones.

Then reopen it:

```bash
eval "$(unsilo find --id 7494e26c --format resume)"
```

`--format resume` prints `cd <cwd> && claude --resume <id>`, so you land in the
right directory. `--format paths` gives the transcript paths for piping, and
neither is ever coloured.

### Undoing it

```bash
unsilo off
```

```
  1 removed, 1 kept
  store untouched: 132 transcripts. unsilo apply to turn it back on
```

`kept` means Claude rewrote that file after unsilo put it there, so it is no
longer ours to remove. The store is never touched, because retention cleanup can
make it the only remaining copy of a transcript. To remove that too:

```bash
unsilo snapshot store --name final
unsilo off --purge          # refused without a store snapshot
```

## How it works

- **Hard links, never symlinks.** Claude's listing filters with `Dirent.isFile()`,
  which is `false` for a symlink, so a symlinked transcript is invisible to it.
- **Link, never move.** The original stays where Claude expects it, and there is
  no window in which a transcript is missing from its project directory.
- **The store outlives retention.** `cleanupPeriodDays` (30 by default) removes
  old transcripts; the link count never reaches zero, so the inode survives. That
  makes the store the last remaining copy of some transcripts, which is why
  nothing deletes from it except a `--purge` that demands a snapshot first.
- **Ledger before file.** Every write outside the store is recorded with its hash
  before it happens. `off` removes only what still hashes the same, so anything
  Claude rewrote afterwards is kept.
- **Colour that turns itself off.** It encodes meaning already present in the
  text, so stripping it loses nothing, and it never reaches `--json` or a pipe.
- **Never guesses an account.** A transcript records none, so where the account
  is not stated by a desktop entry it is inferred from when the conversation
  started, marked as an inference, and left out entirely when the evidence
  disagrees.
- **Refuses what it does not recognise.** If Claude's layout is not one we
  understand, or the remote transcript backend is on, writes exit 3 and only
  reading remains.

## Keeping it current

Account labels can only be learned while that account is signed in, so a session
start hook captures each one the first time you use it. `--learn` writes only
inside unsilo's own store, which is what makes it safe here:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "unsilo label --learn >/dev/null 2>&1 || true"
          }
        ]
      }
    ]
  }
}
```

For an account that is never active again, name it by hand:

```bash
unsilo label --list
unsilo label 1e3fc9c4 work@example.com
```

A manual label is never overwritten by a learned one.

## Development

```bash
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Fixtures are generated from code (`crates/unsilo-testkit`) and never copied from
a real machine, so no conversation content reaches the repository.

`unsilo-testkit` is not published, and `unsilo` refers to it by path with no
version. Cargo drops a path-only dev-dependency when publishing, which is what
lets the binary crate go to crates.io while its fixture crate stays here. Adding
a version to that dependency is what breaks it.
