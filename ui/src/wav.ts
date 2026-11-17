// SPDX-License-Identifier: GPL-3.0-or-later
// A 16-bit PCM WAV file around interleaved little-endian samples.

export function wavFile(data: Uint8Array, rate: number, channels: number): Uint8Array {
  const out = new Uint8Array(44 + data.length);
  const v = new DataView(out.buffer);
  const str = (o: number, s: string) => {
    for (let i = 0; i < s.length; i++) out[o + i] = s.charCodeAt(i);
  };
  str(0, "RIFF");
  v.setUint32(4, 36 + data.length, true);
  str(8, "WAVE");
  str(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true); // PCM
  v.setUint16(22, channels, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2 * channels, true);
  v.setUint16(32, 2 * channels, true);
  v.setUint16(34, 16, true);
  str(36, "data");
  v.setUint32(40, data.length, true);
  out.set(data, 44);
  return out;
}
