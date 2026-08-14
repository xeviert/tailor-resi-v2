// ResiTailor - MAIN world page hook
// Monkey-patches fetch/XHR to intercept JSON responses and find job-shaped objects

(function () {
  'use strict';

  const HOOK_ID = '__resiTailor';
  if (window[HOOK_ID]) return;
  window[HOOK_ID] = true;

  // --- Noise URL filter ---
  const NOISE_PATTERNS = [
    /analytics/i, /telemetry/i, /tracking/i, /pixel/i, /beacon/i,
    /fonts\./i, /\.woff/, /\.ttf/, /sentry/i, /hotjar/i, /segment\./i,
    /google-analytics/i, /gtag/i, /fbevents/i, /doubleclick/i,
    /ads\./i, /adservice/i, /pagead/i, /log[_-]?event/i,
    /csrf/i, /token\/refresh/i, /heartbeat/i, /health[_-]?check/i,
    /\.svg$/, /\.png$/, /\.jpg$/, /\.gif$/, /\.css$/, /\.js$/,
  ];

  function isNoiseUrl(url) {
    for (const pat of NOISE_PATTERNS) {
      if (pat.test(url)) return true;
    }
    return false;
  }

  // --- Job shape detection ---
  const TITLE_KEYS = ['title', 'jobtitle', 'job_title', 'position', 'role', 'name'];
  const DESC_KEYS = ['description', 'jobdescription', 'job_description', 'content', 'body'];
  const EXTRA_KEYS = [
    'company', 'companyname', 'company_name', 'hiringorganization', 'hiring_organization', 'employer',
    'location', 'joblocation', 'job_location', 'city', 'state',
    'salary', 'salaryrange', 'salary_range', 'compensation', 'pay',
  ];

  function findKey(obj, candidates) {
    const keys = Object.keys(obj);
    for (const k of keys) {
      if (candidates.includes(k.toLowerCase())) return obj[k];
    }
    return undefined;
  }

  function isJobShaped(obj) {
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return false;

    // JSON-LD JobPosting — always accept
    if (obj['@type'] === 'JobPosting') return true;

    const title = findKey(obj, TITLE_KEYS);
    const desc = findKey(obj, DESC_KEYS);

    if (typeof title !== 'string' || title.length < 2 || title.length > 200) return false;
    if (typeof desc !== 'string' || desc.length < 50) return false;

    // Must have at least one extra key
    const hasExtra = EXTRA_KEYS.some((k) => {
      const keys = Object.keys(obj);
      return keys.some((ok) => ok.toLowerCase() === k);
    });

    return hasExtra;
  }

  // --- Deep search ---
  function deepFindJobs(data, depth = 0, results = []) {
    if (depth > 10 || results.length >= 5) return results;
    if (!data || typeof data !== 'object') return results;

    if (Array.isArray(data)) {
      for (const item of data) {
        deepFindJobs(item, depth + 1, results);
        if (results.length >= 5) break;
      }
    } else {
      if (isJobShaped(data)) {
        results.push(data);
      }
      for (const val of Object.values(data)) {
        if (val && typeof val === 'object') {
          deepFindJobs(val, depth + 1, results);
          if (results.length >= 5) break;
        }
      }
    }
    return results;
  }

  function postJobs(jobs, source) {
    if (!jobs.length) return;
    window.postMessage({
      type: 'RESITAILOR_JOBS',
      jobs,
      source,
      url: location.href,
    }, '*');
  }

  // --- Monkey-patch fetch ---
  const origFetch = window.fetch;
  window.fetch = async function (...args) {
    const response = await origFetch.apply(this, args);
    try {
      const url = (typeof args[0] === 'string' ? args[0] : args[0]?.url) || '';
      if (isNoiseUrl(url)) return response;

      const ct = response.headers?.get('content-type') || '';
      if (!ct.includes('json')) return response;

      const clone = response.clone();
      clone.json().then((data) => {
        const jobs = deepFindJobs(data);
        postJobs(jobs, `fetch:${url}`);
      }).catch(() => {});
    } catch (_) {}
    return response;
  };

  // --- Monkey-patch XHR ---
  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;

  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    this._rtUrl = url;
    return origOpen.call(this, method, url, ...rest);
  };

  XMLHttpRequest.prototype.send = function (...args) {
    this.addEventListener('load', function () {
      try {
        const url = this._rtUrl || '';
        if (isNoiseUrl(url)) return;

        const ct = this.getResponseHeader('content-type') || '';
        if (!ct.includes('json')) return;

        const data = JSON.parse(this.responseText);
        const jobs = deepFindJobs(data);
        postJobs(jobs, `xhr:${url}`);
      } catch (_) {}
    });
    return origSend.apply(this, args);
  };

  // --- Scan JSON-LD on page load ---
  function scanJsonLd() {
    const scripts = document.querySelectorAll('script[type="application/ld+json"]');
    const jobs = [];
    for (const script of scripts) {
      try {
        const data = JSON.parse(script.textContent);
        deepFindJobs(data, 0, jobs);
      } catch (_) {}
    }
    postJobs(jobs, 'json-ld');
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', scanJsonLd);
  } else {
    scanJsonLd();
  }
})();
