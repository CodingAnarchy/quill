# Version control

Quill can back your manuscript with real version history — the same machinery
Git uses, presented as a writing tool rather than a developer one. Once a project
is under version control you can snapshot it, keep it backed up off-machine, write
the same book on two machines and reconcile the two, keep parallel *drafts* of a
chapter and merge the one you like, resolve any overlap chapter-by-chapter, and
name and revisit points in its history — all without leaving the editor or
touching the `git` command line.

This page is task-oriented: how to turn it on, set a remote, sync between
machines, keep and merge drafts, resolve a conflict, and read the history. It was
built up across issues #54 (the version-control palette), #131–#135 (commit /
history / diff / restore / remote), #587 (the history view), #833/#966
(milestones and their notes), and the 0.7.0 suite — #962 (two-way sync), #963
(conflict resolution), #964 (the branch/merge graph), and #965 (branch-as-draft).

## The model

Version control is **opt-in and per project**. A project is an ordinary folder of
chapter files until you initialize it; from then on Quill keeps a normal Git
repository in the project's `.git` directory alongside your chapters, and every
operation below reads and writes that repository. Nothing leaves your machine
until you set a remote and push, sync, or turn on automatic backup.

Where the state lives, in Git terms you don't have to think about:

- **Snapshots** are commits in `.git`.
- **The remote** is a Git remote URL stored in the repository's config.
- **Drafts** are Git branches.
- **Milestones** are annotated Git tags (the milestone's note is the tag's
  annotation).

Because it's a real repository, a project you've synced or backed up is also
readable by any Git tool — but you never need one; everything here is in-app.

## The version-control menu (`Ctrl-V`)

Everything starts from one menu. Press **`Ctrl-V`** ("version control") to open
it. What it lists depends on where the project is:

- **Not under version control** — a single **Initialize** entry. Choose it and
  Quill puts the project under version control with a sensible `.gitignore` (it
  never clobbers an existing repository or ignore file). Editor scratch — the
  `.quill/` recovery state, the `unused/` folder, `export/` — stays out of
  history.
- **Under version control** — **Commit**, **Drafts**, **Milestone**,
  **History**, **Changes** / **Project changes** (diffs), **Restore chapter** /
  **Restore project**, and **Set remote**. Once a remote is set, **Push**,
  **Pull**, and **Sync** appear too, and "Set remote" becomes "Change remote".

The menu is a palette: type to filter, arrow to a row, Enter to run it.

## Committing

**Commit** (`Ctrl-V → Commit`) snapshots the whole project. Quill saves the active
chapter first, then shows a pre-commit review — the list of changed files and a
colorized diff of exactly what the commit will capture — so you never commit
blind. Confirm the review and Quill asks for a commit message. With nothing
changed since the last commit it says so and does nothing.

A commit is the unit everything else builds on: a milestone marks one, a draft
branches from one, and sync and draft-merge only run on a clean (committed) tree —
Quill asks you to commit first rather than risk overwriting unsaved words.

## Setting a remote

A *remote* is a hosted copy of the repository — a repo on GitHub, GitLab, a
private server, anywhere Git can reach. Setting one is what lets you push, pull,
sync, or turn on automatic backup.

Open **`Ctrl-V → Set remote`** and paste the remote's URL at the `Remote URL:`
prompt. The prompt is prefilled with the current remote (so "Change remote" shows
what's set); a blank entry leaves it unchanged. Quill authenticates with your
existing Git credentials — an SSH key or a credential helper — so an `ssh://` or
`git@…` URL for a repo you already have keys for just works.

Once a remote is set:

- **Push** (`Ctrl-V → Push`) sends your committed work up — a one-way backup.
- **Pull** (`Ctrl-V → Pull`) brings the remote's changes down. It refuses a dirty
  tree (commit first) and, if local and remote have diverged, merges them in-app;
  a merge that conflicts is reported by name, leaving your tree untouched.
- **Sync** (`Ctrl-V → Sync`) is the two-way flow — the one to reach for when you
  write on more than one machine. See below.

All three run in the background, so a slow network never freezes the editor; the
status bar reports the result (and a failure is triaged into a clear cause — an
auth problem, a rejected push, being offline — rather than a raw Git error).

## Working on two machines (two-way sync)

Say you write on a laptop and a desktop, both pointed at the same remote. You add
a scene on the laptop and push; later you edit a different chapter on the desktop.
Now each machine has work the other doesn't. **Sync** reconciles them.

On the desktop: commit your work, then **`Ctrl-V → Sync`**. Quill fetches the
remote and does a *chapter-level* three-way merge:

- Chapters only one side changed merge cleanly and are folded together.
- Chapters **both** sides changed differently come back as conflicts, and Quill
  opens the conflict-resolution flow (below) so you settle them in-app.

Sync guards your words the way a pull does: it saves the active chapter, drops to
single-chapter view, and refuses to run while the tree has uncommitted changes —
so the merge can never overwrite unsaved edits. A sync that lands in the
background while you're mid-rename or in another menu waits until you're back in
the editor before surfacing any conflicts, rather than interrupting you.

The result is one shared history both machines can push and pull, with each
side's edits preserved.

## Resolving a conflict

When a sync — or a draft merge — finds a chapter that changed on both sides, Quill
opens the **conflict-resolution** view instead of dropping you to a command line.
Before it shows anything it tags a safety milestone (`pre-sync-…`) on the current
commit, so both committed versions are recoverable no matter how you resolve.

You resolve one chapter at a time. For each conflicting chapter you see a diff of
the two versions and choose:

| Key   | Meaning                                                            |
| ----- | ----------------------------------------------------------------- |
| `l`   | **Keep mine** — take the local version of this chapter.           |
| `r`   | **Take theirs** — take the remote (or other draft's) version.     |
| `m`   | **Merge by hand** — open a scratch buffer seeded with both versions, clearly marked `<<<<<<< local` / `=======` / `>>>>>>> remote`, so you can blend them. |
| `Esc` | **Abandon** — nothing is committed; both versions stay safe.      |

In the hand-merge buffer, edit the text to what you want, delete the markers, and
press **`Ctrl-S`** to accept it (`Esc` backs out to the choice). Once you've
resolved every conflicting chapter, Quill lands them all as one merge commit, with
both histories reachable. If anything fails partway, nothing is half-applied — the
safety milestone and both original versions survive, and re-running the sync
re-detects the same conflicts to try again.

## Keeping drafts of a chapter

A **draft** is a named alternate version of the whole manuscript — an alt ending,
a POV experiment, a rewrite you're not sure about — that you can create, switch
between, and merge back, all without losing the version you started from. (Drafts
are Git branches; you never see that word.)

Open the drafts overlay with **`Ctrl-V → Drafts`**. It lists every draft with the
current one marked. From here:

| Key     | Action                                                          |
| ------- | -------------------------------------------------------------- |
| `n`     | **New draft** — prompts for a name, then branches off the current state and switches to it. |
| `Enter` | **Switch** to the selected draft — the working copy follows and the open chapter reloads. |
| `m`     | **Merge** the selected draft into the current one.             |
| `↑`/`↓` (or `j`/`k`) | Move the selection.                              |
| `Esc`   | Close the overlay.                                             |

A typical run: on your `main` draft, press `Ctrl-V → Drafts`, `n`, and name it
`alt-ending`. Write the alternate version. If you keep it, switch back to `main`
(Enter on `main`) and merge `alt-ending` in (`m` on it). If you don't, just switch
back and leave the draft where it is — nothing on `main` changed.

Switching and merging guard unsaved work exactly as sync does: they save the
buffer and refuse a dirty tree, so a draft switch can never overwrite your edits —
commit first if Quill asks. A draft **merge** that touches chapters changed on
both drafts routes through the same conflict-resolution flow described above; a
clean merge folds the chapters in and reports how many.

## Milestones

A **milestone** names a point in the history — "first draft done", "sent to
beta readers" — so you can find it and return to it later. Create one with
**`Ctrl-V → Milestone`**: enter a name, then an optional note recording what it
marks. The milestone tags the last committed state (Quill flushes edits first and
hints if you have uncommitted work, rather than tagging a stale snapshot).

Milestones are a coarse, whole-project checkpoint above the per-chapter
save/recovery: from the history view you can **restore the whole project** to a
milestone, or **diff** the current manuscript against one to see everything that's
changed since.

## Reading the history

**History** (`Ctrl-V → History`) opens a navigable log of the project's timeline.
Named milestones lead the list, then the commits — and each row draws a Unicode
**branch/merge graph** in a column beside it (`●` for the commit, `│` for the
lanes carrying through), so a manuscript with drafts and merges is legible at a
glance: you can see where a draft branched off and where it folded back in.

From a selected row:

| Key   | Action                                                             |
| ----- | ----------------------------------------------------------------- |
| `d`   | **Diff** the current chapter against that point.                  |
| `r`   | **Restore** the current chapter to it (into the editor to review and save). |
| `D`   | Diff the **whole project** against that point.                    |
| `R`   | Restore the **whole project** to it (confirms first).             |
| `n`   | Edit the **note** on a selected milestone.                        |
| `/`   | Filter the list — type to narrow by summary, id, or date.         |
| `↑`/`↓` (or `k`/`j`) | Move the selection (skipping filtered-out rows).   |
| `Esc` | Clear an active filter, then (a second press) close the view.     |

A chapter restore loads the recovered version into the editor marked unsaved, so
you review it and save (or discard) rather than the file changing underneath you.
A whole-project restore rewrites the working tree, so it always asks first.

## Automatic backup

Beyond manual pushes, an opt-in `[backup]` table can commit and push a project that
points at a remote on a cadence in the background — an always-current off-site copy
without a manual step. Each backup makes an automatic commit of any uncommitted work
and then pushes (a clean tree makes no commit), so prose you haven't committed by
hand still reaches the remote. Independently, it can keep scheduled local `.zip`
archives. Both are
off by default; the keys and the `Ctrl-V → Set remote` setup are covered under
"Archive & backup" in the [user guide](guide.md) and in the
[configuration reference](configuration.md).

## Related keys, at a glance

| Keys                 | What it does                                          |
| -------------------- | ---------------------------------------------------- |
| `Ctrl-V`             | Open the version-control menu                        |
| `Ctrl-V → Commit`    | Snapshot the project (with a pre-commit review)      |
| `Ctrl-V → Set remote`| Point the project at a hosted remote                 |
| `Ctrl-V → Sync`      | Two-way sync: fetch and merge the remote's edits     |
| `Ctrl-V → Drafts`    | Create / switch / merge named drafts                 |
| `Ctrl-V → Milestone` | Name a point in the history                          |
| `Ctrl-V → History`   | Browse the commit / milestone log and branch graph   |

Within a conflict: `l` keep mine, `r` take theirs, `m` hand-merge (`Ctrl-S`
accepts), `Esc` abandon. Within drafts: `n` new, `Enter` switch, `m` merge, `Esc`
close.
