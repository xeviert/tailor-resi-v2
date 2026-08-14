# Resume Workbench

Resume Workbench is a local resume-tailoring workspace with three main pieces:

- `src-tauri/`: Tauri desktop bridge and AI job-analysis layer. It receives normalized job posts, extracts ATS-relevant keywords and signals, and emits the analysis for later use.
- `browser-extension/`: browser extension that captures job-post data from supported job boards.
- `resume/`: locked DOCX resume templates, editable JSON resume content, render scripts, generated documents, and archived variants.
- `data/job-captures/`: runtime job-post capture payloads, including `latest.json`.

## Current Flow

1. The browser extension captures a job post.
2. The Tauri bridge receives and normalizes the job data.
3. The AI analysis layer extracts ATS keywords, skills, tools, responsibility phrases, and risk signals.
4. The backend automatically uses that analysis to create a truthful tailored resume variant.
5. The resume workbench renders and validates tailored DOCX output from locked templates.

The browser extension still only sends data to `/analyze`; the backend handles
tailoring, rendering, and validation locally. The extension UI does not display
generated resume paths yet.

## AI Models

- `OPENAI_MODEL` controls job-post analysis and defaults to `gpt-5.6-luna`.
- `OPENAI_TAILOR_MODEL` independently controls resume tailoring and defaults to `gpt-5.6-terra`.
- `OPENAI_BASE_URL` can override the OpenAI API base URL.

## Resume Documents

Detailed resume template and render instructions live in `resume/README.md`.
