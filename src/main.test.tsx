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
const capturedJob = {
  domain: 'indeed',
  title: 'Backend Engineer',
  company: 'Example Co',
  url: 'https://jobs.example.com/backend-engineer',
};

function completedResume(status: 'completed' | 'partial' | 'failed' = 'completed') {
  return {
    success: status === 'completed',
    tailoring_status: status,
    variant_slug: status === 'completed' ? 'test-company-role-en' : null,
    validation_status: status === 'completed' ? 'passed' : 'not_run',
    fit_status: status === 'completed' ? 'passed' : 'not_run',
    page_count: status === 'completed' ? 1 : null,
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

  // A summary that does not name the posting is unreadable once more than one job has been
  // tailored: nothing in the stored outcome identifies the job, so the capture supplies it.
  it('names the job post and language the result belongs to', () => {
    const outcome = localOutcome({
      captureId: 2,
      language: 'fr',
      analysis,
      resume: completedResume(),
    });
    render(<RunSummaryPanel outcome={outcome} job={capturedJob} />);

    expect(screen.getByRole('heading', { name: 'Backend Engineer' })).toBeVisible();
    expect(screen.getByTestId('run-summary-job')).toHaveTextContent(
      'Example Co · French resume',
    );
    expect(screen.getByRole('link', { name: 'View job post ->' })).toHaveAttribute(
      'href',
      capturedJob.url,
    );
    // The status heading keeps its place below the identity block.
    expect(screen.getByText('Analysis and tailored resume ready')).toBeVisible();
  });

  it('drops the job link when the run has none', () => {
    const outcome = localOutcome({ captureId: 2, language: 'en', analysis });
    render(
      <RunSummaryPanel
        outcome={outcome}
        job={{ title: 'Backend Engineer', company: 'Example Co' }}
      />,
    );

    expect(screen.getByTestId('run-summary-job')).toHaveTextContent(
      'Example Co · English resume',
    );
    expect(screen.queryByRole('link', { name: 'View job post ->' })).not.toBeInTheDocument();
  });

  it('falls back to the status heading when no capture is supplied', () => {
    const outcome = localOutcome({ captureId: 2, language: 'en', analysis });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByRole('heading', { name: 'ATS analysis ready' })).toBeVisible();
    expect(screen.queryByTestId('run-summary-job')).not.toBeInTheDocument();
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

  it('discloses bullets that were replaced outright', () => {
    const resume = {
      ...completedResume(),
      experience_bullets_changed: 3,
      report: {
        estimated_ats_coverage_score: 88,
        omitted_unsupported_keywords: [],
        bullet_rewrite_decisions: [
          {
            experience_index: 0,
            bullet_index: 0,
            outcome: 'rewritten' as const,
            rationale: 'Added supported job language.',
          },
          {
            experience_index: 1,
            bullet_index: 0,
            outcome: 'replaced' as const,
            rationale: 'Targets event-driven architecture, grounded in the Node.js stack.',
          },
        ],
      },
      content_changes: [
        {
          path: '/experience/1/bullets/0',
          before: 'Contributed to backend development with NestJS.',
          after: 'Designed event-driven service boundaries across the NestJS backend.',
        },
      ],
    };

    render(<ResultPanel result={{ analysis, resume }} action={vi.fn()} />);

    const panel = screen.getByTestId('replaced-bullets');
    expect(panel).toBeVisible();
    expect(panel).toHaveTextContent('Replaced bullets (1)');
    expect(panel).toHaveTextContent(
      'Designed event-driven service boundaries across the NestJS backend.',
    );
    expect(panel).toHaveTextContent('Contributed to backend development with NestJS.');
    expect(panel).toHaveTextContent('Targets event-driven architecture');
    expect(screen.getByText(/1 replaced outright/)).toBeVisible();
  });

  it('omits the replaced-bullet block when nothing was replaced', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 84,
        omitted_unsupported_keywords: [],
        bullet_rewrite_decisions: [
          {
            experience_index: 0,
            bullet_index: 0,
            outcome: 'rewritten' as const,
            rationale: 'Added supported job language.',
          },
        ],
      },
    };

    render(<ResultPanel result={{ analysis, resume }} action={vi.fn()} />);

    expect(screen.queryByTestId('replaced-bullets')).toBeNull();
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

describe('measured ATS coverage', () => {
  const coverage = {
    score: 62,
    covered_weight: 13,
    total_weight: 21,
    editable_covered_weight: 8,
    categories: [
      {
        group: 'required',
        covered: 2,
        partial: 1,
        total: 3,
        covered_weight: 10,
        total_weight: 15,
      },
      {
        group: 'domain',
        covered: 1,
        partial: 0,
        total: 2,
        covered_weight: 3,
        total_weight: 6,
      },
    ],
    terms: [
      {
        term: 'Kubernetes',
        kind: 'technology',
        group: 'required',
        weight: 5,
        covered: false,
        coverage_ratio: 0,
        in_editable_surface: false,
        miss_reason: 'no_evidence' as const,
      },
      {
        term: 'GraphQL',
        kind: 'technology',
        group: 'required',
        weight: 5,
        covered: false,
        coverage_ratio: 0.5,
        in_editable_surface: false,
        miss_reason: 'evidence_not_placed' as const,
      },
      {
        term: 'React',
        kind: 'technology',
        group: 'required',
        weight: 5,
        covered: true,
        coverage_ratio: 1,
        matched_in: 'experience.0.bullets.0',
        in_editable_surface: true,
      },
    ],
  };

  it('shows the measured score rather than the model estimate', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: coverage,
        omitted_unsupported_keywords: ['Kubernetes'],
      },
    };
    const outcome = localOutcome({
      captureId: 9,
      language: 'en',
      analysis,
      resume,
    });
    render(<RunSummaryPanel outcome={outcome} />);

    // 91 is the model's own guess; 62 is what the produced document actually covers.
    expect(screen.getByTestId('run-summary-score')).toHaveTextContent('62');
    expect(screen.getByTestId('run-summary-score')).not.toHaveTextContent('91');
    expect(screen.getByText('ATS keyword coverage')).toBeVisible();
  });

  it('shows partial matches beside full ones so a phrase group does not read as a total failure', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: coverage,
        omitted_unsupported_keywords: [],
      },
    };
    const outcome = localOutcome({
      captureId: 14,
      language: 'en',
      analysis,
      resume,
    });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByTestId('coverage-breakdown')).toHaveTextContent(
      '2/3 +1 part',
    );
  });

  it('reports how much of a partly-present term the resume already carries', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: coverage,
        omitted_unsupported_keywords: [],
      },
    };
    render(<ResultPanel result={{ analysis, resume }} action={vi.fn()} />);

    // GraphQL sits at 0.5, so telling the user it is half-present is more useful than
    // listing it as simply absent.
    expect(screen.getByTestId('unplaced-terms')).toHaveTextContent(
      'GraphQL - 50% present',
    );
  });

  it('breaks coverage down by group', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: coverage,
        omitted_unsupported_keywords: [],
      },
    };
    const outcome = localOutcome({
      captureId: 10,
      language: 'en',
      analysis,
      resume,
    });
    render(<RunSummaryPanel outcome={outcome} />);

    const breakdown = screen.getByTestId('coverage-breakdown');
    expect(breakdown).toHaveTextContent('Required');
    expect(breakdown).toHaveTextContent('2/3');
    expect(breakdown).toHaveTextContent('Domain');
    expect(breakdown).toHaveTextContent('1/2');
  });

  it('falls back to the model estimate for a result stored before scoring existed', () => {
    const outcome = localOutcome({
      captureId: 11,
      language: 'en',
      analysis,
      resume: completedResume(),
    });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.getByTestId('run-summary-score')).toHaveTextContent('84');
    expect(screen.getByText('estimated ATS coverage')).toBeVisible();
    expect(screen.queryByTestId('coverage-breakdown')).toBeNull();
  });

  it('separates supported-but-unplaced terms from terms needing attestation', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: coverage,
        omitted_unsupported_keywords: ['Kubernetes'],
      },
    };
    render(<ResultPanel result={{ analysis, resume }} action={vi.fn()} />);

    // GraphQL is already backed by evidence, so it must not be presented as something the
    // user has to vouch for.
    const unplaced = screen.getByTestId('unplaced-terms');
    expect(unplaced).toHaveTextContent('GraphQL');
    expect(unplaced).not.toHaveTextContent('Kubernetes');
    expect(
      screen.getByRole('button', { name: 'Kubernetes' }),
    ).toBeVisible();
  });

  it('omits the unplaced block when every supported term was used', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: { ...coverage, terms: [coverage.terms[2]] },
        omitted_unsupported_keywords: [],
      },
    };
    render(<ResultPanel result={{ analysis, resume }} action={vi.fn()} />);

    expect(screen.queryByTestId('unplaced-terms')).toBeNull();
  });

  it('marks a re-tailor that lost coverage as a regression', () => {
    const resume = {
      ...completedResume(),
      report: {
        estimated_ats_coverage_score: 91,
        ats_coverage: coverage,
        omitted_unsupported_keywords: [],
      },
      retailor: {
        source_variant_slug: 'source-role-en',
        source_ats_score: 70,
        selected_terms: ['GraphQL'],
      },
    };
    render(<ResultPanel result={{ analysis, resume }} action={vi.fn()} />);

    const delta = screen.getByTestId('retailor-score-delta');
    expect(delta).toHaveTextContent('Previous 70 → current 62 (-8)');
    expect(delta).toHaveTextContent('covers fewer job keywords');
    // A loss must not be styled like a win.
    expect(delta.className).toContain('#9e411e');
  });
});

describe('analysis detail', () => {
  const fullAnalysis = {
    role_target: 'Lead Front Engineer',
    seniority: 'Senior',
    core_keywords: [
      {
        term: 'React',
        category: 'technology',
        importance: 5,
        evidence: 'Named in the requirements list',
      },
      {
        term: 'Grafana',
        category: 'technology',
        importance: 2,
        evidence: 'Mentioned once under nice-to-have',
      },
    ],
    required_skills: ['TypeScript'],
    preferred_skills: ['Svelte'],
    tools_and_platforms: ['Docker'],
    domain_terms: ['Fintech'],
    responsibility_phrases: ['Lead a front-end team'],
    achievement_angles: [],
    ats_phrase_bank: [],
    must_not_claim_without_evidence: ['Kubernetes'],
    summary: 'Prioritize front-end leadership.',
  };

  it('surfaces the extracted keywords behind a disclosure', () => {
    const outcome = localOutcome({
      captureId: 12,
      language: 'en',
      analysis: fullAnalysis,
    });
    render(<RunSummaryPanel outcome={outcome} />);

    const detail = screen.getByTestId('analysis-detail');
    expect(detail).toHaveTextContent('Lead Front Engineer');
    expect(detail).toHaveTextContent('React');
    expect(detail).toHaveTextContent('priority 5/5');
    expect(detail).toHaveTextContent('TypeScript');
    expect(detail).toHaveTextContent('Fintech');
    expect(detail).toHaveTextContent('Kubernetes');
  });

  it('is left out when only a summary is available', () => {
    const outcome = localOutcome({ captureId: 13, language: 'en', analysis });
    render(<RunSummaryPanel outcome={outcome} />);

    expect(screen.queryByTestId('analysis-detail')).toBeNull();
  });
});
