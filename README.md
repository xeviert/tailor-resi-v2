# Resume Workbench

Resume Workbench is a local resume-tailoring workspace with three main pieces:

- `src-tauri/`: Tauri desktop bridge and AI job-analysis layer. It receives normalized job posts, extracts ATS-relevant keywords and signals, and emits the analysis for later use.
- `browser-extension/`: browser extension that captures job-post data from supported job boards.
- `resume/`: locked DOCX resume templates, editable JSON resume content, render scripts, generated documents, and archived variants.
- `data/job-captures/`: runtime job-post capture payloads, including `latest.json`.

## Current Flow

1. The browser extension captures a job post.
2. The Tauri bridge receives and normalizes the job data.
3. The desktop UI shows the normalized job details and waits for your chosen action.
4. The UI runs AI analysis, truthful tailoring, DOCX validation, and a one-page PDF export.
5. Each tailored variant is archived, while `resume/generated/Xevier_T_CV_en.pdf` or `Xevier_T_CV_fr.pdf` is replaced as the ready-to-upload local file.

The browser extension sends captures to `/captures`; this is deliberately separate
from the legacy `/analyze` endpoint, which remains available for integrations that
expect its automatic behavior. Uploading to third-party job forms remains a manual
browser step: use the desktop app's Open PDF/Open folder actions to select the stable file.

## Desktop UI and Linux

Run `npm install`, then `npm run dev` alongside the Tauri development command. PDF
generation requires LibreOffice. The renderer uses Windows PowerShell on Windows and
PowerShell 7 (`pwsh`) on Linux; Linux Tauri builds also need the standard WebKitGTK
system packages for the target distribution.

## AI Models

- `OPENAI_MODEL` controls job-post analysis and defaults to `gpt-5.6-luna`.
- `OPENAI_TAILOR_MODEL` independently controls resume tailoring and defaults to `gpt-5.6-terra`.
- `OPENAI_BASE_URL` can override the OpenAI API base URL.

## Resume Documents

Detailed resume template and render instructions live in `resume/README.md`.
