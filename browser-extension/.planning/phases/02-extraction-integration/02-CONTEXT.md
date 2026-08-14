# Phase 2 Discussion: Extraction & Integration

**Phase:** 02-extraction-integration  
**Auto Mode:** Yes  
**Input:** PROJECT.md, ROADMAP.md (Phase 2), REQUIREMENTS.md, Phase 1 SUMMARY

---

## Context Summary

### What We Have (Phase 1 Complete)

- Manifest V3 extension structure
- Toolbar icon with popup (320x200)
- Content script skeleton (`content-scripts/content.js`)
- Popup click handler ready for connection
- Message listener in content script (listens for `extractJob` action)

### Phase 2 Requirements

| ID | Requirement |
|----|-------------|
| EXT-01 | Extract job title from current web page |
| EXT-02 | Extract company name from current web page |
| EXT-03 | Extract job description text from current web page |
| INT-01 | Send extracted data as JSON to POST localhost:PORT/analyze |
| INT-02 | Handle successful response from backend |
| INT-03 | Handle errors gracefully when backend unavailable |

### Success Criteria (from ROADMAP.md)

1. User can extract job title from current web page
2. User can extract company name from current web page
3. User can extract job description text from current web page
4. Extracted data is sent as JSON to POST localhost:PORT/analyze
5. User sees confirmation when data is successfully sent to backend
6. User sees helpful error message when backend is unavailable

---

## Key Decisions Needed

### 1. Extraction Strategy

**Question:** How should the extension extract job data from arbitrary web pages?

**Options:**

| Option | Description | Pros | Cons |
|--------|-------------|------|------|
| A. Generic selectors | Use common patterns (`h1`, `[class*="title"]`, `meta` tags) | Works on many sites | May miss site-specific layouts |
| B. Heuristic extraction | Multiple selector attempts, fallback chains | More robust | More complex code |
| C. Site templates (v2) | LinkedIn, Indeed, Glassdoor specific | Precise for major sites | Deferred to v2 |

**Recommendation:** Option B (heuristic) for v1. Keep it simple but cover common patterns. Defer precise templates to Phase v2.

**Heuristic approach:**
- Job title: `h1`, `h2`, `meta[property="og:title"]`, common class patterns
- Company: `a[href*="company"]`, `[class*="company"]`, `meta[property="og:site_name"]`
- Description: `meta[name="description"]`, `[class*="description"]`, first `<article>` or `<section>`

---

### 2. Message Flow Architecture

**Question:** How should components communicate?

**Current (Phase 1):**
```
Popup (popup.js) ←→ User click
```

**Phase 2 Architecture:**

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐     ┌─────────────┐
│   Popup     │────▶│   Background │────▶│  Content Script │────▶│   Page DOM  │
│ (popup.js)  │     │ (background) │     │  (content.js)   │     │             │
└─────────────┘     └──────────────┘     └─────────────────┘     └─────────────┘
       │                    │                       │                      │
       │                    │ POST                  │                      │
       │                    └─────────────────────▶│                      │
       │                    │ JSON response         │                      │
       │                    │◀──────────────────────┘                      │
       │                    │                                              │
       │                    ▼                                              │
       │            ┌──────────────┐                                       │
       │            │   Backend    │                                       │
       │            │ localhost    │                                       │
       │            │   :PORT      │                                       │
       │            └──────────────┘                                       │
```

**Decision (recommended):** 
- Add HTTP server (axum) to Tauri backend to match PROJECT.md spec
- Popup sends message to Background
- Background coordinates: injects content script → gets DOM data → POSTs to backend
- Keep content script focused on extraction only
- Default endpoint: `http://localhost:3000/analyze`

---

### 3. Backend Communication Method

**Question:** How should the extension communicate with the Tauri backend?

**PROJECT.md SPEC:** `POST localhost:PORT/analyze`

**CRITICAL FINDING:** The Tauri backend in `../src-tauri` does NOT expose an HTTP server. It only has:
- `tauri::command` for IPC via `invoke_handler`
- Currently only a `ping` command exists

**Options:**

| Option | Description | Pros | Cons |
|--------|-------------|------|------|
| A. Add HTTP server to Tauri | Add axum/warp to Cargo.toml, expose `/analyze` | Matches PROJECT.md exactly | More complex, new dependency |
| B. Use Tauri IPC | Extension invokes `tauri::command` via native messaging | Native to Tauri, simpler | Deviates from spec |
| C. Use Tauri HTTP plugin | Add `tauri-plugin-http` for client-side HTTP | Works within Tauri ecosystem | Still needs server endpoint |

**Recommendation:** Option A - Add HTTP server (axum) to match the PROJECT.md spec exactly. The backend needs to listen on a port regardless for the "local analysis endpoint."

**If Option A (recommended):**
- Add `axum` to Cargo.toml
- Create HTTP route `POST /analyze`
- Default port: 3000 (configurable via env)
- Extension POSTs to `http://localhost:3000/analyze`

**If Option B (simpler):**
- Use `chrome.runtime.sendNativeMessage`
- Tauri command: `#[tauri::command] fn analyze_job(data: JobData)`
- Simpler but diverges from spec

---

### 4. Error Handling Strategy

**Question:** How to handle backend unavailability?

**Requirements:**
- INT-02: Handle successful response
- INT-03: Handle errors gracefully

**Error scenarios:**
1. **Connection refused** - Backend not running
2. **Timeout** - Backend slow to respond (5s default)
3. **HTTP error** - Backend returns 4xx/5xx
4. **Network error** - No internet (rare for localhost)

**Proposed handling:**

| Scenario | User Message | Action |
|----------|--------------|--------|
| Timeout | "Backend took too long. Is it running?" | Show retry button |
| Connection refused | "Cannot connect to backend. Make sure Tauri app is running." | Show retry button |
| HTTP error | "Backend error: {message}" | Show error details |
| Success | "Job data sent successfully!" | Show confirmation |

---

### 5. Extraction Trigger Flow

**Question:** What happens when user clicks "Extract"?

**Flow:**

1. User clicks "Extract Job" button in popup
2. Popup shows "Extracting..." status
3. Popup sends `chrome.runtime.sendMessage({ action: 'extractJob' })` to background
4. Background uses `chrome.scripting.executeScript` to inject content script
5. Content script extracts data from DOM
6. Content script returns data to background
7. Background POSTs JSON to `http://localhost:3000/analyze`
8. Background returns result to popup
9. Popup shows success/error status to user

---

## Scope for Phase 2 (v1)

### In Scope

- [x] Generic DOM extraction (title, company, description)
- [x] Message flow: popup → background → content script
- [x] POST to http://localhost:3000/analyze (matches PROJECT.md)
- [x] Success/error handling with user feedback
- [x] Basic error messages
- [ ] Tauri backend HTTP server (add to src-tauri, outside this extension repo)

### Out of Scope (Deferred to v2)

- Site-specific templates (LinkedIn, Indeed, Glassdoor)
- Context menu trigger
- Keyboard shortcut
- Configurable port in UI
- Retry logic with backoff

---

## Technical Implementation Notes

### Content Script Extraction

```javascript
// Generic selectors to try (priority order)
const titleSelectors = [
  'h1', 'h2', 
  '[class*="title"]', '[class*="job-title"]',
  'meta[property="og:title"]',
  '[data-testid="job-title"]'
];

const companySelectors = [
  '[class*="company"]', '[class*="employer"]',
  'a[href*="/company/"]',
  'meta[property="og:site_name"]'
];

const descriptionSelectors = [
  'meta[name="description"]',
  '[class*="description"]', '[class*="details"]',
  'article', 'section.description'
];
```

### Background Script POST (matches PROJECT.md)

```javascript
// Using fetch API (available in service workers)
fetch('http://localhost:3000/analyze', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ title, company, description, url: location.href })
})
  .then(response => response.json())
  .then(data => {
    // Handle success
  })
  .catch(error => {
    // Handle error
  });
```

**Prerequisite:** Tauri backend must add HTTP server (axum) listening on port 3000.

---

## Proposed Plan Structure

Based on the analysis, Phase 2 should have **1 plan** with **2-3 tasks**:

| Task | Name | Description |
|------|------|-------------|
| 1 | Implement content script extraction | DOM selectors for title, company, description |
| 2 | Wire up message flow | Popup → Background → Content script communication |
| 3 | Implement backend POST & error handling | POST to localhost:3000/analyze, handle success/errors |

**Wave structure:** Single wave (all tasks sequential and interdependent)

**IMPORTANT:** The Tauri backend (`../src-tauri`) needs an HTTP server added. This is **outside** the browser-extension repo. Options:
- Add axum HTTP server to src-tauri (separate work item)
- Or note that backend must be running with HTTP endpoint for this to work

---

## Next Steps

1. **Approve approach** - Confirm extraction strategy and architecture
2. **Proceed to planning** - `/gsd-plan-phase 02`
3. **Execute** - `/gsd-execute-phase 02`

---

*Discussion completed in auto mode*
