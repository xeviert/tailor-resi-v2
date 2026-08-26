# Resume Workbench Codex Context

## Project Goal

Build a local resume-tailoring workflow that captures job posts, analyzes ATS-relevant signals, tailors resume JSON content truthfully, and renders locked-layout DOCX resumes.

## Current Architecture

- `browser-extension/` captures job-post data from supported job boards.
- `src-tauri/src/job_import.rs` is the manual way in when that capture is poor: it fetches a URL or takes pasted text, reads a schema.org `JobPosting` out of the page when the board publishes one (many do not) and falls back to AI extraction when it does not, then hands the result to the same `persist_capture` the extension route uses.
- `src-tauri/` runs the desktop bridge, local HTTP API, job parser, OpenAI job-analysis layer, and resume-tailoring layer. `/analyze` automatically runs analysis, tailoring, DOCX rendering, and locked-section validation when analysis succeeds.
- `resume/` contains the locked DOCX resume pipeline: source originals, templates, canonical content JSON, generated output, variants, QA artifacts, and render scripts.
- `data/job-captures/` stores runtime job-post payloads.

## Hard Invariants

- Do not edit `resume/content/base.en.json` or `resume/content/base.fr.json` for a single job application.
- Job-specific edits must be written as variants under `resume/variants/`.
- DOCX layout is locked. Header/contact details, section headings, and education/formation must not change during tailoring.
- The AI tailoring layer may rewrite only the professional summary, experience bullets, and skills strings unless the user explicitly expands scope.
- Never add unsupported credentials, tools, employers, metrics, responsibilities, certifications, or education. Unsupported high-value ATS terms belong in a report, not in the resume.
- The ATS score is measured, never reported by a model. `src-tauri/src/ats_score.rs` computes it from the generated document; the model's `estimated_ats_coverage_score` is advisory only and must not be used to drive logic or shown as the headline number.
- The evidence preflight and the ATS scorer must build their term ledger from the same `evidence::analysis_candidates`. If they diverge, the app asks the user to confirm one set of terms while scoring another.
- `payload.sourceText` on an imported capture holds the raw page or pasted text for debugging and must never reach a prompt. `prompt_job_view` only ever sees `parsed`, and the import branch of `parse_job_data` never populates `parsed["raw"]`.
- One deliberate exception: `max` bullet keyword emphasis may state a responsibility directly implied by a role's stated stack, title, and scope, and only inside a bullet it replaces outright. This is intentional - see `BulletKeywordEmphasis::Max` and the `invention_rule` branch in `src-tauri/src/tailoring.rs`. Credentials, tools, employers, metrics, certifications, and education stay invention-forbidden at every level, including `max`.

## AI Model Defaults

- Job analysis uses `OPENAI_MODEL`, defaulting to `gpt-5.6-luna` for cost-sensitive extraction and classification.
- Resume tailoring uses `OPENAI_TAILOR_MODEL`, defaulting to `gpt-5.6-terra` for higher-quality constrained rewriting. It does not inherit `OPENAI_MODEL`; set both env vars explicitly when overriding.
- Job import extraction shares `OPENAI_MODEL` with analysis on purpose: it is the same cheap extraction-and-classification workload, and its receipts land under the `job_import` stage.
- Keep model choices explicit; do not casually swap models without updating the rationale.

## AI Layer Notes

- Both OpenAI calls go through `crate::http::shared_client()`, which carries a timeout. Do not construct a bare `reqwest::Client` for a provider call.
- `server::prompt_job_view` is the only view of a capture that should reach a prompt. It drops `description_html`, `raw`, and other fields that duplicate content or carry no ATS signal.
- In `build_tailoring_prompt`, the large static payloads sit contiguously and the volatile retry-feedback blocks trail them. Do not move `concise_instruction` or `correction_instruction` back above the payloads: that breaks the cacheable prefix across the four retry attempts.
- `parse_job_data` routes on the `source` discriminator before it looks at the URL. An imported capture's `sourceUrl` may well point at a board that has a scraper, but the payload was never produced by that scraper, so that parser would read the wrong shape out of `json`.
- The page fetch in `job_import` uses `shared_client()` with a 20-second per-request timeout override. The client-level 300s bound exists for a reasoning model doing a full resume rewrite; applying it to a dead job link turns it into a five-minute hang.
- Never send `Accept-Encoding` on the page fetch. reqwest is built with `default-features = false` and no compression features, so a compressed response would arrive undecoded.
- Preflight heuristics (`is_generic_trait`, `is_job_title`, `is_specific_responsibility`, `inferred_kind`, the `token_set` stop list) are bilingual. An FR resume path ships, so English-only literals silently let French boilerplate through.

## Verification

Run Rust tests from `src-tauri/`:

```powershell
cargo test
```

Render and validate resume documents from the repo root:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 render -Lang en
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 render -Lang fr
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 validate -Lang en -Docx .\resume\generated\Xevier_T_CV_en.generated.docx
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 validate -Lang fr -Docx .\resume\generated\Xevier_T_CV_fr.generated.docx
```

## Notes For Future Agents

- Prefer existing Rust modules and PowerShell render scripts over adding a new document-generation path.
- Keep API additions explicit and backwards-compatible. Existing `/health`, `/analyze`, and `/api/ollama` behavior should remain stable.
- Treat generated Rust build output under `src-tauri/target/` as disposable.
