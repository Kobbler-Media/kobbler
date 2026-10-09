<script lang="ts">
  interface Props {
    id: string;
    title: string;
    coverUrl: string | null;
    /** Optional progress percentage (0-100) */
    progress?: number;
    /** Optional click handler; if omitted, navigates to book */
    onClick?: (id: string) => void;
  }

  let { id, title, coverUrl, progress, onClick }: Props = $props();

  const resolvedCoverUrl = $derived(coverUrl || '/placeholder-cover.png');

  function handleClick() {
    if (onClick) {
      onClick(id);
    } else {
      window.location.href = `/library?book=${id}`;
    }
  }
</script>

<button class="mini-book" type="button" onclick={handleClick} {title}>
  <div class="cover" style={`background-image: url(${resolvedCoverUrl});`}>
    {#if progress !== undefined && progress > 0}
      <span class="progress-badge">{progress}%</span>
    {/if}
  </div>
  <div class="title">{title}</div>
</button>

<style>
  .mini-book {
    appearance: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 70px;
    flex-shrink: 0;
    cursor: pointer;
    background: transparent;
    border: none;
    padding: 0;
    text-align: center;
    color: inherit;
    font: inherit;
  }

  .mini-book:hover .cover {
    transform: translateY(-2px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  }

  :global([data-theme='light']) .mini-book:hover .cover {
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
  }

  .cover {
    aspect-ratio: 2 / 3;
    width: 100%;
    border-radius: 6px;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    background-size: cover;
    background-position: center;
    position: relative;
    overflow: hidden;
    border: 1px solid var(--ring);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
    transition:
      transform 0.15s ease,
      box-shadow 0.15s ease;
  }

  :global([data-theme='light']) .cover {
    background: linear-gradient(140deg, #e5e0d8, #d5cfc5);
    background-size: cover;
    background-position: center;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.12);
  }

  .progress-badge {
    position: absolute;
    bottom: 4px;
    right: 4px;
    font-size: 9px;
    font-weight: 700;
    padding: 2px 4px;
    border-radius: 3px;
    background: var(--accent);
    color: #000;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
  }

  .title {
    font-size: 11px;
    font-weight: 500;
    line-height: 1.2;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    word-break: break-word;
    color: var(--ink);
  }
</style>
