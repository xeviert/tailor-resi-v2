// @vitest-environment jsdom

import '@testing-library/jest-dom/vitest';
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));

import { ImportJobPanel } from './job-import';

const posting =
  'Senior Rust Engineer at Acme. '.repeat(10) +
  'You will build and operate reliable backend services in Rust and Axum, own their deployment, and review the code of a small team.';

beforeEach(() => {
  mocks.invoke.mockReset();
  mocks.invoke.mockResolvedValue({ received_at_ms: 1, payload: {}, parsed: {} });
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function url() {
  return screen.getByLabelText(/^Job post URL/);
}

describe('manual job import', () => {
  it('switches between the url and paste modes', () => {
    render(<ImportJobPanel variant='empty' />);

    expect(screen.queryByLabelText('Job post text')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Fetch and import' })).toBeVisible();

    fireEvent.click(screen.getByRole('button', { name: 'Paste text' }));

    expect(screen.getByLabelText('Job post text')).toBeVisible();
    // The URL stays available in paste mode so the capture keeps a working source link.
    expect(screen.getByLabelText('Job post URL (optional)')).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Import pasted text' }),
    ).toBeVisible();
  });

  it('sends the typed url', async () => {
    render(<ImportJobPanel variant='empty' />);

    fireEvent.change(url(), {
      target: { value: '  https://jobs.example.com/1  ' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Fetch and import' }));

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('import_job_from_url', {
        url: 'https://jobs.example.com/1',
      }),
    );
  });

  it('sends pasted text with the optional source url', async () => {
    render(<ImportJobPanel variant='empty' />);

    fireEvent.click(screen.getByRole('button', { name: 'Paste text' }));
    fireEvent.change(screen.getByLabelText('Job post text'), {
      target: { value: posting },
    });
    fireEvent.change(screen.getByLabelText('Job post URL (optional)'), {
      target: { value: 'https://jobs.example.com/1' },
    });
    fireEvent.click(
      screen.getByRole('button', { name: 'Import pasted text' }),
    );

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('import_job_from_text', {
        text: posting,
        sourceUrl: 'https://jobs.example.com/1',
      }),
    );
  });

  it('sends pasted text with no source url as null', async () => {
    render(<ImportJobPanel variant='empty' />);

    fireEvent.click(screen.getByRole('button', { name: 'Paste text' }));
    fireEvent.change(screen.getByLabelText('Job post text'), {
      target: { value: posting },
    });
    fireEvent.click(
      screen.getByRole('button', { name: 'Import pasted text' }),
    );

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith('import_job_from_text', {
        text: posting,
        sourceUrl: null,
      }),
    );
  });

  it('refuses an empty url and a too-short paste without calling the backend', async () => {
    render(<ImportJobPanel variant='empty' />);

    fireEvent.click(screen.getByRole('button', { name: 'Fetch and import' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Paste the link to the job post first.',
    );

    fireEvent.click(screen.getByRole('button', { name: 'Paste text' }));
    fireEvent.change(screen.getByLabelText('Job post text'), {
      target: { value: 'Rust Engineer' },
    });
    fireEvent.click(
      screen.getByRole('button', { name: 'Import pasted text' }),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'That is only 13 characters',
    );

    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it('shows the backend failure next to the form and re-enables the button', async () => {
    // The blocked-board message is the one that has to survive: it is the only place the
    // paste fallback gets advertised at the moment the user needs it.
    const blocked =
      'The site refused the request (HTTP 403). Many job boards block anything that is not a real browser - open the post in your browser, copy the text, and use the Paste text tab.';
    mocks.invoke.mockRejectedValueOnce(blocked);
    render(<ImportJobPanel variant='empty' />);

    fireEvent.change(url(), { target: { value: 'https://jobs.example.com/1' } });
    const button = screen.getByRole('button', { name: 'Fetch and import' });
    fireEvent.click(button);

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'use the Paste text tab',
    );
    await waitFor(() => expect(button).toBeEnabled());
  });

  it('disables the form while an import is in flight', async () => {
    mocks.invoke.mockReturnValueOnce(new Promise(() => undefined));
    render(<ImportJobPanel variant='empty' />);

    fireEvent.change(url(), { target: { value: 'https://jobs.example.com/1' } });
    fireEvent.click(screen.getByRole('button', { name: 'Fetch and import' }));

    const busy = await screen.findByRole('button', { name: 'Importing...' });
    expect(busy).toBeDisabled();
    expect(url()).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Paste text' })).toBeDisabled();
  });

  it('stays inert while a pipeline run owns the capture', () => {
    render(<ImportJobPanel variant='review' disabled />);

    expect(screen.getByRole('button', { name: 'Fetch and import' })).toBeDisabled();
    expect(url()).toBeDisabled();
  });

  it('clears the form and notifies the host once an import lands', async () => {
    const onImported = vi.fn();
    render(<ImportJobPanel variant='review' onImported={onImported} />);

    fireEvent.change(url(), { target: { value: 'https://jobs.example.com/1' } });
    fireEvent.click(screen.getByRole('button', { name: 'Fetch and import' }));

    await waitFor(() => expect(onImported).toHaveBeenCalledTimes(1));
    expect(url()).toHaveValue('');
  });
});
