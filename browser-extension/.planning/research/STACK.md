# Technology Stack

**Project:** Job Posting Extractor (Browser Extension)
**Researched:** 2026-02-24

## Recommended Stack

### Core Framework
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| WXT | ^0.20.x | Extension framework | Clear 2025 market leader for MV3 browser extensions. Built on Vite, first-class TypeScript, cross-browser support (Chrome/Firefox/Edge), auto-reload, zero-config for Manifest V3. Active maintenance with recent releases. |
| TypeScript | ^5.x | Language | Required per project constraints. WXT uses TS by default. Enables type safety for content script DOM parsing and message passing. |

### Build & Bundling
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| Vite | ^6.x (via WXT) | Bundler | Built into WXT. Fast HMR, tree-shaking, handles extension-specific bundling (content scripts, background, etc). |
| WXT Built-ins | - | Manifest, i18n, auto-icons | Eliminates boilerplate. Generates manifest.json from config, handles browser polyfills automatically. |

### Testing
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| Vitest | ^4.x | Unit testing | Vite-native, fast, works with WXT. Supports browser mode via Playwright for DOM testing. |
| Playwright | ^1.50.x | E2E/integration testing | Official support for Chrome extensions (Chromium only). Use for testing popup, options page, content script behavior in real browser. |

### UI (Optional - for popup/options)
| Technology | Version | Purpose | When to Use |
|------------|---------|---------|-------------|
| React | ^19.x | UI framework | If popup/options page needs complex UI. WXT has React template. |
| Vue | ^3.x | UI framework | Alternative to React. WXT supports natively. |
| TailwindCSS | ^4.x | Styling | If UI framework used. WXT supports UnoCSS and TailCSS modules. |

### Data Handling
| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| @wxt-dev/storage | ^1.x | Extension storage | Official WXT module for chrome.storage with type safety. Handles sync vs local automatically. |
| Native fetch | - | HTTP client | For sending extracted data to Tauri backend. No additional library needed. |

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|--------------|---------|
| Framework | WXT | Plasmo | Plasmo has slower maintenance (last release 8+ months). WXT has more active development, better cross-browser support, and superior DX. |
| Framework | WXT | CRXJS | CRXJS is just a bundler plugin, not a full framework. Requires manual setup of dev server, manifest generation, etc. |
| Framework | WXT | Vanilla + Webpack | WXT provides same outcome with far less boilerplate. Webpack config for extensions is complex. |
| Bundler | Vite (via WXT) | Parcel (via Plasmo) | Plasmo uses Parcel which has slower bundling and less debugging support than Vite. |
| Testing | Vitest + Playwright | Jest | Jest requires extensive config for extensions. Vitest integrates natively with Vite/WXT ecosystem. |
| Storage | @wxt-dev/storage | custom chrome.storage wrapper | Official module provides TypeScript types and simpler API. |

## Not Recommended

| Technology | Why Avoid |
|------------|-----------|
| Manifest V2 | Deprecated by Chrome as of June 2025. All new extensions must use MV3. |
| Plain JavaScript | No type safety for extension APIs, message passing, and DOM parsing. Project requires TypeScript. |
| Older extension frameworks (extensions-node, ext-ts) | Not actively maintained, lack MV3 support. |

## Installation

```bash
# Initialize with WXT (select vanilla TypeScript template)
pnpm dlx wxt@latest init job-extractor

cd job-extractor

# Install testing dependencies
pnpm add -D vitest @vitest/browser-playwright playwright

# Run in development mode
pnpm dev
```

## Project Structure (WXT Default)

```
/
├── entrypoints/
│   ├── background.ts      # Service worker (MV3 background)
│   ├── content.ts         # Content script (injected into pages)
│   ├── popup/
│   │   └── Main.tsx      # Popup UI (optional)
│   └── options/
│       └── Main.tsx      # Options page (optional)
├── public/
│   └── icons/             # Extension icons
├── wxt.config.ts          # WXT configuration
├── package.json
└── tsconfig.json
```

## Sources

- [WXT Official Docs](https://wxt.dev/) — v0.20.18, current as of Feb 2026
- [WXT vs Plasmo vs CRXJS Comparison](https://wxt.dev/guide/resources/compare.html) — Feature matrix
- [2025 State of Browser Extension Frameworks](https://redreamality.com/blog/the-2025-state-of-browser-extension-frameworks-a-comparative-analysis-of-plasmo-wxt-and-crxjs/) — Market analysis
- [Chrome Extension Development 2025](https://www.devkit.best/blog/mdx/chrome-extension-framework-comparison-2025) — Framework comparison
- [Chrome Manifest V3 Migration](https://developer.chrome.com/docs/extensions/develop/migrate/checklist) — MV3 requirements
- [Playwright Chrome Extension Testing](https://playwright.dev/docs/next/chrome-extensions) — E2E testing docs
