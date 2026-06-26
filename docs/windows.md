# Windows & split panes

Quill can show two things at once. You keep writing in your manuscript while a
second pane holds something you want beside it: another chapter, a story-bible
entry, a note, or a read-only report. This is the "write with your references
open" workflow, built up across issues #163 (split + focus), #164 (dock a
report), #165 (reference files), and #166 (resize, close, narrow-terminal).

All of it lives behind one prefix key, Vim's `Ctrl-w`. Press `Ctrl-w`, then a
second key for the window command.

## The model

There are at most two panes. One is always your focused editor (the chapter
you're writing); the other holds the reference. The focused editor is the one
that takes your keystrokes, so editing, navigation, and save always work exactly
as they do without a split.

The second pane holds one of:

- **Another editor** on a chapter or a project file, which you can read and edit.
- **A docked surface**: a progress report, the analysis report, or an AI answer,
  which is read-only and scrollable.
- **The assistant pane**: a conversation with the authorial assistant, where you
  type a request and it runs editor actions to carry it out (#307).

## Opening a split

| Keys        | Action                                                        |
| ----------- | ------------------------------------------------------------- |
| `Ctrl-w s`  | Split stacked (top / bottom). Both panes start on the chapter you're in. |
| `Ctrl-w v`  | Split side by side (left / right), the same way.              |
| `Ctrl-w a`  | Open the assistant pane side by side (a conversation, not an editor). |

`v` is the vertical-divider split (side by side, like Vim's `:vsplit`); `s` is
the stacked split (one above the other, like Vim's `:split`). Navigate one pane
elsewhere to compare two chapters.

`Ctrl-w a` docks the **assistant pane** beside the editor. Type a request and
press `Enter`; the assistant replies and runs editor actions on your behalf,
keeping the conversation across turns. `Esc` cancels a run in flight (or, when
idle, hands keys back to the editor); `Ctrl-w c` closes the pane. With AI off the
pane says so rather than failing silently.

With a report or AI answer already open full-screen, `Ctrl-w s` / `Ctrl-w v`
**docks** it into a pane instead of splitting the editor, so the manuscript stays
visible while you read the finding.

From the chapter browser (`Ctrl-g`), `o` opens the selected chapter in a side
pane, and `f` switches to the reference-file browser, where `Enter` opens a
bible entry or note in a side pane.

Splits are a single-chapter-view feature. Leave the continuous manuscript view
(`Ctrl-l`) before splitting.

## Moving between panes

| Keys                        | Action                                          |
| --------------------------- | ----------------------------------------------- |
| `Ctrl-w w` (or `h/j/k/l`)   | Move focus to the other pane.                   |

Focusing the other editor swaps it in so your keys edit it; the project's active
chapter follows along, so save and chapter navigation track the pane you're in.
Focusing a docked report instead hands it the scroll keys (`j`/`k`, `PageUp` /
`PageDown`, `g`/`G`); `Esc` returns focus to the editor. Focusing the assistant
pane hands it your typing for the request line (`↑`/`↓`, `PageUp` / `PageDown`
scroll the transcript); `Esc` returns focus to the editor.

## Resizing

| Keys                    | Action                                              |
| ----------------------- | --------------------------------------------------- |
| `Ctrl-w >` or `Ctrl-w +`| Grow the focused pane along the split axis.          |
| `Ctrl-w <` or `Ctrl-w -`| Shrink the focused pane.                             |
| `Ctrl-w =`              | Reset the split to an even balance.                  |

Resizing moves the boundary in steps. It stops before either pane gets too thin
to use, so you can lean on the key without collapsing a pane to a sliver. The
ratio you set is remembered if the terminal is resized. When you're done with a
temporary emphasis, `Ctrl-w =` snaps the two panes back to even.

## Closing

| Keys                   | Action                                                |
| ---------------------- | ----------------------------------------------------- |
| `Ctrl-w c` or `Ctrl-w q` | Close the split, returning to the focused editor.   |

Close keeps the pane you're focused on and reclaims the other pane's space. An
edited editor pane is saved before it closes, so you never lose words; a
read-only report just disappears. To keep the *other* pane instead, focus it
first (`Ctrl-w w`), then close.

## Narrow terminals

A split needs room. Below a sensible width (side by side) or height (stacked),
two panes would each be too thin to read, so Quill draws only the focused pane.
The split isn't lost, it's just not shown: widen the terminal and both panes
come back at the ratio you'd set. This mirrors how the chapter/outline pane
yields its strip on a narrow screen.
