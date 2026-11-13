# Background ignore annotations and incremental tree toggles

## Status

Implemented, pending review.

## Context

The Files drawer flattened the whole expanded tree whenever the user toggled a
directory, and the flatten ran `git check-ignore` synchronously at its end.
`click_row` therefore walked every expanded directory and spawned git on the UI
thread on every expand/collapse, even though a comment claimed "no git re-run".
The same synchronous annotation also delayed the first rows of an ordinary
background refresh, because the worker published nothing until the whole rows
vector, ignore marks included, was complete.

## Evidence

On the reference machine one `git check-ignore -z --stdin` over 500 paths took
101-109 ms and over 1000 paths took 309-359 ms. The tree cap is
`MAX_ROWS = 1000`, so the slow case was reachable with a normal project. The
initial `refresh` worker already published rows and VCS state in two stages,
but `tree_rows` still called `SidePanel::mark_ignored` before the first
publication, and `click_row -> rebuild_rows -> tree_rows` re-ran the complete
flatten plus git on the interaction path.

## Decision

`tree_rows`/`tree_rows_wsl` now only enumerate. The snapshot worker publishes
two stages: (1) tree rows with no ignore annotations, then (2) the same rows
annotated by git plus the VCS snapshot. A full refresh therefore paints the
directory listing without waiting for ignore or VCS probing.

`click_row` is incremental. Collapse removes the contiguous DFS descendant run
from the in-memory rows and reads nothing. Expand reads only the newly opened
directory (plus descendants that are already in `expanded` but were hidden by
a collapsed ancestor) and inserts those rows; the global `MAX_ROWS`,
`MAX_PER_DIR`, ordering and `.git` filtering rules are unchanged.

New rows start unannotated. They set `ignored_refresh_pending`, which the next
idle `sync` hands to a background pass that runs one `git check-ignore` over the
current rows. That pass publishes an ignore-only update keyed by absolute path,
so it annotates whatever rows are on screen instead of replacing them with a
stale snapshot. It reuses the existing single snapshot worker and slot: if a
worker is already in flight the request stays pending and coalesces into one
follow-up pass.

Full snapshots also carry `tree_generation`, the identity of the visible tree
shape. Root changes, every expand/collapse and every filter change bump it. A
row-set publication is applied only when its generation still matches; an
out-of-date row set is dropped instead of reverting the incremental change, and
its VCS result is still consumed. Ignore-only updates are path-keyed and remain
safe to apply across generations.

`ignored_paths` first checks `git_repository_root`; directories with no `.git`
ancestor never spawn git. WSL trees remain unannotated, matching the previous
behavior where only host `tree_rows` was marked.

## Rejected alternatives

- Keeping the synchronous re-flatten and only moving git to a thread: the UI
  thread would still walk every expanded directory per click.
- Scheduling a full background refresh per toggle: it restores the ignore marks
  but re-walks the tree and re-runs VCS probes for a purely local shape change.
- Applying a background ignore result as a whole rows vector: a later
  expand/collapse could be overwritten by rows captured before the click.
- Spawning git in directories outside a repository to get a definitive "not
  ignored": git can only fail there, at process-spawn cost.

## Consequences

Ignore marks appear after the panel's existing 100 ms poll rather than in the
same frame as the newly opened rows, and host rows never block on git. WSL
expansion still performs one blocking guest `find`, but it now covers only the
newly opened directory (plus hidden expanded descendants) instead of every
expanded directory. Root/`files_wsl` staleness handling is unchanged; the
ignore-only update is additionally path-keyed, so it cannot revert concurrent
tree edits. A failed WSL expansion keeps the visible rows and re-arms the
regular snapshot path so the existing read-failure state still surfaces.

## Validation

Unit tests cover the two-stage publication order (rows without marks first,
annotated rows second), the no-`.git` guard, the pure collapse/expand row
helpers, and the rule that a stale row set cannot revert an expansion. The
existing side-panel tests continue to exercise ordering, expansion and snapshot
staleness. `scripts/check_architecture.py --base origin/main` passes, and the
changed files are rustfmt-clean in this checkout's line-ending convention. The
crate compile check and the focused side-panel test filter run on the shared
build target separately from this commit.

## Supersedes

None.

## Revisit when

The drawer gains a persistent filesystem watcher that can invalidate ignore
annotations, or ignore state must be exact within a single frame of expansion.
