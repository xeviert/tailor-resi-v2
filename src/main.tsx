import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { JobPanel } from './job-panel';
import './styles.css';

type Language = 'en' | 'fr';
type CapturedJob = { received_at_ms: number; payload: unknown; parsed: Record<string, unknown> };
type Report = { estimated_ats_coverage_score: number; omitted_unsupported_keywords: string[] };
type ResumeResult = {
  success: boolean;
  tailoring_status: 'completed' | 'partial' | 'failed';
  validation_status: string;
  fit_status: string;
  page_count: number | null;
  report: Report | null;
  docx_path: string | null;
  latest_docx_path: string | null;
  pdf_path: string | null;
  latest_pdf_path: string | null;
  error: string | null;
};
type PipelineResult = { analysis: { summary: string }; resume: ResumeResult };
type ProgressStatus = 'started' | 'completed' | 'retrying' | 'failed';
type PipelineProgress = {
  stage: string;
  status: ProgressStatus;
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

function errorText(reason: unknown) {
  if (typeof reason === 'string') return reason;
  if (reason instanceof Error) return reason.message;
  if (reason && typeof reason === 'object') {
    const details = reason as Record<string, unknown>;
    for (const key of ['message', 'error', 'detail']) {
      const value = details[key];
      if (typeof value === 'string' && value.trim()) return value;
    }
    try { return JSON.stringify(reason); } catch { return 'An unexpected error occurred.'; }
  }
  return String(reason || 'An unexpected error occurred.');
}

function ProgressPanel({ events, running }: { events: PipelineProgress[]; running: boolean }) {
  const latest = events[events.length - 1];
  return <section className="pipeline-progress" aria-live="polite">
    <div className="progress-heading"><div><p className="eyebrow">PIPELINE ACTIVITY</p><h2>{running ? 'Building your tailored resume' : latest?.status === 'failed' ? 'Pipeline stopped' : latest?.stage === 'complete' ? 'Pipeline complete' : 'Pipeline activity'}</h2></div>{running && <span className="progress-spinner" aria-label="Pipeline running" />}</div>
    <ol className="progress-steps">
      {PIPELINE_STAGES.map(([stage, label]) => {
        const event = [...events].reverse().find((candidate) => candidate.stage === stage);
        const status = event?.status ?? 'pending';
        const attempt = event?.attempt && event.total_attempts ? ` (${event.attempt}/${event.total_attempts})` : '';
        return <li className={`progress-step ${status}`} key={stage}><span className="step-marker">{status === 'completed' ? '✓' : status === 'failed' ? '!' : ''}</span><span><strong>{label}</strong>{attempt}</span></li>;
      })}
    </ol>
    {latest && <p className={`progress-detail ${latest.status}`}>{latest.message}</p>}
  </section>;
}

function ResultPanel({ result, action }: { result: PipelineResult; action: (command: string) => void }) {
  const { resume } = result;
  const partial = resume.tailoring_status === 'partial';
  const report = resume.report;
  return <section className={`result ${partial ? 'partial-result' : ''}`}>
    <div>
      <p className="eyebrow">{partial ? 'DOCX READY' : 'COMPLETE'}</p>
      <h2>{partial ? 'Validated DOCX saved; PDF not ready' : 'One-page PDF is ready'}</h2>
      <p>{result.analysis.summary}</p>
      <p className="muted">{partial
        ? 'The validated DOCX opened automatically. You can reopen it or show it in its folder below.'
        : `Locked sections validated - ${resume.page_count} page - saved as ${resume.latest_pdf_path}`}</p>
      {partial && resume.error && <p className="partial-error">PDF step: {resume.error}</p>}
    </div>
    {report && <div className="score"><strong>{report.estimated_ats_coverage_score}</strong><span>ATS coverage</span></div>}
    <div className="result-actions">
      <button className="primary" onClick={() => action(partial ? 'open_latest_docx' : 'open_latest_pdf')}>{partial ? 'Open DOCX' : 'Open PDF'}</button>
      <button onClick={() => action(partial ? 'reveal_latest_docx' : 'reveal_latest_pdf')}>Open folder</button>
    </div>
    {report && report.omitted_unsupported_keywords.length > 0 && <p className="unsupported"><b>Not added without evidence:</b> {report.omitted_unsupported_keywords.join(', ')}</p>}
  </section>;
}

function App() {
  const [capture, setCapture] = useState<CapturedJob | null>(null);
  const [language, setLanguage] = useState<Language>('en');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<PipelineResult | null>(null);
  const [progressEvents, setProgressEvents] = useState<PipelineProgress[]>([]);

  useEffect(() => {
    void fetch(BRIDGE_HEALTH_URL).then(async (response) => {
      const health = await response.json() as { bridge?: string };
      if (!response.ok || health.bridge !== 'tauri-rust') {
        throw new Error('The Rust capture bridge is unavailable. Stop any legacy capture server on port 3000, then restart the desktop app.');
      }
    }).catch((reason) => setError(errorText(reason)));
    invoke<CapturedJob | null>('get_latest_job').then(setCapture).catch((reason) => setError(errorText(reason)));
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    void listen<CapturedJob>('job-data-received', (event) => {
      setCapture(event.payload); setResult(null); setProgressEvents([]); setError('');
    }).then((cleanup) => { unlisten = cleanup; }).catch((reason) => setError(errorText(reason)));
    void listen<PipelineProgress>('resume-pipeline-progress', (event) => {
      setProgressEvents((current) => [...current, event.payload]);
    }).then((cleanup) => { unlistenProgress = cleanup; }).catch((reason) => setError(errorText(reason)));
    return () => { unlisten?.(); unlistenProgress?.(); };
  }, []);

  async function generate() {
    setRunning(true); setError(''); setResult(null); setProgressEvents([]);
    try { setResult(await invoke<PipelineResult>('run_resume_pipeline', { language })); }
    catch (reason) { setError(errorText(reason)); }
    finally { setRunning(false); }
  }

  async function action(command: string) {
    try { await invoke(command, { language }); } catch (reason) { setError(errorText(reason)); }
  }

  const job = capture?.parsed;
  return <main>
    <header><div><p className="eyebrow">LOCAL RESUME TAILORING</p><h1>Resume Workbench</h1></div><span className={capture ? 'badge ready' : 'badge'}>{capture ? 'Job captured' : 'Waiting for capture'}</span></header>
    {!job ? <section className="empty"><h2>Capture a job post to begin</h2><p>Open a job post in your browser, then choose <b>Extract Job</b> from the ResiTailor extension. Its details will appear here without running AI or creating files.</p></section> : <>
      <JobPanel job={job} />
      <section className="run-panel"><div><h2>Tailor your resume</h2><p>Analysis, truthful tailoring, layout validation, and one-page PDF export run locally from this action.</p></div><div className="controls"><div className="language" role="group" aria-label="Resume language"><button className={language === 'en' ? 'selected' : ''} onClick={() => setLanguage('en')}>EN</button><button className={language === 'fr' ? 'selected' : ''} onClick={() => setLanguage('fr')}>FR</button></div><button className="primary" disabled={running} onClick={generate}>{running ? 'Generating...' : 'Analyze & Generate PDF'}</button></div></section>
      {(running || progressEvents.length > 0) && <ProgressPanel events={progressEvents} running={running} />}
    </>}
    {error && <p className="error">{error}</p>}
    {result && <ResultPanel result={result} action={action} />}
  </main>;
}

createRoot(document.getElementById('root')!).render(<App />);
