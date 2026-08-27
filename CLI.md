# Unsilo - command reference

This describes what is **implemented**. Where the original design in this document
changed during implementation, it is marked and explained.

```
unsilo doctor
unsilo find     [query] [filters]
unsilo apply    [filters]
unsilo off      [--purge]
unsilo snapshot <claude|store> --name N
unsilo restore  <NAME>
unsilo label    <id> <name> | --learn | --list
```

---

## The "never lose data" invariants

Three rules that run through everything. They are design, not flags, and each has
its own test.

**1. Immutable baseline.** The first `apply` captures a `baseline` snapshot of
Claude's state before writing a single line. It is never rotated or pruned. Every
`apply` that changes something also takes an `auto-apply-<ts>`, of which the last
10 are kept.

**2. Unsilo only removes what Unsilo created, and only if it has not changed.**
The ledger records every write with its hash. `off` removes only what still hashes
the same. If the desktop rewrote a projected file, it is kept and reported.

**3. The store can be the last remaining copy.** `cleanupPeriodDays` removes
transcripts from the project dir; the store's hard link keeps the inode alive.
`off` **never** touches it. `off --purge` refuses without a complete `store`
snapshot.

---

## Shared filters

Accepted by `find` and `apply`. **One type, one resolution, one query**, so that
previewing with one and applying with the other cannot drift apart.

```
--email <ADDR>        account by email. Repeatable
--account <UUID>      account by uuid. Repeatable
--org <UUID>          organization by uuid. Repeatable
--cwd <PATH>          prefix match on the recorded cwd
--project <SLUG>      project directory name
--branch <NAME>       recorded gitBranch
--model <NAME>        claude-opus-5, claude-sonnet-5, ...
--title <SUBSTR>
--id <PREFIX>         session uuid prefix
--since <T>           ISO8601, a date, or relative: 7d, 3w, 6mo, 1y
--until <T>
--surface code|cowork
--origin cli|desktop  where the conversation was started
--archived
--include-deleted     tombstoned sessions are excluded by default
--include-hidden      include sidechains, team sessions and daemon sessions
--confirmed-only      refuse an account inferred from timestamps
--limit <N>
--sort recent|created|size
```

AND between different flags, OR between repetitions of the same one.

### `--email` and `--account` mean **origin**

*(Changed from the original design.)* After an `apply`, a session from account A
has an entry under account B. If the filter counted that entry,
`apply --email B` would select the sessions the apply itself projected and prune
nothing.

The entries Unsilo writes are marked, and account filters ignore them.
`--account X` means "the conversations that were born in X's list".

### Emails that do not resolve

`~/.claude.json` holds `oauthAccount` for **the active account only**, so an
account is only learnable while it is signed in. `unsilo label` covers both
cases: `--learn` captures the active one, and `<id> <name>` names any of them by
hand. A manual label is never overwritten by a learned one. See command 7.

An `--email` that matches no known account is a **usage error (exit 2)**, not an
empty list: returning nothing would let `apply --email typo` prune everything
visible.

### Accounts a conversation never recorded

A transcript records no account at all: not the uuid, not the organization, not
the email. The association exists only in a desktop entry, and only for the
sessions the desktop created, which on a real machine is 6 of 132.

For the rest the account is **inferred** from when the conversation started, by
remembering which account was signed in when. Two kinds of evidence count: a
sighting at that moment, or a nearest-before and nearest-after that name the same
account. A single sighting beforehand is not enough, since unsilo only observes
when it runs.

An inference is never stored as a fact:

- `--email` and `--account` accept one, because that is what a human means
- `--confirmed-only` refuses one
- `find` marks it with a trailing `?`

Coverage starts near zero and grows with use. `doctor` reports it.

---

## Exit codes

```
0   ok
1   runtime error
2   usage error (invalid flags, unknown email, writing outside the roots)
3   unrecognised Claude layout, unsilo dropped to read only
4   dry run with pending changes
5   no results
```

---

## 1. `unsilo doctor`

Read-only. Writes **nothing**, including its own store; a test checks that by
comparing the whole tree before and after.

```
unsilo doctor [--json] [--strict]
```

```
unsilo doctor

layout
  cli config dir      /Users/facundo/.claude
                      132 conversations, 40 project dirs, 143.6 MB
                      109 subagent transcripts (not conversations)
  desktop userData    /Users/facundo/Library/Application Support/Claude
  cli versions        2.1.220 (49), 2.1.237 (20), 2.1.228 (17)
  storage backend     local files
  writes              allowed

accounts
  1e3fc9c4  (email unresolved)
            9410ab45  (unnamed)      4 sessions, 5 deleted
  81774974  fnluzko@gmail.com                 ACTIVE
            06f92962  fnluzko@gmail.com's Org 5 sessions, 0 deleted  <-

  2 desktop sessions NOT visible under the active account

transcripts
  conversations       132
  subagents           109
  with desktop entry  9 of 9
  duplicated          51a2fdd7 across 2 project dirs

retention
  cleanupPeriodDays   30 (default)
  at risk             0 transcripts, 0 B

store
  /Users/facundo/.unsilo
  hardlinks           viable (same volume)
  contents            1 transcripts, 0 ledger entries

problems
  note  1 session(s) exist in more than one project dir
  note  1 account(s) without an email
```

`--strict` exits 1 on any warning.

---

## 2. `unsilo find`

With no arguments it lists everything. That is why there is no `list` command.

```
unsilo find [QUERY] [filters] [--format table|json|paths|resume]
```

`QUERY` is full text (FTS5: `AND`, `"exact phrase"`, `prefix*`, `NOT`) over title
and first prompt.

```
ID        DATE        PROJECT                      SIZE      ACCOUNT            TITLE
3db70634  2026-08-26  ~/code/projects              1.9 MB    fnluzko@gmail.com  Unsilo conversation repository
54e36768  2026-08-24  ~/code/projects              67.9 KB   fnluzko@gmail.com  Conversations do not refresh
2e6ec2ab  2026-08-24  ~                            13.8 KB   (cli only)         (untitled)

  3 of 131 sessions
```

Matches are counted **before** the limit, so the summary can say "3 of 131"
rather than "3 of 3".

`--format resume` replaces an `open` command without adding one:

```
$ unsilo find --id 536e11e3 --format resume
cd /Users/facundo/code/luzu && claude --resume 536e11e3-5219-43dc-ad1c-f199b32ef91c
```

`--format paths` gives one transcript path per line. `--format json` gives the
full record, with scopes resolved and the identity map.

No results exits 5.

`find` builds its index under `$UNSILO_HOME` but **never touches Claude's tree**.

---

## 3. `unsilo apply`

Declarative, not additive: the filter describes the whole visible set.

```
unsilo apply [filters] [--dry-run] [--keep-mcp] [--no-prune] [--adopt-cli-sessions]
```

Steps:

1. reconcile the ledger (rows left `pending` by an interrupted run)
2. ingest: scan, hard link into the store, index
3. **refuses with exit 3** if the layout is not recognised
4. resolve the active account and organization
5. compute the target set from the filters
6. **desktop**: project a copy of `local_<id>.json` for every session in the
   target set that is missing under the active account
7. **cli**: relink from the store the transcripts retention removed
8. remove what an earlier apply projected but the filter no longer selects
9. rotate the automatic snapshots

Before writing it captures the baseline (if absent) and an automatic snapshot.

```
unsilo apply --dry-run

  active account  81774974 / 06f92962  (fnluzko@gmail.com)
  selected        131

  desktop
    + 4a4c4b0e  Resuming conversations in folders  (from 1e3fc9c4/9410ab45, 4.1 KB of mcp dropped)
    + 6709c064  Code comments review  (from 1e3fc9c4/9410ab45, 4.1 KB of mcp dropped)
    = 5 already visible

  2 changes  (dry run, nothing was written)
```

`--dry-run` **prints the plan and also exits 4**, so
`unsilo apply --dry-run || unsilo apply` works in a hook.

### What projection drops

`remoteMcpServersConfig` and `enabledMcpTools` are about 4 of the ~5 KB of an
entry and describe servers and tool grants belonging to the originating account.
They are dropped by default. Verified end to end: the desktop lists and opens the
session either way. `--keep-mcp` keeps them.

### Tombstones

A session deleted from **the target list** is not projected, even if the filter
selects it. One deleted from another list says nothing about this one.

### `--adopt-cli-sessions`

A conversation started from the terminal has no desktop entry anywhere, so the
desktop has never listed it under any account. Without this flag `apply` reports
how many there are and leaves them alone.

With it, an entry is built from the transcript, which already holds everything one
needs. It is marked as CLI-born, so `find` still shows where it came from and
`--origin cli` still separates it. The host id is derived from the transcript id,
so adopting twice is the same entry rather than a second one, and if the desktop
later writes its own entry for that conversation, ours is removed instead of the
conversation appearing twice.

More invasive than copying an entry that already exists, which is why it takes a
flag. Use `--dry-run` first: on a typical machine this is most of them.

---

## 4. `unsilo off`

```
unsilo off [--dry-run] [--purge]
```

Removes what was projected, according to the ledger, verifying hashes. Leaves the
store and the index intact; `unsilo apply` turns it back on without re-ingesting
anything.

```
  - .../local_3312dc49-....json
  ! .../local_f772da0d-....json  modified since, kept

  1 removed, 1 kept
  store untouched: 132 transcripts. unsilo apply to turn it back on
```

### Relinked transcripts are not removed

*(Changed from the original design.)* A transcript put back from the store is
**Claude's own** file returned to where Claude expects it, the same category as a
`restore`. Removing it on `off` would delete a conversation the user can see,
which is the opposite of turning a tool off. That is why it is not recorded in
the ledger.

### `--purge`

Also deletes the store and the index. Refuses without a complete `store`
snapshot, because the store can be the last remaining copy of a transcript.

---

## 5. `unsilo label`

```
unsilo label <id> <name>
unsilo label --learn
unsilo label --list
```

`<id>` is a uuid or enough of its start to be unambiguous. Accounts and
organizations are both accepted; which one it is comes from the id rather than
from a flag, since the two are indistinguishable by shape.

`--learn` captures whatever account is signed in and nothing else. It writes only
inside the store, which is what makes it safe in a session start hook:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "unsilo label --learn >/dev/null 2>&1 || true" }] }
    ]
  }
}
```

`--list` shows what is known and where each name came from.

## 6. `unsilo snapshot`

```
unsilo snapshot claude --name N [--metadata-only]
unsilo snapshot store  --name N [--metadata-only]
```

`claude` captures transcripts, subagents, desktop entries and tombstones.
`store` captures the store, the index and the ledger, excluding earlier snapshots.

```
  scope           Claude
  transcripts     132
  subagents       109
  desktop         9
  deleted         5
  size            219.1 MB -> 41.4 MB
```

Allow list, never deny list: what goes **in** is enumerated. A deny list breaks
the day Claude adds a file with a token in it. A test plants a sentinel in
`.credentials.json` and in `sessions/*.key` and requires that it appears neither
in the compressed archive nor in any extracted body.

Consistency with concurrent appends: exactly the length taken from the open handle
is read. Whatever is appended afterwards is not part of the snapshot.

Deterministic: sorted order and fixed tar headers. Two snapshots of the same tree
are byte identical.

### Format

*(Changed from the original design.)* This document said the archive would use the
desktop's native export shape, to interoperate. A format of its own was used:

```
manifest.json
transcripts/<slug>/<sessionId>.jsonl
transcripts/<slug>/<sessionId>/subagents/<id>.jsonl
desktop/<surface>/<acct>/<org>/<file>
store/...
```

Two reasons: the desktop's shape has no room for sessions born in the CLI, which
have no host id; and there is no way to verify from here that its importer would
accept what we write. A converter can come later without breaking anything.

The manifest records the **origin** of each file, not just its place in the
archive, which is what makes restoring onto a machine with different paths
possible. Every entry carries `len` alongside its `sha256`.

---

## 7. `unsilo restore`

```
unsilo restore <NAME|FILE> [--dry-run] [--force] [--skip-conflicts]
                           [--rewrite-cwd OLD=NEW]
```

The scope is read from the manifest. Four verdicts per file:

```
+  nothing on disk                              -> restore
=  identical                                    -> nothing
>  the local file starts with the snapshot bytes -> it is newer, nothing
!  the prefix differs                           -> conflict
```

The third is exact because transcripts are append only: no merge and no date
heuristic. Conflicts **stop the run** by default; `--skip-conflicts` leaves them,
`--force` overwrites them.

A snapshot from another machine whose recorded origin falls outside this
installation's roots is re-rooted under them, so importing one can never write
somewhere it has no claim to. `--rewrite-cwd` handles the case where the paths are
known.

Restored files do not go into the ledger: they are Claude's.

---

## Flows

```bash
unsilo doctor                              # understand the state, writes nothing
unsilo snapshot claude --name pre-unsilo   # untouched picture
unsilo apply --dry-run                     # see what would change
unsilo apply                               # captures the baseline and applies
```

```bash
unsilo find "the timeout bug"
unsilo find --id 459307c7 --format resume
```

```bash
unsilo apply                               # after switching accounts
unsilo off                                 # turn it off, store untouched
```

```bash
unsilo snapshot store --name final && unsilo off --purge   # uninstall
```

---

## Post-v1

`attach` (hard link a session into another cwd), `prune`, profiles as filter
aliases, watch mode, full text indexing of the body (`scan --full`), and
`--sort messages`, which needs the count only a full scan provides.

Also a saved plan: `--dry-run` recomputes rather than replaying what was
reviewed, so between the preview and the run the state can move.
