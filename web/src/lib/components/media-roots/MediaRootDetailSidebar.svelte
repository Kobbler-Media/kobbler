<script lang="ts">
  import type { MediaRoot } from '$lib/types';
  import MediaRootInfoCard from './MediaRootInfoCard.svelte';
  import MediaRootEditForm from './MediaRootEditForm.svelte';

  interface Props {
    root: MediaRoot;
    loading: boolean;
    editMode: boolean;
    saving: boolean;
    editPath: string;
    editName: string;
    editAutoAdd: boolean;
    editWriteSidecars: boolean;
    editWriteImages: boolean;
    editNamingTemplate: string;
    editMaxConcurrentIo: number;
    editScanIntervalSeconds: number | null;
    onEditPathChange: (v: string) => void;
    onEditNameChange: (v: string) => void;
    onEditAutoAddChange: (v: boolean) => void;
    onEditWriteSidecarsChange: (v: boolean) => void;
    onEditWriteImagesChange: (v: boolean) => void;
    onEditNamingTemplateChange: (v: string) => void;
    onEditMaxConcurrentIoChange: (v: number) => void;
    onEditScanIntervalSecondsChange: (v: number | null) => void;
    onBrowsePath: () => void;
    onScan: (id: string) => Promise<void>;
    onWriteSidecars: (id: string) => Promise<void>;
    onWriteImages: (id: string) => Promise<void>;
    onViewFiles: (id: string) => void;
    onViewScanResults: (id: string) => void;
    onReorganize: (id: string) => void;
    onUpdate?: () => void;
  }

  let {
    root,
    loading,
    editMode,
    saving,
    editPath,
    editName,
    editAutoAdd,
    editWriteSidecars,
    editWriteImages,
    editNamingTemplate,
    editMaxConcurrentIo,
    editScanIntervalSeconds,
    onEditPathChange,
    onEditNameChange,
    onEditAutoAddChange,
    onEditWriteSidecarsChange,
    onEditWriteImagesChange,
    onEditNamingTemplateChange,
    onEditMaxConcurrentIoChange,
    onEditScanIntervalSecondsChange,
    onBrowsePath,
    onScan,
    onWriteSidecars,
    onWriteImages,
    onViewFiles,
    onViewScanResults,
    onReorganize,
    onUpdate
  }: Props = $props();
</script>

{#if editMode}
  <MediaRootEditForm
    {root}
    {saving}
    {editPath}
    {editName}
    {editAutoAdd}
    {editWriteSidecars}
    {editWriteImages}
    {editNamingTemplate}
    {editMaxConcurrentIo}
    {editScanIntervalSeconds}
    {onEditPathChange}
    {onEditNameChange}
    {onEditAutoAddChange}
    {onEditWriteSidecarsChange}
    {onEditWriteImagesChange}
    {onEditNamingTemplateChange}
    {onEditMaxConcurrentIoChange}
    {onEditScanIntervalSecondsChange}
    {onBrowsePath}
  />
{:else}
  <MediaRootInfoCard
    {root}
    {loading}
    {onScan}
    {onWriteSidecars}
    {onWriteImages}
    {onViewFiles}
    {onViewScanResults}
    {onReorganize}
    {onUpdate}
  />
{/if}
