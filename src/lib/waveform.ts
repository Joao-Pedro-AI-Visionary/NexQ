// Waveform helpers shared by the player UI.

/**
 * Max-pool `peaks` down to at most `target` values.
 * Returns the input unchanged when it already fits.
 * Mirrors `downsample_peaks` in src-tauri/src/audio/waveform.rs.
 */
export function downsamplePeaks(peaks: number[], target: number): number[] {
  if (peaks.length <= target) return peaks;
  if (target <= 0) return [];
  const len = peaks.length;
  const out = new Array<number>(target);
  for (let i = 0; i < target; i++) {
    const start = Math.floor((i * len) / target);
    const end = Math.min(len, Math.max(start + 1, Math.floor(((i + 1) * len) / target)));
    let max = 0;
    for (let j = start; j < end; j++) {
      if (peaks[j] > max) max = peaks[j];
    }
    out[i] = max;
  }
  return out;
}
