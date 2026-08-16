// @vitest-environment jsdom

import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  it('shows output language after analysis and opens an immediate focused pipeline', async () => {
    render(<App />);

    expect(await screen.findByText('Unique captured job description.')).toBeVisible();
    expect(
      screen.queryByRole('group', { name: 'Resume output language' }),
    ).not.toBeInTheDocument();

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
});
