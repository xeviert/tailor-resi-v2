# Requirements: Job Posting Extractor

**Defined:** 2026-02-24
**Core Value:** Quickly capture job postings from any website with one click, sending structured data to a local analysis endpoint.

## v1 Requirements

Requirements for initial release. Each maps to roadmap phases.

### Extraction

- [x] **EXT-01**: User can extract job title from current web page via content script
- [x] **EXT-02**: User can extract company name from current web page via content script
- [x] **EXT-03**: User can extract job description text from current web page via content script

### Integration

- [x] **INT-01**: Extension sends extracted data as JSON to POST localhost:PORT/analyze
- [x] **INT-02**: Extension handles successful response from backend
- [x] **INT-03**: Extension handles errors gracefully when backend is unavailable

### Extension Core

- [ ] **CORE-01**: Extension installs via browser (Chrome/Firefox/Edge)
- [ ] **CORE-02**: Extension toolbar icon provides one-click extraction trigger
- [ ] **CORE-03**: Extension uses Manifest V3

## v2 Requirements

Deferred to future release. Tracked but not in current roadmap.

### Site Templates

- **TMPL-01**: Pre-built selectors for LinkedIn job postings
- **TMPL-02**: Pre-built selectors for Indeed job postings
- **TMPL-03**: Pre-built selectors for Glassdoor job postings

### Enhanced Triggers

- **TRIG-01**: Context menu option to trigger extraction
- **TRIG-02**: Keyboard shortcut for extraction

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Built-in AI analysis | Handled by Tauri backend |
| Authentication | Local-only use, no auth needed |
| Cross-tab extraction | Single page at a time |
| Data storage in extension | Data sent directly to backend |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| CORE-01 | Phase 1 | Pending |
| CORE-02 | Phase 1 | Pending |
| CORE-03 | Phase 1 | Pending |
| EXT-01 | Phase 2 | Complete |
| EXT-02 | Phase 2 | Complete |
| EXT-03 | Phase 2 | Complete |
| INT-01 | Phase 2 | Complete |
| INT-02 | Phase 2 | Complete |
| INT-03 | Phase 2 | Complete |

**Coverage:**
- v1 requirements: 9 total
- Mapped to phases: 9
- Unmapped: 0 ✓

---
*Requirements defined: 2026-02-24*
*Last updated: 2026-02-24 after initial definition*
