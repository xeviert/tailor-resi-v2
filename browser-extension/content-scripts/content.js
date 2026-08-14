// ResiTailor Extractor - Content Script (ISOLATED world)
// Bridge between pageHook.js (MAIN world) and background service worker

const collectedJobs = [];

// Listen for jobs from pageHook.js
window.addEventListener('message', (event) => {
  if (event.source !== window) return;
  if (event.data?.type !== 'RESITAILOR_JOBS') return;

  const { jobs, source } = event.data;
  for (const job of jobs) {
    collectedJobs.push({ job, source, timestamp: Date.now() });
  }

  // Forward to background
  chrome.runtime.sendMessage({
    action: 'jobsCaptured',
    jobs,
    source,
    pageUrl: location.href,
    pageTitle: document.title,
  }).catch(() => {});
});

// DOM scraping fallback — extract from JSON-LD and meta tags
function scrapeDOM() {
  const results = [];

  // JSON-LD
  const scripts = document.querySelectorAll('script[type="application/ld+json"]');
  for (const script of scripts) {
    try {
      const data = JSON.parse(script.textContent);
      const items = Array.isArray(data) ? data : [data];
      for (const item of items) {
        if (item['@type'] === 'JobPosting') {
          results.push(item);
        }
        // Check @graph
        if (item['@graph']) {
          for (const node of item['@graph']) {
            if (node['@type'] === 'JobPosting') results.push(node);
          }
        }
      }
    } catch (_) {}
  }

  // Meta tags fallback
  if (results.length === 0) {
    const title = document.querySelector('meta[property="og:title"]')?.content
      || document.querySelector('h1')?.textContent?.trim()
      || document.title;
    const desc = document.querySelector('meta[property="og:description"]')?.content
      || document.querySelector('meta[name="description"]')?.content
      || '';
    if (title && desc && desc.length >= 30) {
      results.push({
        title,
        description: desc,
        source_url: location.href,
        _scraped: true,
      });
    }
  }

  return results;
}

// Handle messages from background/popup
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.action === 'getCollectedJobs') {
    sendResponse({ jobs: collectedJobs.map((c) => c.job) });
  } else if (message.action === 'activeExtract') {
    const scraped = scrapeDOM();
    sendResponse({ jobs: scraped });
  } else if (message.action === 'getPageMeta') {
    sendResponse({ title: document.title, url: location.href });
  }
  return true;
});
