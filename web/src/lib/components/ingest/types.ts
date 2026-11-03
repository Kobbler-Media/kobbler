import type { IngestQueueGroup, IngestQueueStatusCounts, SeriesSummary } from '#lib/types.js';

/** Author with optional Hardcover ID for enrichment */
export interface AuthorInput {
  name: string;
  hardcover_id?: string;
}

/** Data shape from BookForm onSave callback */
export interface BookFormData {
  title: string;
  sort_title?: string;
  authors: AuthorInput[];
  series_id?: string;
  series_name?: string | null;
  series_hardcover_id?: string | null;
  series_index?: number | null;
  publisher?: string | null;
  pubdate?: string | null;
  description?: string | null;
  language?: string | null;
  page_count?: number | null;
  identifiers?: Record<string, string>;
  genres?: string[];
  rating?: number | null;
  cover_url?: string | null;
  want?: boolean;
}

export interface IngestPageCallbacks {
  onApprove: (id: string, scopeType?: string) => Promise<void>;
  onReject: (id: string) => Promise<void>;
  onRestore: (id: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
  onExtractMetadata: (id: string) => Promise<void>;
  onUpdateMetadata?: (id: string, data: BookFormData, coverFile?: File) => Promise<void>;
  onBatchReject?: (ids: string[]) => Promise<void>;
  onUploadBook?: (file: File) => Promise<void>;
  onLoadMore?: () => void;
  onSearch?: (search: string) => void;
  onStatusFilter?: (status: string) => void;
}

export const formatDate = (dateStr: string): string => {
  return new Date(dateStr).toLocaleString();
};

export const formatSize = (bytes: number): string => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
};

export const getStatusColor = (status: string): string => {
  switch (status) {
    case 'extracting':
      return 'extracting';
    case 'pending':
      return 'pending';
    case 'converting':
      return 'converting';
    case 'approved':
      return 'approved';
    case 'rejected':
      return 'rejected';
    case 'error':
      return 'error';
    default:
      return '';
  }
};

export { type IngestQueueGroup, type IngestQueueStatusCounts, type SeriesSummary };
