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

1. Capture or import a job post.

   The usual path is the browser extension: the user opens a job post and clicks
   **Extract Job**. The extension extracts the title, company, description, URL,
   and related page metadata, then posts the payload to
   `POST http://127.0.0.1:3000/captures`.

   The `/captures` route is storage-only. It normalizes the incoming job data,
   writes a timestamped capture under `data/job-captures/`, updates
   `data/job-captures/latest.json`, and emits the captured job to the desktop UI.
   It does not run AI analysis or tailoring.

   The extension is best-effort and has no minimum-score gate, so a page it
   cannot read properly still produces a capture - sometimes one whose whole
   description is a single marketing sentence scraped out of an `og:description`
   meta tag. For that case the desktop app can bring the posting in itself, from
   the **Import a job post** panel on the empty screen or the **Capture looks
   wrong?** panel under a capture that is already on screen. That panel opens on
   its own when the capture carries parser warnings or a suspiciously short
   description.

   Importing takes either a URL or the pasted text of the posting:

   - **From URL** fetches the page and looks for a schema.org `JobPosting` in a
     `<script type="application/ld+json">` block. When a board publishes one it
     is the whole posting, read exactly and for no tokens. Many boards do not -
     several of the big ATS hosts render everything client-side and ship no
     structured data - so when it is missing the stripped page text goes to the
     AI layer for extraction instead.
   - **Paste text** always uses the AI layer, because there is no markup left to
     read structured data out of.

   Expect the URL mode to fail on some boards. LinkedIn and anything behind
   Cloudflare refuse requests that do not come from a real browser, and a page
   that builds itself with JavaScript arrives as an empty shell. Both failures
   say so and point at the paste mode; that is the documented workaround, not a
   bug. Either way the import lands as an ordinary capture and the app stops at
   step 2 for review - it never starts analysis on its own, so a bad extraction
   cannot quietly spend an analysis call.

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
   - high-importance `core_keywords` with evidence from the job post, each
     categorized as `technology`, `method_domain`, or `responsibility`
   - required skills, preferred skills, tools, platforms, and domain terms
   - responsibility phrases and achievement angles
   - an `ats_phrase_bank` for later writing
   - `must_not_claim_without_evidence` terms that are valuable but unsupported
   - `term_variants`: alternate written forms of extracted terms
   - a concise analysis summary for the UI

   `term_variants` exists because applicant tracking systems match literal
   strings: `Kubernetes` and `K8s`, or `CI/CD` and `continuous integration`, are
   one capability but two different tokens. The scorer counts a hit on any listed
   form, and the tailoring layer is told to prefer whichever form the job post
   itself used.

   Only the `parsed` view of the capture is sent, minus fields that duplicate
   another field or carry no ATS signal — notably `description_html`, which is
   the same prose as `description` wrapped in markup. Analysis retries a
   transient rate limit or server fault; every other error is terminal.

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
   bank. Tailoring then receives both those confirmations and every previously
   attested bank entry this job's preflight resolved, so a first run is as
   well-informed as a re-tailor. Unsupported high-value ATS terms remain in the
   report instead of being added to the resume.

   An entry with neither a proof note naming a role or project nor
   `allow_model_role_placement` can only support a skills string, never an
   experience bullet. That boundary is what keeps attestation from turning into
   invention.

5. Tailor the resume and run the locked document pipeline.

   When the user clicks **Generate tailored PDF**, the UI invokes
   `generate_tailored_resume`. The request includes the selected language, the
   prior `JobAnalysis`, selected evidence, and the experience keyword emphasis
   level (`low`, `balanced`, `high`, or `max`).

   The tailoring layer uses `OPENAI_TAILOR_MODEL` and may rewrite only experience
   bullet text and skills strings. It must preserve metadata, companies, titles,
   dates, job order, bullet counts, skill keys, education, section headings, and
   contact/header layout. The generated resume content is validated before any
   document is treated as usable.

   `max` is the strongest level and a strict superset of `high`. On top of
   rewriting every bullet, it replaces 1 to 3 of the least job-relevant bullets
   outright: the original angle is discarded and a new bullet is written against
   the job's highest-importance signals. Replacements happen in place, so job and
   bullet counts never change. At most one bullet per role may be replaced, and
   each replacement must stay grounded in this person's real work in that role -
   other base-resume facts, the attested evidence bank, or a responsibility
   directly implied by that role's stated stack, title, and scope. Employers,
   titles, dates, credentials, certifications, education, tools, and metrics
   remain invention-forbidden at every level. Each swap is recorded in the
   tailoring report as a `replaced` bullet-rewrite decision with a rationale, and
   the desktop UI lists the before/after text so the user can review what changed.

   The pipeline then runs these backend stages:

   - `resume_tailoring`: generate truthful tailored resume JSON and a tailoring
     report.
   - `safety_validation`: verify locked JSON shape and factual constraints; in
     high- and max-emphasis modes, ensure experience bullets were actually
     rewritten. Max additionally enforces the 1-3 replacement budget, the
     one-replacement-per-role limit, and prose sanity checks that reject a
     keyword-stuffed or overlong replacement. A rejected response is sent back to
     the model for correction within the attempt limit. Once a response is
     accepted, this stage measures ATS keyword coverage of the generated content
     and rebuilds the report's covered/omitted lists from that measurement.
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
   measured ATS coverage, changed fields, omitted keywords, and available artifact
   actions. Successful variants include `variant.json`, `tailoring-report.json`,
   the rendered DOCX, and usually a one-page PDF.

   The summary is headed by the job it belongs to - the captured title, the company,
   the output language, the emphasis level that produced it, and a link back to the
   original post. None of that is stored in the result; it comes from the capture on
   screen, which every recovery path already pins to the same `capture_received_at_ms`.

   Stepping back to the captured job keeps the finished result in memory. The review
   screen then offers **Back to tailored result** to reopen the completed summary
   without spending another tailoring run, and its primary action reads **Re-run
   tailoring** to make clear that it would overwrite the existing result.

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

### ATS Coverage Scoring

The ATS score is computed in `src-tauri/src/ats_score.rs`, not reported by a model.

The job analysis yields a weighted ledger of the terms the post asks for, built by
`evidence::analysis_candidates` — the same function the evidence preflight uses, so
the two can never disagree about what the job wants. Weights are `required_skills`
5, `tools_and_platforms` and `responsibility_phrases` 4, `preferred_skills` and
`domain_terms` 3, and for `core_keywords` the model's own 1-5 importance. Terms
appearing in several arrays are consolidated, keeping the highest weight.

Each term is then tokenized — lowercased, split on non-alphanumerics but keeping
`+`, `#`, and `.` so `C++`, `C#`, and `Node.js` survive, with stop words dropped in
both English and French, and French inclusive-writing suffixes stripped so
`référent·e` matches `référent`. A term's `coverage_ratio` is the share of its
tokens present in the document, taking the best of the term's own wording and its
`term_variants`. The score is the weighted sum of those ratios over the total
weight.

Matching runs against the whole document at once — experience bullets, skills
strings, and the locked company names, titles, dates, and education — because an
ATS reads it as one text. Requiring every token inside a single bullet would report
`state management avancé` as absent from a resume whose skills lines say
`state management` and `debugging avancé`.

Partial credit exists because the analysis returns multi-word requirement phrases,
not only single keywords. Scoring `produire et maintenir des RFC / ADR` all-or-
nothing against a resume that names ADR but not RFC both understates the score and
hides which half is missing. `covered` stays strict — it means every token is
present — so the per-group counts report full matches, with partial matches shown
alongside them.

A term matched only in locked text is reported as covered but flagged
`in_editable_surface: false`, so a keyword that happens to sit in an employer name
is not credited to the tailoring pass.

Every miss is classified:

- `no_evidence`: nothing in the base resume or the evidence bank supports it. The
  user must attest to it before it can be claimed. These become the selectable
  pills in the "Still not added" block.
- `evidence_not_placed`: the preflight already cleared it and the tailoring pass
  still did not use it. This is free, truthful coverage that was left on the table,
  and the UI lists it separately so the user is never asked to vouch for something
  they have already vouched for.

`covered_keywords` and `omitted_unsupported_keywords` in the tailoring report are
rebuilt from this measurement. The model also emits its own
`estimated_ats_coverage_score`; it is retained for comparison but drives nothing.
The re-tailor delta is computed from two measured scores, and a negative delta
renders as a regression rather than in the same green as a gain.

### Alternate and Legacy Paths

The browser extension sends captures to `/captures`; this is deliberately separate
from the legacy `/analyze` endpoint. `/analyze` remains available for older
integrations that expect a single HTTP call to persist the capture, run analysis,
run tailoring, render DOCX, validate layout, and return a combined response.

The desktop UI uses the reviewed command path instead:

- `import_job_from_url` and `import_job_from_text` for manual job import. Both
  persist through the same `persist_capture` the extension route uses and emit
  the same `job-data-received` event, so the desktop UI cannot tell an imported
  job from a captured one.
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
future token and cost investigations can be tied to the `job_import`,
`job_analysis`, or tailoring stage without persisting API keys or prompt
contents. A URL import that found JSON-LD writes no receipt at all, because it
never called a model.

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
