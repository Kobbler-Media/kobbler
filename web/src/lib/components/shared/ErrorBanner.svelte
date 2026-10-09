<script lang="ts">
  interface Props {
    message: string;
    onRetry?: () => void;
    onDismiss?: () => void;
  }

  let { message, onRetry, onDismiss }: Props = $props();
</script>

<div class="error-banner" role="alert">
  <div class="error-content">
    <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" class="error-icon">
      <path
        d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z"
      ></path>
    </svg>
    <span class="error-text">{message}</span>
  </div>
  <div class="error-actions">
    {#if onRetry}
      <button type="button" class="retry-btn" onclick={onRetry}>Retry</button>
    {/if}
    {#if onDismiss}
      <button type="button" class="dismiss-btn" onclick={onDismiss} aria-label="Dismiss error">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
          <path
            d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
          ></path>
        </svg>
      </button>
    {/if}
  </div>
</div>

<style>
  .error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--danger) 30%, transparent);
    border-radius: 8px;
    color: var(--danger);
    font-size: 13px;
  }

  .error-content {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .error-icon {
    flex-shrink: 0;
    opacity: 0.8;
  }

  .error-text {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .error-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .retry-btn {
    padding: 4px 12px;
    background: var(--danger);
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .retry-btn:hover {
    opacity: 0.9;
  }

  .dismiss-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--danger);
    cursor: pointer;
    opacity: 0.7;
  }

  .dismiss-btn:hover {
    opacity: 1;
    background: color-mix(in srgb, var(--danger) 15%, transparent);
  }
</style>
