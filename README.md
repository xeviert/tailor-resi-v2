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

   An analysis already stored for this capture is reused instead of bought
   twice. The analysis prompt never sees the output language, so one analysis
   serves both EN and FR — which is why switching language has always reused it
   — and keying the lookup on the capture is what makes reuse safe: a different
   job post is a different capture and cannot match, so what comes back is
   always an analysis of the post on screen. A stored analysis with no summary,
   which is what a failed run leaves behind, is never reused. Evidence preflight
   still runs fresh every time, because the evidence bank does change. **Re-analyze
   job** on the review screen forces a new call when the stored analysis itself
   is what looks wrong.

   Analysis reports its two stages — `ats_analysis` and `evidence_preflight` —
   through the same `resume-pipeline-progress` event the document pipeline uses,
   so the review screen shows a live stage list, an elapsed counter, and a
   **Cancel** button rather than a button that only reads "Working...". A long
   posting can legitimately occupy the model for minutes, and a silent wait is
   indistinguishable from a hung one.

   Cancel abandons the wait, not the work: the request has already been sent and
   its tokens are already spent, so the Rust call runs to completion and still
   writes its usage receipt. What Cancel guarantees is that the abandoned run can
   never repaint the screen afterwards — its progress and result events are
   dropped until the next deliberate run starts.

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

   Attesting to a term is the authorization to use it. A saved entry can support
   an experience bullet, not only a skills string, and the tailoring layer is told
   to place it in the most plausible existing role. The boundary that keeps this
   from becoming invention is the term itself: the attestation covers the named
   capability and nothing adjacent to it - no metric, employer, title, date, or
   credential comes along with it.

5. Tailor the resume and run the locked document pipeline.

   When the user clicks **Generate tailored PDF**, the UI invokes
   `generate_tailored_resume`. The request includes the selected language, the
   prior `JobAnalysis`, and the selected evidence. There is no emphasis setting:
   there is one tailoring mode and it is the most aggressive one.

   The tailoring layer uses `OPENAI_TAILOR_MODEL` and may rewrite only experience
   bullet text and skills strings. It must preserve metadata, companies, titles,
   dates, job order, bullet counts, skill keys, education, section headings, and
   contact/header layout. The generated resume content is validated before any
   document is treated as usable.

   Experience bullets are treated as the primary ATS surface. Every bullet comes
   back with different, truthful text before any skills string changes, and on top
   of that the model replaces bullets outright: the original angle is discarded and
   a new bullet is written against the job's highest-importance signals. There is
   no cap and no per-role limit - replacing every bullet in the resume is a valid
   answer to a job the base bullets do not speak to - and at least one replacement
   is required, so a run cannot pass on rephrasing alone. Replacements happen in
   place, so job and bullet counts never change.

   Each replacement must stay grounded in this person's real work in that role -
   other base-resume facts, the attested evidence bank, or a responsibility
   directly implied by that role's stated stack, title, and scope. Employers,
   titles, dates, credentials, certifications, education, tools, and metrics
   remain invention-forbidden. Each swap is recorded in the tailoring report as a
   `replaced` bullet-rewrite decision with a rationale, and the desktop UI lists
   the before/after text so the user can review what changed.

   The pipeline then runs these backend stages:

   - `resume_tailoring`: generate truthful tailored resume JSON and a tailoring
     report.
   - `safety_validation`: verify locked JSON shape and factual constraints, ensure
     every experience bullet was actually rewritten, and require at least one
     replacement. A shape or locked-field violation is handed back to the model as
     a correction rather than ending the run, so a response that has already been
     billed gets a chance to be fixed instead of being discarded — the constraint
     is unchanged, only the point at which the run gives up. Replacements also face length and prose checks that reject a
     keyword-stuffed, overlong, or too-short bullet, because the DOCX layout is
     locked to one page. A rejected response is sent back to the model for
     correction within the attempt limit. Once a response is accepted, this stage
     measures ATS keyword coverage of the generated content and rebuilds the
     report's covered/omitted lists from that measurement. If that measurement shows
     the model dropped a high-weight keyword this person can already prove, the first
     attempt hands those terms back and asks once more - see "ATS Coverage Scoring".
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
   measured ATS coverage, changed fields, the keywords the resume still does not
   carry, and available artifact actions. Successful variants include `variant.json`, `tailoring-report.json`,
   the rendered DOCX, and usually a one-page PDF.

   The summary is headed by the job it belongs to - the captured title, the company,
   the output language, and a link back to the original post. None of that is stored
   in the result; it comes from the capture on screen, which every recovery path
   already pins to the same `capture_received_at_ms`.

   Stepping back to the captured job keeps the finished result in memory. The review
   screen then offers **Back to tailored result** to reopen the completed summary
   without spending another tailoring run, and its primary action reads **Re-run
   tailoring** to make clear that it would overwrite the existing result.

   **Start over** is how the next job begins. It appears on both the summary and the
   review screen, asks for confirmation, and then clears `data/job-captures/latest.json`
   through `clear_latest_job`, returning the app to the empty "Capture a job post to
   begin" screen with the import panel ready. Only the pointer is removed: the
   timestamped capture, the variants, and the result snapshot all stay on disk, so the
   job is abandoned rather than erased. Because the pointer is what the app opens with,
   the reset survives a restart — which is the difference between starting over and
   merely navigating away.

   A verified employer-facing artifact is atomically published to the stable
   `Downloads/Xevier_T_CV_<lang>.pdf` filename. If PDF output is not available
   but a DOCX passed validation, the validated DOCX is published instead. Only one
   of those two stable employer-facing files is kept per language. Each published
   artifact gets an `artifact-manifest.json` provenance file in its variant
   directory.

7. Optionally re-run tailoring on the keywords it missed.

   After a completed or partial run the summary shows one **Keywords not in this
   resume** block. It is built from the measured coverage described below, and it
   splits the misses by *why* the document does not carry them:

   - **Ready to add - nothing to confirm** (`evidence_not_placed`): the base resume
     or a saved evidence-bank entry already backs these, so claiming them is not a
     new claim. They arrive **pre-selected**, because this is coverage the user
     already owns and declining it should be the action that costs a click.
   - **Needs your confirmation first** (`no_evidence`): nothing supports these, so
     the AI may not claim them. Ticking one is the attestation, and it is saved to
     the evidence bank for future jobs.

   Both groups are selectable and both feed the same **Re-run tailoring with N
   keywords** button, which invokes `retailor_resume_with_evidence`. A short
   disclosure beside them answers the question the block used to raise and not
   answer: approving a term at the evidence step is permission, not obligation - the
   resume has a fixed bullet count and one page to spend - and ticking a keyword here
   is what turns it into a hard requirement of the next run.

   These were two blocks, and they overlapped completely: the selectable list is
   rebuilt from `omitted_unsupported_keywords`, which is filtered on `!covered`
   alone, so every already-supported miss also appeared as a pill asking the user to
   vouch for it. The panel reads `miss_reason` off `ats_coverage.terms` instead, and
   falls back to the flat list only for a result stored before coverage measurement
   existed - where, with no measurement, everything has to count as unproven.

   Re-tailoring loads the previous source variant and analysis, validates that the
   selected terms came from the source result's omitted list, records them as
   user-attested placement terms, and reruns the same tailoring and document
   pipeline. The result records the source variant, previous score, and selected
   terms so the UI can show the score delta.

   Selecting an already-supported term does not write an evidence-bank entry for it.
   The bank is permanent and every future job reads it, so recording an attestation
   for a claim the base resume already makes would fill it with claims the user was
   never asked to give. Both kinds still travel as placement terms, so what the model
   is required to place is the same either way.

   Placement is checked against each bullet on its own, not against all of them
   joined together. A phrase whose words happen to be scattered one apiece across
   unrelated bullets is not a claim the resume makes, and counting it as placed is
   what previously let a selected term be reported as covered while appearing
   nowhere. When a term genuinely cannot be placed within the attempt limit the run
   fails and says so, rather than quietly reporting success.

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
  user must attest to it before it can be claimed.
- `evidence_not_placed`: the preflight already cleared it and the tailoring pass
  still did not use it. This is free, truthful coverage that was left on the table.

Both kinds land in `omitted_unsupported_keywords`, which is filtered on `!covered`
and nothing else. The summary screen therefore reads `miss_reason` directly rather
than that list; using the list for the "needs attestation" side is what used to ask
the user to vouch for terms the block above had just told them were already backed.

A high-weight `evidence_not_placed` miss also costs the run one extra attempt.
Approval at the evidence step is permission, not obligation - the prompt asks the
model to incorporate supported terms "aggressively" and nothing enforced it, so
proven coverage was dropped in silence and the user was left to go and fetch it by
hand. After coverage is measured, the first attempt hands those terms back to the
model as a correction and asks again. It fires once, only on the first attempt, so
the rest of the budget stays with the validators; it applies at weight 4 and above,
because a whole generation is too expensive to spend on something the post itself
called optional; and it never fails the run - if the second response drops them too,
that is the answer and the run proceeds.

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
- `clear_latest_job` to forget the current capture and start over.
- `prepare_evidence_preflight` when switching EN/FR after analysis.
- `generate_tailored_resume` for reviewed tailoring and artifact generation.
- `retailor_resume_with_evidence` for the optional omitted-term loop.

Uploading to third-party job forms remains a manual browser step: use the desktop
app's Open PDF/Open folder actions to select the stable file. The `/health`
response includes `result_protocol_version`; a mismatch tells the user to fully
restart the desktop app instead of silently running an incompatible UI/backend
pair.

OpenAI calls also write usage-only receipts under `data/api-usage/` so future token
and cost investigations can be tied to the `job_import`, `job_analysis`, or tailoring
stage without persisting API keys or prompt contents. A receipt is written before the
response is parsed, so a refused, truncated, or malformed response — billed like any
other — still appears in the ledger. Each one carries the capture it was serving and,
for tailoring, which attempt it was, so a receipt can be joined to its run rather than
guessed at by timestamp. A URL import that found JSON-LD writes no receipt at all, because it
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

### Prompt caching

Cached input bills at a fraction of the normal rate, but only for a prefix that is
byte-identical to a recent request and at least 1024 tokens long. Both stages send a
`prompt_cache_key` per stage — not per job — because everything sharing a stage also
shares that stage's constant prefix, and routing them together is the point. Bump the
version in those keys (`src-tauri/src/http.rs`) whenever a stage's constant text changes.

The tailoring prompt is therefore built in three zones, most-constant first: the
instruction text and base resume, then the per-job language, job post, analysis and
matched evidence, then the per-attempt retry feedback. Nothing volatile may move above
something constant, or the constant text below it stops being a shared prefix and is
billed in full on every call. `tailoring_prompt_keeps_a_stable_cacheable_prefix` is the
guard; if it fails, the prompt edit is what needs revisiting.

Analysis gets no discount from this and is not expected to. Its instructions come to
roughly 650 tokens, under the floor, and the constant output schema travels in
`text.format.schema`, which serializes below the volatile job post and so cannot extend
the prefix. Padding the instructions to clear the bar would buy the discount by adding
the very tokens being discounted. Reusing the stored analysis, above, is what saves money
in that stage instead.

`python scripts/api-usage-report.py` summarizes the receipts under `data/api-usage/` by
stage, including cache hit rate; `--runs` groups by capture so the retries behind a single
resume are visible.

## Resume Documents

Detailed resume template and render instructions live in `resume/README.md`.
