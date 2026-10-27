export const BOOK_RATING_SCALE = 5;
export const VIDEO_RATING_SCALE = 10;

const STARS = 5;

export function ratingScale(mediaType: string): number {
  return mediaType === 'book' ? BOOK_RATING_SCALE : VIDEO_RATING_SCALE;
}

export function formatRating(rating: number, scale: number): string {
  const value = rating.toFixed(1);
  return scale === BOOK_RATING_SCALE ? value : `${value}/${scale}`;
}

export function starFill(rating: number | null | undefined, scale: number, star: number): number {
  if (rating == null) return 0;
  return Math.min(1, Math.max(0, (rating * STARS) / scale - star + 1));
}
