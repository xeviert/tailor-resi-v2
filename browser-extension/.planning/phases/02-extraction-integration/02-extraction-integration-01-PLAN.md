---
phase: 02-extraction-integration
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - content-scripts/content.js
  - background/background.js
  - popup/popup.js
  - popup/popup.html
autonomous: true
requirements: [EXT-01, EXT-02, EXT-03, INT-01, INT-02, INT-03]
---

<objective>
Extract job posting data (title, company, description) from web pages and send to Tauri backend at localhost:3000/analyze with proper error handling.
</objective>

<context>
@.planning/phases/01-extension-foundation/01-extension-foundation-SUMMARY.md

**Phase 1 completed:**
- Manifest V3 extension with toolbar icon
- Popup (320x200) with "Extract Job" button
- Content script skeleton listening for `extractJob` action
- Background service worker skeleton

**Phase 2 requirements:**
- EXT-01: Extract job title from current web page
- EXT-02: Extract company name from current web page  
- EXT-03: Extract job description text from current web page
- INT-01: Send extracted data as JSON to POST localhost:PORT/analyze
- INT-02: Handle successful response from backend
- INT-03: Handle errors gracefully when backend is unavailable
</context>

<tasks>

<task type="auto">
  <name>Task 1: Implement content script DOM extraction</name>
  <files>content-scripts/content.js</files>
  <action>
    Replace the existing content script skeleton with full extraction logic:

    1. Add selector arrays for job title, company, and description (use heuristic approach - multiple selectors in priority order):
       - Title selectors: h1, h2, [class*="title"], [class*="job-title"], meta[property="og:title"]
       - Company selectors: [class*="company"], [class*="employer"], a[href*="/company/"], meta[property="og:site_name"]
       - Description selectors: meta[name="description"], [class*="description"], [class*="details"], article, section

    2. Create function extractJobData() that:
       - Tries each selector in priority order until text is found
       - Returns null for fields not found
       - Includes the page URL in the returned data

    3. Update the message listener to call extractJobData() and return the actual extracted data instead of { status: 'ready' }

    **Important:** Do NOT use any external libraries - pure JavaScript DOM manipulation only.
  </action>
  <verify>
    Load extension, navigate to a job posting page, check console for extracted data output.
  </verify>
  <done>
    Content script extracts job title, company, description, and URL from any web page and returns them via sendResponse.
  </done>
</task>

<task type="auto">
  <name>Task 2: Wire up message flow and backend POST</name>
  <files>background/background.js, popup/popup.js</files>
  <action>
    **Background script (background.js) changes:**

    1. Update the message listener to handle the full extraction flow:
       - Receive message with action 'extractJob' from popup
       - Get the active tab using chrome.tabs.query
       - Use chrome.scripting.executeScript to inject content script into the tab
       - Receive extracted data from content script
       - POST the data to http://localhost:3000/analyze using fetch API
       - Return success/error response back to popup

    2. Implement the POST request:
       - Endpoint: http://localhost:3000/analyze
       - Method: POST
       - Headers: Content-Type: application/json
       - Body: JSON.stringify({ title, company, description, url })

    **Popup script (popup.js) changes:**

    1. Replace placeholder with actual extraction call:
       - On button click, send message to background via chrome.runtime.sendMessage
       - Include action: 'extractJob'
       - Handle response and update status div with result
       - Show "Extracting..." while waiting for response
  </action>
  <verify>
    Click "Extract Job" button in popup - should trigger extraction flow and POST to backend (may fail if backend not running, but flow should work).
  </verify>
  <done>
    Popup button triggers extraction, content script runs on page, data is POSTed to localhost:3000/analyze, result is returned to popup.
  </done>
</task>

<task type="auto">
  <name>Task 3: Add error handling and user feedback</name>
  <files>popup/popup.js, popup/popup.html</files>
  <action>
    **Popup UI improvements:**

    1. Add loading state - change button to disabled state and show "Extracting..." status while waiting

    2. Add error handling in popup.js:
       - Connection refused / ECONNREFUSED: "Cannot connect to backend. Make sure Tauri app is running on port 3000."
       - Timeout (network timeout): "Backend took too long. Is it running?"
       - HTTP error (4xx/5xx): "Backend error: {statusCode}"
       - Success: "Job data sent successfully!"

    3. Add retry functionality:
       - After error, re-enable button so user can retry
       - Keep error message visible until next extraction attempt

    4. Add CSS styling for error/success states in popup.html (optional: use color coding - green for success, red for error)

    **Note:** Ensure the popup handles the case where no data is extracted (empty title/company) - still send to backend but let backend handle validation.
  </action>
  <verify>
    Test with backend not running - should show connection error. Test with backend running - should show success message.
  </verify>
  <done>
    User sees appropriate feedback: "Extracting..." during extraction, success message on success, helpful error messages on failure with retry capability.
  </done>
</task>

</tasks>

<verification>
- [ ] Content script extracts title, company, description, url from any page
- [ ] Popup → Background → Content script → Backend flow works end-to-end
- [ ] POST to localhost:3000/analyze with JSON payload
- [ ] Success shows confirmation message
- [ ] Errors (connection refused, timeout, HTTP errors) show appropriate messages
- [ ] Button re-enables after error for retry
</verification>

<success_criteria>
User can extract job title, company, and description from any web page by clicking "Extract Job" in the popup. The extracted data is sent as JSON to POST localhost:3000/analyze. User sees success confirmation or helpful error message if backend is unavailable.
</success_criteria>

<output>
After completion, create `.planning/phases/02-extraction-integration/02-extraction-integration-01-SUMMARY.md`
</output>
