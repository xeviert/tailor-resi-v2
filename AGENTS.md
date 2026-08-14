# Resume Workbench Codex Context

## Project Goal

Build a local resume-tailoring workflow that captures job posts, analyzes ATS-relevant signals, tailors resume JSON content truthfully, and renders locked-layout DOCX resumes.

## Current Architecture

- `browser-extension/` captures job-post data from supported job boards.
- `src-tauri/` runs the desktop bridge, local HTTP API, job parser, OpenAI job-analysis layer, and resume-tailoring layer. `/analyze` automatically runs analysis, tailoring, DOCX rendering, and locked-section validation when analysis succeeds.
- `resume/` contains the locked DOCX resume pipeline: source originals, templates, canonical content JSON, generated output, variants, QA artifacts, and render scripts.
- `data/job-captures/` stores runtime job-post payloads.

## Hard Invariants

- Do not edit `resume/content/base.en.json` or `resume/content/base.fr.json` for a single job application.
- Job-specific edits must be written as variants under `resume/variants/`.
- DOCX layout is locked. Header/contact details, section headings, and education/formation must not change during tailoring.
- The AI tailoring layer may rewrite only experience bullets and skills strings unless the user explicitly expands scope.
- Never add unsupported credentials, tools, employers, metrics, responsibilities, certifications, or education. Unsupported high-value ATS terms belong in a report, not in the resume.

## AI Model Defaults

- Job analysis uses `OPENAI_MODEL`, defaulting to `gpt-5.6-luna` for cost-sensitive extraction and classification.
- Resume tailoring uses `OPENAI_TAILOR_MODEL`, defaulting to `gpt-5.6-terra` for higher-quality constrained rewriting. It does not inherit `OPENAI_MODEL`; set both env vars explicitly when overriding.
- Keep model choices explicit; do not casually swap models without updating the rationale.

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
