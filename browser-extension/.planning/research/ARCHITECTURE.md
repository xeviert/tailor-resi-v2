# Architecture Research

**Domain:** Browser Extension (Manifest V3)
**Researched:** 2026-02-24
**Confidence:** HIGH

## Standard Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Browser Extension                         │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────┐    │
│  │                  Popup / Side Panel                  │    │
│  │              (User Interface Layer)                  │    │
│  └─────────────────────────┬───────────────────────────┘    │
│                            │ messaging                        │
├────────────────────────────┼────────────────────────────────┤
│                    Service Worker                           │
│  ┌─────────────┐  ┌────────┴───────┐  ┌────────────────┐    │
│  │  Extension  │  │   Message      │  │   Storage     │    │
│  │  Lifecycle  │  │   Handler      │  │   Manager     │    │
│  └─────────────┘  └────────────────┘  └────────────────┘    │
│                            │                                  │
├────────────────────────────┼────────────────────────────────┤
│                    Content Scripts                          │
│  ┌─────────────┐  ┌────────┴───────┐  ┌────────────────┐    │
│  │    DOM      │  │    Data        │  │   Message     │    │
│  │   Parser    │  │   Extractor    │  │   Sender      │    │
│  └─────────────┘  └────────────────┘  └────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                      Web Page (Target)                      │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              Job Posting HTML / DOM                  │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    External Services                        │
│  ┌─────────────────────────────────────────────────────┐    │
│  │           Tauri Backend (localhost:PORT)              │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| `manifest.json` | Extension config, permissions, entry points | JSON file in root |
| Service Worker | Event handling, message routing, API calls | `service-worker.js` in `background` |
| Content Scripts | DOM parsing, data extraction | Injected JS matching target URLs |
| Popup UI | User interaction, trigger extraction | HTML/CSS/JS in `action.default_popup` |
| Storage API | Persist settings, cached data | `chrome.storage.local` or `sync` |
| Message Passing | Inter-component communication | `chrome.runtime.sendMessage` / `onMessage` |

## Recommended Project Structure

```
src/
├── manifest.json           # Extension manifest (MV3)
├── background/
│   └── service-worker.ts  # Service worker entry point
├── content/
│   ├── content-script.ts  # DOM extraction logic
│   └── selectors/         # CSS selectors for job sites
├── popup/
│   ├── popup.html         # Popup UI
│   ├── popup.ts          # Popup logic
│   └── styles.css        # Popup styles
├── shared/
│   ├── types.ts          # Shared TypeScript types
│   └── messaging.ts      # Message handling utilities
├── utils/
│   ├── storage.ts        # Storage API wrapper
│   └── http.ts           # HTTP client for backend
└── assets/
    ├── icons/            # Extension icons
    └── images/           # UI images
```

### Structure Rationale

- **`background/`:** Service worker is the central hub; all extension logic that survives page navigations lives here
- **`content/`:** Isolated extraction logic that runs in page context; separates DOM manipulation from extension APIs
- **`popup/`:** UI layer that user interacts with; lightweight to avoid slowing extension load
- **`shared/`:** Types and utilities used across components; single source of truth
- **`utils/`:** Reusable helpers for storage and HTTP; keeps components clean

## Architectural Patterns

### Pattern 1: Content Script → Service Worker → Backend

**What:** Three-tier data flow where content script extracts, service worker aggregates, backend receives
**When to use:** Data extraction from web pages sent to external API
**Trade-offs:** Simple to implement; adds latency vs direct content→backend

**Example:**
```typescript
// content-script.ts - runs in page context
chrome.runtime.sendMessage({
  type: 'JOB_DATA_EXTRACTED',
  payload: { title, company, description }
});

// service-worker.ts - runs in extension context
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.type === 'JOB_DATA_EXTRACTED') {
    fetch('http://localhost:8080/analyze', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(message.payload)
    });
  }
});
```

### Pattern 2: Programmatic Injection (ActiveTab)

**What:** Content script only runs when user clicks extension icon
**When to use:** Extensions that need page access on demand, not continuously
**Trade-offs:** Better privacy (no persistent page access); requires user action

**Example:**
```typescript
// manifest.json
{
  "permissions": ["activeTab", "scripting"],
  "action": { "default_title": "Extract Job" }
}

// service-worker.ts
chrome.action.onClicked.addListener(async (tab) => {
  await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    files: ['content/content-script.js']
  });
});
```

### Pattern 3: Message Channel for Long-Running Tasks

**What:** Use `chrome.runtime.Port` for streaming data or long operations
**When to use:** When content script needs to send multiple messages or maintain connection
**Trade-offs:** More complex than simple message passing; enables bidirectional streaming

**Example:**
```typescript
// content-script.ts
const port = chrome.runtime.connect({ name: 'extraction-channel' });
port.postMessage({ action: 'START_EXTRACTION' });
port.onMessage.addListener((msg) => console.log(msg));

// service-worker.ts
chrome.runtime.onConnect.addListener((port) => {
  port.onMessage.addListener((msg) => {
    if (msg.action === 'START_EXTRACTION') {
      // stream results back
      port.postMessage({ status: 'EXTRACTING' });
      port.postMessage({ status: 'DONE', data: {...} });
    }
  });
});
```

## Data Flow

### Request Flow

```
[User clicks extension icon]
        ↓
[Popup opens / Action triggered]
        ↓
[User clicks "Extract"]
        ↓
[Service Worker receives message]
        ↓
[Content Script injected via scripting API]
        ↓
[Content Script extracts DOM data]
        ↓
[Data sent to Service Worker via messaging]
        ↓
[Service Worker POSTs to Tauri backend]
        ↓
[Backend responds / Popup shows success]
```

### Key Data Flows

1. **Extraction Flow:** User action → Content script injection → DOM parsing → Message to service worker → HTTP POST to backend
2. **Configuration Flow:** Popup → Service worker storage → `chrome.storage.local` → Persisted settings
3. **Error Flow:** Backend error → Service worker → Popup notification / Content script feedback

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| Single user, local | Simple structure above sufficient |
| Multiple users, local network | Add retry logic, connection pooling in service worker |
| Cloud backend (future) | Add authentication, rate limiting, queueing |

### Scaling Priorities

1. **First bottleneck:** Backend availability — add graceful degradation if localhost:PORT unreachable
2. **Second bottleneck:** Large DOM parsing — content script should be efficient, avoid deep traversal

## Anti-Patterns

### Anti-Pattern 1: Content Script Makes External Requests Directly

**What people do:** Using `fetch()` directly in content script to call backend
**Why it's wrong:** Content scripts run in page context; CORS blocks cross-origin requests; security risk exposing backend
**Do this instead:** Send data to service worker via `chrome.runtime.sendMessage`, service worker makes the HTTP request

### Anti-Pattern 2: Storing Sensitive Data in chrome.storage.local

**What people do:** Storing API keys, tokens, or user credentials in extension storage
**Why it's wrong:** Storage is not encrypted; visible to anyone with extension access; synced to user's Google Account if using `storage.sync`
**Do this instead:** Use ephemeral in-memory storage; for authenticated features, use proper token handling with identity API

### Anti-Pattern 3: Heavy Computation in Content Script

**What people do:** Running complex parsing, regex, or data transformation in content script
**Why it's wrong:** Blocks page rendering; may kill extension if takes too long; poor isolation
**Do this instead:** Extract raw DOM data, pass to service worker for processing; or use offscreen document for CPU-heavy tasks

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| Tauri Backend | HTTP POST from service worker | Requires `host_permissions` for localhost |
| Chrome Storage | `chrome.storage.local/sync` | For settings, not sensitive data |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| Popup ↔ Service Worker | `chrome.runtime.sendMessage` | Lightweight request/response |
| Content Script ↔ Service Worker | `chrome.runtime.sendMessage` / Port | Bi-directional, can be streaming |
| Service Worker ↔ Storage | `chrome.storage` API | Async, event-driven updates |

## Sources

- [Chrome Extensions Architecture Overview](https://developer.chrome.com/docs/extensions/mv3/architecture-overview) - Official Chrome documentation
- [Content Scripts](https://developer.chrome.com/docs/extensions/develop/concepts/content-scripts) - Official MV3 content script guide
- [Service Workers](https://developer.chrome.com/docs/extensions/develop/concepts/service-workers) - Official MV3 background worker docs
- [Message Passing](https://developer.chrome.com/docs/extensions/develop/concepts/messaging) - Official messaging guide

---
*Architecture research for: Browser Extension (Job Posting Extractor)*
*Researched: 2026-02-24*
