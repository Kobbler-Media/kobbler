import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
  keepStream: vi.fn(),
  getKeepStatus: vi.fn(),
  getRecentStreams: vi.fn()
}));

vi.mock('#lib/api/client.js', async (importOriginal) => ({
  ...(await importOriginal<typeof import('#lib/api/client.js')>()),
  ...api
}));

const { ApiError } = await import('#lib/api/client.js');
const { useKeep, findStreamSession } = await import('./useKeep.svelte');

function setup() {
  const onMessage = vi.fn();
  return { keep: useKeep({ onMessage }), onMessage };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
});

describe('useKeep', () => {
  it('posts keep and reports quiet progress until saved', async () => {
    api.keepStream.mockResolvedValue({ job_id: 'j', state: 'queued' });
    api.getKeepStatus
      .mockResolvedValueOnce({ state: 'downloading', progress: 0.4 })
      .mockResolvedValueOnce({ state: 'importing', progress: 1 })
      .mockResolvedValueOnce({ state: 'done', progress: 1 });
    const { keep } = setup();
    expect(keep.label).toBe('Keep in library');
    await keep.keep('s1');
    expect(api.keepStream).toHaveBeenCalledWith('s1');
    expect(keep.label).toBe('Saving to library…');
    expect(keep.busy).toBe(true);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(keep.label).toBe('Saving to library… 40%');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(keep.label).toBe('Saving to library… 100%');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(keep.label).toBe('Saved');
    expect(keep.saved).toBe(true);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(api.getKeepStatus).toHaveBeenCalledTimes(3);
  });

  it('is saved at once when the server already has it', async () => {
    api.keepStream.mockResolvedValue({ job_id: 'j', state: 'done' });
    const { keep } = setup();
    await keep.keep('s1');
    expect(keep.label).toBe('Saved');
    expect(api.getKeepStatus).not.toHaveBeenCalled();
  });

  it('says why and offers keep again when the import fails', async () => {
    api.keepStream.mockResolvedValue({ job_id: 'j', state: 'queued' });
    api.getKeepStatus.mockResolvedValue({ state: 'failed', error: 'Disk full' });
    const { keep, onMessage } = setup();
    await keep.keep('s1');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(onMessage).toHaveBeenCalledWith('Disk full');
    expect(keep.label).toBe('Keep in library');
  });

  it('says why when the keep is refused', async () => {
    api.keepStream.mockRejectedValue(new ApiError(409, 'Already importing'));
    const { keep, onMessage } = setup();
    await keep.keep('s1');
    expect(onMessage).toHaveBeenCalledWith('Already importing');
    expect(keep.busy).toBe(false);
  });

  it('picks up a keep already running for the session', async () => {
    api.getKeepStatus
      .mockResolvedValueOnce({ state: 'downloading', progress: 0.25 })
      .mockResolvedValueOnce({ state: 'done' });
    const { keep } = setup();
    await keep.resume('s1');
    expect(keep.label).toBe('Saving to library… 25%');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(keep.label).toBe('Saved');
  });

  it('stays idle for a session that was never kept', async () => {
    api.getKeepStatus.mockRejectedValue(new ApiError(404, 'nope'));
    const { keep, onMessage } = setup();
    await keep.resume('s1');
    expect(keep.label).toBe('Keep in library');
    expect(onMessage).not.toHaveBeenCalled();
  });

  it('stops polling once disposed', async () => {
    api.keepStream.mockResolvedValue({ job_id: 'j', state: 'queued' });
    api.getKeepStatus.mockResolvedValue({ state: 'downloading', progress: 0.1 });
    const { keep } = setup();
    await keep.keep('s1');
    keep.dispose();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(api.getKeepStatus).not.toHaveBeenCalled();
  });
});

describe('findStreamSession', () => {
  it('finds the latest stream of this title', async () => {
    api.getRecentStreams.mockResolvedValue({
      sessions: [
        { id: 'other', media_id: 'm2' },
        { id: 'mine', media_id: 'm1' },
        { id: 'older', media_id: 'm1' }
      ]
    });
    expect(await findStreamSession('m1')).toBe('mine');
    expect(await findStreamSession('m3')).toBeNull();
  });

  it('is quiet when the list cannot be read', async () => {
    api.getRecentStreams.mockRejectedValue(new Error('down'));
    expect(await findStreamSession('m1')).toBeNull();
  });
});

describe('useTitleKeep', () => {
  it('offers Keep only for a title that was streamed', async () => {
    const { useTitleKeep } = await import('./useKeep.svelte');
    api.getRecentStreams.mockResolvedValue({ sessions: [{ id: 's1', media_id: 'm1' }] });
    api.getKeepStatus.mockRejectedValue(new ApiError(404, 'nope'));
    api.keepStream.mockResolvedValue({ job_id: 'j', state: 'done' });
    const title = useTitleKeep(vi.fn());
    expect(title.action).toBeNull();
    await title.refresh('m2');
    expect(title.action).toBeNull();
    await title.refresh('m1');
    expect(title.action).toMatchObject({ label: 'Keep in library', icon: 'library_add' });
    title.action!.onSelect();
    await vi.advanceTimersByTimeAsync(0);
    expect(api.keepStream).toHaveBeenCalledWith('s1');
    expect(title.action).toMatchObject({ label: 'Saved', disabled: true });
  });
});
