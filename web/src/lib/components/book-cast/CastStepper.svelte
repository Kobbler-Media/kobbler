<script lang="ts" module>
  export interface CastStep {
    id: string;
    title: string;
    detail: string;
    status: 'done' | 'todo' | 'working' | 'locked';
  }
</script>

<script lang="ts">
  interface Props {
    steps: CastStep[];
    extras: { id: string; title: string }[];
    active: string;
    onSelect: (id: string) => void;
  }

  let { steps, extras, active, onSelect }: Props = $props();

  const ICONS = { done: 'check', working: 'autorenew', todo: '', locked: 'lock' };
</script>

<nav class="stepper" aria-label="Full cast steps">
  <ol>
    {#each steps as s, i (s.id)}
      <li>
        <button
          type="button"
          class="step {s.status}"
          class:active={active === s.id}
          aria-current={active === s.id ? 'step' : undefined}
          disabled={s.status === 'locked'}
          onclick={() => onSelect(s.id)}
        >
          <span class="dot">
            {#if ICONS[s.status]}
              <span class="material-icons-outlined">{ICONS[s.status]}</span>
            {:else}
              {i + 1}
            {/if}
          </span>
          <span class="text">
            <span class="title">{s.title}</span>
            <span class="detail">{s.detail}</span>
          </span>
        </button>
      </li>
    {/each}
  </ol>
  <div class="extras">
    {#each extras as x (x.id)}
      <button
        type="button"
        class="extra"
        class:active={active === x.id}
        onclick={() => onSelect(x.id)}>{x.title}</button
      >
    {/each}
  </div>
</nav>

<style>
  .stepper {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }

  ol {
    display: flex;
    gap: 4px;
    margin: 0;
    padding: 4px;
    list-style: none;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
  }

  li {
    flex: 1;
    min-width: 140px;
  }

  .step {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    font: inherit;
    text-align: left;
    padding: 8px 12px;
    border: none;
    border-radius: 10px;
    background: none;
    color: var(--ink);
    cursor: pointer;
  }

  .step:not(:disabled):hover {
    background: var(--bg-2);
  }

  .step.active {
    background: var(--bg-2);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .step:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .dot {
    display: grid;
    place-items: center;
    flex: none;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: 1.5px solid var(--ring);
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
  }

  .dot .material-icons-outlined {
    font-size: 16px;
  }

  .done .dot {
    background: var(--success);
    border-color: var(--success);
    color: #000;
  }

  .todo .dot {
    border-color: var(--accent);
    color: var(--accent);
  }

  .working .dot .material-icons-outlined {
    animation: spin 1.6s linear infinite;
    color: var(--accent);
  }

  .text {
    display: grid;
    min-width: 0;
  }

  .title {
    font-size: 13px;
    font-weight: 600;
  }

  .detail {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .extras {
    display: flex;
    gap: 4px;
  }

  .extra {
    font: inherit;
    font-size: 13px;
    padding: 6px 14px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    cursor: pointer;
  }

  .extra.active {
    background: var(--card);
    border-color: var(--accent);
    font-weight: 600;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .working .dot .material-icons-outlined {
      animation: none;
    }
  }
</style>
