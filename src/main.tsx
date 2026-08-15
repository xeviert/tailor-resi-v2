import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
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
type ContentChange = { path: string; before: string; after: string };
type ResumeResult = {
  success: boolean;
  tailoring_status: 'completed' | 'partial' | 'failed';
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
  error: string | null;
};
type Analysis = { summary: string };
type PipelineResult = { analysis: Analysis; resume: ResumeResult };
type EvidenceKind = 'technology' | 'method_domain' | 'responsibility';
type PreflightItem = {
  term: string;
  kind: EvidenceKind;
  importance: number;
  source: 'base_resume' | 'evidence_bank' | 'needs_approval';
  proof_note: string | null;
};
type PreflightResult = { analysis: Analysis; items: PreflightItem[] };
type EvidenceEntry = {
  term: string;
  kind: EvidenceKind;
  proof_note: string | null;
  user_attested: boolean;
};
type EvidenceBank = { version: number; entries: EvidenceEntry[] };
type PipelineProgress = {
  stage: string;
  status: 'started' | 'completed' | 'retrying' | 'failed';
  message: string;
  attempt: number | null;
  total_attempts: number | null;
};
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

function ResultPanel({
  result,
  action,
}: {
  result: PipelineResult;
  action: (command: string) => void;
}) {
  const { resume } = result;
  const partial = resume.tailoring_status === 'partial';
  const report = resume.report;
  const saveMessage = partial
    ? resume.downloads_docx_path
      ? `Validated DOCX copied to Downloads: ${resume.downloads_docx_path}`
      : `Validated DOCX saved at: ${resume.latest_docx_path}`
    : resume.downloads_pdf_path
      ? `Locked sections validated - ${resume.page_count} page - copied to Downloads.`
      : `Locked sections validated - ${resume.page_count} page - saved as ${resume.latest_pdf_path}`;
  return (
    <section
      className={`mt-7 grid grid-cols-[1fr_auto] gap-[18px] rounded-[14px] border bg-white p-7 shadow-[0_8px_24px_#1f2a2110] max-[680px]:grid-cols-1 ${
        partial ? 'border-[#e2c77c]' : 'border-[#dde3dc]'
      }`}
    >
      <div>
        <p className={eyebrowClass}>{partial ? 'DOCX READY' : 'COMPLETE'}</p>
        <h2 className='mb-2 text-[22px] font-bold'>
          {partial
            ? 'Validated DOCX saved; PDF not ready'
            : 'One-page PDF is ready'}
        </h2>
        <p className='mt-0'>{result.analysis.summary}</p>
        <p className={mutedClass}>{saveMessage}</p>
        <p className='mt-3.5 mb-0 text-[13px] font-bold capitalize text-[#176a46]'>
          {resume.experience_bullets_changed} experience bullets tailored -{' '}
          {resume.bullet_keyword_emphasis} emphasis
        </p>
        {partial && resume.error && (
          <p className='mt-3.5 mb-0 rounded-lg bg-[#fff6df] px-3 py-2.5 text-[13px] text-[#795b13] [overflow-wrap:anywhere]'>
            PDF step: {resume.error}
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
      {report && (
        <div className='min-w-[110px] text-center max-[680px]:text-left'>
          <strong className='block text-4xl text-[#176a46]'>
            {report.estimated_ats_coverage_score}
          </strong>
          <span className='text-xs text-[#627067]'>estimated ATS coverage</span>
        </div>
      )}
      <div className='col-span-full flex items-center gap-2.5 max-[680px]:flex-wrap'>
        <button
          className={primaryButtonClass}
          onClick={() =>
            action(partial ? 'open_latest_docx' : 'open_latest_pdf')
          }
        >
          {partial ? 'Open DOCX' : 'Open PDF'}
        </button>
        <button
          className={secondaryButtonClass}
          onClick={() =>
            action(partial ? 'reveal_latest_docx' : 'reveal_latest_pdf')
          }
        >
          Open folder
        </button>
      </div>
      {report && report.omitted_unsupported_keywords.length > 0 && (
        <p className='col-span-full m-0 text-[#6f5521]'>
          <b>Still not added:</b>{' '}
          {report.omitted_unsupported_keywords.join(', ')}
        </p>
      )}
      {resume.tailored_content !== null && (
        <TailoringChanges
          content={resume.tailored_content}
          changes={resume.content_changes}
        />
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
        Confirm what you can truthfully claim
      </h2>
      <p className={mutedClass}>
        Base-resume matches are already available. Selected evidence is saved to
        your local bank and can be reused on future jobs.
      </p>
      {groups.map(([kind, title, hint]) => {
        const items = preflight.items.filter((item) => item.kind === kind);
        return items.length ? (
          <div
            className='mt-[22px] border-t border-[#e7ebe7] pt-[18px]'
            key={kind}
          >
            <h3 className='mt-0 mb-[3px] text-[15px] font-bold'>{title}</h3>
            <p className='mb-3 text-[13px] text-[#627067]'>{hint}</p>
            {items.map((item) => {
              const isBase = item.source === 'base_resume';
              const checked = isBase || selected.has(item.term);
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
                      disabled={isBase}
                      onChange={() => onToggle(item.term)}
                    />
                    <span>
                      <strong className='block'>{item.term}</strong>
                      <small className='mt-[3px] block text-xs leading-snug text-[#627067]'>
                        {isBase
                          ? 'Already supported by your base resume'
                          : item.source === 'evidence_bank'
                            ? 'Saved in your evidence bank'
                            : 'Needs your approval'}{' '}
                        - priority {item.importance}/5
                      </small>
                    </span>
                  </label>
                  {!isBase && checked && (
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
  return (
    <section className='mt-4 flex items-center justify-between gap-4 rounded-xl border border-[#a9ddba] bg-[#f8fcf9] px-[18px] py-4 max-[680px]:flex-col max-[680px]:items-start'>
      <div>
        <p className={`${eyebrowClass} mb-1`}>EVIDENCE CONFIRMED</p>
        <strong>{approved} job signals are available for tailoring</strong>
      </div>
      <button
        className='cursor-pointer whitespace-nowrap rounded-lg border border-[#a9ddba] bg-white px-[11px] py-[9px] font-bold text-[#176a46]'
        onClick={onReopen}
      >
        Review evidence
      </button>
    </section>
  );
}

function App() {
  const [capture, setCapture] = useState<CapturedJob | null>(null);
  const [language, setLanguage] = useState<Language>('en');
  const [bulletKeywordEmphasis, setBulletKeywordEmphasis] =
    useState<BulletKeywordEmphasis>('balanced');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<PipelineResult | null>(null);
  const [progressEvents, setProgressEvents] = useState<PipelineProgress[]>([]);
  const [preflight, setPreflight] = useState<PreflightResult | null>(null);
  const [preflightCollapsed, setPreflightCollapsed] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [proofs, setProofs] = useState<Record<string, string>>({});
  const [bank, setBank] = useState<EvidenceBank | null>(null);
  const loadBank = () =>
    invoke<EvidenceBank>('get_evidence_bank')
      .then(setBank)
      .catch((reason) => setError(errorText(reason)));
  useEffect(() => {
    void fetch(BRIDGE_HEALTH_URL)
      .then(async (response) => {
        const health = (await response.json()) as { bridge?: string };
        if (!response.ok || health.bridge !== 'tauri-rust')
          throw new Error(
            'The Rust capture bridge is unavailable. Stop any legacy capture server on port 3000, then restart the desktop app.',
          );
      })
      .catch((reason) => setError(errorText(reason)));
    void invoke<CapturedJob | null>('get_latest_job')
      .then(setCapture)
      .catch((reason) => setError(errorText(reason)));
    void loadBank();
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    void listen<CapturedJob>('job-data-received', (event) => {
      setCapture(event.payload);
      setResult(null);
      setPreflight(null);
      setPreflightCollapsed(false);
      setProgressEvents([]);
      setError('');
    })
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch((reason) => setError(errorText(reason)));
    void listen<PipelineProgress>('resume-pipeline-progress', (event) =>
      setProgressEvents((current) => [...current, event.payload]),
    )
      .then((cleanup) => {
        unlistenProgress = cleanup;
      })
      .catch((reason) => setError(errorText(reason)));
    return () => {
      unlisten?.();
      unlistenProgress?.();
    };
  }, []);
  async function analyze() {
    setRunning(true);
    setError('');
    setResult(null);
    setProgressEvents([]);
    try {
      const next = await invoke<PreflightResult>('analyze_latest_job', {
        language,
      });
      setPreflight(next);
      setPreflightCollapsed(false);
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
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setRunning(false);
    }
  }
  async function generate() {
    if (!preflight) return analyze();
    setPreflightCollapsed(true);
    setRunning(true);
    setError('');
    setResult(null);
    setProgressEvents([]);
    const selectedEvidence = preflight.items
      .filter(
        (item) => item.source !== 'base_resume' && selected.has(item.term),
      )
      .map((item) => ({
        term: item.term,
        kind: item.kind,
        proof_note: proofs[item.term]?.trim() || null,
      }));
    try {
      const resume = await invoke<ResumeResult>('generate_tailored_resume', {
        request: {
          language,
          analysis: preflight.analysis,
          selected_evidence: selectedEvidence,
          bullet_keyword_emphasis: bulletKeywordEmphasis,
        },
      });
      setResult({ analysis: preflight.analysis, resume });
      void loadBank();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setRunning(false);
    }
  }
  async function action(command: string) {
    try {
      await invoke(command, { language });
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
  function changeLanguage(next: Language) {
    setLanguage(next);
    setPreflight(null);
    setPreflightCollapsed(false);
    setSelected(new Set());
    setProofs({});
    setResult(null);
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
      {!job ? (
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
                  ? 'Review evidence, then tailor'
                  : 'Analyze job requirements'}
              </h2>
              <p className='mt-0 mb-0'>
                {preflight
                  ? 'Choose the relevant terms you can support before generating.'
                  : 'Analyze ATS signals before deciding what belongs in this application.'}
              </p>
            </div>
            <div className='flex flex-wrap items-start gap-2.5 max-[680px]:w-full'>
              <div
                className='flex overflow-hidden rounded-lg border border-[#cbd4cc] self-center'
                role='group'
                aria-label='Resume language'
              >
                <button
                  className={`cursor-pointer border-0 px-[13px] py-[11px] font-bold ${
                    language === 'en'
                      ? 'bg-[#e7f1ea] text-[#12673d]'
                      : 'bg-white text-[#19221d]'
                  } disabled:cursor-wait disabled:opacity-65`}
                  disabled={running}
                  onClick={() => changeLanguage('en')}
                >
                  EN
                </button>
                <button
                  className={`cursor-pointer border-0 border-l border-[#cbd4cc] px-[13px] py-[11px] font-bold ${
                    language === 'fr'
                      ? 'bg-[#e7f1ea] text-[#12673d]'
                      : 'bg-white text-[#19221d]'
                  } disabled:cursor-wait disabled:opacity-65`}
                  disabled={running}
                  onClick={() => changeLanguage('fr')}
                >
                  FR
                </button>
              </div>
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
                      disabled={running}
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
              <button
                className={primaryButtonClass}
                disabled={running}
                onClick={preflight ? generate : analyze}
              >
                {running
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
      {bank && bank.entries.length > 0 && (
        <section className={compactPanelClass}>
          <p className={eyebrowClass}>SAVED EVIDENCE</p>
          <h2 className='mb-2 text-[22px] font-bold'>
            Your local capability bank
          </h2>
          <div className='flex flex-wrap gap-2'>
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
        </section>
      )}
      {error && (
        <p className='mt-4 mb-0 rounded-lg bg-[#fff3eb] px-3 py-2.5 text-[#9e411e]'>
          {error}
        </p>
      )}
      {result && <ResultPanel result={result} action={action} />}
    </main>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
