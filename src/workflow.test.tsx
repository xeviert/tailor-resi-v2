// @vitest-environment jsdom

import '@testing-library/jest-dom/vitest';
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));

import { App } from './main';

const analysis = { summary: 'Prioritize reliable backend delivery.' };
const captured = {
  received_at_ms: 42,
  payload: {},
  parsed: {
    domain: 'indeed',
    title: 'Backend Engineer',
    company: 'Example Co',
    description: 'Unique captured job description.',
  },
};
const preflight = {
  analysis,
  items: [],
};
function completedResume(score = 73, omitted = ['Angular in experience']) {
  return {
    success: true,
    tailoring_status: 'completed' as const,
    variant_slug: score === 73 ? 'source-role-en' : 'retailored-role-en',
    validation_status: 'passed',
    fit_status: 'passed',
    page_count: 1,
    experience_bullets_changed: 3,
    report: {
      estimated_ats_coverage_score: score,
      omitted_unsupported_keywords: omitted,
    },
    tailored_content: { summary: '', experience: [], skills: {} },
    content_changes: [],
    docx_path: null,
    latest_docx_path: null,
    pdf_path: 'resume/variants/result/resume.pdf',
    latest_pdf_path: 'resume/variants/result/resume.pdf',
    downloads_docx_path: null,
    downloads_docx_error: null,
    downloads_pdf_path: null,
    downloads_error: null,
    docx_opened: false,
    docx_open_error: null,
    artifact: null,
    retailor: null,
    error: null,
  };
}
let rejectTailoring = false;

beforeEach(() => {
  rejectTailoring = false;
  mocks.invoke.mockReset();
  mocks.listen.mockReset();
  mocks.listen.mockResolvedValue(() => undefined);
  mocks.invoke.mockImplementation((command: string) => {
    switch (command) {
      case 'get_latest_job':
        return Promise.resolve(captured);
      case 'get_latest_pipeline_result_any_language':
      case 'get_latest_pipeline_result':
        return Promise.resolve(null);
      case 'get_evidence_bank':
        return Promise.resolve({ version: 1, entries: [] });
      case 'analyze_latest_job':
      case 'prepare_evidence_preflight':
        return Promise.resolve(preflight);
      case 'generate_tailored_resume':
        return rejectTailoring
          ? Promise.reject(new Error('Tailoring failed before rendering.'))
          : new Promise(() => undefined);
      default:
        return Promise.resolve(null);
    }
  });
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        bridge: 'tauri-rust',
        result_protocol_version: 2,
      }),
    }),
  );
  Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
    configurable: true,
    value: vi.fn(),
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// Wrapped in act() so the resulting state updates are flushed before we assert;
// otherwise an assertion can pass against the pre-event render.
async function emit(event: string, payload: unknown) {
  const handlers = mocks.listen.mock.calls
    .filter(([name]) => name === event)
    .map(([, handler]) => handler as (e: { payload: unknown }) => void);
  expect(handlers.length).toBeGreaterThan(0);
  await act(async () => {
    for (const handler of handlers) handler({ payload });
  });
}

describe('review-to-tailoring workflow', () => {
  it('shows output language before analysis and opens an immediate focused pipeline', async () => {
    render(<App />);

    expect(await screen.findByText('Unique captured job description.')).toBeVisible();
    // Offered up front so a wrong auto-detect can be corrected before an analysis call.
    expect(
      screen.getByRole('group', { name: 'Resume output language' }),
    ).toBeVisible();

    fireEvent.click(screen.getByRole('button', { name: 'Analyze job' }));

    expect(
      await screen.findByRole('group', { name: 'Resume output language' }),
    ).toBeVisible();
    expect(
      mocks.invoke.mock.calls.filter(([command]) => command === 'analyze_latest_job'),
    ).toHaveLength(1);

    fireEvent.click(screen.getByRole('button', { name: 'FR' }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('prepare_evidence_preflight', {
        language: 'fr',
        analysis,
      }),
    );
    expect(
      mocks.invoke.mock.calls.filter(([command]) => command === 'analyze_latest_job'),
    ).toHaveLength(1);

    fireEvent.click(
      screen.getByRole('button', { name: 'Generate tailored PDF' }),
    );

    expect(screen.getByTestId('focused-pipeline')).toBeVisible();
    expect(screen.getByTestId('pipeline-stage-ats_analysis')).toHaveAttribute(
      'data-status',
      'completed',
    );
    expect(
      screen.getByText('Starting resume tailoring with your reviewed evidence.'),
    ).toBeVisible();
    expect(
      screen.queryByText('Unique captured job description.'),
    ).not.toBeInTheDocument();
    expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled();
    expect(mocks.invoke).toHaveBeenCalledWith('generate_tailored_resume', {
      request: expect.objectContaining({ language: 'fr' }),
    });
  });

  // There is one tailoring mode now, so there is nothing to pick and nothing to send.
  it('offers no emphasis choice and sends no emphasis field', async () => {
    render(<App />);
    expect(await screen.findByText('Unique captured job description.')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Analyze job' }));
    expect(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    ).toBeVisible();

    expect(
      screen.queryByRole('group', { name: 'Experience keyword emphasis' }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole('button', { name: 'Generate tailored PDF' }),
    );

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith(
      'generate_tailored_resume',
      { request: expect.not.objectContaining({ bullet_keyword_emphasis: expect.anything() }) },
    ));
  });

  it('keeps a failed pipeline visible until the user returns to review', async () => {
    render(<App />);
    expect(await screen.findByText('Unique captured job description.')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Analyze job' }));
    expect(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    ).toBeVisible();

    rejectTailoring = true;
    fireEvent.click(
      screen.getByRole('button', { name: 'Generate tailored PDF' }),
    );

    expect(await screen.findByText('Pipeline stopped')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Back to evidence review' }),
    ).toBeVisible();
    expect(screen.getByTestId('focused-pipeline')).toBeVisible();

    fireEvent.click(
      screen.getByRole('button', { name: 'Back to evidence review' }),
    );
    expect(screen.getByText('Unique captured job description.')).toBeVisible();
  });

  it('arrives with the already-proven misses selected and the re-run ready', async () => {
    const base = completedResume();
    const sourceResume = {
      ...base,
      report: {
        ...base.report,
        ats_coverage: {
          score: 73,
          covered_weight: 10,
          total_weight: 20,
          editable_covered_weight: 8,
          categories: [],
          terms: [
            {
              term: 'Angular in experience',
              kind: 'technology',
              group: 'required',
              weight: 5,
              covered: false,
              coverage_ratio: 0,
              in_editable_surface: false,
              miss_reason: 'evidence_not_placed' as const,
            },
            {
              term: 'GCP in experience',
              kind: 'technology',
              group: 'required',
              weight: 5,
              covered: false,
              coverage_ratio: 0,
              in_editable_surface: false,
              miss_reason: 'no_evidence' as const,
            },
          ],
        },
      },
    };
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_latest_pipeline_result_any_language':
          return Promise.resolve({
            schema_version: 2,
            capture_received_at_ms: 42,
            language: 'en',
            recovered_from_artifacts: false,
            status: 'completed',
            summary: analysis.summary,
            failed_stage: null,
            error: null,
            analysis,
            resume: sourceResume,
          });
        case 'get_evidence_bank':
          return Promise.resolve({ version: 2, entries: [] });
        default:
          return Promise.resolve(null);
      }
    });
    render(<App />);

    // Coverage this person can already prove costs them nothing, so declining it should be the
    // action that takes a click - not claiming it. Landing on the summary with an inert button
    // and a list of terms the run "could have used" is what made the block read as a report
    // rather than as something to do.
    expect(
      await screen.findByRole('button', { name: 'Angular in experience' }),
    ).toHaveAttribute('aria-pressed', 'true');
    expect(
      screen.getByRole('button', { name: 'GCP in experience' }),
    ).toHaveAttribute('aria-pressed', 'false');
    expect(
      screen.getByRole('button', { name: 'Re-run tailoring with 1 keyword' }),
    ).toBeEnabled();
  });

  it('re-tailors selected omitted pills from the recovered analysis result', async () => {
    const sourceResume = completedResume();
    const retailoredResume = {
      ...completedResume(81, ['GCP in experience']),
      retailor: {
        source_variant_slug: 'source-role-en',
        source_ats_score: 73,
        selected_terms: ['Angular in experience'],
      },
    };
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_latest_pipeline_result_any_language':
          return Promise.resolve({
            schema_version: 2,
            capture_received_at_ms: 42,
            language: 'en',
            recovered_from_artifacts: false,
            status: 'completed',
            summary: analysis.summary,
            failed_stage: null,
            error: null,
            analysis,
            resume: sourceResume,
          });
        case 'get_latest_pipeline_result':
          return Promise.resolve(null);
        case 'get_evidence_bank':
          return Promise.resolve({ version: 2, entries: [] });
        case 'retailor_resume_with_evidence':
          return Promise.resolve(retailoredResume);
        default:
          return Promise.resolve(null);
      }
    });
    render(<App />);

    const angular = await screen.findByRole('button', {
      name: 'Angular in experience',
    });
    fireEvent.click(angular);
    fireEvent.click(
      screen.getByRole('button', { name: 'Re-run tailoring with 1 keyword' }),
    );

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith(
        'retailor_resume_with_evidence',
        {
          request: {
            capture_id: 42,
            language: 'en',
            source_variant_slug: 'source-role-en',
            selected_terms: ['Angular in experience'],
          },
        },
      ),
    );
    expect(await screen.findByTestId('retailor-score-delta')).toHaveTextContent(
      'Previous 73 → current 81 (+8)',
    );
    expect(
      screen.getByRole('button', { name: 'GCP in experience' }),
    ).toBeVisible();
  });

  it('keeps the prior successful result and pill selection when re-tailoring fails', async () => {
    const sourceResume = completedResume();
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_latest_pipeline_result_any_language':
          return Promise.resolve({
            schema_version: 2,
            capture_received_at_ms: 42,
            language: 'en',
            recovered_from_artifacts: false,
            status: 'completed',
            summary: analysis.summary,
            failed_stage: null,
            error: null,
            analysis,
            resume: sourceResume,
          });
        case 'get_evidence_bank':
          return Promise.resolve({ version: 2, entries: [] });
        case 'retailor_resume_with_evidence':
          return Promise.reject(new Error('Selected claim could not be placed.'));
        default:
          return Promise.resolve(null);
      }
    });
    render(<App />);

    const angular = await screen.findByRole('button', {
      name: 'Angular in experience',
    });
    fireEvent.click(angular);
    fireEvent.click(
      screen.getByRole('button', { name: 'Re-run tailoring with 1 keyword' }),
    );

    expect(await screen.findByText(/Re-tailoring failed:/)).toHaveTextContent(
      'Selected claim could not be placed.',
    );
    expect(screen.getByTestId('run-summary-score')).toHaveTextContent('73');
    expect(
      screen.getByRole('button', { name: 'Angular in experience' }),
    ).toHaveAttribute('aria-pressed', 'true');
  });

  it('rebuilds preflight from a recovered analysis instead of re-analyzing', async () => {
    const fullAnalysis = {
      role_target: 'Backend Engineer',
      seniority: 'Mid-level',
      core_keywords: [],
      required_skills: ['Rust'],
      preferred_skills: [],
      tools_and_platforms: [],
      domain_terms: [],
      responsibility_phrases: [],
      achievement_angles: [],
      ats_phrase_bank: [],
      must_not_claim_without_evidence: [],
      summary: analysis.summary,
    };
    const sourceResume = completedResume();
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_latest_pipeline_result_any_language':
          return Promise.resolve({
            schema_version: 2,
            capture_received_at_ms: 42,
            language: 'en',
            recovered_from_artifacts: false,
            status: 'completed',
            summary: fullAnalysis.summary,
            failed_stage: null,
            error: null,
            analysis: fullAnalysis,
            resume: sourceResume,
          });
        case 'get_latest_pipeline_result':
          return Promise.resolve(null);
        case 'get_evidence_bank':
          return Promise.resolve({ version: 1, entries: [] });
        case 'prepare_evidence_preflight':
          return Promise.resolve({ analysis: fullAnalysis, items: [] });
        default:
          return Promise.resolve(null);
      }
    });

    render(<App />);

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('prepare_evidence_preflight', {
        language: 'en',
        analysis: fullAnalysis,
      }),
    );
    expect(
      mocks.invoke.mock.calls.filter(
        ([command]) => command === 'analyze_latest_job',
      ),
    ).toHaveLength(0);

    fireEvent.click(
      screen.getByRole('button', { name: 'Back to captured job' }),
    );
    // A result already exists, so the primary action is relabelled to say it overwrites it.
    expect(
      await screen.findByRole('button', { name: 'Re-run tailoring' }),
    ).toBeVisible();
  });

  // Stepping back to the job post used to null `result`, which was the only copy of the
  // completion screen in memory: the summary could then only be reached by tailoring again.
  it('reopens the finished result after stepping back to the captured job', async () => {
    const sourceResume = completedResume();
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_latest_pipeline_result_any_language':
          return Promise.resolve({
            schema_version: 2,
            capture_received_at_ms: 42,
            language: 'en',
            recovered_from_artifacts: false,
            status: 'completed',
            summary: analysis.summary,
            failed_stage: null,
            error: null,
            analysis,
            resume: sourceResume,
          });
        case 'get_evidence_bank':
          return Promise.resolve({ version: 1, entries: [] });
        default:
          return Promise.resolve(null);
      }
    });

    render(<App />);

    expect(await screen.findByTestId('completion-result-panel')).toBeVisible();

    fireEvent.click(
      await screen.findByRole('button', { name: 'Back to captured job' }),
    );
    expect(await screen.findByRole('button', { name: 'Analyze job' })).toBeVisible();
    expect(
      screen.queryByTestId('completion-result-panel'),
    ).not.toBeInTheDocument();

    const before = mocks.invoke.mock.calls.length;
    fireEvent.click(screen.getByTestId('back-to-result'));

    expect(await screen.findByTestId('completion-result-panel')).toBeVisible();
    // Nothing was regenerated: no tailoring command ran on the way back.
    const commands = mocks.invoke.mock.calls
      .slice(before)
      .map(([command]) => command);
    expect(commands).not.toContain('generate_tailored_resume');
    expect(commands).not.toContain('retailor_resume_with_evidence');
  });
});

describe('render stability during a run', () => {
  function storedResult(overrides: Record<string, unknown> = {}) {
    return {
      schema_version: 2,
      capture_received_at_ms: 42,
      language: 'en',
      recovered_from_artifacts: false,
      status: 'completed',
      summary: analysis.summary,
      failed_stage: null,
      error: null,
      analysis,
      resume: completedResume(),
      ...overrides,
    };
  }

  async function startTailoring() {
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Analyze job' }));
    fireEvent.click(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    );
    expect(await screen.findByTestId('focused-pipeline')).toBeVisible();
  }

  // The backend emits `resume-pipeline-result` before the command promise resolves.
  // `result` used to outrank `workflowPhase`, so that event swapped the pipeline for the
  // completion screen and a later payload swapped it back.
  it('keeps the pipeline mounted when a result event lands mid-run', async () => {
    await startTailoring();

    await emit('resume-pipeline-result', storedResult());

    await waitFor(() =>
      expect(screen.getByTestId('focused-pipeline')).toBeVisible(),
    );
    expect(screen.queryByTestId('completion-screen')).not.toBeInTheDocument();
  });

  it('does not fall back to the review screen while a run is in flight', async () => {
    await startTailoring();

    // An interim analysis-only snapshot carries no resume and used to null `result`.
    await emit(
      'resume-pipeline-result',
      storedResult({ status: 'analysis_ready', resume: null }),
    );

    await waitFor(() =>
      expect(screen.getByTestId('focused-pipeline')).toBeVisible(),
    );
    expect(
      screen.queryByRole('button', { name: 'Generate tailored PDF' }),
    ).not.toBeInTheDocument();
  });

  it('commits one outcome when the event and the command report the same result', async () => {
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_latest_pipeline_result_any_language':
        case 'get_latest_pipeline_result':
          return Promise.resolve(null);
        case 'get_evidence_bank':
          return Promise.resolve({ version: 1, entries: [] });
        case 'analyze_latest_job':
        case 'prepare_evidence_preflight':
          return Promise.resolve(preflight);
        case 'generate_tailored_resume':
          return Promise.resolve(completedResume());
        default:
          return Promise.resolve(null);
      }
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Analyze job' }));
    const generateButton = await screen.findByRole('button', {
      name: 'Generate tailored PDF',
    });
    const before = mocks.invoke.mock.calls.filter(
      ([command]) => command === 'record_ui_result_state',
    ).length;

    fireEvent.click(generateButton);
    expect(await screen.findByTestId('completion-screen')).toBeVisible();

    const diagnostics = () =>
      mocks.invoke.mock.calls.filter(
        ([command]) => command === 'record_ui_result_state',
      ).length - before;
    // The diagnostic write is scheduled in a requestAnimationFrame.
    await waitFor(() => expect(diagnostics()).toBe(1));

    // The pushed event repeats what the resolved command already delivered.
    await emit('resume-pipeline-result', storedResult());
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });

    expect(diagnostics()).toBe(1);
  });

  it('ignores a repeated capture event for the job already on screen', async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Analyze job' }));
    expect(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    ).toBeVisible();

    // A stale extension service worker can post the same capture to both routes.
    await emit('job-data-received', captured);

    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Generate tailored PDF' }),
      ).toBeVisible(),
    );
  });
});

describe('importing a job by hand', () => {
  it('offers the import panel when there is no capture at all', async () => {
    mocks.invoke.mockImplementation((command: string) =>
      command === 'get_evidence_bank'
        ? Promise.resolve({ version: 1, entries: [] })
        : Promise.resolve(null),
    );
    render(<App />);

    expect(await screen.findByText('Capture a job post to begin')).toBeVisible();
    expect(screen.getByRole('group', { name: 'Import mode' })).toBeVisible();
    expect(screen.getByLabelText('Job post URL')).toBeVisible();
  });

  it('opens the re-import panel unprompted when the capture looks thin', async () => {
    // The failure this feature exists for: the extension shipped an og:description scrape.
    mocks.invoke.mockImplementation((command: string) => {
      if (command === 'get_latest_job') {
        return Promise.resolve({
          received_at_ms: 42,
          payload: {},
          parsed: {
            domain: 'wellfound',
            title: 'Fronted Engineer',
            company: '',
            description: 'Censys is hiring a Fronted Engineer - Apply now!',
            warnings: ['Missing field: company'],
          },
        });
      }
      if (command === 'get_evidence_bank')
        return Promise.resolve({ version: 1, entries: [] });
      return Promise.resolve(null);
    });
    render(<App />);

    const disclosure = await screen.findByText(
      'Capture looks wrong? Import this job another way',
    );
    expect(disclosure.closest('details')).toHaveAttribute('open');
  });

  it('leaves the re-import panel collapsed for a healthy capture', async () => {
    mocks.invoke.mockImplementation((command: string) => {
      if (command === 'get_latest_job') {
        return Promise.resolve({
          received_at_ms: 42,
          payload: {},
          parsed: {
            domain: 'indeed',
            title: 'Backend Engineer',
            company: 'Example Co',
            description: 'A full posting. '.repeat(40),
            warnings: [],
          },
        });
      }
      if (command === 'get_evidence_bank')
        return Promise.resolve({ version: 1, entries: [] });
      return Promise.resolve(null);
    });
    render(<App />);

    const disclosure = await screen.findByText(
      'Capture looks wrong? Import this job another way',
    );
    // It must never compete with the primary Analyze action when nothing is wrong.
    expect(disclosure.closest('details')).not.toHaveAttribute('open');
  });

  it('replaces a bad capture and stops at the review screen', async () => {
    render(<App />);
    expect(
      await screen.findByText('Unique captured job description.'),
    ).toBeVisible();

    fireEvent.change(screen.getByLabelText('Job post URL'), {
      target: { value: 'https://boards.greenhouse.io/acme/jobs/1' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Fetch and import' }));

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('import_job_from_url', {
        url: 'https://boards.greenhouse.io/acme/jobs/1',
      }),
    );

    // The backend drives the swap through the same event an extension capture uses; the panel
    // deliberately does not apply the capture it was handed back.
    await emit('job-data-received', {
      received_at_ms: 99,
      payload: {},
      parsed: {
        domain: 'boards.greenhouse.io',
        title: 'Senior Rust Engineer',
        company: 'Acme',
        description: 'The complete posting, requirements and all.',
      },
    });

    expect(
      await screen.findByText('The complete posting, requirements and all.'),
    ).toBeVisible();
    expect(
      screen.queryByText('Unique captured job description.'),
    ).not.toBeInTheDocument();

    // Stopping at review is the whole point: a bad extraction must not silently spend an
    // analysis call before the user has looked at it.
    expect(screen.getByRole('button', { name: 'Analyze job' })).toBeVisible();
    expect(
      mocks.invoke.mock.calls.filter(
        ([command]) => command === 'analyze_latest_job',
      ),
    ).toHaveLength(0);
  });
});

describe('starting over on a new job', () => {
  function storedFor(captureId: number) {
    return {
      schema_version: 1,
      capture_received_at_ms: captureId,
      language: 'en',
      recovered_from_artifacts: false,
      status: 'completed',
      summary: analysis.summary,
      failed_stage: null,
      error: null,
      analysis,
      resume: completedResume(),
    };
  }

  async function reachCompletion() {
    // The shared mock leaves tailoring pending on purpose; this suite needs it to finish.
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_evidence_bank':
          return Promise.resolve({ version: 1, entries: [] });
        case 'analyze_latest_job':
        case 'prepare_evidence_preflight':
          return Promise.resolve(preflight);
        case 'generate_tailored_resume':
          return Promise.resolve(completedResume());
        default:
          return Promise.resolve(null);
      }
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Analyze job' }));
    fireEvent.click(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    );
    await emit('resume-pipeline-result', storedFor(42));
    return screen.findByTestId('completion-screen');
  }

  // Clearing the capture pointer is what makes the reset survive a restart, and it is
  // also unrecoverable from the UI - so it must not be one click deep.
  it('asks for confirmation before discarding the finished job', async () => {
    expect(await reachCompletion()).toBeVisible();

    fireEvent.click(screen.getByTestId('start-over'));

    expect(screen.getByText('Discard this job?')).toBeVisible();
    expect(
      mocks.invoke.mock.calls.filter(([command]) => command === 'clear_latest_job'),
    ).toHaveLength(0);
    expect(screen.getByTestId('completion-screen')).toBeVisible();
  });

  it('keeps the result when the confirmation is declined', async () => {
    expect(await reachCompletion()).toBeVisible();

    fireEvent.click(screen.getByTestId('start-over'));
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));

    expect(screen.queryByText('Discard this job?')).not.toBeInTheDocument();
    expect(screen.getByTestId('completion-screen')).toBeVisible();
  });

  it('clears the capture and returns to the empty import screen', async () => {
    expect(await reachCompletion()).toBeVisible();

    fireEvent.click(screen.getByTestId('start-over'));
    fireEvent.click(screen.getByTestId('confirm-start-over'));

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('clear_latest_job'),
    );
    await waitFor(() =>
      expect(screen.queryByTestId('completion-screen')).not.toBeInTheDocument(),
    );
    expect(screen.getByText('Capture a job post to begin')).toBeVisible();
    expect(screen.getByTestId('import-job-panel')).toBeVisible();
    // The summary band lives above every screen, so a stale one would survive the reset.
    expect(screen.queryByText(analysis.summary)).not.toBeInTheDocument();
    expect(screen.getByText('Waiting for capture')).toBeVisible();
  });

  it('keeps the job on screen when clearing it fails', async () => {
    expect(await reachCompletion()).toBeVisible();
    mocks.invoke.mockImplementation((command: string) =>
      command === 'clear_latest_job'
        ? Promise.reject({ code: 'Message', message: 'latest.json is read-only.' })
        : Promise.resolve(null),
    );

    fireEvent.click(screen.getByTestId('start-over'));
    fireEvent.click(screen.getByTestId('confirm-start-over'));

    // An AppError is an object, not an Error; it used to reach the user as raw JSON.
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'latest.json is read-only.',
    );
    expect(screen.getByTestId('completion-screen')).toBeVisible();
  });
});

describe('waiting on the analysis layer', () => {
  it('shows analysis progress and elapsed time instead of a bare Working button', async () => {
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_evidence_bank':
          return Promise.resolve({ version: 1, entries: [] });
        // Never resolves: this is the wait the user reported as stuck.
        case 'analyze_latest_job':
          return new Promise(() => undefined);
        default:
          return Promise.resolve(null);
      }
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Analyze job' }));

    await emit('resume-pipeline-progress', {
      stage: 'ats_analysis',
      status: 'started',
      message: 'AI is analyzing ATS keywords, requirements, and role signals.',
      attempt: null,
      total_attempts: null,
    });

    expect(screen.getByText('Analyzing the job post')).toBeVisible();
    expect(screen.getByTestId('pipeline-stage-ats_analysis')).toHaveAttribute(
      'data-status',
      'started',
    );
    // The document stages belong to tailoring; claiming them here would be a lie.
    expect(screen.queryByTestId('pipeline-stage-docx_render')).not.toBeInTheDocument();
    expect(screen.getByTestId('elapsed-time')).toBeVisible();
    expect(screen.getByTestId('cancel-run')).toBeVisible();
  });

  it('cancels a run and ignores the result it eventually reports', async () => {
    mocks.invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_latest_job':
          return Promise.resolve(captured);
        case 'get_evidence_bank':
          return Promise.resolve({ version: 1, entries: [] });
        case 'analyze_latest_job':
          return new Promise(() => undefined);
        default:
          return Promise.resolve(null);
      }
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Analyze job' }));
    await emit('resume-pipeline-progress', {
      stage: 'ats_analysis',
      status: 'started',
      message: 'Working on it.',
      attempt: null,
      total_attempts: null,
    });

    fireEvent.click(screen.getByTestId('cancel-run'));

    expect(await screen.findByRole('button', { name: 'Analyze job' })).toBeEnabled();
    expect(screen.getByRole('alert')).toHaveTextContent('Stopped waiting');

    // The abandoned run finishes anyway. It must not repaint the screen minutes later.
    await emit('resume-pipeline-result', {
      schema_version: 1,
      capture_received_at_ms: 42,
      language: 'en',
      recovered_from_artifacts: false,
      status: 'analysis_ready',
      summary: analysis.summary,
      failed_stage: null,
      error: null,
      analysis,
      resume: null,
    });
    expect(screen.queryByText(analysis.summary)).not.toBeInTheDocument();

    // A deliberate re-run re-opens the door.
    fireEvent.click(screen.getByRole('button', { name: 'Analyze job' }));
    await emit('resume-pipeline-progress', {
      stage: 'ats_analysis',
      status: 'started',
      message: 'Working on it.',
      attempt: null,
      total_attempts: null,
    });
    expect(screen.getByTestId('pipeline-stage-ats_analysis')).toHaveAttribute(
      'data-status',
      'started',
    );
  });
});
