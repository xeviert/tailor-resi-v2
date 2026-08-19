import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  Component,
  type ErrorInfo,
  memo,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { createRoot } from 'react-dom/client';
import { JobPanel } from './job-panel';
import './styles.css';

type Language = 'en' | 'fr';
type BulletKeywordEmphasis = 'low' | 'balanced' | 'high' | 'max';
type CapturedJob = {
  received_at_ms: number;
  payload: unknown;
  parsed: Record<string, unknown>;
};
type BulletRewriteDecision = {
  experience_index: number;
  bullet_index: number;
  outcome: 'rewritten' | 'no_relevant_match' | 'replaced';
  rationale: string;
};
type MissReason = 'no_evidence' | 'evidence_not_placed';
type TermCoverage = {
  term: string;
  kind: string;
  group: string;
  weight: number;
  covered: boolean;
  coverage_ratio: number;
  matched_in?: string | null;
  in_editable_surface: boolean;
  miss_reason?: MissReason | null;
};
type CategoryCoverage = {
  group: string;
  covered: number;
  partial: number;
  total: number;
  covered_weight: number;
  total_weight: number;
};
type AtsCoverage = {
  score: number;
  covered_weight: number;
  total_weight: number;
  editable_covered_weight: number;
  categories: CategoryCoverage[];
  terms: TermCoverage[];
};
type Report = {
  /**
   * The tailoring model's own guess. Kept only so it can be compared against the measured
   * score; `ats_coverage.score` is the number to trust and to show.
   */
  estimated_ats_coverage_score: number;
  ats_coverage?: AtsCoverage | null;
  covered_keywords?: string[];
  omitted_unsupported_keywords: string[];
  safety_notes?: string[];
  bullet_rewrite_decisions?: BulletRewriteDecision[];
};

/**
 * Falls back to the model's estimate for results stored before coverage was measured, so old
 * runs still render a score instead of a blank.
 */
function coverageScore(report: Report | null): number | null {
  if (!report) return null;
  return report.ats_coverage?.score ?? report.estimated_ats_coverage_score;
}

const GROUP_LABELS: Record<string, string> = {
  required: 'Required',
  core: 'Core',
  tools: 'Tools',
  responsibilities: 'Responsibilities',
  preferred: 'Preferred',
  domain: 'Domain',
};
type RetailorMetadata = {
  source_variant_slug: string;
  source_ats_score: number;
  selected_terms: string[];
};
type ContentChange = { path: string; before: string; after: string };
type ArtifactProvenance = {
  variant_slug: string;
  format: 'pdf' | 'docx';
  source_path: string;
  downloads_path: string;
  sha256: string;
  manifest_path: string;
  verification_status: string;
};
type ResumeResult = {
  success: boolean;
  tailoring_status: 'completed' | 'partial' | 'failed';
  variant_slug: string | null;
  validation_status: string;
  fit_status: string;
  page_count: number | null;
  bullet_keyword_emphasis: BulletKeywordEmphasis;
  experience_bullets_changed: number;
  report: Report | null;
  tailored_content: unknown | null;
  content_changes: ContentChange[];
  docx_path: string | null;
  latest_docx_path: string | null;
  pdf_path: string | null;
  latest_pdf_path: string | null;
  downloads_docx_path: string | null;
  downloads_docx_error: string | null;
  downloads_pdf_path: string | null;
  downloads_error: string | null;
  docx_opened: boolean;
  docx_open_error: string | null;
  artifact: ArtifactProvenance | null;
  retailor?: RetailorMetadata | null;
  error: string | null;
};
type KeywordSignal = {
  term: string;
  category: string;
  importance: number;
  evidence: string;
};
type TermVariants = { term: string; variants: string[] };
type JobAnalysis = {
  role_target: string;
  seniority: string;
  core_keywords: KeywordSignal[];
  required_skills: string[];
  preferred_skills: string[];
  tools_and_platforms: string[];
  domain_terms: string[];
  responsibility_phrases: string[];
  achievement_angles: string[];
  ats_phrase_bank: string[];
  must_not_claim_without_evidence: string[];
  term_variants?: TermVariants[];
  summary: string;
};
type Analysis = JobAnalysis | { summary: string };
type ResultSource = 'command' | 'event' | 'recovery';
type RunStatus = 'analysis_ready' | 'completed' | 'partial' | 'failed';
type PipelineResult = {
  analysis: Analysis;
  resume: ResumeResult;
  recovered_from_artifacts?: boolean;
  result_source?: ResultSource;
};
type StoredPipelineResult = {
  schema_version: number;
  capture_received_at_ms: number;
  language: Language;
  recovered_from_artifacts: boolean;
  status?: RunStatus;
  summary?: string;
  failed_stage?: string | null;
  error?: string | null;
  analysis: Analysis | null;
  resume: ResumeResult | null;
  result_source?: ResultSource;
};
type EvidenceKind = 'technology' | 'method_domain' | 'responsibility';
type PreflightItem = {
  term: string;
  kind: EvidenceKind;
  importance: number;
  source: 'base_resume' | 'evidence_bank' | 'needs_approval';
  resolution: 'auto_available' | 'confirmation_required' | 'auto_omitted';
  resolution_reason: string;
  matched_term: string | null;
  proof_note: string | null;
  eligible_for_bullets: boolean;
  allow_model_role_placement: boolean;
};
type PreflightResult = { analysis: Analysis; items: PreflightItem[] };
type EvidenceEntry = {
  term: string;
  kind: EvidenceKind;
  proof_note: string | null;
  user_attested: boolean;
  allow_model_role_placement?: boolean;
};
type EvidenceBank = { version: number; entries: EvidenceEntry[] };
type PipelineProgress = {
  stage: string;
  status: 'started' | 'completed' | 'retrying' | 'failed';
  message: string;
  attempt: number | null;
  total_attempts: number | null;
};
type WorkflowPhase = 'job' | 'tailoring';
type Screen = 'empty' | 'review' | 'pipeline' | 'completion';
const BRIDGE_HEALTH_URL = 'http://127.0.0.1:3000/health';
const PIPELINE_STAGES = [
  ['ats_analysis', 'ATS analysis'],
  ['resume_tailoring', 'Resume tailoring'],
  ['safety_validation', 'Safety validation'],
  ['variant_write', 'Save variant'],
  ['docx_render', 'DOCX render'],
  ['locked_validation', 'Layout validation'],
  ['pdf_fit', 'PDF one-page fit'],
] as const;

const INITIAL_TAILORING_PROGRESS: PipelineProgress[] = [
  {
    stage: 'ats_analysis',
    status: 'completed',
    message: 'Job analysis and evidence review completed.',
    attempt: null,
    total_attempts: null,
  },
  {
    stage: 'resume_tailoring',
    status: 'started',
    message: 'Starting resume tailoring with your reviewed evidence.',
    attempt: null,
    total_attempts: null,
  },
];

class AppErrorBoundary extends Component<
  { children: ReactNode },
  { error: string }
> {
  state = { error: '' };

  static getDerivedStateFromError(reason: unknown) {
    return { error: errorText(reason) };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(
      '[ui-result] React render failure',
      error,
      info.componentStack,
    );
  }

  render() {
    if (this.state.error) {
      return (
        <main className={pageClass}>
          <section className={panelClass} role='alert'>
            <p className={eyebrowClass}>RESULT VIEW ERROR</p>
            <h1 className='mb-2 text-[24px] font-bold'>
              The result could not be rendered
            </h1>
            <p className='mt-0'>{this.state.error}</p>
            <button
              className={primaryButtonClass}
              onClick={() => window.location.reload()}
            >
              Reload and recover result
            </button>
          </section>
        </main>
      );
    }
    return this.props.children;
  }
}

const pageClass =
  'mx-auto max-w-[980px] px-7 pt-[52px] pb-[72px] font-sans text-[#19221d] max-[680px]:px-4 max-[680px]:py-7';
const panelClass =
  'mt-7 rounded-[14px] border border-[#dde3dc] bg-white p-7 shadow-[0_8px_24px_#1f2a2110]';
const compactPanelClass =
  'mt-4 rounded-[14px] border border-[#dde3dc] bg-white px-7 py-6 shadow-[0_8px_24px_#1f2a2110] max-[680px]:px-5 max-[680px]:py-[22px]';
const eyebrowClass =
  'mb-2 text-[11px] font-bold uppercase tracking-[0.12em] text-[#668074]';
const mutedClass = 'm-0 text-[#627067]';
const primaryButtonClass =
  'cursor-pointer rounded-lg border-0 bg-[#176a46] px-4 py-3 font-bold text-white disabled:cursor-wait disabled:opacity-65';
const fieldGroupClass = 'grid w-[232px] gap-1.5 max-[680px]:w-full';
const fieldLabelClass = 'text-[11px] font-bold text-[#526259]';
const segmentedGroupClass =
  'grid grid-cols-2 overflow-hidden rounded-lg border border-[#cbd4cc] bg-white';
const segmentedButtonClass = (active: boolean, first: boolean) =>
  [
    'cursor-pointer border-0 px-3 py-[11px] text-[13px] font-bold tracking-[0.02em] transition-colors',
    'focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-[#176a46]',
    first ? '' : 'border-l border-[#cbd4cc]',
    active
      ? 'bg-[#e7f1ea] text-[#12673d] shadow-[inset_0_0_0_1px_#a9ddba]'
      : 'bg-white text-[#19221d] hover:bg-[#f3f6f3]',
    'disabled:cursor-wait disabled:opacity-65',
  ].join(' ');
const fieldHintClass = 'min-h-[42px] text-[11px] leading-tight text-[#627067]';
const secondaryButtonClass =
  'cursor-pointer rounded-lg border border-[#cbd4cc] bg-white px-[13px] py-[11px] font-bold text-[#19221d]';
const statusBadgeClass =
  'rounded-full border border-[#cbd4cc] px-[11px] py-[7px] text-[13px]';
const readyBadgeClass =
  'rounded-full border border-[#a9ddba] bg-[#e2f4e8] px-[11px] py-[7px] text-[13px] text-[#12673d]';
const progressTextClass: Record<
  PipelineProgress['status'] | 'pending',
  string
> = {
  pending: 'text-[#8a968e]',
  started: 'text-[#176a46]',
  retrying: 'text-[#176a46]',
  completed: 'text-[#385347]',
  failed: 'text-[#a33f22]',
};
const progressMarkerClass: Record<
  PipelineProgress['status'] | 'pending',
  string
> = {
  pending: 'border-[#d8dfda]',
  started: 'border-[#176a46] bg-[#176a46] shadow-[inset_0_0_0_4px_#fff]',
  retrying: 'border-[#176a46] bg-[#176a46] shadow-[inset_0_0_0_4px_#fff]',
  completed: 'border-[#176a46] bg-[#176a46]',
  failed: 'border-[#a33f22] bg-[#a33f22]',
};
const progressDetailClass: Record<PipelineProgress['status'], string> = {
  started: 'bg-[#eef4ef] text-[#385347]',
  completed: 'bg-[#eef4ef] text-[#385347]',
  retrying: 'bg-[#fff6df] text-[#795b13]',
  failed: 'bg-[#fff0eb] text-[#9e411e]',
};

function errorText(reason: unknown) {
  if (typeof reason === 'string') return reason;
  if (reason instanceof Error) return reason.message;
  try {
    return JSON.stringify(reason);
  } catch {
    return 'An unexpected error occurred.';
  }
}

// Accented characters carry the signal for words like "expérience" or "responsabilités";
// matching them as a character class avoids the \b pitfall that accented letters are not
// word characters, so /\bêtre\b/ never matches " être ".
const FRENCH_SIGNALS =
  /[éèêëàâçùûôîïœ]|\b(vous|nous|votre|notre|nos|poste|le|la|les|des|une|pour|avec|dans|est|au|du|sur|et|en|par|qui|que|plus)\b/g;
const ENGLISH_SIGNALS =
  /\b(the|and|you|your|our|we|are|will|with|for|team|experience|requirements|responsibilities|skills)\b/g;

function plainText(html: string) {
  if (!html) return '';
  return (
    new DOMParser().parseFromString(html, 'text/html').body.textContent ?? ''
  );
}

export function detectLanguage(
  job: Record<string, unknown> | undefined,
): Language {
  const read = (key: string) =>
    typeof job?.[key] === 'string' ? (job[key] as string) : '';
  // Job boards disagree on which field carries the posting body: most emit `description`,
  // Welcome to the Jungle emits `description_html`. Sample every candidate so detection
  // never degrades to the title alone, which is often English even on a French post.
  const text = [
    read('title'),
    read('description'),
    plainText(read('description_html')),
    read('qualifications'),
  ]
    .join(' ')
    .toLowerCase();
  const frSignals = (text.match(FRENCH_SIGNALS) ?? []).length;
  const enSignals = (text.match(ENGLISH_SIGNALS) ?? []).length;
  if (frSignals > enSignals) return 'fr';
  if (enSignals > frSignals) return 'en';
  // No decisive signal, usually an empty or unparsed capture. English stays the default.
  return 'en';
}

// Identity of a run's reported state. Two payloads with the same signature describe the
// same outcome even though the event, the command reply and the disk re-read each hand us
// a freshly allocated object.
export function outcomeSignature(candidate: StoredPipelineResult) {
  return JSON.stringify([
    candidate.capture_received_at_ms,
    candidate.language,
    candidate.status ?? null,
    candidate.failed_stage ?? null,
    candidate.error ?? null,
    candidate.summary ?? null,
    candidate.resume?.tailoring_status ?? null,
    candidate.resume?.variant_slug ?? null,
    candidate.resume?.report?.estimated_ats_coverage_score ?? null,
    candidate.resume?.content_changes?.length ?? null,
    candidate.analysis ? Object.keys(candidate.analysis).length : null,
  ]);
}

export function normalizeOutcome(
  candidate: StoredPipelineResult,
): StoredPipelineResult {
  const analysisSummary = candidate.analysis?.summary?.trim();
  const errorMessage = candidate.error?.trim();
  const summary =
    candidate.summary?.trim() ||
    analysisSummary ||
    (errorMessage
      ? `No AI analysis was produced. The run failed: ${errorMessage}`
      : 'This run finished without a usable analysis summary.');
  const status =
    candidate.status ??
    candidate.resume?.tailoring_status ??
    (candidate.analysis ? 'analysis_ready' : 'failed');
  return { ...candidate, status, summary };
}

export function localOutcome({
  captureId,
  language,
  analysis,
  resume = null,
  error = null,
  failedStage = null,
}: {
  captureId: number;
  language: Language;
  analysis: Analysis | null;
  resume?: ResumeResult | null;
  error?: string | null;
  failedStage?: string | null;
}): StoredPipelineResult {
  const status: RunStatus = error
    ? 'failed'
    : (resume?.tailoring_status ?? 'analysis_ready');
  const stage = failedStage?.replace(/_/g, ' ') ?? 'processing';
  const summary = error
    ? analysis
      ? `${analysis.summary} The run then failed during ${stage}: ${error}`
      : `No AI analysis was produced. The run failed during ${stage}: ${error}`
    : (analysis?.summary ?? 'This run finished without an analysis summary.');
  return {
    schema_version: 2,
    capture_received_at_ms: captureId,
    language,
    recovered_from_artifacts: false,
    status,
    summary,
    failed_stage: failedStage,
    error,
    analysis,
    resume,
    result_source: 'command',
  };
}

const ProgressPanel = memo(function ProgressPanel({
  events,
  running,
}: {
  events: PipelineProgress[];
  running: boolean;
}) {
  const latest = events[events.length - 1];
  // One pass over the events instead of a reversed copy per stage; `events` grows for
  // the whole run and this panel re-renders on every progress message.
  const statusByStage = useMemo(() => {
    const byStage = new Map<string, PipelineProgress['status']>();
    for (const event of events) byStage.set(event.stage, event.status);
    return byStage;
  }, [events]);
  return (
    <section className={compactPanelClass} aria-live='polite'>
      <div className='flex items-center justify-between gap-5'>
        <div>
          <p className={eyebrowClass}>PIPELINE ACTIVITY</p>
          <h2 className='m-0 text-[22px] font-bold'>
            {running
              ? 'Building your tailored resume'
              : latest?.status === 'failed'
                ? 'Pipeline stopped'
                : 'Pipeline activity'}
          </h2>
        </div>
        {running && (
          <span
            className='h-[22px] w-[22px] rounded-full border-[3px] border-[#cfe0d4] border-t-[#176a46] animate-[progress-spin_0.8s_linear_infinite]'
            aria-label='Pipeline running'
          />
        )}
      </div>
      <ol className='mt-[22px] mb-4 grid list-none grid-cols-7 gap-2 p-0 max-[820px]:grid-cols-2 max-[820px]:gap-3 max-[480px]:grid-cols-1'>
        {PIPELINE_STAGES.map(([stage, label]) => {
          const status = statusByStage.get(stage) ?? 'pending';
          return (
            <li
              className={`flex items-start gap-[7px] text-xs leading-snug ${progressTextClass[status]}`}
              data-testid={`pipeline-stage-${stage}`}
              data-status={status}
              key={stage}
            >
              <span
                className={`grid h-5 w-5 flex-none place-items-center rounded-full border-2 text-[11px] font-extrabold text-white ${progressMarkerClass[status]}`}
              >
                {status === 'completed' ? 'OK' : status === 'failed' ? '!' : ''}
              </span>
              <span>
                <strong>{label}</strong>
              </span>
            </li>
          );
        })}
      </ol>
      {latest && (
        <p
          className={`m-0 rounded-lg px-[13px] py-[11px] text-[13px] ${progressDetailClass[latest.status]}`}
        >
          {latest.message}
        </p>
      )}
      {events.some(
        (event) =>
          (event.stage === 'resume_tailoring' ||
            event.stage === 'safety_validation') &&
          event.attempt !== null,
      ) && (
        <ol className='mt-3 mb-0 grid list-none gap-2 p-0 text-[12px] leading-snug'>
          {events
            .filter(
              (event) =>
                (event.stage === 'resume_tailoring' ||
                  event.stage === 'safety_validation') &&
                event.attempt !== null,
            )
            .map((event, index) => (
              <li
                className={`rounded-md px-3 py-2 ${progressDetailClass[event.status]}`}
                key={`${event.stage}-${event.attempt}-${event.status}-${index}`}
              >
                <strong>
                  Attempt {event.attempt} of {event.total_attempts ?? 3} ·{' '}
                  {event.stage === 'resume_tailoring'
                    ? 'Resume tailoring'
                    : 'Safety validation'}
                </strong>
                <span> — {event.message}</span>
              </li>
            ))}
        </ol>
      )}
    </section>
  );
});

function childPath(path: string, key: string | number) {
  return `${path}/${String(key).replace(/~/g, '~0').replace(/\//g, '~1')}`;
}

const JsonReviewValue = memo(function JsonReviewValue({
  value,
  path,
  changedPaths,
  depth = 0,
}: {
  value: unknown;
  path: string;
  changedPaths: Set<string>;
  depth?: number;
}) {
  const indent = '  '.repeat(depth);
  const nextIndent = '  '.repeat(depth + 1);
  if (Array.isArray(value))
    return (
      <>
        <span>[</span>
        {value.map((item, index) => (
          <div className='json-line' key={index}>
            {nextIndent}
            <JsonReviewValue
              value={item}
              path={childPath(path, index)}
              changedPaths={changedPaths}
              depth={depth + 1}
            />
            {index < value.length - 1 ? ',' : ''}
          </div>
        ))}
        <div className='json-line'>{indent}]</div>
      </>
    );
  if (value !== null && typeof value === 'object') {
    const entries = Object.entries(value);
    return (
      <>
        <span>{'{'}</span>
        {entries.map(([key, item], index) => (
          <div className='json-line' key={key}>
            {nextIndent}
            <span className='json-key'>{JSON.stringify(key)}</span>:{' '}
            <JsonReviewValue
              value={item}
              path={childPath(path, key)}
              changedPaths={changedPaths}
              depth={depth + 1}
            />
            {index < entries.length - 1 ? ',' : ''}
          </div>
        ))}
        <div className='json-line'>
          {indent}
          {'}'}
        </div>
      </>
    );
  }
  return (
    <span
      className={changedPaths.has(path) ? 'json-value changed' : 'json-value'}
    >
      {JSON.stringify(value)}
    </span>
  );
});

function TailoringChanges({
  content,
  changes,
}: {
  content: unknown;
  changes: ContentChange[];
}) {
  const changedPaths = useMemo(
    () => new Set(changes.map((change) => change.path)),
    [changes],
  );
  return (
    <details className='col-span-full border-t border-[#e7ebe7] pt-[18px]' open>
      <summary className='cursor-pointer text-[#176a46]'>
        <span className='grid gap-[3px]'>
          <b>Tailoring changes</b>
          <small className='text-xs font-normal text-[#627067]'>
            {changes.length} changed value{changes.length === 1 ? '' : 's'}{' '}
            highlighted in the variant JSON
          </small>
        </span>
      </summary>
      <div className='my-4 grid gap-2.5'>
        {changes.length ? (
          changes.map((change) => (
            <article
              className='rounded-[9px] border border-[#d7e4d9] bg-[#f8fcf9] p-3'
              key={change.path}
            >
              <code className='text-xs font-bold text-[#176a46]'>
                {change.path}
              </code>
              <p className='mt-2 mb-0 grid grid-cols-[70px_1fr] gap-2 text-[13px] leading-snug max-[680px]:grid-cols-1'>
                <span className='font-bold text-[#627067]'>Base</span>
                {change.before}
              </p>
              <p className='mt-2 mb-0 grid grid-cols-[70px_1fr] gap-2 text-[13px] leading-snug text-[#176a46] max-[680px]:grid-cols-1'>
                <span className='font-bold text-[#627067]'>Tailored</span>
                {change.after}
              </p>
            </article>
          ))
        ) : (
          <p className={mutedClass}>
            No editable content changed for this job.
          </p>
        )}
      </div>
      <div className='mt-[18px]'>
        <p className={eyebrowClass}>TAILORED VARIANT JSON</p>
        <pre
          className='m-0 max-h-[520px] overflow-auto rounded-[9px] bg-[#18231c] p-4 font-mono text-xs leading-relaxed text-[#e6eee8] whitespace-pre max-[680px]:max-h-[400px] max-[680px]:text-[11px]'
          aria-label='Tailored resume JSON'
        >
          <code>
            <JsonReviewValue
              value={content}
              path=''
              changedPaths={changedPaths}
            />
          </code>
        </pre>
      </div>
    </details>
  );
}

export function RunSummaryPanel({
  outcome,
}: {
  outcome: StoredPipelineResult;
}) {
  const status = outcome.status ?? 'failed';
  const report = outcome.resume?.report ?? null;
  const failed = status === 'failed';
  const heading =
    status === 'analysis_ready'
      ? 'ATS analysis ready'
      : status === 'completed'
        ? 'Analysis and tailored resume ready'
        : status === 'partial'
          ? 'Analysis ready; document output is partial'
          : 'Run analysis and failure report';
  return (
    <section
      data-testid='run-summary'
      aria-live='polite'
      role={failed ? 'alert' : undefined}
      className={`mt-5 grid grid-cols-[1fr_auto] gap-5 rounded-[14px] border p-6 shadow-[0_8px_24px_#1f2a2110] max-[680px]:grid-cols-1 ${
        failed
          ? 'border-[#df9f8b] bg-[#fff8f5]'
          : status === 'partial'
            ? 'border-[#e2c77c] bg-[#fffdf7]'
            : 'border-[#a9ddba] bg-[#f8fcf9]'
      }`}
    >
      <div>
        <p className={eyebrowClass}>ATS ANALYSIS SUMMARY</p>
        <h2 className='mt-0 mb-2 text-[22px] font-bold'>{heading}</h2>
        <p className='m-0' data-testid='run-summary-text'>
          {outcome.summary}
        </p>
        {outcome.failed_stage && (
          <p className='mt-3 mb-0 text-[13px] font-bold text-[#9e411e]'>
            Failed stage: {outcome.failed_stage.replace(/_/g, ' ')}
          </p>
        )}
        {outcome.recovered_from_artifacts && (
          <p className='mt-3 mb-0 text-xs font-bold text-[#176a46]'>
            Restored from the saved artifacts for this job.
          </p>
        )}
        <AnalysisDetail analysis={outcome.analysis} />
      </div>
      {report && (
        <div className='min-w-[190px] max-[680px]:text-left'>
          <div className='text-center max-[680px]:text-left'>
            <strong
              className='block text-4xl text-[#176a46]'
              data-testid='run-summary-score'
            >
              {coverageScore(report)}
            </strong>
            <span className='text-xs text-[#627067]'>
              {report.ats_coverage
                ? 'ATS keyword coverage'
                : 'estimated ATS coverage'}
            </span>
          </div>
          {report.ats_coverage && (
            <CoverageBreakdown coverage={report.ats_coverage} />
          )}
        </div>
      )}
    </section>
  );
}

function isJobAnalysis(analysis: Analysis | null): analysis is JobAnalysis {
  return analysis !== null && 'core_keywords' in analysis;
}

function TermList({ label, terms }: { label: string; terms: string[] }) {
  if (terms.length === 0) return null;
  return (
    <div className='mt-3'>
      <p className='m-0 text-[11px] font-bold uppercase tracking-wide text-[#8a9690]'>
        {label}
      </p>
      <div className='mt-1.5 flex flex-wrap gap-1.5'>
        {terms.map((term) => (
          <span
            className='rounded-full border border-[#dde3dc] bg-[#f7f9f7] px-2.5 py-1 text-[12px] text-[#3d4a41]'
            key={term}
          >
            {term}
          </span>
        ))}
      </div>
    </div>
  );
}

/**
 * The full analysis, behind a disclosure.
 *
 * Everything here was already extracted and used to drive tailoring, but only the one-line
 * summary ever reached the screen. Seeing the actual keyword list is what lets the user judge
 * whether a low score means a bad tailoring pass or a job that simply asks for things they
 * have not done.
 */
function AnalysisDetail({ analysis }: { analysis: Analysis | null }) {
  if (!isJobAnalysis(analysis)) return null;
  const core = [...analysis.core_keywords].sort(
    (left, right) => right.importance - left.importance,
  );
  return (
    <details className='col-span-full mt-4' data-testid='analysis-detail'>
      <summary className='cursor-pointer text-[13px] font-bold text-[#176a46]'>
        What the job post asks for
      </summary>
      <div className='mt-3 rounded-[10px] border border-[#e7ebe7] bg-white p-4'>
        <p className='m-0 text-[13px] text-[#627067]'>
          Target role:{' '}
          <strong className='text-[#3d4a41]'>{analysis.role_target}</strong>
          {analysis.seniority ? ` - ${analysis.seniority}` : ''}
        </p>
        {core.length > 0 && (
          <div className='mt-3'>
            <p className='m-0 text-[11px] font-bold uppercase tracking-wide text-[#8a9690]'>
              Highest-priority signals
            </p>
            <ul className='mt-1.5 mb-0 list-none space-y-1 p-0'>
              {core.map((signal) => (
                <li className='text-[13px] text-[#3d4a41]' key={signal.term}>
                  <strong>{signal.term}</strong>
                  <span className='text-[#8a9690]'>
                    {' '}
                    - priority {signal.importance}/5 - {signal.evidence}
                  </span>
                </li>
              ))}
            </ul>
          </div>
        )}
        <TermList label='Required' terms={analysis.required_skills} />
        <TermList label='Preferred' terms={analysis.preferred_skills} />
        <TermList
          label='Tools and platforms'
          terms={analysis.tools_and_platforms}
        />
        <TermList label='Domain' terms={analysis.domain_terms} />
        <TermList
          label='Responsibilities'
          terms={analysis.responsibility_phrases}
        />
        <TermList
          label='Do not claim without evidence'
          terms={analysis.must_not_claim_without_evidence}
        />
      </div>
    </details>
  );
}

/**
 * Per-group coverage, so the score is readable as a claim about specific keywords rather than
 * an opaque number. Weights differ by group, so the bar tracks weight while the count stays
 * the plain "how many of them" the user actually wants.
 */
function CoverageBreakdown({ coverage }: { coverage: AtsCoverage }) {
  return (
    <div className='mt-4 space-y-1.5' data-testid='coverage-breakdown'>
      {coverage.categories.map((category) => {
        const percent =
          category.total_weight === 0
            ? 0
            : Math.round(
                (category.covered_weight / category.total_weight) * 100,
              );
        return (
          <div key={category.group} className='text-[11px] text-[#627067]'>
            <div className='flex items-baseline justify-between gap-2'>
              <span>{GROUP_LABELS[category.group] ?? category.group}</span>
              <span className='font-bold tabular-nums text-[#3d4a41]'>
                {category.covered}/{category.total}
                {category.partial > 0 && (
                  <span className='font-normal text-[#8a9690]'>
                    {' '}
                    +{category.partial} part
                  </span>
                )}
              </span>
            </div>
            <div
              className='mt-0.5 h-1.5 overflow-hidden rounded-full bg-[#e2ebe4]'
              role='presentation'
            >
              <div
                className='h-full rounded-full bg-[#176a46]'
                style={{ width: `${percent}%` }}
              />
            </div>
          </div>
        );
      })}
      <p
        className='mt-2.5 mb-0 text-[11px] leading-snug text-[#8a9690]'
        data-testid='coverage-model-estimate'
      >
        Measured against the generated resume.
      </p>
    </div>
  );
}

/**
 * Terms the preflight already cleared that still did not reach the document.
 *
 * These are the actionable misses: nothing needs attesting, the tailoring pass simply did not
 * use them. Separating them from unsupported terms keeps the user from being asked to vouch
 * for something they had already vouched for.
 */
function UnplacedTerms({ coverage }: { coverage: AtsCoverage }) {
  const unplaced = coverage.terms.filter(
    (term) => term.miss_reason === 'evidence_not_placed',
  );
  if (unplaced.length === 0) return null;
  return (
    <div
      className='col-span-full border-t border-[#e7ebe7] pt-[18px]'
      data-testid='unplaced-terms'
    >
      <p className='m-0 font-bold text-[#1c4f77]'>
        Supported, but not used in this resume
      </p>
      <p className='mt-1.5 mb-3 max-w-[780px] text-[13px] leading-relaxed text-[#627067]'>
        Your base resume or saved evidence already backs these, so no
        attestation is needed. Re-tailoring, or a higher emphasis level, is what
        gets them placed.
      </p>
      <div className='flex flex-wrap gap-2'>
        {unplaced.map((term) => (
          <span
            className='rounded-full border border-[#b6cfe2] bg-[#f2f8fc] px-3 py-1.5 text-[13px] font-bold text-[#1c4f77]'
            key={term.term}
          >
            {term.term}
            {term.coverage_ratio > 0 && (
              <span className='font-normal text-[#5b7c96]'>
                {' '}
                - {Math.round(term.coverage_ratio * 100)}% present
              </span>
            )}
          </span>
        ))}
      </div>
    </div>
  );
}

export function ResultPanel({
  result,
  action,
  selectedOmittedTerms = new Set<string>(),
  onToggleOmittedTerm,
  onRetailor,
  retailoring = false,
}: {
  result: PipelineResult;
  action: (
    command: string,
    variantSlug?: string,
    format?: 'pdf' | 'docx',
  ) => void;
  selectedOmittedTerms?: Set<string>;
  onToggleOmittedTerm?: (term: string) => void;
  onRetailor?: () => void;
  retailoring?: boolean;
}) {
  const { resume } = result;
  const partial = resume.tailoring_status === 'partial';
  const report = resume.report;
  const contentChanges = Array.isArray(resume.content_changes)
    ? resume.content_changes
    : [];
  const omittedKeywords = Array.isArray(report?.omitted_unsupported_keywords)
    ? report.omitted_unsupported_keywords
    : [];
  const replacedBullets = (report?.bullet_rewrite_decisions ?? [])
    .filter((decision) => decision.outcome === 'replaced')
    .map((decision) => {
      const path = `/experience/${decision.experience_index}/bullets/${decision.bullet_index}`;
      const change = contentChanges.find((entry) => entry.path === path);
      return {
        ...decision,
        before: change?.before ?? '',
        after: change?.after ?? '',
      };
    })
    .filter((entry) => entry.after !== '');
  const hasPdf = resume.artifact
    ? resume.artifact.format === 'pdf'
    : Boolean(resume.latest_pdf_path);
  const hasDocx = resume.artifact
    ? resume.artifact.format === 'docx'
    : Boolean(resume.latest_docx_path);
  const artifactFormat: 'pdf' | 'docx' = hasPdf ? 'pdf' : 'docx';
  const currentScore = coverageScore(report);
  const scoreDelta =
    currentScore !== null && resume.retailor
      ? currentScore - resume.retailor.source_ats_score
      : null;
  const saveMessage = hasPdf
    ? resume.downloads_pdf_path
      ? `Locked sections validated - ${resume.page_count} page - copied to Downloads.`
      : `PDF saved at: ${resume.latest_pdf_path ?? resume.pdf_path}`
    : hasDocx
      ? resume.downloads_docx_path
        ? `Validated DOCX copied to Downloads: ${resume.downloads_docx_path}`
        : `DOCX saved at: ${resume.latest_docx_path ?? resume.docx_path}`
      : 'The ATS summary is available, but no document artifact was produced.';
  return (
    <section
      data-testid='completion-result-panel'
      className={`mt-7 grid grid-cols-[1fr_auto] gap-[18px] rounded-[14px] border bg-white p-7 shadow-[0_8px_24px_#1f2a2110] max-[680px]:grid-cols-1 ${
        partial ? 'border-[#e2c77c]' : 'border-[#dde3dc]'
      }`}
    >
      <div>
        <p className={eyebrowClass}>{partial ? 'DOCX READY' : 'COMPLETE'}</p>
        <h2 className='mb-2 text-[22px] font-bold'>
          {hasPdf
            ? 'One-page PDF is ready'
            : hasDocx
              ? 'Tailoring summary ready; PDF not ready'
              : 'Tailoring summary ready'}
        </h2>
        <p className={mutedClass}>{saveMessage}</p>
        <p className='mt-3.5 mb-0 text-[13px] font-bold capitalize text-[#176a46]'>
          {resume.experience_bullets_changed} experience bullets{' '}
          {resume.bullet_keyword_emphasis === 'high' ||
          resume.bullet_keyword_emphasis === 'max'
            ? 'rewritten before skills'
            : 'tailored'}
          {replacedBullets.length > 0
            ? `, ${replacedBullets.length} replaced outright`
            : ''}{' '}
          - {resume.bullet_keyword_emphasis} emphasis
        </p>
        {resume.retailor && scoreDelta !== null && (
          <p
            className={`mt-3.5 mb-0 text-[13px] font-bold ${
              scoreDelta < 0 ? 'text-[#9e411e]' : 'text-[#176a46]'
            }`}
            data-testid='retailor-score-delta'
          >
            Previous {resume.retailor.source_ats_score} → current {currentScore}{' '}
            ({scoreDelta >= 0 ? '+' : ''}
            {scoreDelta})
            {scoreDelta < 0 ? ' - this variant covers fewer job keywords' : ''}
          </p>
        )}
        {resume.error && (
          <p className='mt-3.5 mb-0 rounded-lg bg-[#fff6df] px-3 py-2.5 text-[13px] text-[#795b13] [overflow-wrap:anywhere]'>
            Output step: {resume.error}
          </p>
        )}
        {partial && resume.docx_opened && (
          <p className='mt-3.5 mb-0 text-[13px] text-[#176a46]'>
            The validated DOCX opened automatically.
          </p>
        )}
        {partial && resume.docx_open_error && (
          <p className='mt-3.5 mb-0 rounded-lg bg-[#fff6df] px-3 py-2.5 text-[13px] text-[#795b13] [overflow-wrap:anywhere]'>
            The DOCX was saved but could not be opened automatically:{' '}
            {resume.docx_open_error}
          </p>
        )}
        {partial && resume.downloads_docx_error && (
          <p className='mt-3.5 mb-0 rounded-lg bg-[#fff6df] px-3 py-2.5 text-[13px] text-[#795b13] [overflow-wrap:anywhere]'>
            The DOCX was saved but could not be copied to Downloads:{' '}
            {resume.downloads_docx_error}
          </p>
        )}
        {resume.downloads_error && (
          <p className='mt-3.5 mb-0 rounded-lg bg-[#fff6df] px-3 py-2.5 text-[13px] text-[#795b13] [overflow-wrap:anywhere]'>
            Your PDF is ready, but it could not be copied to Downloads:{' '}
            {resume.downloads_error}
          </p>
        )}
      </div>
      {(hasPdf || hasDocx) && (
        <div className='col-span-full flex items-center gap-2.5 max-[680px]:flex-wrap'>
          <button
            className={primaryButtonClass}
            onClick={() => {
              if (resume.variant_slug) {
                action(
                  'open_result_artifact',
                  resume.variant_slug,
                  artifactFormat,
                );
              } else {
                action(hasPdf ? 'open_latest_pdf' : 'open_latest_docx');
              }
            }}
          >
            {hasPdf ? 'Open PDF' : 'Open DOCX'}
          </button>
          <button
            className={secondaryButtonClass}
            onClick={() => {
              if (resume.variant_slug) {
                action(
                  'reveal_result_artifact',
                  resume.variant_slug,
                  artifactFormat,
                );
              } else {
                action(hasPdf ? 'reveal_latest_pdf' : 'reveal_latest_docx');
              }
            }}
          >
            Open folder
          </button>
        </div>
      )}
      {replacedBullets.length > 0 && (
        <div
          className='col-span-full border-t border-[#e7ebe7] pt-[18px]'
          data-testid='replaced-bullets'
        >
          <p className='m-0 font-bold text-[#12673d]'>
            Replaced bullets ({replacedBullets.length})
          </p>
          <p className='mt-1.5 mb-3 max-w-[780px] text-[13px] leading-relaxed text-[#627067]'>
            Max emphasis swapped these bullets for new ones aimed at this job.
            Read each one before you send the resume - you have to be able to
            stand behind it in an interview.
          </p>
          <ul className='m-0 grid list-none gap-3 p-0'>
            {replacedBullets.map((entry) => (
              <li
                className='rounded-lg border border-[#dde3dc] bg-[#f8faf8] p-3'
                key={`${entry.experience_index}-${entry.bullet_index}`}
              >
                <p className='m-0 text-[12px] leading-relaxed text-[#8b968d] line-through'>
                  {entry.before}
                </p>
                <p className='mt-1.5 mb-0 text-[13px] leading-relaxed font-bold text-[#19221d]'>
                  {entry.after}
                </p>
                <p className='mt-1.5 mb-0 text-[12px] leading-relaxed text-[#627067]'>
                  {entry.rationale}
                </p>
              </li>
            ))}
          </ul>
        </div>
      )}
      {report?.ats_coverage && <UnplacedTerms coverage={report.ats_coverage} />}
      {omittedKeywords.length > 0 && (
        <div className='col-span-full border-t border-[#e7ebe7] pt-[18px]'>
          <p className='m-0 font-bold text-[#6f5521]'>Still not added</p>
          <p className='mt-1.5 mb-3 max-w-[780px] text-[13px] leading-relaxed text-[#627067]'>
            Select only claims that are true. Your selection authorizes the AI
            to place each claim in the most plausible existing role and replace
            a lower-value bullet while preserving the locked layout.
          </p>
          <div
            className='flex flex-wrap gap-2'
            aria-label='Omitted ATS phrases'
          >
            {omittedKeywords.map((term) => {
              const pressed = selectedOmittedTerms.has(term);
              return (
                <button
                  type='button'
                  className={`cursor-pointer rounded-full border px-3 py-1.5 text-[13px] font-bold transition-colors disabled:cursor-not-allowed disabled:opacity-65 ${
                    pressed
                      ? 'border-[#176a46] bg-[#176a46] text-white'
                      : 'border-[#d6c58d] bg-[#fff9e8] text-[#6f5521] hover:border-[#aa8734]'
                  }`}
                  aria-pressed={pressed}
                  disabled={retailoring || !onToggleOmittedTerm}
                  key={term}
                  onClick={() => onToggleOmittedTerm?.(term)}
                >
                  {term}
                </button>
              );
            })}
          </div>
          {onRetailor && (
            <button
              type='button'
              className={`${primaryButtonClass} mt-4`}
              disabled={retailoring || selectedOmittedTerms.size === 0}
              onClick={onRetailor}
            >
              {retailoring
                ? 'Re-tailoring...'
                : `Re-tailor selected (${selectedOmittedTerms.size})`}
            </button>
          )}
        </div>
      )}
      {omittedKeywords.length === 0 && resume.retailor && (
        <p className='col-span-full m-0 border-t border-[#e7ebe7] pt-[18px] font-bold text-[#176a46]'>
          Every job keyword this resume can truthfully carry is now in the
          document.
        </p>
      )}
      {resume.tailored_content !== null && (
        <div className='contents' data-testid='completion-json-changes'>
          <TailoringChanges
            content={resume.tailored_content}
            changes={contentChanges}
          />
        </div>
      )}
      {resume.tailored_content === null && (
        <p className='col-span-full m-0 border-t border-[#e7ebe7] pt-[18px] text-[#6f5521]'>
          ATS score and JSON changes are unavailable because tailoring did not
          complete.
        </p>
      )}
    </section>
  );
}

function PreflightPanel({
  preflight,
  selected,
  proofs,
  onToggle,
  onProof,
}: {
  preflight: PreflightResult;
  selected: Set<string>;
  proofs: Record<string, string>;
  onToggle: (term: string) => void;
  onProof: (term: string, proof: string) => void;
}) {
  const confirmationItems = preflight.items.filter(
    (item) => item.resolution === 'confirmation_required',
  );
  const availableCount = preflight.items.filter(
    (item) => item.resolution === 'auto_available',
  ).length;
  const omittedCount = preflight.items.filter(
    (item) => item.resolution === 'auto_omitted',
  ).length;
  const groups: [EvidenceKind, string, string][] = [
    [
      'technology',
      'Technologies & tools',
      'Exact tools, platforms, and frameworks.',
    ],
    [
      'method_domain',
      'Methods & domains',
      'Working approaches and domain vocabulary.',
    ],
    [
      'responsibility',
      'Responsibilities',
      'Claims about what you have done; a role/project proof note is required for bullet use.',
    ],
  ];
  return (
    <section className={compactPanelClass}>
      <p className={eyebrowClass}>EVIDENCE PREFLIGHT</p>
      <h2 className='mb-2 text-[22px] font-bold'>
        Confirm only the unresolved claims
      </h2>
      <p className={mutedClass}>
        {availableCount} supported signal
        {availableCount === 1 ? ' is' : 's are'} ready automatically.{' '}
        {omittedCount} lower-value or unsupported signal
        {omittedCount === 1 ? ' was' : 's were'} omitted without interrupting
        you.
      </p>
      {groups.map(([kind, title, hint]) => {
        const items = confirmationItems.filter((item) => item.kind === kind);
        return items.length ? (
          <div
            className='mt-[22px] border-t border-[#e7ebe7] pt-[18px]'
            key={kind}
          >
            <h3 className='mt-0 mb-[3px] text-[15px] font-bold'>{title}</h3>
            <p className='mb-3 text-[13px] text-[#627067]'>{hint}</p>
            {items.map((item) => {
              const checked = selected.has(item.term);
              const proof = proofs[item.term] ?? item.proof_note ?? '';
              return (
                <article
                  className={`mt-[9px] rounded-[10px] border p-[13px] ${
                    item.source === 'evidence_bank'
                      ? 'border-[#a9ddba] bg-[#f8fcf9]'
                      : 'border-[#dde3dc] bg-white'
                  }`}
                  key={item.term}
                >
                  <label className='flex cursor-pointer items-start gap-2.5'>
                    <input
                      className='mt-[3px] h-4 w-4 accent-[#176a46]'
                      type='checkbox'
                      checked={checked}
                      onChange={() => onToggle(item.term)}
                    />
                    <span>
                      <strong className='block'>{item.term}</strong>
                      <small className='mt-[3px] block text-xs leading-snug text-[#627067]'>
                        Needs confirmation - priority {item.importance}/5
                      </small>
                    </span>
                  </label>
                  {checked && (
                    <label className='mt-3 ml-[26px] block max-[680px]:ml-0'>
                      <span className='mb-[5px] block text-xs font-bold text-[#627067]'>
                        Proof note (optional; required for experience bullets)
                      </span>
                      <input
                        className='w-full rounded-[7px] border border-[#cbd4cc] px-2.5 py-[9px] text-[13px] font-[inherit]'
                        value={proof}
                        placeholder='e.g. Used on StealthX RAG platform'
                        onChange={(event) =>
                          onProof(item.term, event.target.value)
                        }
                      />
                    </label>
                  )}
                </article>
              );
            })}
          </div>
        ) : null;
      })}
    </section>
  );
}

function PreflightSummary({
  preflight,
  selected,
  onReopen,
}: {
  preflight: PreflightResult;
  selected: Set<string>;
  onReopen: () => void;
}) {
  const approved = preflight.items.filter(
    (item) => item.source === 'base_resume' || selected.has(item.term),
  ).length;
  const questions = preflight.items.filter(
    (item) => item.resolution === 'confirmation_required',
  ).length;
  const omitted = preflight.items.filter(
    (item) => item.resolution === 'auto_omitted',
  ).length;
  return (
    <section className='mt-4 flex items-center justify-between gap-4 rounded-xl border border-[#a9ddba] bg-[#f8fcf9] px-[18px] py-4 max-[680px]:flex-col max-[680px]:items-start'>
      <div>
        <p className={`${eyebrowClass} mb-1`}>EVIDENCE READY</p>
        <strong>{approved} supported job signals are available</strong>
        <small className='mt-1 block text-xs font-normal text-[#627067]'>
          {questions} clarification{questions === 1 ? '' : 's'} reviewed ·{' '}
          {omitted} low-value or unsupported signal{omitted === 1 ? '' : 's'}{' '}
          omitted
        </small>
      </div>
      <button
        className='cursor-pointer whitespace-nowrap rounded-lg border border-[#a9ddba] bg-white px-[11px] py-[9px] font-bold text-[#176a46]'
        onClick={onReopen}
      >
        Review decisions
      </button>
    </section>
  );
}

function jobText(value: unknown, fallback: string) {
  return typeof value === 'string' && value.trim() ? value.trim() : fallback;
}

function FocusedPipeline({
  job,
  language,
  bulletKeywordEmphasis,
  events,
  running,
  onBack,
}: {
  job: Record<string, unknown>;
  language: Language;
  bulletKeywordEmphasis: BulletKeywordEmphasis;
  events: PipelineProgress[];
  running: boolean;
  onBack: () => void;
}) {
  const latest = events[events.length - 1];
  const stopped = !running && latest?.status === 'failed';
  return (
    <section data-testid='focused-pipeline'>
      <section className={compactPanelClass}>
        <p className={eyebrowClass}>TAILORING RUN</p>
        <h2 className='mt-0 mb-2 text-[22px] font-bold'>
          {jobText(job.title, 'Captured job')}
        </h2>
        <p className={mutedClass}>
          {jobText(job.company, 'Company not provided')} ·{' '}
          {language === 'en' ? 'English' : 'French'} resume ·{' '}
          <span className='capitalize'>{bulletKeywordEmphasis}</span> keyword
          emphasis
        </p>
      </section>
      <ProgressPanel events={events} running={running} />
      {stopped && (
        <div className='mt-4'>
          <button className={secondaryButtonClass} onClick={onBack}>
            Back to evidence review
          </button>
        </div>
      )}
    </section>
  );
}

export function App() {
  const [capture, setCapture] = useState<CapturedJob | null>(null);
  const [language, setLanguage] = useState<Language>('en');
  const [languageChanging, setLanguageChanging] = useState(false);
  const [bulletKeywordEmphasis, setBulletKeywordEmphasis] =
    useState<BulletKeywordEmphasis>('high');
  const [workflowPhase, setWorkflowPhase] = useState<WorkflowPhase>('job');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<PipelineResult | null>(null);
  const [outcome, setOutcome] = useState<StoredPipelineResult | null>(null);
  const [progressEvents, setProgressEvents] = useState<PipelineProgress[]>([]);
  const [preflight, setPreflight] = useState<PreflightResult | null>(null);
  const [preflightCollapsed, setPreflightCollapsed] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [selectedOmittedTerms, setSelectedOmittedTerms] = useState<Set<string>>(
    new Set(),
  );
  const [proofs, setProofs] = useState<Record<string, string>>({});
  const [bank, setBank] = useState<EvidenceBank | null>(null);
  const summaryRef = useRef<HTMLDivElement | null>(null);
  const pipelineRef = useRef<HTMLDivElement | null>(null);
  const captureRef = useRef<CapturedJob | null>(null);
  const languageRef = useRef<Language>('en');
  // Every run gets a generation id. The backend emits `resume-pipeline-result` before the
  // command promise resolves, and recovery re-reads the same snapshot from disk, so results
  // arrive from three sources with no ordering guarantee. Comparing capture id and language
  // alone cannot tell a current result from one belonging to an earlier run of the same job.
  const runIdRef = useRef(0);
  // Mirrors `preflight` for callbacks created in the mount effect, whose closure would
  // otherwise capture the initial `null` forever.
  const preflightRef = useRef<PreflightResult | null>(null);
  const outcomeSignatureRef = useRef<string | null>(null);
  const loadBank = () =>
    invoke<EvidenceBank>('get_evidence_bank')
      .then(setBank)
      .catch((reason) => setError(errorText(reason)));
  function applyPreflight(next: PreflightResult) {
    preflightRef.current = next;
    setPreflight(next);
    setPreflightCollapsed(
      !next.items.some((item) => item.resolution === 'confirmation_required'),
    );
    setSelected(
      new Set(
        next.items
          .filter((item) => item.source === 'evidence_bank')
          .map((item) => item.term),
      ),
    );
    setProofs(
      Object.fromEntries(
        next.items
          .filter((item) => item.proof_note)
          .map((item) => [item.term, item.proof_note ?? '']),
      ),
    );
  }
  function acceptResult(
    candidate: StoredPipelineResult,
    source: ResultSource,
    captureId: number,
    targetLanguage: Language,
    runId: number = runIdRef.current,
  ) {
    if (
      captureRef.current?.received_at_ms !== captureId ||
      languageRef.current !== targetLanguage ||
      runId !== runIdRef.current
    ) {
      console.warn('[ui-result] rejected stale result', {
        source,
        captureId,
        targetLanguage,
        runId,
        currentRunId: runIdRef.current,
      });
      return false;
    }
    const normalized = normalizeOutcome(candidate);
    // The same run reports through the event, the command reply and the disk re-read.
    // Committing each one re-renders the result panels and re-fires the scroll effect,
    // so ignore a payload that says nothing new.
    const signature = outcomeSignature(normalized);
    if (signature === outcomeSignatureRef.current) {
      console.info('[ui-result] skipped duplicate result', {
        source,
        captureId,
      });
      return true;
    }
    console.info('[ui-result] accepted result', {
      source,
      captureId,
      targetLanguage,
      status: normalized.status,
      score: normalized.resume?.report?.estimated_ats_coverage_score,
      changes: normalized.resume?.content_changes?.length ?? 0,
    });
    outcomeSignatureRef.current = signature;
    const accepted = { ...normalized, result_source: source };
    setOutcome(accepted);
    setResult(
      accepted.resume && accepted.resume.tailoring_status !== 'failed'
        ? {
            analysis: accepted.analysis ?? { summary: accepted.summary ?? '' },
            resume: accepted.resume,
            recovered_from_artifacts: accepted.recovered_from_artifacts,
            result_source: source,
          }
        : null,
    );
    setSelectedOmittedTerms(new Set());
    setError('');
    return true;
  }
  function beginRun() {
    outcomeSignatureRef.current = null;
    runIdRef.current += 1;
    return runIdRef.current;
  }
  async function recoverResultFor(
    captured: CapturedJob,
    targetLanguage: Language,
    reason: string,
  ) {
    console.info('[ui-result] recovery requested', {
      reason,
      captureId: captured.received_at_ms,
      language: targetLanguage,
    });
    try {
      const stored = await invoke<StoredPipelineResult | null>(
        'get_latest_pipeline_result',
        {
          language: targetLanguage,
          captureId: captured.received_at_ms,
        },
      );
      if (!stored) {
        console.info('[ui-result] no matching stored result', { reason });
        return null;
      }
      return acceptResult(
        stored,
        'recovery',
        stored.capture_received_at_ms,
        stored.language,
      )
        ? normalizeOutcome(stored)
        : null;
    } catch (reason) {
      console.error('[ui-result] recovery failed', reason);
      setError(`Result recovery failed: ${errorText(reason)}`);
      return null;
    }
  }
  async function recoverLatestResultForCapture(
    captured: CapturedJob,
    reason: string,
  ) {
    console.info('[ui-result] any-language recovery requested', {
      reason,
      captureId: captured.received_at_ms,
    });
    try {
      const stored = await invoke<StoredPipelineResult | null>(
        'get_latest_pipeline_result_any_language',
        { captureId: captured.received_at_ms },
      );
      if (!stored) {
        console.info('[ui-result] no matching result in either language', {
          reason,
        });
        return false;
      }
      // Only let a snapshot re-pin the language when it belongs to the capture on screen;
      // otherwise a stale English result overrides what detectLanguage just worked out.
      if (stored.capture_received_at_ms !== captured.received_at_ms) {
        console.info('[ui-result] ignoring result from a different capture', {
          reason,
          storedCaptureId: stored.capture_received_at_ms,
          captureId: captured.received_at_ms,
        });
        return false;
      }
      languageRef.current = stored.language;
      setLanguage(stored.language);
      const runId = runIdRef.current;
      const accepted = acceptResult(
        stored,
        'recovery',
        stored.capture_received_at_ms,
        stored.language,
        runId,
      );
      if (
        accepted &&
        !preflightRef.current &&
        stored.analysis &&
        'required_skills' in stored.analysis
      ) {
        try {
          const rebuilt = await invoke<PreflightResult>(
            'prepare_evidence_preflight',
            { language: stored.language, analysis: stored.analysis },
          );
          // A run may have started while this was in flight; applyPreflight would reset
          // the evidence the user just confirmed.
          if (runId !== runIdRef.current) {
            console.info('[ui-result] discarded late preflight rebuild', {
              reason,
            });
            return accepted;
          }
          applyPreflight(rebuilt);
        } catch (preflightReason) {
          console.error(
            '[ui-result] preflight rebuild from recovery failed',
            preflightReason,
          );
        }
      }
      return accepted;
    } catch (reason) {
      console.error('[ui-result] any-language recovery failed', reason);
      setError(`Result recovery failed: ${errorText(reason)}`);
      return false;
    }
  }
  const job = capture?.parsed;
  // Single source of truth for what is on screen. `result` used to outrank `workflowPhase`
  // in the render tree, so a result landing mid-run swapped the pipeline for the completion
  // screen and back. An in-flight run now keeps the pipeline mounted until it finishes.
  const screen: Screen = useMemo(() => {
    if (!job) return 'empty';
    if (workflowPhase === 'tailoring' && running) return 'pipeline';
    if (result) return 'completion';
    if (workflowPhase === 'tailoring') return 'pipeline';
    return 'review';
  }, [job, workflowPhase, running, result]);
  useEffect(() => {
    void fetch(BRIDGE_HEALTH_URL)
      .then(async (response) => {
        const health = (await response.json()) as {
          bridge?: string;
          result_protocol_version?: number;
        };
        if (!response.ok || health.bridge !== 'tauri-rust')
          throw new Error(
            'The Rust capture bridge is unavailable. Stop any legacy capture server on port 3000, then restart the desktop app.',
          );
        if (health.result_protocol_version !== 2)
          throw new Error(
            'The desktop UI and Rust backend are out of date with each other. Fully stop and restart ResiTailor before running another analysis.',
          );
      })
      .catch((reason) => setError(errorText(reason)));
    void invoke<CapturedJob | null>('get_latest_job')
      .then((latest) => {
        captureRef.current = latest;
        setCapture(latest);
        if (latest) {
          languageRef.current = detectLanguage(latest.parsed);
          setLanguage(languageRef.current);
          void recoverLatestResultForCapture(latest, 'startup');
        }
      })
      .catch((reason) => setError(errorText(reason)));
    void loadBank();
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    let unlistenResult: (() => void) | undefined;
    void listen<CapturedJob>('job-data-received', (event) => {
      // `/captures` and the legacy `/analyze` route both emit this event, so a stale
      // extension service worker can deliver the same capture twice. Re-running the reset
      // would tear the view down a second time for no new data.
      if (event.payload.received_at_ms === captureRef.current?.received_at_ms) {
        console.info('[ui-result] ignoring duplicate capture event', {
          captureId: event.payload.received_at_ms,
        });
        return;
      }
      beginRun();
      captureRef.current = event.payload;
      setCapture(event.payload);
      languageRef.current = detectLanguage(event.payload.parsed);
      setLanguage(languageRef.current);
      setResult(null);
      setOutcome(null);
      preflightRef.current = null;
      setPreflight(null);
      setPreflightCollapsed(false);
      setSelectedOmittedTerms(new Set());
      setWorkflowPhase('job');
      setProgressEvents([]);
      // A new capture supersedes whatever was running; leaving `running` set would keep
      // the loader and the disabled buttons alive with nothing behind them.
      setRunning(false);
      setError('');
    })
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch((reason) => setError(errorText(reason)));
    void listen<PipelineProgress>('resume-pipeline-progress', (event) => {
      setProgressEvents((current) => [...current, event.payload]);
    })
      .then((cleanup) => {
        unlistenProgress = cleanup;
      })
      .catch((reason) => setError(errorText(reason)));
    void listen<StoredPipelineResult>('resume-pipeline-result', (event) => {
      const stored = event.payload;
      console.info('[ui-result] result event received', {
        captureId: stored.capture_received_at_ms,
        language: stored.language,
        status: stored.status ?? stored.resume?.tailoring_status,
      });
      acceptResult(
        stored,
        'event',
        stored.capture_received_at_ms,
        stored.language,
      );
    })
      .then((cleanup) => {
        unlistenResult = cleanup;
      })
      .catch((reason) => setError(errorText(reason)));
    const onWindowError = (event: ErrorEvent) =>
      console.error('[ui-result] window error', event.error ?? event.message);
    const onUnhandledRejection = (event: PromiseRejectionEvent) =>
      console.error('[ui-result] unhandled rejection', event.reason);
    window.addEventListener('error', onWindowError);
    window.addEventListener('unhandledrejection', onUnhandledRejection);
    return () => {
      unlisten?.();
      unlistenProgress?.();
      unlistenResult?.();
      window.removeEventListener('error', onWindowError);
      window.removeEventListener('unhandledrejection', onUnhandledRejection);
    };
  }, []);
  // One scroll owner. A layout effect used to jump to the pipeline while a separate
  // [outcome] effect smooth-scrolled to the summary on every result write, so the two
  // fought each other several times per run. `outcome` now changes at most once per run
  // because acceptResult drops duplicate payloads.
  useEffect(() => {
    const element =
      screen === 'pipeline' ? pipelineRef.current : summaryRef.current;
    if (!element) return;
    element.scrollIntoView({ behavior: 'auto', block: 'start' });
    element.focus({ preventScroll: true });
  }, [screen, outcome]);
  useEffect(() => {
    if (!outcome) return;
    console.info('[ui-result] outcome committed to React', {
      status: outcome.status,
      score: outcome.resume?.report?.estimated_ats_coverage_score,
      changes: outcome.resume?.content_changes?.length ?? 0,
      recoveredFromArtifacts: outcome.recovered_from_artifacts,
    });
    const frame = window.requestAnimationFrame(() => {
      const element = summaryRef.current;
      const rect = element?.getBoundingClientRect();
      const diagnostic = {
        capture_id: outcome.capture_received_at_ms,
        language: outcome.language,
        source: outcome.result_source ?? 'recovery',
        completion_mounted: Boolean(element),
        completion_visible: Boolean(
          rect && rect.bottom > 0 && rect.top < window.innerHeight,
        ),
        score: outcome.resume?.report?.estimated_ats_coverage_score ?? null,
        change_count: outcome.resume?.content_changes?.length ?? 0,
        viewport_height: window.innerHeight,
        rect_top: rect?.top ?? null,
        rect_bottom: rect?.bottom ?? null,
      };
      console.info('[ui-result] completion screen measured', diagnostic);
      void invoke('record_ui_result_state', { diagnostic }).catch((reason) =>
        console.error('[ui-result] diagnostic write failed', reason),
      );
    });
    return () => window.cancelAnimationFrame(frame);
  }, [outcome]);
  async function analyze() {
    console.info('[ui-result] analysis command started', { language });
    const commandCapture = captureRef.current;
    if (!commandCapture) {
      setError('The captured job is unavailable. Capture the job again.');
      return;
    }
    const targetLanguage = language;
    const runId = beginRun();
    setRunning(true);
    setError('');
    setResult(null);
    setProgressEvents([]);
    try {
      const next = await invoke<PreflightResult>('analyze_latest_job', {
        language,
      });
      console.info('[ui-result] analysis command resolved');
      applyPreflight(next);
      // The resolved command is authoritative and the backend already pushed the same
      // snapshot through `resume-pipeline-result`; re-reading it from disk only produced
      // a third identical commit. Recovery stays on the failure path, where it matters.
      acceptResult(
        localOutcome({
          captureId: commandCapture.received_at_ms,
          language: targetLanguage,
          analysis: next.analysis,
        }),
        'command',
        commandCapture.received_at_ms,
        targetLanguage,
        runId,
      );
    } catch (reason) {
      console.error('[ui-result] analysis command rejected', reason);
      const message = errorText(reason);
      setError(message);
      const recovered = await recoverResultFor(
        commandCapture,
        targetLanguage,
        'analysis-command-rejected',
      );
      if (
        !recovered ||
        recovered.status !== 'failed' ||
        recovered.error?.trim() !== message.trim()
      ) {
        acceptResult(
          localOutcome({
            captureId: commandCapture.received_at_ms,
            language: targetLanguage,
            analysis: null,
            error: message,
            failedStage: 'ats_analysis',
          }),
          'command',
          commandCapture.received_at_ms,
          targetLanguage,
          runId,
        );
      }
    } finally {
      setRunning(false);
    }
  }
  async function generate() {
    if (!preflight) return analyze();
    const commandCaptureId = captureRef.current?.received_at_ms;
    if (commandCaptureId === undefined) {
      setError('The captured job is unavailable. Capture the job again.');
      return;
    }
    const runId = beginRun();
    setWorkflowPhase('tailoring');
    setRunning(true);
    setError('');
    setResult(null);
    setOutcome(null);
    setProgressEvents(INITIAL_TAILORING_PROGRESS);
    const selectedEvidence = preflight.items
      .filter(
        (item) => item.source !== 'base_resume' && selected.has(item.term),
      )
      .map((item) => ({
        term: item.term,
        kind: item.kind,
        proof_note: proofs[item.term]?.trim() || null,
        allow_model_role_placement: item.allow_model_role_placement,
      }));
    console.info('[ui-result] tailoring command started', {
      captureId: captureRef.current?.received_at_ms,
      language,
      bulletKeywordEmphasis,
    });
    try {
      const resume = await invoke<ResumeResult>('generate_tailored_resume', {
        request: {
          language,
          analysis: preflight.analysis,
          selected_evidence: selectedEvidence,
          bullet_keyword_emphasis: bulletKeywordEmphasis,
        },
      });
      console.info('[ui-result] tailoring command resolved', {
        status: resume.tailoring_status,
        score: resume.report?.estimated_ats_coverage_score,
        changes: resume.content_changes.length,
      });
      // Authoritative payload; the pushed event carried the same snapshot. The extra
      // disk re-read that used to follow only added a duplicate commit and delayed
      // `setRunning(false)` by a further IPC round-trip.
      acceptResult(
        localOutcome({
          captureId: commandCaptureId,
          language,
          analysis: preflight.analysis,
          resume,
        }),
        'command',
        commandCaptureId,
        language,
        runId,
      );
      void loadBank();
    } catch (reason) {
      console.error('[ui-result] tailoring command rejected', reason);
      const message = errorText(reason);
      setError(message);
      setProgressEvents((current) =>
        current[current.length - 1]?.status === 'failed'
          ? current
          : [
              ...current,
              {
                stage: 'resume_tailoring',
                status: 'failed',
                message,
                attempt: null,
                total_attempts: null,
              },
            ],
      );
      const currentCapture = captureRef.current;
      if (currentCapture) {
        const recovered = await recoverResultFor(
          currentCapture,
          language,
          'tailoring-command-rejected',
        );
        if (
          !recovered ||
          recovered.status !== 'failed' ||
          recovered.error?.trim() !== message.trim()
        ) {
          acceptResult(
            localOutcome({
              captureId: currentCapture.received_at_ms,
              language,
              analysis: preflight.analysis,
              error: message,
              failedStage: 'resume_tailoring',
            }),
            'command',
            currentCapture.received_at_ms,
            language,
            runId,
          );
        }
      }
    } finally {
      setRunning(false);
    }
  }
  async function retailorSelectedTerms() {
    if (!result || !outcome || selectedOmittedTerms.size === 0) return;
    const sourceResult = result;
    const sourceOutcome = outcome;
    const sourceVariantSlug = sourceResult.resume.variant_slug;
    if (!sourceVariantSlug) {
      setError('The current result has no saved variant to re-tailor.');
      return;
    }
    const selectedTerms = [...selectedOmittedTerms];
    const targetLanguage = sourceOutcome.language;
    const captureId = sourceOutcome.capture_received_at_ms;
    const runId = beginRun();
    setWorkflowPhase('tailoring');
    setRunning(true);
    setError('');
    setResult(null);
    setProgressEvents([
      INITIAL_TAILORING_PROGRESS[0],
      {
        stage: 'resume_tailoring',
        status: 'started',
        message: `Starting re-tailoring with ${selectedTerms.length} selected claim${selectedTerms.length === 1 ? '' : 's'}.`,
        attempt: null,
        total_attempts: null,
      },
    ]);
    try {
      const resume = await invoke<ResumeResult>(
        'retailor_resume_with_evidence',
        {
          request: {
            capture_id: captureId,
            language: targetLanguage,
            source_variant_slug: sourceVariantSlug,
            selected_terms: selectedTerms,
          },
        },
      );
      acceptResult(
        localOutcome({
          captureId,
          language: targetLanguage,
          analysis: sourceResult.analysis,
          resume,
        }),
        'command',
        captureId,
        targetLanguage,
        runId,
      );
      void loadBank();
    } catch (reason) {
      const message = errorText(reason);
      console.error('[ui-result] re-tailoring command rejected', reason);
      setError(`Re-tailoring failed: ${message}`);
      setProgressEvents((current) => [
        ...current,
        {
          stage: 'resume_tailoring',
          status: 'failed',
          message,
          attempt: null,
          total_attempts: null,
        },
      ]);
      outcomeSignatureRef.current = outcomeSignature(sourceOutcome);
      setResult(sourceResult);
      setOutcome(sourceOutcome);
      setWorkflowPhase('job');
    } finally {
      setRunning(false);
    }
  }
  async function action(
    command: string,
    variantSlug?: string,
    format?: 'pdf' | 'docx',
  ) {
    try {
      await invoke(
        command,
        variantSlug && format ? { variantSlug, format } : { language },
      );
    } catch (reason) {
      setError(errorText(reason));
    }
  }
  async function remove(term: string) {
    try {
      setBank(
        await invoke<EvidenceBank>('remove_evidence_bank_entry', { term }),
      );
    } catch (reason) {
      setError(errorText(reason));
    }
  }
  async function changeLanguage(next: Language) {
    if (next === language || languageChanging) return;
    const currentCapture = captureRef.current;
    if (!currentCapture) {
      setError('The captured job is unavailable. Capture the job again.');
      return;
    }
    // Before analysis there is no evidence preflight to rebuild, so the choice is just
    // the language the upcoming run will use.
    if (!preflight) {
      languageRef.current = next;
      setLanguage(next);
      setError('');
      return;
    }
    const previous = language;
    const currentAnalysis = preflight.analysis;
    languageRef.current = next;
    const runId = beginRun();
    setLanguage(next);
    setLanguageChanging(true);
    setError('');
    setResult(null);
    setWorkflowPhase('job');
    setProgressEvents([]);
    try {
      const prepared = await invoke<PreflightResult>(
        'prepare_evidence_preflight',
        { language: next, analysis: currentAnalysis },
      );
      applyPreflight(prepared);
      acceptResult(
        localOutcome({
          captureId: currentCapture.received_at_ms,
          language: next,
          analysis: prepared.analysis,
        }),
        'command',
        currentCapture.received_at_ms,
        next,
        runId,
      );
    } catch (reason) {
      languageRef.current = previous;
      setLanguage(previous);
      acceptResult(
        localOutcome({
          captureId: currentCapture.received_at_ms,
          language: previous,
          analysis: currentAnalysis,
        }),
        'command',
        currentCapture.received_at_ms,
        previous,
        runId,
      );
      setError(`Output language could not be changed: ${errorText(reason)}`);
    } finally {
      setLanguageChanging(false);
    }
  }
  return (
    <main className={pageClass}>
      <header className='flex items-center justify-between gap-6 max-[680px]:flex-col max-[680px]:items-start'>
        <div>
          <p className={eyebrowClass}>LOCAL RESUME TAILORING</p>
          <h1 className='m-0 text-[32px] font-bold'>Resume Workbench</h1>
        </div>
        <span className={capture ? readyBadgeClass : statusBadgeClass}>
          {capture ? 'Job captured' : 'Waiting for capture'}
        </span>
      </header>
      {error && (
        <p
          className='mt-4 mb-0 rounded-lg bg-[#fff3eb] px-3 py-2.5 text-[#9e411e]'
          role='alert'
        >
          {error}
        </p>
      )}
      {outcome && (
        <div
          ref={summaryRef}
          tabIndex={-1}
          className='scroll-mt-4 outline-none'
        >
          <RunSummaryPanel outcome={outcome} />
        </div>
      )}
      {screen === 'completion' && result ? (
        <section data-testid='completion-screen'>
          <ResultPanel
            result={result}
            action={action}
            selectedOmittedTerms={selectedOmittedTerms}
            onToggleOmittedTerm={(term) =>
              setSelectedOmittedTerms((current) => {
                const next = new Set(current);
                next.has(term) ? next.delete(term) : next.add(term);
                return next;
              })
            }
            onRetailor={() => void retailorSelectedTerms()}
            retailoring={running}
          />
          <div className='mt-4 flex flex-wrap gap-2.5'>
            <button
              className={secondaryButtonClass}
              onClick={() => {
                setResult(null);
                setWorkflowPhase('job');
              }}
            >
              Back to captured job
            </button>
          </div>
          {progressEvents.length > 0 && (
            <details className={compactPanelClass}>
              <summary className='cursor-pointer font-bold text-[#176a46]'>
                View completed pipeline activity
              </summary>
              <ProgressPanel events={progressEvents} running={false} />
            </details>
          )}
        </section>
      ) : screen === 'pipeline' && job ? (
        <div
          ref={pipelineRef}
          tabIndex={-1}
          className='scroll-mt-4 outline-none'
        >
          <FocusedPipeline
            job={job}
            language={language}
            bulletKeywordEmphasis={bulletKeywordEmphasis}
            events={progressEvents}
            running={running}
            onBack={() => {
              setWorkflowPhase('job');
              setProgressEvents([]);
            }}
          />
        </div>
      ) : screen === 'empty' || !job ? (
        <section className={`${panelClass} max-w-[650px]`}>
          <h2 className='mb-2 text-[22px] font-bold'>
            Capture a job post to begin
          </h2>
          <p className='mt-0 mb-0'>
            Open a job post in your browser, then choose <b>Extract Job</b> from
            the ResiTailor extension.
          </p>
        </section>
      ) : (
        <>
          <JobPanel job={job} />
          <section
            className={`${panelClass} flex flex-col items-start gap-[18px]`}
          >
            <div>
              <h2 className='mb-2 text-[22px] font-bold'>
                {preflight
                  ? preflight.items.some(
                      (item) => item.resolution === 'confirmation_required',
                    )
                    ? 'Review the remaining questions, then tailor'
                    : 'Evidence resolved — ready to tailor'
                  : 'Analyze job requirements'}
              </h2>
              <p className='mt-0 mb-0'>
                {preflight
                  ? preflight.items.some(
                      (item) => item.resolution === 'confirmation_required',
                    )
                    ? 'Only important claims not found in your resume or saved evidence need confirmation.'
                    : 'Supported evidence was reused automatically; weaker unsupported signals were omitted.'
                  : 'Analyze ATS signals before deciding what belongs in this application.'}
              </p>
            </div>
            <div className='flex flex-wrap items-end gap-3 max-[680px]:w-full'>
              {/* Available before analysis so a wrong auto-detect can be corrected
                  without spending an OpenAI call first. */}
              {job && (
                <div className={fieldGroupClass}>
                  <span className={fieldLabelClass}>
                    Resume output language
                  </span>
                  <div
                    className={segmentedGroupClass}
                    role='group'
                    aria-label='Resume output language'
                  >
                    {(['en', 'fr'] as const).map((option, index) => (
                      <button
                        aria-pressed={language === option}
                        className={segmentedButtonClass(
                          language === option,
                          index === 0,
                        )}
                        disabled={running || languageChanging}
                        key={option}
                        onClick={() => void changeLanguage(option)}
                        type='button'
                      >
                        {option.toUpperCase()}
                      </button>
                    ))}
                  </div>
                  <small className={fieldHintClass}>
                    Used for evidence matching and the tailored resume output.
                  </small>
                </div>
              )}
              {preflight && (
                <div className={fieldGroupClass}>
                  <span className={fieldLabelClass}>
                    Experience keyword emphasis
                  </span>
                  <div
                    className={segmentedGroupClass}
                    role='group'
                    aria-label='Experience keyword emphasis'
                  >
                    {(['high', 'max'] as const).map((level, index) => (
                      <button
                        aria-pressed={bulletKeywordEmphasis === level}
                        className={`${segmentedButtonClass(
                          bulletKeywordEmphasis === level,
                          index === 0,
                        )} capitalize`}
                        disabled={running || languageChanging}
                        key={level}
                        onClick={() => setBulletKeywordEmphasis(level)}
                        type='button'
                      >
                        {level}
                      </button>
                    ))}
                  </div>
                  <small className={fieldHintClass}>
                    High rewrites every bullet with supported job language. Max
                    also swaps 1-3 low-relevance bullets for new ones aimed at
                    this job.
                  </small>
                </div>
              )}
              <button
                className={`${primaryButtonClass} max-[680px]:w-full self-center`}
                disabled={running || languageChanging}
                onClick={preflight ? generate : analyze}
              >
                {languageChanging
                  ? 'Preparing language...'
                  : running
                    ? 'Working...'
                    : preflight
                      ? 'Generate tailored PDF'
                      : 'Analyze job'}
              </button>
            </div>
          </section>
          {preflight &&
            (preflightCollapsed ? (
              <PreflightSummary
                preflight={preflight}
                selected={selected}
                onReopen={() => setPreflightCollapsed(false)}
              />
            ) : (
              <PreflightPanel
                preflight={preflight}
                selected={selected}
                proofs={proofs}
                onToggle={(term) =>
                  setSelected((current) => {
                    const next = new Set(current);
                    next.has(term) ? next.delete(term) : next.add(term);
                    return next;
                  })
                }
                onProof={(term, proof) =>
                  setProofs((current) => ({ ...current, [term]: proof }))
                }
              />
            ))}
          {progressEvents.length > 0 && (
            <ProgressPanel events={progressEvents} running={running} />
          )}
        </>
      )}
      {workflowPhase === 'job' &&
        !result &&
        bank &&
        bank.entries.length > 0 && (
          <details className={compactPanelClass}>
            <summary className='cursor-pointer list-none [&::-webkit-details-marker]:hidden'>
              <span className='flex items-center justify-between gap-4'>
                <span>
                  <span className={`${eyebrowClass} block`}>
                    SAVED EVIDENCE
                  </span>
                  <strong className='text-[22px]'>
                    Your local capability bank
                  </strong>
                  <small className='mt-1 block text-xs font-normal text-[#627067]'>
                    {bank.entries.length} saved capabilities · collapsed by
                    default
                  </small>
                </span>
                <span className='text-sm font-bold text-[#176a46]'>Show</span>
              </span>
            </summary>
            <div className='mt-5 flex flex-wrap gap-2 border-t border-[#e7ebe7] pt-5'>
              {bank.entries.map((entry) => (
                <span
                  className='inline-flex items-center gap-[7px] rounded-full border border-[#d7e4d9] bg-[#eef4ef] py-[5px] pr-[7px] pl-2.5 text-[13px]'
                  key={entry.term}
                >
                  {entry.term}
                  <button
                    className='cursor-pointer border-0 bg-transparent px-0.5 py-0 text-lg leading-none text-[#385347]'
                    aria-label={`Remove ${entry.term}`}
                    onClick={() => remove(entry.term)}
                  >
                    x
                  </button>
                </span>
              ))}
            </div>
          </details>
        )}
    </main>
  );
}

const rootElement = document.getElementById('root');
if (rootElement) {
  createRoot(rootElement).render(
    <AppErrorBoundary>
      <App />
    </AppErrorBoundary>,
  );
}
