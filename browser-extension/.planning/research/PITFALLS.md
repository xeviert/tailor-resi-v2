# Pitfalls Research

**Domain:** Browser Extension Development (Manifest V3)
**Researched:** 2026-02-24
**Confidence:** MEDIUM

## Critical Pitfalls

### Pitfall 1: Content Script Message Passing Failures

**What goes wrong:**
Messages between background script and content script silently fail. The content script never receives messages, or `sendResponse` callbacks never fire.

**Why it happens:**
- In Manifest V3, background scripts run as service workers that can be terminated and restarted at any time
- Content scripts are not re-injected when the service worker wakes up
- The content script may not be loaded when trying to send a message (timing issue)
- Not returning `true` from `onMessage` listener when using async sendResponse

**How to avoid:**
- Always return `true` from `chrome.runtime.onMessage` listener if you need async sendResponse
- Use connection-based messaging (`chrome.runtime.connect`) instead of one-off messages
- Check if content script is loaded before sending messages using `chrome.tabs.sendMessage` with error handling
- Re-inject content script programmatically if needed using `chrome.scripting.executeScript`

**Warning signs:**
- "Could not establish connection. Receiving end does not exist" errors in console
- Messages work on first load but fail after extension reload
- Content script logs not appearing after background worker restarts

**Phase to address:** Implementation phase - build robust communication layer early

---

### Pitfall 2: Manifest V3 Service Worker Lifecycle Issues

**What goes wrong:**
Background code stops executing unexpectedly. Timers don't fire, event listeners don't respond, and the extension appears "dead" until manually reloaded.

**Why it happens:**
- Service workers in Manifest V3 are non-persistent — Chrome terminates them after inactivity
- `setTimeout`/`setInterval` don't work reliably (Chrome may not wake service worker)
- No DOM access in background (can't use inline event handlers)

**How to avoid:**
- Use `chrome.alarms` API instead of `setTimeout`/`setInterval` for scheduled tasks
- Don't rely on global state — re-initialize on each service worker wake
- Use `chrome.storage` for persistence, not in-memory variables
- Register for events that wake the service worker (like `chrome.runtime.onStartup`)

**Warning signs:**
- Extension stops working after leaving it idle for several minutes
- Background console shows "Service worker started" repeatedly
- State disappears after extension auto-update

**Phase to address:** Architecture phase - design stateless service worker pattern

---

### Pitfall 3: Content Security Policy Violations

**What goes wrong:**
Extension fails to load or throws CSP errors when trying to execute inline scripts, fetch external resources, or make network requests.

**Why it happens:**
- Manifest V3 has stricter CSP by default
- Cannot use `eval()`, inline `<script>` tags, or load remote scripts
- External network requests require explicit permissions

**How to avoid:**
- Use external scripts only via `script` tag with `src` attribute (not inline code)
- Move all logic to external JS files
- For fetch/XHR, add URLs to `permissions` or `host_permissions` in manifest
- Use `chrome.scripting.executeScript` to inject code dynamically

**Warning signs:**
- "Refused to execute inline script" errors
- "CSP directive violation" warnings in Chrome extensions management page
- Network requests blocked with "URL not allowed" errors

**Phase to address:** Implementation phase - verify CSP compliance early

---

### Pitfall 4: Host Permissions Misconfiguration

**What goes wrong:**
Extension cannot access page content, gets "Cannot access contents of url" error, or fails to inject content scripts on certain sites.

**Why it happens:**
- Confusing `permissions` vs `host_permissions` in Manifest V3
- `host_permissions` required for accessing page URLs
- Some sites require specific host patterns, not wildcards
- `activeTab` permission only grants access when user clicks extension icon

**How to avoid:**
- Add target domains to `host_permissions` array in manifest.json
- Use `<all_urls>` only if needed for every website
- For localhost development, explicitly add `http://localhost:*`
- Understand that `activeTab` gives temporary access on click, not persistent access

**Warning signs:**
- "Extension manifest must request permission to access this host" errors
- Content script injects on some sites but not others
- Works in development but fails on specific production domains

**Phase to address:** Implementation phase - configure permissions early with test domains

---

### Pitfall 5: Using Web Storage Instead of chrome.storage

**What goes wrong:**
Data doesn't persist, isn't accessible across extension contexts, or gets cleared unexpectedly when user clears browsing data.

**Why it happens:**
- `window.localStorage` in content scripts is isolated to that page's origin
- Web storage doesn't work in background script at all
- Data clears when user clears cache/cookies (chrome.storage persists)
- Quota limits on localStorage are different

**How to avoid:**
- Always use `chrome.storage.local` or `chrome.storage.sync` for extension data
- Use `chrome.storage` API in all extension contexts (background, popup, content scripts)
- Add `"storage"` permission in manifest
- Use `navigator.storage.persist()` with `"unlimitedStorage"` permission for large data

**Warning signs:**
- Popup shows no data that background script saved
- Data disappears after browser restart
- localStorage works in popup but not in content script

**Phase to address:** Architecture phase - establish storage pattern early

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Using localStorage for quick prototyping | No permission needed | Breaks across contexts, data loss | Never in production |
| Hardcoding target URLs | Skip permissions setup | Breaks when adding new sites | MVP only, must refactor |
| Ignoring service worker lifecycle | Simpler code | Random failures, state loss | Never |
| Single message without connection | Less setup | No reconnection, race conditions | Never for persistent features |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Tauri Backend | Using `fetch` without proper CORS/permissions | Add localhost to host_permissions, use proper error handling |
| External APIs | Assuming fetch works like in regular web app | Request full URL in permissions, handle service worker wake |
| Content Script → Background | Not checking if background is ready | Use connection-based messaging, handle disconnection |
| Popup → Content | Sending message to wrong tab | Always get current tab ID first with `chrome.tabs.query` |

---

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Large data in storage | Slow operations, quota errors | Chunk large data, use IndexedDB for big payloads | >1MB data |
| Too many content script injections | Slow page loads | Use `run_at: document_idle`, declarative patterns | 10+ sites, SPA navigation |
| Service worker wake spam | High CPU, battery drain | Debounce events, use coalescing | High-frequency tab switching |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Overly broad host permissions (`*://*/*`) | User trust loss, security review issues | Request only needed domains |
| Storing sensitive data in chrome.storage without encryption | Data exposure if device compromised | Don't store secrets; use proper backend auth |
| Not validating messages from content scripts | Injection attacks | Always validate message structure and origin |
| Including keys/secrets in extension code | Exposed to all users | Never embed secrets; use external auth flow |

---

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|------------------|
| No feedback during extraction | User doesn't know if it worked | Show toast/notification on completion |
| Asking for all permissions upfront | User distrust, installation drop | Use optional permissions, request on first need |
| Breaking on page navigation (SPA) | Works once then stops | Listen to `chrome.webNavigation` or use `chrome.tabs.onUpdated` |
| No handling for extraction failures | Silent failures, no recourse | Show clear error messages, allow retry |

---

## "Looks Done But Isn't" Checklist

- [ ] **Message passing:** Tested after service worker restart, not just initial load
- [ ] **Permissions:** Verified on actual target sites, not just dev localhost
- [ ] **Storage:** Verified data persists across browser restart and extension reload
- [ ] **Content script:** Works on SPA pages that navigate without full reload
- [ ] **Popup:** Works when opened after service worker has been terminated
- [ ] **Error handling:** All chrome API calls handle `chrome.runtime.lastError`

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Message passing broken | MEDIUM | Refactor to connection-based messaging, add reconnection logic |
| Service worker state loss | HIGH | Redesign to stateless pattern, move state to chrome.storage |
| Permission denied | LOW | Update manifest, push update, users must re-enable |
| Storage data loss | MEDIUM | Migrate to chrome.storage, implement data recovery if possible |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Message Passing | Implementation - Build robust comms early | Test after service worker reload |
| Service Worker Lifecycle | Architecture - Design stateless pattern | Leave idle, verify still works |
| CSP Violations | Implementation - Check manifest config | Load on new site, check console |
| Permissions | Implementation - Configure early | Test on target domains |
| Storage Mistakes | Architecture - Choose chrome.storage | Verify persistence across restarts |
| Cross-browser | Implementation - Test Firefox/Safari | Run on multiple browsers |

---

## Sources

- [Chrome Extensions MV3 Migration Guide](https://developer.chrome.com/docs/extensions/develop/migrate/checklist) - Official migration checklist
- [Firefox Extension Workshop - Manifest V3](https://extensionworkshop.com/documentation/develop/manifest-v3-migration-guide) - Cross-browser differences
- [Stack Overflow - Common Chrome Extension Issues](https://stackoverflow.com/questions/tagged/google-chrome-extension) - Community problem patterns
- [Chrome for Developers - Storage](https://developer.chrome.com/docs/extensions/develop/concepts/storage-and-cookies) - Official storage documentation
- [MDN - Cross-browser Extensions](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Build_a_cross_browser_extension) - Browser compatibility

---
*Pitfalls research for: Browser Extension Development*
*Researched: 2026-02-24*
