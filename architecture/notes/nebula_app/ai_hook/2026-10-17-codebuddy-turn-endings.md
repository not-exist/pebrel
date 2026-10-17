# CodeBuddy turn endings: questions, interrupts and denials

## Status

Implemented, with one known gap (permission denial).

## Context

The first CodeBuddy mapping copied Claude's turn semantics. Captures from
CodeBuddy CLI 2.163.0 in TUI mode show three places where CodeBuddy differs:

1. `AskUserQuestion`. When the question appears, CodeBuddy sends
   `Notification(permission_prompt)` ("needs your permission to use
   AskUserQuestion"). It sends this under `bypassPermissions` too.
   `PreToolUse(AskUserQuestion)` only arrives after the user has answered, and
   `PostToolUse` follows within a few milliseconds. Claude sends `PreToolUse`
   before it shows the question.
2. Esc interrupt. Pressing Esc while the model is generating or a tool is running
   still sends `Stop`; during a tool, `PostToolUse` comes first and no
   `PostToolUseFailure` is sent. The `Stop` payload carries no interrupt flag or
   stop reason. CodeBuddy sets `last_assistant_message` to `"Interrupted by user"`,
   a fixed constant in its bundle that is not localized.
3. Permission denial. Pressing Esc at a permission prompt selects "No, and tell
   CodeBuddy what to do differently". The turn ends and the input box returns,
   but CodeBuddy sends no hook: no `Stop`, no `PostToolUseFailure` and no
   `PermissionDenied`. That last event exists in the bundle, but a command hook
   registered for it did not fire on a denial in the TUI. `FinalStop` is
   documented as internal-only and did not fire either.

## Evidence

- `scripts/tests/fixtures/codebuddy-2.163.0/ask-user-question.jsonl`: a question
  turn, which contains a single `permission_prompt`.
- `scripts/tests/fixtures/codebuddy-2.163.0/esc-interrupt.jsonl`: Esc during a
  running `Bash` tool.
- The denial capture logged every hook event, plus `PermissionDenied`,
  `FinalStop`, `SubagentStart` and `SubagentStop`, through a raw logger hook.
  Nothing arrived between `Notification(permission_prompt)` and `SessionEnd`.

## Decision

- `PreToolUse(AskUserQuestion)` signals a wait only for `claude`. For
  `codebuddy` it is an ordinary tool boundary (ToolComplete). Only the earlier
  `permission_prompt` asks for attention.
- A codebuddy `Stop` whose `last_assistant_message` is exactly
  `"Interrupted by user"` is `AiTurnOutcome::Cancelled`. The pane returns to
  Idle and no completion notification fires. Any other text, including the
  same string from `claude`, stays `Succeeded`.
- Permission denial is not handled. With no event, the pane stays Blocked until
  the next `UserPromptSubmit` or `SessionEnd`. Lifecycle coverage disables
  screen detection, so nothing corrects the pane in between.

## Rejected alternatives

- Treating every codebuddy `PreToolUse(AskUserQuestion)` as a wait. It fired a
  second attention notification just after the user answered.
- Detecting interrupts from the transcript file. The transcript is written
  asynchronously, and it adds file I/O to the hook path for a signal the
  payload already carries.
- Re-enabling screen detection for a Blocked codebuddy pane to catch a denial.
  This is a coverage-model change that should also cover Claude, which has the
  same class of gap. It needs its own decision.

## Consequences

If CodeBuddy localizes or rewords the interrupt text, interrupts are reported
as completions again. The fixture test pins the current text, so a re-capture
during the upgrade review catches the change.

## Validation

- `ai_hook::lifecycle::tests::codebuddy_question_turn_asks_for_attention_once`
- `ai_hook::lifecycle::tests::codebuddy_interrupted_turn_returns_to_idle`
- `ai_hook::tests::codebuddy_events_map_to_the_shared_lifecycle`
- `ai_hook::native_tests::codebuddy_interrupt_does_not_announce_completion`

## Supersedes

None.

## Revisit when

CodeBuddy sends a hook on permission denial or adds a stop reason to `Stop`, or
Pebrel adopts a recovery path for Blocked panes that have no closing hook.
