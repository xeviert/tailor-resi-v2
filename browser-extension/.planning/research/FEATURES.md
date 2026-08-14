# Feature Research

**Domain:** Job Posting Extractor Browser Extension
**Researched:** 2026-02-24
**Confidence:** MEDIUM

Based on analysis of Thunderbit, HuntingPad, Indeed Scraper, LinkedIn Job Scraper, Data Miner, and other tools in the market.

## Feature Landscape

### Table Stakes (Users Expect These)

Features users assume exist. Missing these = product feels incomplete.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Extract job title | Core purpose - primary field users need | LOW | DOM parsing via content script |
| Extract company name | Core purpose - identifies the employer | LOW | Often in meta tags or specific DOM selectors |
| Extract job description | Core purpose - main content users want | MEDIUM | Variable HTML structures across sites |
| One-click extraction trigger | Minimal friction UX expectation | LOW | Toolbar icon click or context menu |
| Send to backend/endpoint | Data must go somewhere for analysis | LOW | POST request to configurable endpoint |
| Works on common job sites | Users apply to LinkedIn, Indeed, etc. | MEDIUM | Site-specific selectors needed |
| JSON export | Structured data format | LOW | POST body format |

### Differentiators (Competitive Advantage)

Features that set the product apart. Not required, but valuable.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Auto-detect job posting pages | No manual triggering needed | HIGH | Heuristics to identify job pages |
| AI-powered field parsing | Handles varied site structures | HIGH | Use LLM or ML for extraction |
| Multiple export formats | Flexible downstream use | LOW | CSV, Excel, Notion, Airtable |
| Local job storage | Review saved jobs offline | MEDIUM | Browser storage or extension storage |
| Context menu trigger | Works from anywhere on page | LOW | chrome.contextMenus API |
| Duplicate detection | Avoid saving same job twice | LOW | Hash-based comparison |
| Subpage extraction | Get full details from listing pages | HIGH | Click into job detail pages |
| Site templates | Pre-built selectors for major sites | MEDIUM | Maintain selector library |
| Data enrichment | Extract salary, location, remote status | HIGH | Requires parsing beyond basic text |

### Anti-Features (Commonly Requested, Often Problematic)

Features that seem good but create problems.

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Universal scraping (any site) | Wants to extract from anywhere | Site structures vary wildly; maintenance nightmare | Site-specific templates with user selection |
| Real-time monitoring | Always have latest jobs | Battery drain, browser resource overhead | Scheduled pulls from backend |
| Built-in analysis/AI in extension | Want immediate insights | Violates "simple extractor" scope; costs add up | Send to Tauri backend for analysis |
| Multiple export formats v1 | Flexibility seems valuable | Scope creep; JSON to backend is sufficient for v1 | Focus on core extraction first |
| User accounts/auth | Seems professional | Local-only app doesn't need it | Skip authentication |
| Browser-wide scraping | Capture all job postings seen | Privacy concerns, permission issues | Context-menu triggered extraction |

## Feature Dependencies

```
One-click Extraction
    └──requires──> Content Script Injection
                        └──requires──> DOM Parsing

Auto-detect Job Pages
    └──requires──> Site-specific Selectors
                        └──requires──> Selector Templates

Subpage Extraction
    └──requires──> One-click Extraction
    └──requires──> Click-through Navigation

Data Enrichment
    └──requires──> Basic Extraction
```

### Dependency Notes

- **One-click extraction requires content script injection:** The extension must inject into the page to access DOM
- **Auto-detect requires site-specific selectors:** Different job sites have different structures; must maintain template library
- **Subpage extraction requires one-click as base:** Must first reliably extract from current page before navigating
- **Data enrichment requires basic extraction:** Must have raw text before applying intelligent parsing

## MVP Definition

### Launch With (v1)

Minimum viable product — what's needed to validate the concept.

- [x] Extract job title — core field, simplest to parse
- [x] Extract company name — identifies employer
- [x] Extract job description — main content
- [x] One-click toolbar trigger — minimal UX friction
- [x] Send to POST localhost:PORT/analyze — backend integration (PROJECT.md requirement)
- [x] JSON payload — structured data format (PROJECT.md requirement)

### Add After Validation (v1.x)

Features to add once core is working.

- [ ] Context menu trigger — more flexible extraction
- [ ] Site templates for major sites — improve accuracy on LinkedIn, Indeed
- [ ] Local storage of extracted jobs — review history
- [ ] Duplicate detection — avoid re-saving

### Future Consideration (v2+)

Features to defer until product-market fit is established.

- [ ] Auto-detect job posting pages — eliminate manual trigger
- [ ] AI-powered field parsing — handle any site structure
- [ ] Multiple export formats — Notion, Airtable, CSV
- [ ] Subpage extraction — get full job details

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Extract job title | HIGH | LOW | P1 |
| Extract company name | HIGH | LOW | P1 |
| Extract job description | HIGH | MEDIUM | P1 |
| Send to backend endpoint | HIGH | LOW | P1 |
| One-click trigger | HIGH | LOW | P1 |
| Context menu trigger | MEDIUM | LOW | P2 |
| Site templates | HIGH | MEDIUM | P2 |
| Local job storage | MEDIUM | MEDIUM | P2 |
| Duplicate detection | LOW | LOW | P3 |
| Auto-detect job pages | MEDIUM | HIGH | P3 |
| AI field parsing | MEDIUM | HIGH | P3 |
| Multiple export formats | LOW | MEDIUM | P3 |

**Priority key:**
- P1: Must have for launch
- P2: Should have, add when possible
- P3: Nice to have, future consideration

## Competitor Feature Analysis

| Feature | Thunderbit | Indeed Scraper | HuntingPad | Our Approach |
|---------|------------|----------------|------------|--------------|
| Job title extraction | Yes (AI) | Yes | Yes (LLM) | Manual selectors (v1) |
| Company extraction | Yes (AI) | Yes | Yes (LLM) | Manual selectors (v1) |
| Description extraction | Yes (AI) | Yes | Yes (LLM) | Manual selectors (v1) |
| One-click trigger | Yes | Yes | Context menu | Toolbar icon |
| Export to backend | API integration | File export | API integration | POST to localhost |
| Auto-detect pages | Yes | Per-site | No | No (v1) |
| AI parsing | Yes | No | Yes (paid) | Delegate to Tauri backend |
| Free tier | Limited | Yes | Limited | N/A (local tool) |

**Key insight:** Most competitors either do client-side AI parsing or file export. Our differentiation: send raw data to local Tauri backend for analysis, keeping extension simple.

## Sources

- Thunderbit (thunderbit.com/blog/best-job-scraping-tools) — Feature comparison
- HuntingPad dev story (forem.com/galihm) — Implementation challenges
- Chrome Web Store extensions: Indeed Scraper, LinkedIn Job Scraper
- General job scraping software: ParseHub, Octoparse, Data Miner

---
*Feature research for: Job Posting Extractor Browser Extension*
*Researched: 2026-02-24*
