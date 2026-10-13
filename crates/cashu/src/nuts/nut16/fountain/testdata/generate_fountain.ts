// Regenerate with Bun and a checkout of Egge21M/nut-fountain at v0.1.0-alpha.0:
// bun crates/cashu/src/nuts/nut16/fountain/testdata/generate_fountain.ts /path/to/nut-fountain
// The byte core has no npm dependencies. Output is deterministic.
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(process.argv[2]!);
const commit = Bun.spawnSync(["git", "-C", root, "rev-parse", "HEAD"]);
if (new TextDecoder().decode(commit.stdout).trim() !== "55fe48b20627a19d3152a16d628f4fb98731a1f4") {
  throw new Error("Expected nut-fountain v0.1.0-alpha.0 commit");
}
const { FountainEncoder, FountainDecoder } = await import(pathToFileURL(resolve(root, "packages/nut-fountain/src/core.ts")).href);
const { coefficients } = await import(pathToFileURL(resolve(root, "packages/nut-fountain/src/internal/core/equations.ts")).href);
const { serializeFrame } = await import(pathToFileURL(resolve(root, "packages/nut-fountain/src/internal/core/wire.ts")).href);
const { crc32 } = await import(pathToFileURL(resolve(root, "packages/nut-fountain/src/internal/crc32.ts")).href);
const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex");
const cases = [
  { name: "empty", size: 1, message: new Uint8Array() },
  { name: "single", size: 3, message: Uint8Array.of(1, 2, 3) },
  { name: "mixed", size: 1, message: Uint8Array.of(0x10, 0x20, 0x30, 0x40) },
  { name: "binary", size: 3, message: Uint8Array.of(0, 255, 1, 128, 7, 0, 12) },
  { name: "padded", size: 10, message: Uint8Array.from({length: 73}, (_, i) => i * 13) },
  { name: "byte_range", size: 128, message: Uint8Array.from({length: 256}, (_, i) => i) },
  { name: "max_count", size: 1, message: Uint8Array.from({length: 256}, (_, i) => i) },
];
const fixtures = cases.map(({name, size, message}) => {
  const encoder = new FountainEncoder(message, {fragmentSize: size});
  const decoder = new FountainDecoder();
  const frames = [];
  const accepted = [];
  // Include all source frames plus enough repair-only frames to recover.
  for (let i = 0; i < encoder.fragmentCount; i++) frames.push(hex(encoder.nextFrame()));
  for (let i = 0; i < encoder.fragmentCount + 64 && !decoder.isComplete; i++) {
    const frame = encoder.nextFrame();
    frames.push(hex(frame));
    accepted.push(decoder.receive(frame));
  }
  if (!decoder.isComplete || hex(decoder.result) !== hex(message)) throw new Error(name);
  const highSequenceFrames = [0x7fffffff, 0x80000000, 0xffffffff].map(sequence => {
    const data = new Uint8Array(size);
    coefficients(sequence, encoder.fragmentCount).forEach((bit: number, i: number) => {
      if (bit) message.subarray(i * size, (i + 1) * size).forEach((byte: number, j: number) => { data[j] ^= byte; });
    });
    const frame = serializeFrame({sequence, count: encoder.fragmentCount, length: message.length, checksum: crc32(message), data});
    return hex(frame);
  });
  return { name, size, message: hex(message), frames, accepted, high_sequence_frames: highSequenceFrames };
});
await Bun.write(new URL("fountain.json", import.meta.url), JSON.stringify(fixtures, null, 2) + "\n");
