# Git snapshots resolve only what the active view consumes

## Status

Proposed for review with implementation.

## Context

`SidePanel::refresh` spawned one snapshot worker on every drawer event. The
worker always ran `git status --porcelain=v2 -z --branch`,
`git diff --shortstat HEAD` and `git log --all --date-order --max-count=128`.
Measured on this repository, that is roughly 37–55 ms, 33–47 ms and 48–63 ms
respectively per refresh (the cold log run reached 680 ms). The Files view only
renders branch, staged/unstaged paths and conflicts; only the VCS drawer reads
`plus`/`minus` and `history`. WSL roots additionally pay one `wsl.exe` round
trip per command.

View switching itself sets `needs_refresh`, so every Files ↔ VCS switch repeated
all three commands.

## Evidence

- `nebula_app/src/display/side_panel/mod.rs` call site previously invoked
  `read_git`/`read_git_wsl` without a scope.
- `nebula_app/src/display/side_panel/vcs.rs` `collect_git_info` ran diff and log
  unconditionally after parsing status.
- `nebula_app/src/gpui_shell/workspace/vcs_panel.rs` is the only consumer of
  `GitInfo::history`; the legacy renderer reads `plus`/`minus` only in the Git
  view.
- Files row rendering and summary use branch/staged/unstaged/conflicts only.

## Decision

Add `GitSnapshotScope { Status, History }`. Files refreshes pass `Status` and
stop after `git status`; the VCS drawer passes `History` and keeps diff/log.
`PanelSnapshot` carries the requested scope and `SidePanel` remembers the scope
of the landed snapshot. Switching Files → VCS triggers the usual pending
refresh only when that snapshot lacks history. A status-scoped snapshot that
lands after the switch re-arms one history refresh in harvest, so an in-flight
Files worker cannot leave the drawer empty. SVN and "not a repository" results
are complete by definition because they have no git history to fetch.

## Rejected alternatives

- Cache history separately per root and merge it into status refreshes: adds a
  second snapshot shape and staleness rules for values the Files view never
  reads.
- Skip the whole refresh on every view switch: loses the working-tree status
  refresh that users currently get when opening the drawer.
- Poll or background-prefetch history: the project already rejected periodic
  directory/VCS polling; an extra watcher would add process traffic for data
  only one subview displays.

## Consequences

A Files refresh costs one git process instead of three (one `wsl.exe` instead of
three on WSL). Switching to VCS with an already history-bearing snapshot costs
no git process; switching without one costs one full snapshot. Returning to
Files downgrades the landed snapshot to status-only, so a later return to VCS
refreshes history again. `GitInfo` field semantics and both parsers are
unchanged; SVN and no-repository fallbacks still return `None`/SVN data exactly
as before.

## Validation

`git_snapshot_scope_only_reads_what_the_active_view_consumes` records the git
arguments from a fake runner: the Files scope emits only `STATUS_ARGS`, the
History scope emits status + diff + log and parses their results.
`switching_to_vcs_refreshes_only_until_history_is_cached` asserts the view
switch requests a refresh without history and none once history is cached.
Existing side-panel and VCS list tests remain green. These are command-count
and state-transition checks, not a whole-application latency benchmark.

## Supersedes

None.

## Revisit when

The Files view starts consuming diff totals or history, or snapshots become
incremental per data source. Re-measure before adding more scope levels.
