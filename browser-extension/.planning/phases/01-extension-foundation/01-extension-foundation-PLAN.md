---
phase: 01-extension-foundation
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - package.json
  - wxt.config.ts
  - tsconfig.json
  - entry/popup/MainPopup.vue
  - entry/popup/main.ts
autonomous: false
requirements: [CORE-01, CORE-02, CORE-03]

must_haves:
  truths:
    - Extension installs in Chrome/Firefox/Edge via browser's extension manager
    - Toolbar icon appears in browser after installation
    - Clicking toolbar icon shows popup UI (one-click trigger ready)
    - Extension uses Manifest V3 format
  artifacts:
    - path: "wxt.config.ts"
      provides: "WXT framework configuration"
      contains: "manifest"
    - path: "package.json"
      provides: "Project dependencies and scripts"
      contains: "wxt"
    - path: "entry/popup/MainPopup.vue"
      provides: "One-click extraction popup UI"
      contains: "template"
    - path: "entry/content-scripts/index.ts"
      provides: "Content script entry point"
      contains: "defineContentScript"
  key_links:
    - from: "entry/popup/MainPopup.vue"
      to: "manifest"
      via: "WXT auto-generation"
      pattern: "popup in manifest"
---

<objective>
Set up WXT browser extension project with TypeScript, Manifest V3, and toolbar icon with popup UI.

Purpose: Establish the foundation for the job posting extractor extension. WXT provides cross-browser support, zero-config manifest generation, and auto-reload during development.

Output: Functional browser extension shell that installs and displays a toolbar icon with popup.
</objective>

<execution_context>
@C:/Users/xevie/.config/opencode/get-shit-done/workflows/execute-plan.md
@C:/Users/xevie/.config/opencode/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/ROADMAP.md
@.planning/REQUIREMENTS.md
@.planning/research/SUMMARY.md
</context>

<tasks>

<task type="auto">
  <name>Initialize WXT project with TypeScript</name>
  <files>package.json, wxt.config.ts, tsconfig.json</files>
  <action>
    1. Run `npx wxt init` in the project root to scaffold WXT project
    2. Select TypeScript when prompted
    3. Update package.json with proper project name: "resi-tailor-extractor"
    4. Verify project structure created:
       - entry/popup/ (popup entry)
       - entry/content-scripts/ (content script entry)
       - entry/background/ (service worker entry)
    5. Run `npm install` to install dependencies
    6. Verify WXT version is 0.20.x per research recommendation
  </action>
  <verify>
    - Run `npx wxt --version` to confirm WXT installed
    - Run `npx wxt build` to verify project builds
    - Check package.json contains wxt dependency
  </verify>
  <done>
    - WXT project scaffolded with TypeScript
    - `npm run dev` starts dev server without errors
    - `npm run build` produces extension output
  </done>
</task>

<task type="auto">
  <name>Configure Manifest V3 and toolbar icon</name>
  <files>wxt.config.ts, package.json</files>
  <action>
    1. Configure wxt.config.ts with:
       - manifest: { name: "ResiTailor Extractor", version: "1.0.0" }
       - permissions: ["activeTab", "storage", "scripting"]
       - host_permissions: ["<all_urls>"] for job site access
       - action: { default_popup: "popup/index.html", default_icon: {16, 48, 128} }
    2. Create icon files in public/icons/ (16x16, 48x48, 128x128 PNG)
       - Use simple extraction/trigger icon design
    3. Configure popup in entry/popup/main.ts to mount MainPopup.vue
    4. Verify manifest.json is generated with correct fields
  </action>
  <verify>
    - Check .output/chrome-mv3/manifest.json has "action" with popup
    - Check icons exist in .output/chrome-mv3/icons/
    - Verify "manifest_version": 3 in manifest
  </verify>
  <done>
    - Manifest V3 configured with toolbar icon
    - Popup defined in manifest
    - Extension has proper permissions for extraction
  </done>
</task>

<task type="auto">
  <name>Create popup UI for one-click trigger</name>
  <files>entry/popup/MainPopup.vue, entry/popup/main.ts</files>
  <action>
    1. Create simple popup UI in entry/popup/MainPopup.vue:
       - Display extension name and brief instruction
       - Add "Extract Job" button as primary action
       - Add status indicator area (for future extraction feedback)
    2. Style popup with minimal CSS (WXT uses UnoCSS by default)
    3. Wire button to trigger extraction (Phase 2 will connect the actual extraction)
       - For now, button logs to console or shows "Ready to extract" message
    4. Set popup dimensions: 320x200px (reasonable for minimal UI)
  </action>
  <verify>
    - Run `npx wxt` to start dev server
    - Click toolbar icon in Chrome - popup should appear
    - Verify "Extract Job" button is visible and clickable
    - Button click shows "Ready to extract" message
  </verify>
  <done>
    - Popup UI displays when toolbar icon clicked
    - Extract button is visible and interactive
    - User sees clear call-to-action
  </done>
</task>

<task type="checkpoint:human-verify">
  <name>Verify extension installation</name>
  <action>Human verification required - extension must be loaded in browser and tested</action>
  <files>N/A - Manual browser verification</files>
  <what-built>Complete Phase 1 extension foundation (WXT + TypeScript + Manifest V3 + toolbar icon + popup)</what-built>
  <how-to-verify>
    1. Load extension in Chrome:
       - Open chrome://extensions/
       - Enable "Developer mode" (top right)
       - Click "Load unpacked"
       - Select the .output/chrome-mv3 directory
    2. Verify extension installs:
       - No errors in Chrome console
       - Extension icon appears in toolbar
    3. Test toolbar icon:
       - Click extension icon in toolbar
       - Popup appears with "Extract Job" button
       - Button is clickable and shows feedback
    4. Confirm Manifest V3:
       - Check chrome://extensions details shows "Manifest Version 3"
  </how-to-verify>
  <verify>Manual browser verification</verify>
  <done>Extension installs correctly and toolbar icon works</done>
  <resume-signal>Type "approved" or describe issues found</resume-signal>
</task>

</tasks>

<verification>
- [ ] WXT project initializes without errors
- [ ] TypeScript compiles without errors
- [ ] Manifest V3 generated correctly
- [ ] Toolbar icon visible in browser
- [ ] Popup opens on icon click
- [ ] Extract button present in popup
- [ ] Extension loads without console errors
</verification>

<success_criteria>
- Extension installs in Chrome via chrome://extensions (CORE-01)
- Toolbar icon appears and triggers popup (CORE-02)
- Extension uses Manifest V3 format (CORE-03)
- One-click trigger UI is ready for Phase 2 extraction logic
</success_criteria>

<output>
After completion, create `.planning/phases/01-extension-foundation/01-extension-foundation-SUMMARY.md`
</output>
