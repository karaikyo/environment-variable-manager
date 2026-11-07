# Controlled native acceptance session

This is a procedure, not passing evidence. Record the release hash, Windows build,
display scale, account context, date and result for each check. Automated command
and fixture tests do not replace these desktop checks. The approved specification
and acceptance gates remain authoritative.

## Safe setup

Use a clean extracted application folder with disposable `data`, separate from
any personal history. Keep the existing application exited. Record the application's
startup preference and its own registration before the session. Do not change
unrelated startup entries or existing environment variables.

Choose fresh user and system variable names containing a newly generated unique
suffix. Confirm neither name exists in either scope before creation. Track exactly
which fixtures were created; stop on a collision. Cleanup may remove only those
owned fixtures whose current state is still known. Do not suspend the active PC,
restart Explorer or invoke administrator approval until the user has agreed to
that concrete session.

## Main window and history

1. Browse both scopes, search by name and raw value, and switch scope filters using
   the keyboard. Verify visible scope, labels, focus and selected-variable state.
2. Create a fresh user fixture through the app. Exercise spaces, commas, duplicate
   entries, leading/trailing empty entries, an empty whole value, raw percent
   references and embedded CR/LF inside an entry. Verify one logical entry per row
   and exact saved text/type; use disposable fixtures for each case as needed.
3. Edit two fixtures, switch selection/filter, and discard one draft. Confirm the
   other draft survives and navigation never saves. Save directly without review.
4. Inspect selected and all-variable history. Verify added/edited entries in After,
   deleted entries in Before, neutral unchanged entries, text labels, exact empty
   entries and distinct absence. Check paging and inline clear/cancel on disposable
   history; current values and recovery state must survive clearing.
5. Repeat keyboard/focus checks in the history window, at the intended high-DPI
   settings and with high contrast. Check screen-reader names and state reporting.

## External writers and lifecycle

1. While monitoring is healthy, change only owned fixtures through the Windows
   environment-variable UI and a persistent command-line writer. Verify observed
   history, unknown actor/observation time, and preserved conflicting drafts.
   A process-local shell assignment must not create persistent-variable history.
2. Close to tray, change an owned fixture externally, and reopen. Confirm continued
   observation and preserved drafts. Exercise Cancel exit and Exit and discard
   unsaved edits, including a draft on an unselected variable. Check clean exit.
3. In an agreed session, verify sign-in startup, explicit disablement, enabled
   folder-move repair and tray recovery after Explorer restart. Preserve unrelated
   registrations; restore the user's intended application startup preference.
4. In an agreed power-transition session, test ordinary sleep and hibernation,
   unchanged and changed baselines, successive cycles and a preserved draft.
   Verify watcher recovery, labeled net gap differences, no duplicates, and a
   visible interrupted state with writes gated if recovery fails.

## System writes and release proof

1. In an agreed elevated session, create/edit/delete only a fresh system fixture
   using same-account UAC consent. Exercise cancellation, rejection and interrupted
   outcomes. Main window/history must remain unelevated. Do not automate UAC.
2. Run the release's documented resource procedure with the reference workload:
   30 seconds settling, then 60 one-second private-byte samples across all app
   processes, initially and after history/resume. Record CPU, working set, peaks,
   hardware, cold/warm readiness and archive/extracted size separately.
3. Record manual update, migration/backup and moved-folder evidence using disposable
   data. Keep actual fault results distinct from injected or simulated failures.

For every unperformed or failed check, record that state and its reason. Do not
close acceptance gates on the strength of this checklist alone.
