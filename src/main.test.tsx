// @vitest-environment jsdom

import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  detectLanguage,
  localOutcome,
  normalizeOutcome,
  outcomeSignature,
  ResultPanel,
  RunSummaryPanel,
} from './main';

afterEach(cleanup);

const analysis = { summary: 'Prioritize reliable backend delivery.' };

function completedResume(status: 'completed' | 'partial' | 'failed' = 'completed') {
  return {
    success: status === 'completed',
    tailoring_status: status,
    variant_slug: status === 'completed' ? 'test-company-role-en' : null,
    validation_status: status === 'completed' ? 'passed' : 'not_run',
    fit_status: status === 'completed' ? 'passed' : 'not_run',
    page_count: status === 'completed' ? 1 : null,
    bullet_keyword_emphasis: 'balanced' as const,
    experience_bullets_changed: 3,
    report: {
      estimated_ats_coverage_score: 84,
      omitted_unsupported_keywords: ['Kubernetes'],
    },
    tailored_content: null,
    content_changes: [],
    docx_path: null,
    latest_docx_path: null,
    pdf_path: null,
    latest_pdf_path: status === 'completed' ? 'resume/generated/resume.pdf' : null,
    downloads_docx_path: null,
    downloads_docx_error: null,
    downloads_pdf_path: null,
    downloads_error: null,
    docx_opened: false,
    docx_open_error: null,
    artifact: null,
    error: status === 'failed' ? 'Rendering failed' : null,
  };
}

describe('always-visible run summary', () => {
  it('renders analysis-only success before tailoring', () => {
    const outcome = localOutcome({ captureId: 1, language: 'en', analysis });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByTestId('run-summary')).toBeVisible();
    expect(screen.getByText('ATS analysis ready')).toBeVisible();
    expect(screen.getByTestId('run-summary-text')).toHaveTextContent(analysis.summary);
  });

  it('renders the AI summary and ATS score after PDF success', () => {
    const outcome = localOutcome({
      captureId: 2,
      language: 'en',
      analysis,
      resume: completedResume(),
    });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByText('Analysis and tailored resume ready')).toBeVisible();
    expect(screen.getByTestId('run-summary-score')).toHaveTextContent('84');
  });

  it('keeps genuine analysis when a downstream stage fails', () => {
    const outcome = localOutcome({
      captureId: 3,
      language: 'fr',
      analysis,
      error: 'LibreOffice failed',
      failedStage: 'docx_render',
    });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByRole('alert')).toBeVisible();
    expect(screen.getByTestId('run-summary-text')).toHaveTextContent(analysis.summary);
    expect(screen.getByTestId('run-summary-text')).toHaveTextContent('LibreOffice failed');
    expect(screen.getByText('Failed stage: docx render')).toBeVisible();
  });

  it('states honestly that no AI analysis exists when analysis fails', () => {
    const outcome = localOutcome({
      captureId: 4,
      language: 'en',
      analysis: null,
      error: 'Request timed out',
      failedStage: 'ats_analysis',
    });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByTestId('run-summary-text')).toHaveTextContent(
      'No AI analysis was produced.',
    );
    expect(screen.getByTestId('run-summary-text')).toHaveTextContent('Request timed out');
  });

  it('normalizes a legacy persisted result for restart recovery', () => {
    const outcome = normalizeOutcome({
      schema_version: 1,
      capture_received_at_ms: 5,
      language: 'en',
      recovered_from_artifacts: true,
      analysis,
      resume: completedResume('partial'),
    });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByText('Analysis ready; document output is partial')).toBeVisible();
    expect(screen.getByText('Restored from the saved artifacts for this job.')).toBeVisible();
  });

  it('opens the exact variant represented by the result instead of a language-level latest file', () => {
    const action = vi.fn();
    const resume = {
      ...completedResume(),
      artifact: {
        variant_slug: 'test-company-role-en',
        format: 'pdf' as const,
        source_path: 'resume/variants/test-company-role-en/Xevier_T_CV_en.pdf',
        downloads_path: 'C:/Users/Test/Downloads/Xevier_T_CV_en.pdf',
        sha256: 'abc123',
        manifest_path: 'resume/variants/test-company-role-en/artifact-manifest.json',
        verification_status: 'verified',
      },
    };
    render(<ResultPanel result={{ analysis, resume }} action={action} />);

    screen.getByRole('button', { name: 'Open PDF' }).click();
    expect(action).toHaveBeenCalledWith(
      'open_result_artifact',
      'test-company-role-en',
      'pdf',
    );
  });

  it('renders omitted phrases as selectable pills and shows the re-tailor score delta', () => {
    const action = vi.fn();
    const onToggle = vi.fn();
    const onRetailor = vi.fn();
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 81,
        omitted_unsupported_keywords: [
          'Angular in experience',
          'GCP in experience',
        ],
      },
      retailor: {
        source_variant_slug: 'source-role-en',
        source_ats_score: 73,
        selected_terms: ['Domain-Driven Design'],
      },
    };
    render(
      <ResultPanel
        result={{ analysis, resume }}
        action={action}
        selectedOmittedTerms={new Set(['Angular in experience'])}
        onToggleOmittedTerm={onToggle}
        onRetailor={onRetailor}
      />,
    );

    const angular = screen.getByRole('button', {
      name: 'Angular in experience',
    });
    expect(angular).toHaveAttribute('aria-pressed', 'true');
    expect(
      screen.getByRole('button', { name: 'GCP in experience' }),
    ).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(angular);
    expect(onToggle).toHaveBeenCalledWith('Angular in experience');
    fireEvent.click(screen.getByRole('button', { name: 'Re-tailor selected (1)' }));
    expect(onRetailor).toHaveBeenCalledOnce();
    expect(screen.getByTestId('retailor-score-delta')).toHaveTextContent(
      'Previous 73 → current 81 (+8)',
    );
  });
});

describe('output language auto-detection', () => {
  // Welcome to the Jungle emits `description_html` and no plain `description`, and its
  // titles are frequently English even on a French posting. Reading only title +
  // description scored 0 French / 0 English and silently fell through to 'en'.
  it('detects French from description_html when there is no plain description', () => {
    expect(
      detectLanguage({
        title: 'Senior Product & Software Engineer',
        description_html:
          '<h3><strong>Notre équipe</strong></h3><p>Plus de 90 ingénieurs issus des meilleures écoles.</p>' +
          '<p>Tu apprendras à concevoir et développer des services pour nos clients.</p>',
        qualifications:
          'Nous recherchons une personne avec au moins 4 ans d’expérience.',
      }),
    ).toBe('fr');
  });

  it('detects French from qualifications alone', () => {
    expect(
      detectLanguage({
        title: 'Software Engineer',
        qualifications:
          'Nous recherchons une personne avec au moins 4 ans d’expérience en tant que développeur.',
      }),
    ).toBe('fr');
  });

  it('keeps English for an English posting', () => {
    expect(
      detectLanguage({
        title: 'Senior Backend Engineer',
        description:
          'We are looking for an engineer with strong experience in Rust. You will join our platform team and own the requirements for our services.',
      }),
    ).toBe('en');
  });

  it('keeps English for an English description_html posting', () => {
    expect(
      detectLanguage({
        title: 'Backend Engineer',
        description_html:
          '<p>You will work with our team on the requirements and responsibilities for the platform.</p>',
      }),
    ).toBe('en');
  });

  it('falls back to English when there is no signal at all', () => {
    expect(detectLanguage(undefined)).toBe('en');
    expect(detectLanguage({})).toBe('en');
    expect(detectLanguage({ title: '' })).toBe('en');
  });
});

describe('outcomeSignature', () => {
  it('matches payloads describing the same run and differs when the run moves on', () => {
    const stored = localOutcome({
      captureId: 1,
      language: 'en',
      analysis,
      resume: completedResume(),
    });
    // The event, the command reply and the disk re-read each allocate a fresh object.
    expect(outcomeSignature({ ...stored })).toBe(outcomeSignature(stored));
    expect(
      outcomeSignature(
        localOutcome({ captureId: 1, language: 'en', analysis }),
      ),
    ).not.toBe(outcomeSignature(stored));
    expect(
      outcomeSignature(
        localOutcome({ captureId: 2, language: 'en', analysis, resume: completedResume() }),
      ),
    ).not.toBe(outcomeSignature(stored));
  });
});
