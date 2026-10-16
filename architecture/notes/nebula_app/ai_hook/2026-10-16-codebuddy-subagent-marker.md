# CodeBuddy subagent marker

## Status

Implemented.

## Context

CodeBuddy subagents share the parent `session_id`. If their events go through,
they can finish or replace the primary turn. So the CodeBuddy protocol gate drops
events that look like they come from a subagent. The first version of that gate,
modeled on Claude's payload documentation, treated either a non-empty `agent_id`
or a non-empty `agent_type` as the subagent marker.

In CodeBuddy CLI 2.163.0, the main session sends `"agent_type":"cli"` on
`PreToolUse`, `PostToolUse`, `PermissionRequest`, permission `Notification` and
`Stop`, and sends no `agent_id`. Only `SessionStart`, `UserPromptSubmit` and the
session-less `auth_success` notification lack the field. With the first gate, a
CodeBuddy pane went to Working on prompt submit and never left it: no Done, no
Blocked on permission requests, and no screen fallback in lifecycle mode to
correct the state.

## Evidence

- `scripts/tests/fixtures/codebuddy-2.163.0/main-session.jsonl` holds main-session
  payloads captured from CodeBuddy 2.163.0 through the installed command hooks.
  Paths are redacted. `Stop` comes from a second run of the same version, with
  its `session_id` rewritten to match the others.
- Before this change, sending those payloads to `parse_remote_envelope` dropped
  every event that carries `agent_type`. With the gate keyed on `agent_id`, they
  map to ToolComplete, NeedsAttention and TurnDone.
- `ai_hook::tests::codebuddy_main_session_payloads_drive_the_turn` and
  `ai_hook::lifecycle::tests::codebuddy_main_session_turn_reaches_done` cover this.

## Decision

Only a non-empty `agent_id` marks a CodeBuddy subagent event. An empty string
counts as no marker. `agent_type` is ignored by the gate. This matches the
existing Codex rule in `nebula_app/src/ai_hook/protocol.rs`.

## Rejected alternatives

- Treating `agent_type` as a subagent marker unless it equals `"cli"`. This
  passes the captured payloads, but the main session would be cut off again if
  CodeBuddy labels it with another value (for example IDE or SDK hosts).
- Keeping both fields and adding a carve-out per event. The marker stays wrong
  and only moves the failure to whichever event was not carved out.

## Consequences

If a CodeBuddy release ever sends subagent events with `agent_type` but no
`agent_id`, those events reach the primary turn. The worst case is an early Done
or a transient Blocked in the parent pane, which is easier to recover from than
a primary turn that can never finish.

## Validation

- The fixture tests above fail on the previous gate and pass with this one.
- `codebuddy_subagent_events_cannot_drive_the_primary_turn` still rejects payloads
  that carry `agent_id`.

## Supersedes

None.

## Revisit when

A real CodeBuddy subagent payload has been captured (Agent tool) and shows a
different marker, or a CodeBuddy upgrade changes the hook payload shape. The
upgrade review duty is recorded in `nebula_app/src/ai_hook.rs`.
