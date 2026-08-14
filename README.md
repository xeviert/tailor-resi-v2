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
