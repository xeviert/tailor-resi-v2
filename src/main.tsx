import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  Component,
  type ErrorInfo,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { createRoot } from 'react-dom/client';
import { JobPanel } from './job-panel';
import './styles.css';

type Language = 'en' | 'fr';
type BulletKeywordEmphasis = 'low' | 'balanced' | 'high';
type CapturedJob = {
  received_at_ms: number;
  payload: unknown;
  parsed: Record<string, unknown>;
};
type Report = {
  estimated_ats_coverage_score: number;
  omitted_unsupported_keywords: string[];
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
type Analysis = { summary: string };
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
  resolution:
    | 'auto_available'
    | 'confirmation_required'
    | 'auto_omitted';
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
    console.error('[ui-result] React render failure', error, info.componentStack);
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
  'cursor-pointer rounded-lg border-0 bg-[#176a46] px-4 py-3 font-bold text-white disabled:cursor-wait disabled:opacity-65 self-center';
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

export function normalizeOutcome(candidate: StoredPipelineResult): StoredPipelineResult {
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
    : resume?.tailoring_status ?? 'analysis_ready';
  const stage = failedStage?.replace(/_/g, ' ') ?? 'processing';
  const summary = error
    ? analysis
      ? `${analysis.summary} The run then failed during ${stage}: ${error}`
      : `No AI analysis was produced. The run failed during ${stage}: ${error}`
    : analysis?.summary ?? 'This run finished without an analysis summary.';
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

function ProgressPanel({
  events,
  running,
}: {
  events: PipelineProgress[];
  running: boolean;
}) {
  const latest = events[events.length - 1];
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
          const event = [...events]
            .reverse()
            .find((candidate) => candidate.stage === stage);
          const status = event?.status ?? 'pending';
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
}

function childPath(path: string, key: string | number) {
  return `${path}/${String(key).replace(/~/g, '~0').replace(/\//g, '~1')}`;
}

function JsonReviewValue({
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
}

function TailoringChanges({
  content,
  changes,
}: {
  content: unknown;
  changes: ContentChange[];
}) {
  const changedPaths = new Set(changes.map((change) => change.path));
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

export function RunSummaryPanel({ outcome }: { outcome: StoredPipelineResult }) {
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
      </div>
      {report && (
        <div className='min-w-[110px] text-center max-[680px]:text-left'>
          <strong className='block text-4xl text-[#176a46]' data-testid='run-summary-score'>
            {report.estimated_ats_coverage_score}
          </strong>
          <span className='text-xs text-[#627067]'>estimated ATS coverage</span>
        </div>
      )}
    </section>
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
  action: (command: string, variantSlug?: string, format?: 'pdf' | 'docx') => void;
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
  const hasPdf = resume.artifact
    ? resume.artifact.format === 'pdf'
    : Boolean(resume.latest_pdf_path);
  const hasDocx = resume.artifact
    ? resume.artifact.format === 'docx'
    : Boolean(resume.latest_docx_path);
  const artifactFormat: 'pdf' | 'docx' = hasPdf ? 'pdf' : 'docx';
  const scoreDelta =
    report && resume.retailor
      ? report.estimated_ats_coverage_score - resume.retailor.source_ats_score
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
          {resume.bullet_keyword_emphasis === 'high'
            ? 'rewritten before skills'
            : 'tailored'}{' '}
          - {resume.bullet_keyword_emphasis} emphasis
        </p>
        {report && resume.retailor && scoreDelta !== null && (
          <p
            className='mt-3.5 mb-0 text-[13px] font-bold text-[#176a46]'
            data-testid='retailor-score-delta'
          >
            Previous {resume.retailor.source_ats_score} → current{' '}
            {report.estimated_ats_coverage_score} ({scoreDelta >= 0 ? '+' : ''}
            {scoreDelta})
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
            The DOCX was saved but could not be opened automatically: {resume.docx_open_error}
          </p>
        )}
        {partial && resume.downloads_docx_error && (
          <p className='mt-3.5 mb-0 rounded-lg bg-[#fff6df] px-3 py-2.5 text-[13px] text-[#795b13] [overflow-wrap:anywhere]'>
            The DOCX was saved but could not be copied to Downloads: {resume.downloads_docx_error}
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
                action('open_result_artifact', resume.variant_slug, artifactFormat);
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
                action('reveal_result_artifact', resume.variant_slug, artifactFormat);
              } else {
                action(hasPdf ? 'reveal_latest_pdf' : 'reveal_latest_docx');
              }
            }}
          >
            Open folder
          </button>
        </div>
      )}
      {omittedKeywords.length > 0 && (
        <div className='col-span-full border-t border-[#e7ebe7] pt-[18px]'>
          <p className='m-0 font-bold text-[#6f5521]'>Still not added</p>
          <p className='mt-1.5 mb-3 max-w-[780px] text-[13px] leading-relaxed text-[#627067]'>
            Select only claims that are true. Your selection authorizes the AI
            to place each claim in the most plausible existing role and replace
            a lower-value bullet while preserving the locked layout.
          </p>
          <div className='flex flex-wrap gap-2' aria-label='Omitted ATS phrases'>
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
          All selected claims were added to this variant.
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
        {availableCount} supported signal{availableCount === 1 ? ' is' : 's are'}{' '}
        ready automatically. {omittedCount} lower-value or unsupported signal
        {omittedCount === 1 ? ' was' : 's were'} omitted without interrupting you.
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
          {omitted} low-value or unsupported signal{omitted === 1 ? '' : 's'} omitted
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
    useState<BulletKeywordEmphasis>('balanced');
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
  const loadBank = () =>
    invoke<EvidenceBank>('get_evidence_bank')
      .then(setBank)
      .catch((reason) => setError(errorText(reason)));
  function applyPreflight(next: PreflightResult) {
    setPreflight(next);
    setPreflightCollapsed(
      !next.items.some(
        (item) => item.resolution === 'confirmation_required',
      ),
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
  ) {
    if (
      captureRef.current?.received_at_ms !== captureId ||
      languageRef.current !== targetLanguage
    ) {
      console.warn('[ui-result] rejected stale result', {
        source,
        captureId,
        targetLanguage,
      });
      return false;
    }
    const normalized = normalizeOutcome(candidate);
    console.info('[ui-result] accepted result', {
      source,
      captureId,
      targetLanguage,
      status: normalized.status,
      score: normalized.resume?.report?.estimated_ats_coverage_score,
      changes: normalized.resume?.content_changes?.length ?? 0,
    });
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
      languageRef.current = stored.language;
      setLanguage(stored.language);
      return acceptResult(
        stored,
        'recovery',
        stored.capture_received_at_ms,
        stored.language,
      );
    } catch (reason) {
      console.error('[ui-result] any-language recovery failed', reason);
      setError(`Result recovery failed: ${errorText(reason)}`);
      return false;
    }
  }
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
        if (latest) void recoverLatestResultForCapture(latest, 'startup');
      })
      .catch((reason) => setError(errorText(reason)));
    void loadBank();
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    let unlistenResult: (() => void) | undefined;
    void listen<CapturedJob>('job-data-received', (event) => {
      captureRef.current = event.payload;
      setCapture(event.payload);
      setResult(null);
      setOutcome(null);
      setPreflight(null);
      setPreflightCollapsed(false);
      setSelectedOmittedTerms(new Set());
      setWorkflowPhase('job');
      setProgressEvents([]);
      setError('');
    })
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch((reason) => setError(errorText(reason)));
    void listen<PipelineProgress>('resume-pipeline-progress', (event) => {
      setProgressEvents((current) => [...current, event.payload]);
      if (event.payload.stage === 'complete') {
        const currentCapture = captureRef.current;
        if (currentCapture) {
          void recoverResultFor(
            currentCapture,
            languageRef.current,
            'pipeline-complete-event',
          );
        }
      }
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
  useLayoutEffect(() => {
    if (workflowPhase !== 'tailoring') return;
    const element = pipelineRef.current;
    element?.scrollIntoView({ behavior: 'auto', block: 'start' });
    element?.focus({ preventScroll: true });
  }, [workflowPhase]);
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
      element?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      element?.focus({ preventScroll: true });
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
      acceptResult(
        localOutcome({
          captureId: commandCapture.received_at_ms,
          language: targetLanguage,
          analysis: next.analysis,
        }),
        'command',
        commandCapture.received_at_ms,
        targetLanguage,
      );
      await recoverResultFor(commandCapture, targetLanguage, 'analysis-command-resolved');
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
    setWorkflowPhase('tailoring');
    setRunning(true);
    setError('');
    setResult(null);
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
      );
      const currentCapture = captureRef.current;
      if (currentCapture) {
        await recoverResultFor(currentCapture, language, 'tailoring-command-resolved');
      }
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
      const resume = await invoke<ResumeResult>('retailor_resume_with_evidence', {
        request: {
          capture_id: captureId,
          language: targetLanguage,
          source_variant_slug: sourceVariantSlug,
          selected_terms: selectedTerms,
        },
      });
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
      );
      const currentCapture = captureRef.current;
      if (currentCapture) {
        await recoverResultFor(
          currentCapture,
          targetLanguage,
          'retailoring-command-resolved',
        );
      }
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
    if (next === language || languageChanging || !preflight) return;
    const previous = language;
    const currentAnalysis = preflight.analysis;
    const currentCapture = captureRef.current;
    if (!currentCapture) {
      setError('The captured job is unavailable. Capture the job again.');
      return;
    }
    languageRef.current = next;
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
      );
      setError(`Output language could not be changed: ${errorText(reason)}`);
    } finally {
      setLanguageChanging(false);
    }
  }
  const job = capture?.parsed;
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
        <p className='mt-4 mb-0 rounded-lg bg-[#fff3eb] px-3 py-2.5 text-[#9e411e]' role='alert'>
          {error}
        </p>
      )}
      {outcome && (
        <div ref={summaryRef} tabIndex={-1} className='scroll-mt-4 outline-none'>
          <RunSummaryPanel outcome={outcome} />
        </div>
      )}
      {result ? (
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
      ) : workflowPhase === 'tailoring' && job ? (
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
      ) : !job ? (
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
            <div className='flex flex-wrap items-start gap-2.5 max-[680px]:w-full'>
              {preflight && (
                <div className='grid gap-1 pt-2'>
                  <span className='text-[11px] font-bold text-[#526259]'>
                    Resume output language
                  </span>
                  <div
                    className='flex overflow-hidden rounded-lg border border-[#cbd4cc] self-center'
                    role='group'
                    aria-label='Resume output language'
                  >
                    <button
                      className={`cursor-pointer border-0 px-[13px] py-[11px] font-bold ${
                        language === 'en'
                          ? 'bg-[#e7f1ea] text-[#12673d]'
                          : 'bg-white text-[#19221d]'
                      } disabled:cursor-wait disabled:opacity-65`}
                      disabled={running || languageChanging}
                      onClick={() => void changeLanguage('en')}
                    >
                      EN
                    </button>
                    <button
                      className={`cursor-pointer border-0 border-l border-[#cbd4cc] px-[13px] py-[11px] font-bold ${
                        language === 'fr'
                          ? 'bg-[#e7f1ea] text-[#12673d]'
                          : 'bg-white text-[#19221d]'
                      } disabled:cursor-wait disabled:opacity-65`}
                      disabled={running || languageChanging}
                      onClick={() => void changeLanguage('fr')}
                    >
                      FR
                    </button>
                  </div>
                  <small className='max-w-[245px] text-[11px] leading-tight text-[#627067]'>
                    Used for evidence matching and the tailored resume output.
                  </small>
                </div>
              )}
              {preflight && (
                <div className='grid gap-1 pt-2'>
                  <span className='text-[11px] font-bold text-[#526259]'>
                    Experience keyword emphasis
                  </span>
                  <div
                    className='flex overflow-hidden rounded-lg border border-[#cbd4cc] justify-evenly'
                    role='group'
                    aria-label='Experience keyword emphasis'
                  >
                    {(['low', 'balanced', 'high'] as const).map((level) => (
                      <button
                        className={`cursor-pointer border-0 px-[13px] py-[11px] font-bold capitalize ${
                          level === 'low' ? '' : 'border-l border-[#cbd4cc]'
                        } ${
                          bulletKeywordEmphasis === level
                            ? 'bg-[#e7f1ea] text-[#12673d]'
                            : 'bg-white text-[#19221d]'
                        } disabled:cursor-wait disabled:opacity-65`}
                        disabled={running || languageChanging}
                        key={level}
                        onClick={() => setBulletKeywordEmphasis(level)}
                      >
                        {level}
                      </button>
                    ))}
                  </div>
                  <small className='max-w-[245px] text-[11px] leading-tight text-[#627067]'>
                    Higher levels spread supported job language across more
                    relevant bullets.
                  </small>
                </div>
              )}
              <button
                className={primaryButtonClass}
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
          {(running || progressEvents.length > 0) && (
            <ProgressPanel events={progressEvents} running={running} />
          )}
        </>
      )}
      {workflowPhase === 'job' && !result && bank && bank.entries.length > 0 && (
        <details className={compactPanelClass}>
          <summary className='cursor-pointer list-none [&::-webkit-details-marker]:hidden'>
            <span className='flex items-center justify-between gap-4'>
              <span>
                <span className={`${eyebrowClass} block`}>SAVED EVIDENCE</span>
                <strong className='text-[22px]'>Your local capability bank</strong>
                <small className='mt-1 block text-xs font-normal text-[#627067]'>
                  {bank.entries.length} saved capabilities · collapsed by default
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
