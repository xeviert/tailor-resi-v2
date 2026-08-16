# Resume Workbench token-cost evidence

Generated for the artifact-integrity incident reported on 2026-08-16.

## What happened

The application produced job-specific variants, but employer-facing Downloads files and Open/Reveal actions were addressed by a single overwriteable filename per language. A result summary could therefore be associated with a stale or different language-level artifact, especially for DOCX fallbacks, recovered results, or overlapping runs.

## Historical evidence available locally

- Variant window: 2026-08-14 through 2026-08-16.
- Variant directories currently present: 19.
- Default analysis model: `gpt-5.6-luna` unless `OPENAI_MODEL` was overridden.
- Default tailoring model: `gpt-5.6-terra` unless `OPENAI_TAILOR_MODEL` was overridden.
- Local evidence: `resume/variants/*/variant.json`, `tailoring-report.json`, rendered DOCX/PDF timestamps, and `data/tailoring-results/*.json`.
- Limitation: historical Responses API token usage and request IDs were not persisted, and a variant count is not a valid token estimate because retries can issue multiple tailoring requests.

## Cost reconciliation

Use the OpenAI organization Usage endpoint grouped by model and API key, and the Costs endpoint for the same 2026-08-14 through 2026-08-16 window. The official API reference is:

https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage

Future successful analysis and tailoring calls are recorded under `data/api-usage/` with response ID, requested/returned model, input tokens, cached input tokens, output tokens, reasoning tokens, and total tokens. API keys and prompts are not written to those records.

## Copy-ready support request

> A local resume-tailoring application made OpenAI Responses API calls for analysis and constrained resume rewriting between 2026-08-14 and 2026-08-16. A verified application bug caused employer-facing stable download filenames to become detached from the job-specific result shown in the UI, so some paid tailoring runs did not produce the artifact the user intended to submit. Please review the attached Usage/Costs export for the affected project/API key and determine whether any service credit is appropriate. The local run inventory and artifact timestamps are included for correlation.

OpenAI support determines whether any credit or refund is available; this application cannot issue account credits.
