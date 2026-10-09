<script lang="ts">
  interface Props {
    name: string;
    role: string | null;
    imageUrl: string | null;
  }

  let { name, role, imageUrl }: Props = $props();
</script>

<div class="person-card">
  <div class="person-photo" style={imageUrl ? `background-image: url(${imageUrl})` : ''}>
    {#if !imageUrl}
      <span class="material-icons-outlined placeholder-icon">person</span>
    {/if}
  </div>
  <div class="person-info">
    <span class="person-name">{name}</span>
    {#if role}
      <span class="person-role">{role}</span>
    {/if}
  </div>
</div>

<style>
  .person-card {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
  }

  .person-photo {
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: var(--bg-2);
    background-size: cover;
    background-position: center;
    border: 1px solid var(--ring);
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .placeholder-icon {
    font-size: 24px;
    color: var(--muted);
  }

  .person-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .person-name {
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .person-role {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
