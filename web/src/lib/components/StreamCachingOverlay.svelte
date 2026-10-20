<script lang="ts">
  interface Props {
    progress: number;
    onCancel: () => void;
    onPickStream?: () => void;
  }

  let { progress, onCancel, onPickStream }: Props = $props();

  let percent = $derived(Math.round(progress * 100));
</script>

<div class="overlay-backdrop">
  <div class="overlay-card">
    <span class="material-icons-outlined status-icon spin">sync</span>
    <div class="status-text">Preparing stream...</div>
    <div class="progress-container">
      <div class="progress-bar">
        <div class="progress-fill" style="width: {percent}%"></div>
      </div>
      <div class="progress-label">{percent}%</div>
    </div>
    <button type="button" class="btn btn-secondary" onclick={onCancel}>Cancel</button>
    {#if onPickStream}
      <button type="button" class="link-btn" onclick={onPickStream}>Choose a release</button>
    {/if}
  </div>
</div>

<style>
  .overlay-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.6);
    backdrop-filter: blur(4px);
  }

  .overlay-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    padding: 32px 40px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    min-width: 300px;
    max-width: 400px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  }

  .status-icon {
    font-size: 40px;
    color: var(--accent);
  }

  .status-text {
    font-size: 16px;
    font-weight: 600;
    color: var(--ink);
  }

  .progress-container {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .progress-bar {
    width: 100%;
    height: 8px;
    background: var(--ring);
    border-radius: 4px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 4px;
    transition: width 0.3s ease;
  }

  .progress-label {
    font-size: 14px;
    font-weight: 600;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }

  .btn {
    padding: 8px 20px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    border: 1px solid transparent;
    transition:
      background 0.15s,
      border-color 0.15s;
  }

  .btn-secondary {
    background: transparent;
    border-color: var(--ring);
    color: var(--muted);
  }

  .btn-secondary:hover {
    border-color: var(--ink);
    color: var(--ink);
  }

  .link-btn {
    background: none;
    border: none;
    color: var(--muted);
    font-size: 12px;
    cursor: pointer;
    text-decoration: underline;
    padding: 4px;
    transition: color 0.15s;
  }

  .link-btn:hover {
    color: var(--ink);
  }

  .spin {
    animation: spin 1.5s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
