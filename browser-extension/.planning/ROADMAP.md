# Roadmap: Job Posting Extractor

## Project Overview

- **Core Value:** Quickly capture job postings from any website with one click, sending structured data to a local analysis endpoint.
- **Total v1 Requirements:** 9
- **Phases:** 2

## Phases

- [ ] **Phase 1: Extension Foundation** - Project setup, Manifest V3, toolbar icon
- [ ] **Phase 2: Extraction & Integration** - DOM extraction, backend POST, error handling

---

## Phase Details

### Phase 1: Extension Foundation
**Goal:** Browser extension installs and provides one-click trigger for extraction  
**Depends on:** Nothing (first phase)  
**Requirements:** CORE-01, CORE-02, CORE-03  

**Success Criteria** (what must be TRUE):
1. Extension installs in Chrome/Firefox/Edge via browser's extension manager
2. Toolbar icon appears in browser after installation
3. Clicking toolbar icon triggers extraction workflow (Phase 2)
4. Extension uses Manifest V3 format

**Plans:** 1 plan

- [ ] 01-extension-foundation-01-PLAN.md — WXT setup, Manifest V3, toolbar icon, popup UI

---

### Phase 2: Extraction & Integration
**Goal:** Extract job posting data from web pages and send to Tauri backend  
**Depends on:** Phase 1  
**Requirements:** EXT-01, EXT-02, EXT-03, INT-01, INT-02, INT-03  

**Success Criteria** (what must be TRUE):
1. User can extract job title from current web page
2. User can extract company name from current web page
3. User can extract job description text from current web page
4. Extracted data is sent as JSON to POST localhost:PORT/analyze
5. User sees confirmation when data is successfully sent to backend
6. User sees helpful error message when backend is unavailable

**Plans:** 1 plan

- [ ] 02-extraction-integration-01-PLAN.md — DOM extraction, backend POST, error handling

---

## Progress Table

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Extension Foundation | 1/1 | Planning complete | 2026-02-24 |
| 2. Extraction & Integration | 0/1 | Planning complete | - |

---

## Coverage Map

| Phase | Requirements |
|-------|--------------|
| 1 - Extension Foundation | CORE-01, CORE-02, CORE-03 |
| 2 - Extraction & Integration | EXT-01, EXT-02, EXT-03, INT-01, INT-02, INT-03 |

**Coverage:** 9/9 requirements mapped ✓

---

*Roadmap created: 2026-02-24*
