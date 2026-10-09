<script lang="ts">
  interface Props {
    watched: boolean;
    loading: boolean;
    size?: 'normal' | 'small';
    onToggle: () => void;
  }

  let { watched, loading, size = 'normal', onToggle }: Props = $props();
</script>

<button
  class="watch-toggle"
  class:watched
  class:small={size === 'small'}
  type="button"
  onclick={onToggle}
  disabled={loading}
>
  <span class="material-icons-outlined">
    {watched ? 'visibility' : 'visibility_off'}
  </span>
  {watched ? 'Watched' : 'Not Watched'}
</button>

<style>
  .watch-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 7px 14px;
    border-radius: 100px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    font-size: 13px;
    font-weight: 600;
    transition: all 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  }

  .watch-toggle.small {
    padding: 6px 12px;
    font-size: 12px;
  }

  .watch-toggle:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--ink);
    transform: translateY(-1px);
  }

  .watch-toggle.watched {
    background: var(--accent-dim);
    border-color: var(--accent);
    color: var(--accent);
  }

  .watch-toggle:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .watch-toggle .material-icons-outlined {
    font-size: 16px;
  }

  .watch-toggle.small .material-icons-outlined {
    font-size: 14px;
  }
</style>
