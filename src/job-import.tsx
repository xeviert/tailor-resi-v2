import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

// Style constants and `errorText` are copied rather than imported from `main.tsx`, which
// imports this file: reaching back the other way would make the two modules a cycle. This is
// the same trade `job-panel.tsx` already makes with its own card styles.
const panelClass =
  'mt-7 rounded-[14px] border border-[#dde3dc] bg-white p-7 shadow-[0_8px_24px_#1f2a2110]';
const eyebrowClass =
  'mb-2 text-[11px] font-bold uppercase tracking-[0.12em] text-[#668074]';
const mutedClass = 'm-0 text-[#627067]';
const primaryButtonClass =
  'cursor-pointer rounded-lg border-0 bg-[#176a46] px-4 py-3 font-bold text-white disabled:cursor-wait disabled:opacity-65';
const fieldLabelClass = 'text-[11px] font-bold text-[#526259]';
const inputClass =
  'w-full rounded-lg border border-[#cbd4cc] bg-white px-3 py-2.5 text-[14px] text-[#19221d] focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-[#176a46] disabled:opacity-65';
const segmentedGroupClass =
  'grid w-[300px] grid-cols-2 overflow-hidden rounded-lg border border-[#cbd4cc] bg-white max-[680px]:w-full';
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
const secondaryButtonClass =
  'cursor-pointer rounded-lg border border-[#cbd4cc] bg-white px-[13px] py-[11px] font-bold text-[#19221d]';
const alertClass =
  'mt-3 mb-0 rounded-lg bg-[#fff3eb] px-3 py-2.5 text-[#9e411e]';

// Mirrors the guard in job_import.rs, so an obviously empty form never costs a round trip.
const MIN_PASTED_CHARS = 200;

type Mode = 'url' | 'text';

/** Seconds spent so far. A counter that moves is the difference between slow and hung. */
function ElapsedTime({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [since]);
  const total = Math.max(0, Math.floor((now - since) / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return <>{`${minutes}:${String(seconds).padStart(2, '0')}`}</>;
}

function errorText(reason: unknown) {
  if (typeof reason === 'string') return reason;
  if (reason instanceof Error) return reason.message;
  // A rejected Tauri command arrives as the serialized `AppError`, `{code, message}`. Without
  // this branch every backend failure reached the user as raw JSON.
  if (
    reason &&
    typeof reason === 'object' &&
    typeof (reason as { message?: unknown }).message === 'string'
  ) {
    const message = (reason as { message: string }).message.trim();
    if (message) return message;
  }
  try {
    return JSON.stringify(reason);
  } catch {
    return 'An unexpected error occurred.';
  }
}

/**
 * Bring a job post in by hand, when the extension could not.
 *
 * Nothing here applies the capture it receives back. The backend emits `job-data-received`,
 * and that listener owns the entire new-capture reset; applying the returned capture here
 * would make the listener discard its own event as a duplicate and skip the reset, leaving
 * the previous run's result and preflight stranded on top of a different job. The awaited
 * promise is used for nothing but the busy state and the error message.
 */
export function ImportJobPanel({
  variant,
  disabled,
  onImported,
}: {
  variant: 'empty' | 'review';
  disabled?: boolean;
  onImported?: () => void;
}) {
  const [mode, setMode] = useState<Mode>('url');
  const [url, setUrl] = useState('');
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [error, setError] = useState('');
  // Bumped by Cancel. The in-flight import keeps running - if it succeeds it still emits
  // `job-data-received` and the app moves onto the new job - but this form stops treating
  // itself as busy and stops clearing the fields out from under the user. A ref, because
  // the awaited continuation below closes over the value from before the cancel.
  const attemptRef = useRef(0);

  const locked = busy || Boolean(disabled);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (locked) return;

    const trimmedUrl = url.trim();
    const trimmedText = text.trim();
    if (mode === 'url' && !trimmedUrl) {
      setError('Paste the link to the job post first.');
      return;
    }
    if (mode === 'text' && trimmedText.length < MIN_PASTED_CHARS) {
      setError(
        `That is only ${trimmedText.length} characters. Paste the whole job post, including the requirements section.`,
      );
      return;
    }

    const thisAttempt = attemptRef.current;
    setBusy(true);
    setStartedAt(Date.now());
    setError('');
    try {
      if (mode === 'url') {
        await invoke('import_job_from_url', { url: trimmedUrl });
      } else {
        await invoke('import_job_from_text', {
          text: trimmedText,
          sourceUrl: trimmedUrl || null,
        });
      }
      if (thisAttempt !== attemptRef.current) return;
      setUrl('');
      setText('');
      onImported?.();
    } catch (reason) {
      if (thisAttempt !== attemptRef.current) return;
      console.error('[job-import] import rejected', reason);
      setError(errorText(reason));
    } finally {
      if (thisAttempt === attemptRef.current) {
        setBusy(false);
        setStartedAt(null);
      }
    }
  }

  function cancel() {
    attemptRef.current += 1;
    setBusy(false);
    setStartedAt(null);
    setError(
      'Stopped waiting. The import may still finish on its own - if it does, the job will appear here.',
    );
  }

  return (
    <section
      className={variant === 'empty' ? `${panelClass} max-w-[650px]` : 'mt-4'}
      data-testid='import-job-panel'
    >
      {variant === 'empty' && (
        <>
          <p className={eyebrowClass}>OR IMPORT IT YOURSELF</p>
          <h2 className='mb-2 text-[22px] font-bold'>Import a job post</h2>
        </>
      )}
      <p className={mutedClass}>
        Paste the link and the app will read the posting from the page. If the
        site blocks that, or builds its page with JavaScript, copy the posting
        text and paste that instead.
      </p>

      <div
        className={`${segmentedGroupClass} mt-4`}
        role='group'
        aria-label='Import mode'
      >
        <button
          type='button'
          className={segmentedButtonClass(mode === 'url', true)}
          aria-pressed={mode === 'url'}
          disabled={locked}
          onClick={() => {
            setMode('url');
            setError('');
          }}
        >
          From URL
        </button>
        <button
          type='button'
          className={segmentedButtonClass(mode === 'text', false)}
          aria-pressed={mode === 'text'}
          disabled={locked}
          onClick={() => {
            setMode('text');
            setError('');
          }}
        >
          Paste text
        </button>
      </div>

      <form className='mt-4 grid gap-3' onSubmit={submit}>
        {mode === 'text' && (
          <label className='grid gap-1.5'>
            <span className={fieldLabelClass}>Job post text</span>
            <textarea
              className={inputClass}
              rows={10}
              value={text}
              disabled={locked}
              placeholder='Paste the whole posting: title, company, responsibilities and requirements.'
              onChange={(event) => setText(event.target.value)}
            />
          </label>
        )}
        <label className='grid gap-1.5'>
          <span className={fieldLabelClass}>
            {mode === 'url' ? 'Job post URL' : 'Job post URL (optional)'}
          </span>
          <input
            className={inputClass}
            type='url'
            value={url}
            disabled={locked}
            placeholder='https://boards.greenhouse.io/acme/jobs/12345'
            onChange={(event) => setUrl(event.target.value)}
          />
        </label>
        <div className='flex flex-wrap items-center gap-2.5'>
          <button className={primaryButtonClass} type='submit' disabled={locked}>
            {busy
              ? 'Importing...'
              : mode === 'url'
                ? 'Fetch and import'
                : 'Import pasted text'}
          </button>
          {busy && (
            <button
              className={secondaryButtonClass}
              data-testid='cancel-import'
              onClick={cancel}
              type='button'
            >
              Cancel
            </button>
          )}
        </div>
      </form>

      {/* A silent wait reads as a hang, so say what is happening and keep a counter moving.
          The old copy promised 10-30 seconds against a backend that can legitimately spend
          minutes on a long posting, which made every slow import look broken. */}
      <p className='mt-3 mb-0 min-h-[18px] text-[12px] text-[#627067]' aria-live='polite'>
        {busy && startedAt !== null ? (
          <>
            {mode === 'url'
              ? 'Fetching the page and reading the posting'
              : 'Reading the posting'}
            {' - '}
            <ElapsedTime since={startedAt} /> elapsed. Usually under a minute; a long
            posting can take several.
          </>
        ) : (
          ''
        )}
      </p>

      {error && (
        <p className={alertClass} role='alert'>
          {error}
        </p>
      )}
    </section>
  );
}
