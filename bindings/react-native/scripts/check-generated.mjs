/**
 * Refuses to pack a tree that is missing pipeline output. None of it is in git,
 * so a pack from a fresh clone would otherwise ship an empty package.
 */
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

const required = [
  ["src/generated", "just nitro-bindings"],
  ["cpp/generated", "just nitro-bindings"],
  ["nitrogen/generated", "just nitro-bindings"],
  ["ios/generated/cashu_ffi.xcframework", "just nitro-ios"],
  ["android/src/main/jniLibs", "just nitro-android"],
];

const missing = required.filter(([path]) => !existsSync(join(root, path)));

if (missing.length > 0) {
  for (const [path, recipe] of missing) {
    console.error(`missing ${path}: run \`${recipe}\` from the repository root`);
  }
  console.error("`just nitro-package` runs every step in order");
  process.exit(1);
}
