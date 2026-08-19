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
    bullet_keyword_emphasis: 'balanced' as const,
    experience_bullets_changed: 3,
    report: {
      estimated_ats_coverage_score: score,
      omitted_unsupported_keywords: omitted,
    },
    tailored_content: { experience: [], skills: {} },
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

  it('defaults to high emphasis and sends max only when the user picks it', async () => {
    render(<App />);
    expect(await screen.findByText('Unique captured job description.')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Analyze job' }));
    expect(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    ).toBeVisible();

    const emphasis = screen.getByRole('group', {
      name: 'Experience keyword emphasis',
    });
    expect(within(emphasis).getByRole('button', { name: 'high' })).toBeVisible();
    fireEvent.click(within(emphasis).getByRole('button', { name: 'max' }));

    fireEvent.click(
      screen.getByRole('button', { name: 'Generate tailored PDF' }),
    );

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('generate_tailored_resume', {
        request: expect.objectContaining({ bullet_keyword_emphasis: 'max' }),
      }),
    );
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
      screen.getByRole('button', { name: 'Re-tailor selected (1)' }),
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
      screen.getByRole('button', { name: 'Re-tailor selected (1)' }),
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
    expect(
      await screen.findByRole('button', { name: 'Generate tailored PDF' }),
    ).toBeVisible();
  });
});

describe('render stability during a run', () => {
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
