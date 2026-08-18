# Resume Workbench

Resume Workbench is a local resume-tailoring workspace with three main pieces:

- `src-tauri/`: Tauri desktop bridge and AI job-analysis layer. It receives normalized job posts, extracts ATS-relevant keywords and signals, and emits the analysis for later use.
- `browser-extension/`: browser extension that captures job-post data from supported job boards.
- `resume/`: locked DOCX resume templates, editable JSON resume content, render scripts, generated documents, and archived variants.
- `data/job-captures/`: runtime job-post capture payloads, including `latest.json`.
- `data/tailoring-results/`: versioned terminal outcomes used to restore the latest analysis summary after missed events or an app restart.

## Current User Flow

The normal desktop workflow contains six main steps, plus an optional re-tailoring
loop after reviewing the result. The user's five-step outline is mostly correct,
but the evidence preflight step is separate from raw analysis because it decides
which analyzed terms are already supported, which need user attestation, and which
must stay out of the resume.

1. Capture a job post from the browser extension.

   The user opens a job post in the browser and clicks **Extract Job** in the
   ResiTailor extension. The extension extracts the title, company, description,
   URL, and related page metadata, then posts the payload to
   `POST http://127.0.0.1:3000/captures`.

   The `/captures` route is storage-only. It normalizes the incoming job data,
   writes a timestamped capture under `data/job-captures/`, updates
   `data/job-captures/latest.json`, and emits the captured job to the desktop UI.
   It does not run AI analysis or tailoring.

2. Review the captured job in the desktop app.

   The desktop UI loads the latest capture and shows the normalized job details.
   At this point the app is waiting for the user to choose the output language
   and start analysis. If no capture exists, the UI stays on the "Capture a job
   post to begin" state.

3. Analyze the job post for ATS signals.

   When the user clicks **Analyze job**, the UI invokes `analyze_latest_job`.
   The backend sends the normalized job JSON to the OpenAI Responses API using
   `OPENAI_MODEL` and returns a structured `JobAnalysis`.

   The analysis schema captures:

   - role target and seniority
   - high-importance `core_keywords` with evidence from the job post
   - required skills, preferred skills, tools, platforms, and domain terms
   - responsibility phrases and achievement angles
   - an `ats_phrase_bank` for later writing
   - `must_not_claim_without_evidence` terms that are valuable but unsupported
   - a concise analysis summary for the UI

   This step identifies ATS-relevant terms and phrases, but it does not rewrite
   resume content.

4. Resolve evidence and select terms for the internal bank.

   After analysis, the backend runs evidence preflight for the selected language.
   It compares the analyzed terms against the locked base resume and the local
   evidence bank at `resume/evidence-bank.json`.

   Each candidate term receives one of three resolutions:

   - `auto_available`: already supported by the base resume or a previously saved
     evidence-bank entry.
   - `confirmation_required`: important factual claim not found in existing
     evidence. The user can select it and provide a proof note before tailoring.
   - `auto_omitted`: weak, generic, or unsupported signal that should not be
     placed into the resume automatically.

   When the user proceeds, selected confirmations are saved back to the evidence
   bank and passed into tailoring as user-attested evidence. Unsupported high-value
   ATS terms remain in the report instead of being added to the resume.

5. Tailor the resume and run the locked document pipeline.

   When the user clicks **Generate tailored PDF**, the UI invokes
   `generate_tailored_resume`. The request includes the selected language, the
   prior `JobAnalysis`, selected evidence, and the experience keyword emphasis
   level (`low`, `balanced`, or `high`).

   The tailoring layer uses `OPENAI_TAILOR_MODEL` and may rewrite only experience
   bullet text and skills strings. It must preserve metadata, companies, titles,
   dates, job order, bullet counts, skill keys, education, section headings, and
   contact/header layout. The generated resume content is validated before any
   document is treated as usable.

   The pipeline then runs these backend stages:

   - `resume_tailoring`: generate truthful tailored resume JSON and a tailoring
     report.
   - `safety_validation`: verify locked JSON shape and factual constraints; in
     high-emphasis mode, ensure experience bullets were actually rewritten.
   - `variant_write`: save the job-specific variant under `resume/variants/`.
   - `docx_render`: render the locked-layout DOCX using the existing resume
     PowerShell pipeline.
   - `locked_validation`: validate that locked sections did not change.
   - `pdf_fit`: export PDF and confirm it fits on one page.

   If the PDF exceeds one page, the backend asks the tailoring model for a more
   concise rewrite and retries within the configured attempt limit. If PDF export
   or one-page fit fails after a valid DOCX is produced, the result is marked
   `partial` and the validated DOCX can still be published.

6. Receive the result, artifacts, and analysis summary.

   The UI receives a persisted pipeline result and shows the analysis summary,
   ATS score, changed fields, omitted unsupported keywords, and available artifact
   actions. Successful variants include `variant.json`, `tailoring-report.json`,
   the rendered DOCX, and usually a one-page PDF.

   A verified employer-facing artifact is atomically published to the stable
   `Downloads/Xevier_T_CV_<lang>.pdf` filename. If PDF output is not available
   but a DOCX passed validation, the validated DOCX is published instead. Only one
   of those two stable employer-facing files is kept per language. Each published
   artifact gets an `artifact-manifest.json` provenance file in its variant
   directory.

7. Optionally re-tailor from omitted terms.

   After a completed or partial run, omitted unsupported keywords are shown as
   selectable pills. If the user selects one or more omitted terms and clicks
   **Re-tailor selected**, the UI invokes `retailor_resume_with_evidence`.

   Re-tailoring loads the previous source variant and analysis, validates that the
   selected terms came from the source result's omitted list, records them as
   user-attested placement terms where allowed, and reruns the same tailoring and
   document pipeline. The result records the source variant, previous score, and
   selected terms so the UI can show the score delta.

### Alternate and Legacy Paths

The browser extension sends captures to `/captures`; this is deliberately separate
from the legacy `/analyze` endpoint. `/analyze` remains available for older
integrations that expect a single HTTP call to persist the capture, run analysis,
run tailoring, render DOCX, validate layout, and return a combined response.

The desktop UI uses the reviewed command path instead:

- `analyze_latest_job` for analysis plus evidence preflight.
- `prepare_evidence_preflight` when switching EN/FR after analysis.
- `generate_tailored_resume` for reviewed tailoring and artifact generation.
- `retailor_resume_with_evidence` for the optional omitted-term loop.

Uploading to third-party job forms remains a manual browser step: use the desktop
app's Open PDF/Open folder actions to select the stable file. The `/health`
response includes `result_protocol_version`; a mismatch tells the user to fully
restart the desktop app instead of silently running an incompatible UI/backend
pair.

Successful OpenAI calls also write usage-only receipts under `data/api-usage/` so
future token and cost investigations can be tied to the analysis or tailoring
stage without persisting API keys or prompt contents.

## Desktop UI and Linux

Run `npm install`, then `npm run dev:desktop` to start the Vite frontend and Tauri
desktop window together. The desktop command uses a pinned Tauri CLI through `npx`,
so it also works when a stale `node_modules` directory does not yet contain that CLI.
Use `npm run dev` only when you want the browser frontend server by itself. PDF generation requires LibreOffice. The renderer uses Windows PowerShell on Windows and
PowerShell 7 (`pwsh`) on Linux; Linux Tauri builds also need the standard WebKitGTK
system packages for the target distribution.

### Local AI configuration

The debug desktop app loads a local `.env` file from the repository root. Copy the
template, add your own API key, then start the app:

```powershell
Copy-Item .env.example .env
# Edit .env and set OPENAI_API_KEY=sk-proj-...
npm run dev:desktop
```

`.env` is ignored by Git. Process environment variables take precedence over `.env`
when both are set. The release build does not load `.env`; the packaged app will use
an in-app Settings flow backed by the operating system's secure credential store.

## AI Models

- `OPENAI_API_KEY` is required for both AI stages.
- `OPENAI_MODEL` controls job-post analysis and defaults to `gpt-5.6-luna`.
- `OPENAI_TAILOR_MODEL` independently controls resume tailoring and defaults to `gpt-5.6-terra`.
- `OPENAI_BASE_URL` is optional and overrides the default OpenAI API base URL (`https://api.openai.com/v1`).

## Resume Documents

Detailed resume template and render instructions live in `resume/README.md`.
