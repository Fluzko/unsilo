<div align="center" style="text-align: center;">
  <img src="assets/banner.svg" alt="unsilo" width="500">
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
      { "hooks": [{ "type": "command", "command": "unsilo label --learn >/dev/null 2>&1 || true" }] }
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

The test that matters:

```rust
let before = w.claude_digest();
apply::run(...)?;
assert!(applied.changes() > 0, "apply made no changes, the test proves nothing");
off::run(...)?;
assert_eq!(before, w.claude_digest());
```

It compares content, permissions **and which paths share an inode**: a leaked
hard link changes no bytes, so without that the test would pass while leaking
files.
