import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { JobPanel } from './job-panel';
import './styles.css';

type Language = 'en' | 'fr';
type CapturedJob = { received_at_ms: number; payload: unknown; parsed: Record<string, unknown> };
type Report = { estimated_ats_coverage_score: number; omitted_unsupported_keywords: string[] };
type PipelineResult = { analysis: { summary: string }; resume: { page_count: number; report: Report; latest_pdf_path: string } };
const BRIDGE_HEALTH_URL = 'http://127.0.0.1:3000/health';

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

function App() {
  const [capture, setCapture] = useState<CapturedJob | null>(null);
  const [language, setLanguage] = useState<Language>('en');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<PipelineResult | null>(null);

  useEffect(() => {
    void fetch(BRIDGE_HEALTH_URL).then(async (response) => {
      const health = await response.json() as { bridge?: string };
      if (!response.ok || health.bridge !== 'tauri-rust') {
        throw new Error('The Rust capture bridge is unavailable. Stop any legacy capture server on port 3000, then restart the desktop app.');
      }
    }).catch((reason) => setError(errorText(reason)));
    invoke<CapturedJob | null>('get_latest_job').then(setCapture).catch((reason) => setError(errorText(reason)));
    let unlisten: (() => void) | undefined;
    void listen<CapturedJob>('job-data-received', (event) => {
      setCapture(event.payload); setResult(null); setError('');
    }).then((cleanup) => { unlisten = cleanup; }).catch((reason) => setError(errorText(reason)));
    return () => unlisten?.();
  }, []);

  async function generate() {
    setRunning(true); setError(''); setResult(null);
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
    </>}
    {error && <p className="error">{error}</p>}
    {result && <section className="result"><div><p className="eyebrow">COMPLETE</p><h2>One-page PDF is ready</h2><p>{result.analysis.summary}</p><p className="muted">Locked sections validated - {result.resume.page_count} page - saved as {result.resume.latest_pdf_path}</p></div><div className="score"><strong>{result.resume.report.estimated_ats_coverage_score}</strong><span>ATS coverage</span></div><div className="result-actions"><button className="primary" onClick={() => action('open_latest_pdf')}>Open PDF</button><button onClick={() => action('reveal_latest_pdf')}>Open folder</button></div>{result.resume.report.omitted_unsupported_keywords.length > 0 && <p className="unsupported"><b>Not added without evidence:</b> {result.resume.report.omitted_unsupported_keywords.join(', ')}</p>}</section>}
  </main>;
}

createRoot(document.getElementById('root')!).render(<App />);
