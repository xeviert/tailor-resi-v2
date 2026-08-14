# Job Posting Extractor

## What This Is

A browser extension that extracts job posting data (title, company, description) from web pages and sends it to a local Tauri backend for analysis.

## Core Value

Quickly capture job postings from any website with one click, sending structured data to a local analysis endpoint.

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] Extract job title from web page
- [ ] Extract company name from web page
- [ ] Extract job description text from web page
- [ ] Send extracted data to POST localhost:PORT/analyze
- [ ] Standard WebExtension with TypeScript

### Out of Scope

- Any analysis/processing in the extension itself (handled by Tauri backend)
- Multiple data formats (JSON only)
- Authentication (local-only use)

## Context

- Connects to Tauri app in parent directory (`../`)
- Target endpoint: `POST localhost:PORT/analyze`
- Standard WebExtension APIs (manifest v3)
- TypeScript throughout

## Constraints

- **Tech**: Standard WebExtension with TypeScript
- **Data**: JSON payload only
- **Target**: Local Tauri backend (localhost)

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| TypeScript | Type safety for extension development | — Pending |
| Content script extraction | Need to parse page DOM for job data | — Pending |

---
*Last updated: 2026-02-24 after initialization*
