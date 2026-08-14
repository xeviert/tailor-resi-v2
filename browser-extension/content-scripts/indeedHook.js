(function () {
  if (window.__resiTailorIndeed) return;
  window.__resiTailorIndeed = true;

  // --- _initialData probes (various Indeed page types) ---
  function probeInitialData() {
    // Direct job page: indeed.com/viewjob?jk=X
    const m1 = window._initialData?.jobInfoWrapperModel?.jobInfoModel;
    if (m1?.jobTitle) return m1;

    // Search results page: window.mosaic.providerData
    const mosaic = window.mosaic?.providerData;
    if (mosaic) {
      const m2 = mosaic['mosaic-provider-jobcards']?.state?.selectedJob;
      if (m2?.jobTitle) return m2;
      // Alternative mosaic paths
      for (const key of Object.keys(mosaic)) {
        const val = mosaic[key]?.state?.jobInfoWrapperModel?.jobInfoModel;
        if (val?.jobTitle) return val;
      }
    }

    // Nested _initialData paths
    const id = window._initialData;
    if (!id) return null;
    // Sometimes nested under a module key
    for (const key of Object.keys(id)) {
      const val = id[key]?.jobInfoWrapperModel?.jobInfoModel;
      if (val?.jobTitle) return val;
    }
    return null;
  }

  // --- DOM scraping fallback ---
  function scrapePanel() {
    // Indeed's right-panel job detail selectors (tested Feb 2026)
    const title =
      document.querySelector('[data-testid="jobsearch-JobInfoHeader-title"]')?.textContent?.trim() ||
      document.querySelector('.jobsearch-JobInfoHeader-title')?.textContent?.trim() ||
      document.querySelector('.jobTitle')?.textContent?.trim();

    const company =
      document.querySelector('[data-testid="inlineHeader-companyName"] a')?.textContent?.trim() ||
      document.querySelector('[data-testid="inlineHeader-companyName"]')?.textContent?.trim() ||
      document.querySelector('[class*="companyName"]')?.textContent?.trim();

    const location =
      document.querySelector('[data-testid="inlineHeader-companyLocation"]')?.textContent?.trim() ||
      document.querySelector('[data-testid="job-location"]')?.textContent?.trim();

    const descEl =
      document.querySelector('#jobDescriptionText') ||
      document.querySelector('[class*="jobsearch-jobDescriptionText"]') ||
      document.querySelector('[class*="jobDescriptionText"]');
    const description = descEl?.innerText?.trim();

    if (!title || !description || description.length < 50) return null;
    return { title, company, location, description };
  }

  // --- Main extract-and-post ---
  let lastPostedUrl = '';

  function extractAndPost() {
    const url = location.href;

    // Try _initialData first
    const model = probeInitialData();
    if (model) {
      const job = {
        title: model.jobTitle || model.title,
        company: model.companyName,
        location:
          model.jobLocationModel?.displayName ||
          [model.jobLocationModel?.city, model.jobLocationModel?.stateCode]
            .filter(Boolean).join(', '),
        description:
          model.sanitizedJobDescription ||
          model.jobDescription ||
          model.jobDescriptionText,
        employmentType: model.employmentType,
        salary:
          model.salarySnippet?.text ||
          model.salaryInfoModel?.formattedSalaryText,
        '@type': 'JobPosting',
      };
      if (job.title && job.description) {
        lastPostedUrl = url;
        window.postMessage(
          { type: 'RESITAILOR_JOBS', jobs: [job], source: 'indeed-hook', url },
          '*'
        );
        return;
      }
    }

    // DOM fallback
    const domJob = scrapePanel();
    if (domJob) {
      lastPostedUrl = url;
      window.postMessage(
        { type: 'RESITAILOR_JOBS', jobs: [{ ...domJob, '@type': 'JobPosting' }], source: 'indeed-dom', url },
        '*'
      );
    }
  }

  // --- MutationObserver: watch right panel for content swaps ---
  function observePanel() {
    // Indeed swaps content inside .jobsearch-RightPane or #jobDetailPageBig
    const target =
      document.querySelector('.jobsearch-RightPane') ||
      document.querySelector('#jobDetailPageBig') ||
      document.querySelector('[class*="RightPane"]') ||
      document.body;

    let debounce;
    new MutationObserver(() => {
      clearTimeout(debounce);
      debounce = setTimeout(() => {
        if (location.href !== lastPostedUrl) extractAndPost();
      }, 500);
    }).observe(target, { childList: true, subtree: true });
  }

  // --- Initial load ---
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => {
      extractAndPost();
      observePanel();
    });
  } else {
    extractAndPost();
    observePanel();
  }

  // --- SPA navigation (kept as belt-and-suspenders) ---
  const origPushState = history.pushState.bind(history);
  history.pushState = function (...args) {
    origPushState(...args);
    setTimeout(extractAndPost, 800);
  };
  window.addEventListener('popstate', () => setTimeout(extractAndPost, 800));
})();
