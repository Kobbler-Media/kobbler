// hls.js restarts the media clock at 0 on every playlist a resume or a seek opens.
export function streamPosition(seekOffset: number, mediaTime: number): number {
  return seekOffset + mediaTime;
}
