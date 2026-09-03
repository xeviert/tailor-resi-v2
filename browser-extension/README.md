# Job Posting Extractor

A browser extension that extracts job posting data (title, company, description) from any web page and sends it to a local Tauri backend for analysis.

## Features

- **One-click extraction** - Click the extension icon, then "Extract Job" to grab data from the current page
- **Heuristic selectors** - Works on generic job posting pages without site-specific templates
- **Local backend integration** - Sends extracted data to `POST 127.0.0.1:3000/captures`
- **Error handling** - Graceful error messages when backend is unavailable

## Installation

Packaged ResiTailor builds include this directory. In the desktop app, expand **App setup**
and choose **Open extension folder**; use that persistent installed folder below. The manifest
public key keeps the unpacked extension ID stable across machines and upgrades.

1. Open Chrome and navigate to `chrome://extensions/`
2. Enable **Developer mode** (toggle in top right)
3. Click **Load unpacked**
4. Select the `browser-extension/` directory

## Usage

1. Navigate to a job posting page (e.g., LinkedIn, Indeed, company careers page)
2. Click the extension icon in the browser toolbar
3. Click **Extract Job** in the popup
4. Data is sent to `127.0.0.1:3000/captures` and appears in the desktop app.
5. Choose EN or FR beside **Analyze & Generate PDF** in the desktop app to run the AI pipeline.

### Expected Backend Payload

```json
{
  "title": "Software Engineer",
  "company": "Acme Corp",
  "description": "Job description text...",
  "url": "https://example.com/job/123"
}
```

## Development

The extension uses Manifest V3 with vanilla JavaScript (no build tooling required).

### File Structure

```
browser-extension/
├── manifest.json           # Extension manifest (MV3)
├── popup/
│   ├── popup.html          # Popup UI
│   └── popup.js            # Popup logic
├── background/
│   └── background.js       # Service worker
├── content-scripts/
│   └── content.js          # DOM extraction
└── icons/
    ├── icon16.png
    ├── icon48.png
    └── icon128.png
```

### Reloading Changes

After making changes to source files:
1. Go to `chrome://extensions/`
2. Click the refresh icon on the extension card

## Backend Requirement

This extension expects the Rust/Tauri bridge on `127.0.0.1:3000` that handles `POST /captures` requests. Reload the unpacked extension after updating its source so an old service worker cannot continue posting to the retired `/analyze` endpoint.

The extension will display friendly error messages if the backend is not running.

## Requirements Met

- [x] EXT-01: Extract job title from current web page
- [x] EXT-02: Extract company name from current web page
- [x] EXT-03: Extract job description text from current web page
- [x] INT-01: Send extracted data as JSON to POST 127.0.0.1:3000/captures
- [x] INT-02: Handle successful response from backend
- [x] INT-03: Handle errors gracefully when backend is unavailable
- [x] CORE-01: Extension installs via browser
- [x] CORE-02: Extension toolbar icon provides one-click extraction trigger
- [x] CORE-03: Extension uses Manifest V3
