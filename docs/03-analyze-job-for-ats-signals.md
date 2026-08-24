# Step 3: Analyze The Job For ATS Signals

## Purpose

Analysis converts the normalized captured job into a structured `JobAnalysis` for later evidence resolution and resume tailoring. It identifies what the posting asks for; it does not change resume content, measure final ATS coverage, write a variant, render a document, or publish an artifact.

The desktop UI calls the `analyze_latest_job` Tauri command only after the user explicitly chooses **Analyze job** from review.

## Command Flow

`analyze_latest_job(app, language)` performs the following sequence:

1. Load the current capture from `latest.json` and derive its timestamp as the capture identity.
2. Emit an `ats_analysis` `started` event on `resume-pipeline-progress`.
3. Load `AnalysisConfig` from the environment; `OPENAI_API_KEY` is required.
4. Call `analysis::analyze_job` with the captured normalized `parsed` job.
5. Emit `ats_analysis` completion or failure.
6. Run evidence preflight for the requested language, emitting `evidence_preflight` progress events.
7. Persist and emit an `analysis_ready` result snapshot, then return `PreflightResult { analysis, items }` to the UI.

The evidence preflight is part of the analysis command because it decides which analyzed terms are already supported and which require user attestation. It is not resume tailoring. The next workflow step consumes the returned items and asks the user to resolve them before generation.

## Model Configuration And Request

`AnalysisConfig::from_env` reads:

- `OPENAI_API_KEY`: required non-empty API key.
- `OPENAI_MODEL`: analysis and import-extraction model, defaulting to `gpt-5.6-luna`.
- `OPENAI_BASE_URL`: optional Responses API base URL, defaulting to `https://api.openai.com/v1`.

`analyze_job` sends a structured-output request to the OpenAI Responses API with the shared HTTP client. It retries transient request failures, rate limits, and retryable server failures; non-retryable HTTP errors and invalid structured responses are terminal. Successful responses record usage-only metadata under `data/api-usage/` with the `job_analysis` stage. Receipts contain no API key or prompt body.

Do not use a bare `reqwest::Client` for this call. The shared client defines the request timeout expected by the analysis and tailoring layers.

## Prompt-Safe Input

`build_analysis_prompt` serializes `server::prompt_job_view(parsed_job)`, never the original capture payload. The view removes fields such as `description_html`, `raw`, and `sourceText`, drops null values, and preserves the normalized ATS-relevant job fields.

This is especially important for manual imports. Their raw fetched or pasted page text lives in `payload.sourceText` solely for debugging. It must not be forwarded to the model again during analysis. Imported captures are routed by `source` before board URL parsing so their normalized `parsed` shape stays correct.

## JobAnalysis Contract

The response schema maps to `analysis::JobAnalysis`:

- `role_target` and `seniority` identify the target role.
- `core_keywords` contains a term, category (`technology`, `method_domain`, or `responsibility`), 1-5 importance, and job-post evidence.
- `required_skills`, `preferred_skills`, `tools_and_platforms`, `domain_terms`, and `responsibility_phrases` provide the ATS term groups.
- `achievement_angles` and `ats_phrase_bank` support later constrained writing.
- `must_not_claim_without_evidence` identifies valuable claims that cannot enter a resume without support.
- `term_variants` groups literal equivalents such as an acronym and its expansion so downstream matching can recognize either spelling.
- `summary` is the concise review/result explanation.

The prompt requires every extracted field to use the posting's language, asks for grounded and complete job signals, and explicitly forbids resume rewriting. The schema is strict: additions to `JobAnalysis` require matching updates to the Rust type, response schema, prompt, UI types, test fixtures, and downstream consumers.

## Progress, Results, And Cancellation

Analysis reports only two progress stages to the UI: `ats_analysis` and `evidence_preflight`. The document pipeline stages must not be shown during this operation because no document work has been requested.

The UI can abandon its wait through Cancel, but that does not abort the already-dispatched Rust/OpenAI request. The backend completes its request and usage recording; the UI increments its run ID and drops late events and results until the user deliberately starts another run. Failures are persisted as result snapshots with the failed stage and message so recovery after a command rejection can show the correct terminal state.

## Evidence Handoff

`prepare_preflight_result` compares the analysis with the selected language's base resume and `resume/evidence-bank.json`. Its candidate ledger is built by `evidence::analysis_candidates`, the same function used later by `ats_score`. This shared source is an invariant: preflight must not ask the user to confirm a term set different from the set later measured for ATS coverage.

Preflight items are resolved as `auto_available`, `confirmation_required`, or `auto_omitted`. The analysis document should not describe these states as resume claims: only later evidence resolution and tailoring determine what can be placed.

## Files And Ownership

- `src-tauri/src/commands.rs`: `analyze_latest_job`, analysis/evidence progress reporting, failure snapshots, and the `PreflightResult` command response.
- `src-tauri/src/analysis.rs`: configuration, `JobAnalysis` types, prompt, strict response schema, Responses API request, retry policy, and response parsing.
- `src-tauri/src/evidence.rs`: analysis candidate ledger and evidence-preflight classification.
- `src-tauri/src/server.rs`: `prompt_job_view` and normalized capture loading.
- `src-tauri/src/http.rs` and `src-tauri/src/api_usage.rs`: shared HTTP behavior, retry helpers, and usage-only receipt recording.
- `src/main.tsx`: analysis invocation, two-stage progress display, run/cancel behavior, and application of the returned preflight.

## Relevant Tests

- `src-tauri/src/analysis.rs` tests validate prompt contents, strict output parsing, term variants, and retry/error handling.
- `src-tauri/src/commands.rs` tests validate analysis outcomes, language handling, evidence-preflight handoff, and persisted result recovery.
- `src-tauri/src/server.rs` tests verify that `prompt_job_view` excludes duplicate HTML and raw import text.
- `src-tauri/src/evidence.rs` and `src-tauri/src/ats_score.rs` tests protect the shared candidate-ledger invariant.
- `src/workflow.test.tsx` verifies explicit analysis invocation, visible analysis stages, and cancellation behavior.
