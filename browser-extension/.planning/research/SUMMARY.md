# Project Research Summary

**Project:** Job Posting Extractor Browser Extension
**Domain:** Browser Extension (Manifest V3)
**Researched:** 2026-02-24
**Confidence:** MEDIUM-HIGH

## Executive Summary

This is a browser extension that extracts job posting data (title, company, description) from job websites like LinkedIn and Indeed, then sends the structured data to a local Tauri backend for analysis. The recommended approach uses **WXT** as the framework (v0.20.x) — the 2025 market leader for MV3 extensions — combined with TypeScript, Vitest, and Playwright for testing.

The MVP should deliver: content script DOM extraction, one-click toolbar trigger, and HTTP POST to `localhost:PORT/analyze`. Architecture follows the standard MV3 three-tier pattern: Content Script → Service Worker → Tauri Backend. The main risks are around MV3 service worker lifecycle (can terminate unexpectedly), message passing reliability, and host permissions configuration. These must be addressed in the implementation phase with robust communication patterns.

## Key Findings

### Recommended Stack

**Core technologies:**
- **WXT (v0.20.x):** MV3 browser extension framework — built on Vite, first-class TypeScript, cross-browser support (Chrome/Firefox/Edge), auto-reload, zero-config manifest generation. Active maintenance in 2026.
- **TypeScript (v5.x):** Required per project constraints. Enables type safety for content script DOM parsing and message passing.
- **Vitest (v4.x):** Vite-native unit testing, fast, integrates with WXT ecosystem.
- **Playwright (v1.50.x):** Official Chrome extension testing support for E2E/popup testing.
- **@wxt-dev/storage (v1.x):** Official WXT module for chrome.storage with type safety.

**Why WXT over alternatives:** Active maintenance (vs Plasmo's 8+ month gap), superior cross-browser support, better DX than CRXJS, far less boilerplate than vanilla + Webpack.

### Expected Features

**Must have (table stakes):**
- Extract job title — core field, simplest to parse
- Extract company name — identifies employer
- Extract job description — main content
- One-click toolbar trigger — minimal UX friction
- POST to localhost:PORT/analyze — backend integration per PROJECT.md requirement
- JSON payload — structured data format

**Should have (competitive):**
- Context menu trigger — more flexible extraction
- Site templates for major sites — improve accuracy on LinkedIn, Indeed
- Local storage of extracted jobs — review history

**Defer (v2+):**
- Auto-detect job posting pages — eliminate manual trigger
- AI-powered field parsing — handle any site structure
- Multiple export formats — Notion, Airtable, CSV

### Architecture Approach

The standard MV3 three-tier architecture:

1. **Content Scripts:** Inject into target job pages, parse DOM, extract structured data, send to service worker via message passing
2. **Service Worker:** Central hub, handles messages from content scripts and popup, POSTs data to Tauri backend, manages chrome.storage
3. **Popup UI:** Lightweight user interface for triggering extraction, showing status

**Key pattern:** Use `chrome.runtime.connect` for reliable message passing instead of one-off messages — this handles service worker restarts gracefully.

### Critical Pitfalls

 Worker Lifecycle (MV1. **Service3):** Service workers are non-persistent — Chrome terminates them after inactivity. Use `chrome.alarms` instead of ` rely on insetTimeout`, don't-memory state, re-initialize on each wake.

2. **Message Passing Failures:** Content script messages silently fail after service worker restarts. Use connection-based messaging (`chrome.runtime.connect`), always return `true` from `onMessage` for async responses.

3. **Host Permissions:** Confusing `permissions` vs `host_permissions` in Manifest V3. Add target domains to `host_permissions` explicitly; `activeTab` only gives temporary access on click.

4. **Storage Mistakes:** Using `localStorage` instead of `chrome.storage` — web storage is isolated to page origin and doesn't work in background at all. Always use `chrome.storage.local`.

5. **CSP Violations:** Manifest V3 has stricter CSP. Use external scripts via `src` attribute only, not inline code. Add URLs to permissions for fetch/XHR.

## Implications for Roadmap

Based on research, suggested phase structure:

### Phase 1: Extension Foundation
**Rationale:** Core infrastructure must work before any features — establishes the MV3 development environment and communication layer.
**Delivers:** WXT project setup, TypeScript config, basic content script injection, service worker skeleton, manifest with permissions.
**Addresses:** Pitfalls 1, 2, 4, 5 — establish robust communication and storage patterns early.
**Avoids:** Anti-pattern of using localStorage; ensures message passing works after service worker restart.

### Phase 2: Core Extraction
**Rationale:** This is the MVP — the minimum needed to validate the concept with the Tauri backend.
**Delivers:** DOM parsing for job title/company/description, one-click toolbar trigger, POST to localhost:PORT/analyze with JSON payload.
**Uses:** Stack: WXT, TypeScript, content scripts, chrome.storage
**Implements:** Architecture Pattern 1 (Content Script → Service Worker → Backend)
**Features:** All P1 items from FEATURES.md

### Phase 3: Polish & Reliability
**Rationale:** MVP is working but needs hardening — improves extraction accuracy and user feedback.
**Delivers:** Site templates for LinkedIn/Indeed, context menu trigger option, error handling with user feedback, local job storage.
**Avoids:** Pitfall 3 — proper permissions testing on real sites.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 2 (Core Extraction):** May need research on specific CSS selectors for target job sites — selector library maintenance.
- **Phase 3 (Polish):** Site-specific selectors for less common job boards may require individual research.

Phases with standard patterns (skip research-phase):
- **Phase 1 (Foundation):** WXT provides standard MV3 patterns — well-documented.
- **Phase 2 (Core):** DOM extraction is straightforward, message passing patterns are standard.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | WXT is current 2025/2026 market leader with active maintenance. Official docs, multiple comparison articles. |
| Features | MEDIUM | Feature analysis based on competitor research (Thunderbit, HuntingPad, Indeed Scraper). MVP definition aligns with PROJECT.md requirements. |
| Architecture | HIGH | Standard MV3 patterns from official Chrome docs. Three-tier architecture well-established. |
| Pitfalls | HIGH | Official Chrome documentation on MV3 migration, service worker lifecycle. Community patterns from Stack Overflow. |

**Overall confidence:** MEDIUM-HIGH

### Gaps to Address

- **Site selector library:** Research assumes we can parse DOM, but specific selectors for each job site need building. Handled in Phase 2-3.
- **Backend contract:** The Tauri backend API (`/analyze` endpoint) is assumed but not detailed. Coordinate with backend team.
- **Cross-browser testing:** Research focuses on Chrome; Firefox/Safari testing may reveal edge cases in Phase 1.

## Sources

### Primary (HIGH confidence)
- WXT Official Docs (wxt.dev) — Framework documentation, v0.20.18
- Chrome Extensions Architecture Overview — Official MV3 documentation
- Chrome Extensions MV3 Migration Guide — Official migration checklist

### Secondary (MEDIUM confidence)
- 2025 State of Browser Extension Frameworks (redreamality.com) — WXT vs Plasmo comparison
- Thunderbit, Indeed Scraper, HuntingPad — Competitor feature analysis
- Stack Overflow - Common Chrome Extension Issues — Community patterns

### Tertiary (LOW confidence)
- Firefox Extension Workshop — Cross-browser differences noted but not deeply tested

---
*Research completed: 2026-02-24*
*Ready for roadmap: yes*
