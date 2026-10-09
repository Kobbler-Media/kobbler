<script lang="ts">
  import { Modal } from '@koblibs/ui';
  import { browseDirectories } from '$lib/api/client';
  import type { DirectoryEntry } from '$lib/api/client';

  interface Props {
    open: boolean;
    initialPath?: string;
    onSelect: (path: string) => void;
    onClose: () => void;
  }

  let { open, initialPath, onSelect, onClose }: Props = $props();

  let currentPath = $state('/');
  let parentPath = $state<string | null>(null);
  let directories = $state<DirectoryEntry[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  const breadcrumbs = $derived.by(() => {
    const parts = currentPath.split('/').filter(Boolean);
    const crumbs = [{ name: '/', path: '/' }];
    let accumulated = '';
    for (const part of parts) {
      accumulated += '/' + part;
      crumbs.push({ name: part, path: accumulated });
    }
    return crumbs;
  });

  async function loadDirectory(path: string) {
    loading = true;
    error = null;
    try {
      const resp = await browseDirectories(path);
      currentPath = resp.current_path;
      parentPath = resp.parent_path;
      directories = resp.directories;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to browse directory';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) {
      loadDirectory(initialPath || '/');
    }
  });
</script>

{#if open}
  <Modal title="Browse Directory" {onClose} width="520px">
    <div class="browser">
      <nav class="breadcrumbs">
        {#each breadcrumbs as crumb, i (crumb.path)}
          {#if i > 0}<span class="separator">/</span>{/if}
          <button
            class="crumb"
            type="button"
            onclick={() => loadDirectory(crumb.path)}
            disabled={loading}
          >
            {crumb.name}
          </button>
        {/each}
      </nav>

      <div class="current-path">
        <code>{currentPath}</code>
      </div>

      {#if error}
        <div class="error-msg">{error}</div>
      {/if}

      <div class="dir-list">
        {#if parentPath}
          <button
            class="dir-item"
            type="button"
            onclick={() => loadDirectory(parentPath!)}
            disabled={loading}
          >
            <svg class="folder-icon" viewBox="0 0 24 24" width="16" height="16" fill="currentColor">
              <path d="M20 10H4V6h5.17l2 2H20v2zm0 2H4v6h16v-6z"></path>
            </svg>
            <span class="dir-name">..</span>
          </button>
        {/if}
        {#if loading}
          <div class="loading-msg">Loading...</div>
        {:else if directories.length === 0 && !error}
          <div class="empty-msg">No subdirectories</div>
        {:else}
          {#each directories as dir (dir.path)}
            <button
              class="dir-item"
              type="button"
              onclick={() => loadDirectory(dir.path)}
              disabled={loading}
            >
              <svg
                class="folder-icon"
                viewBox="0 0 24 24"
                width="16"
                height="16"
                fill="currentColor"
              >
                <path
                  d="M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10z"
                ></path>
              </svg>
              <span class="dir-name">{dir.name}</span>
            </button>
          {/each}
        {/if}
      </div>
    </div>

    {#snippet footer()}
      <button class="btn alt" type="button" onclick={onClose}>Cancel</button>
      <button class="btn" type="button" onclick={() => onSelect(currentPath)} disabled={loading}>
        Select This Directory
      </button>
    {/snippet}
  </Modal>
{/if}

<style>
  .browser {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .breadcrumbs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
    font-size: 12px;
  }

  .crumb {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 4px;
    font-size: 12px;
  }

  .crumb:hover:not(:disabled) {
    background: var(--ring);
  }

  .separator {
    color: var(--muted);
  }

  .current-path {
    padding: 6px 10px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .current-path code {
    font-size: 12px;
    word-break: break-all;
  }

  .dir-list {
    max-height: 340px;
    overflow-y: auto;
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .dir-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .dir-item:hover:not(:disabled) {
    background: var(--ring);
  }

  .dir-item + .dir-item {
    border-top: 1px solid var(--ring);
  }

  .folder-icon {
    flex-shrink: 0;
    color: var(--muted);
  }

  .dir-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .loading-msg,
  .empty-msg,
  .error-msg {
    padding: 16px;
    text-align: center;
    font-size: 12px;
    color: var(--muted);
  }

  .error-msg {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    border-radius: 8px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }
</style>
