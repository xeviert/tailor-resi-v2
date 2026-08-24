# Step 2: Review The Captured Job

## Purpose

The review screen is the decision point between capture and paid AI work. It presents the normalized job posting, lets the user choose the resume language, exposes import recovery when capture quality is suspect, and waits for an explicit **Analyze job** action. No analysis starts as a side effect of receiving, importing, loading, or rendering a capture.

This boundary prevents an incomplete extension extraction or a poor manual import from silently consuming an analysis request.

## How The Screen Gets Its Job

At startup, `src/main.tsx` invokes `get_latest_job`. The backend reads `data/job-captures/latest.json` through `server::load_latest_capture`; an absent or malformed pointer produces the empty state rather than an application failure.

While the app is running, a new capture arrives through the `job-data-received` Tauri event. Its payload is a `CapturedJob`. The listener compares `received_at_ms` with the capture already on screen, ignores an exact duplicate, and otherwise delegates to `resetForCapture`.

`resetForCapture` is the single owner of capture replacement. It sets the current capture, detects an initial output language from the job text, and clears the previous job's analysis, evidence preflight, selected terms, proofs, result/outcome, progress events, run state, and errors. Bypassing it can leave a prior job's result or evidence selections attached to a new posting.

## Job Rendering

`JobPanel` receives only `CapturedJob.parsed`. It has specialized views for Welcome to the Jungle, Wellfound, and Indeed; other sources use the generic view. Common normalized fields include title, company, location, job type, skills, qualifications, description, source URL, warnings, and optional company logo.

The panel sanitizes external source links to HTTP(S) and renders HTML descriptions through a narrow DOM-to-React conversion. It does not inject arbitrary markup. Parser warnings are visible in the job frame. A description is rendered in a bounded scrolling area to keep the review controls reachable for long postings.

The derived `captureLooksThin` flag is true if the normalized capture has parser warnings or its description is shorter than 400 characters. In that case the **Capture looks wrong? Import this job another way** disclosure starts open; otherwise it remains available but closed. The import panel reuses the Step 1 commands and, after success, relies on the common `job-data-received` reset path.

## Language And Transition To Analysis

The review screen holds an `en` or `fr` output-language selection. The initial selection comes from `detectLanguage(capture.parsed)` and can be changed before analysis. The analysis command receives this language because its evidence preflight must compare the job signals with the selected locked base resume.

Clicking **Analyze job** starts `analyze()`. It captures the current `received_at_ms` and selected language, begins a new UI run, clears a stale result, and invokes `analyze_latest_job`. The next screen state remains review-oriented while the analysis stages are in progress. Evidence resolution belongs to the result of analysis, but tailoring does not begin until the user later chooses document generation.

The screen model is intentionally narrow:

- `empty`: no current capture; show the manual import panel.
- `review`: capture loaded; show job details, language control, import recovery, and analysis/evidence controls.
- `pipeline`: tailoring has been requested.
- `completion`: a persisted pipeline result is being shown, unless the user explicitly returned to review.

## Stale And Abandoned Work

Every run increments a UI run ID. Results are accepted only when capture ID, language, and run ID match the current state. This prevents old command completions, events, or recovery reads from repainting the screen after a new capture or language choice.

The review screen's Cancel action abandons waiting, rather than cancelling the backend request. It restores usable review controls, records an explanatory error, increments the run ID, and ignores later progress/result events from the abandoned run. A later deliberate run reopens event acceptance.

## Files And Ownership

- `src/main.tsx`: application state, startup capture loading, Tauri event listeners, language selection, screen transitions, run identity, reset/cancel behavior, and the Analyze action.
- `src/job-panel.tsx`: safe rendering of normalized job details, board-specific layouts, warnings, tags, source links, and description content.
- `src/job-import.tsx`: the recovery panel mounted in empty and review states.
- `src-tauri/src/commands.rs`: `get_latest_job`, `clear_latest_job`, and the analysis command consumed from review.
- `src-tauri/src/server.rs`: `load_latest_capture` and capture storage semantics used by startup and reset flows.

## Relevant Tests

- `src/workflow.test.tsx` covers empty/review/completion transitions, initial and changed language behavior, capture replacement, duplicate capture events, thin-capture disclosure, and abandoned-run event suppression.
- `src/job-panel.tsx` should be changed alongside or covered by focused frontend tests whenever normalized job fields or source-specific rendering change.
- `src/job-import.test.tsx` protects the recovery panel's command contracts.
