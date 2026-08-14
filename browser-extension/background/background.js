// ResiTailor Extractor - Background Service Worker

console.log('ResiTailor background service worker loaded');

const BACKEND_URL = 'http://127.0.0.1:3000/captures';

// Per-tab job buffer: tabId → [{ job, score, source, timestamp }]
const tabJobs = new Map();

chrome.runtime.onInstalled.addListener((details) => {
  console.log('ResiTailor installed:', details.reason);
});

// Clean up when tab closes
chrome.tabs.onRemoved.addListener((tabId) => {
  tabJobs.delete(tabId);
});

// Reset on navigation
chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === 'loading') {
    tabJobs.delete(tabId);
    chrome.action.setBadgeText({ text: '', tabId });
  }
});

// --- Scoring ---

const BONUS_FIELDS = [
  'salary', 'salaryRange', 'compensation',
  'employmentType', 'employment_type',
  'datePosted', 'date_posted',
  'qualifications', 'requirements', 'skills',
  'benefits', 'remote', 'experienceLevel',
];

function scoreJob(job) {
  let score = 0;

  // Field richness
  const keys = Object.keys(job);
  score += Math.min(keys.length, 20);

  // Description length
  const desc = job.description || job.jobDescription || job.job_description || job.content || '';
  if (typeof desc === 'string') {
    score += Math.min(desc.length / 100, 20);
  }

  // JSON-LD bonus
  if (job['@type'] === 'JobPosting') score += 15;

  // Bonus fields
  for (const field of BONUS_FIELDS) {
    if (job[field] !== undefined) score += 3;
  }

  return score;
}

function updateBadge(tabId) {
  const jobs = tabJobs.get(tabId);
  const count = jobs ? jobs.length : 0;
  chrome.action.setBadgeText({ text: count > 0 ? String(count) : '', tabId });
  chrome.action.setBadgeBackgroundColor({ color: '#2563eb', tabId });
}

// --- Message handling ---

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.action === 'jobsCaptured') {
    const tabId = sender.tab?.id;
    if (!tabId) return;

    if (!tabJobs.has(tabId)) tabJobs.set(tabId, []);
    const buffer = tabJobs.get(tabId);

    for (const job of message.jobs) {
      buffer.push({
        job,
        score: scoreJob(job),
        source: message.source,
        timestamp: Date.now(),
      });
    }

    console.log(`Tab ${tabId}: ${buffer.length} jobs captured (source: ${message.source})`);
    updateBadge(tabId);
  }

  if (message.action === 'extractJob') {
    console.log('Extraction request received');
    handleExtractJob().then(sendResponse);
    return true; // async
  }

  return true;
});

// --- Extraction pipeline ---

async function handleExtractJob() {
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tabs?.length) return { success: false, error: 'No active tab found' };

  const tab = tabs[0];
  const tabId = tab.id;

  // Try buffered jobs first
  const buffer = tabJobs.get(tabId) || [];
  let bestJob = null;
  let bestScore = -1;

  for (const entry of buffer) {
    if (entry.score > bestScore) {
      bestScore = entry.score;
      bestJob = entry.job;
    }
  }

  // If no buffered jobs, try active DOM scrape
  if (!bestJob) {
    try {
      const response = await chrome.tabs.sendMessage(tabId, { action: 'activeExtract' });
      if (response?.jobs?.length) {
        for (const job of response.jobs) {
          const s = scoreJob(job);
          if (s > bestScore) {
            bestScore = s;
            bestJob = job;
          }
        }
      }
    } catch (e) {
      console.warn('Active extract failed:', e.message);
    }
  }

  if (!bestJob) {
    return { success: false, error: 'No job data found. Navigate to a job posting and wait a moment.' };
  }

  // Get page meta
  let pageTitle = tab.title || '';
  let sourceUrl = tab.url || '';
  try {
    const meta = await chrome.tabs.sendMessage(tabId, { action: 'getPageMeta' });
    if (meta) {
      pageTitle = meta.title || pageTitle;
      sourceUrl = meta.url || sourceUrl;
    }
  } catch (_) {}

  const payload = {
    sourceUrl,
    pageTitle,
    score: bestScore,
    json: bestJob,
    capturedCount: buffer.length,
  };

  console.log('Sending to backend, score:', bestScore);

  try {
    const response = await fetch(BACKEND_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    if (!response.ok) throw new Error(`HTTP error: ${response.status}`);

    const result = await response.json();
    console.log('Backend response:', result);
    return { success: true, data: result };
  } catch (error) {
    console.error('Backend send error:', error);
    return { success: false, error: error.message };
  }
}

console.log('ResiTailor background service worker ready');
