// Host loader for the heap wasm example (tier 4.09). The module is a
// wasm32-unknown-unknown reactor: it bundles its own allocator + heap-core
// runtime, so it instantiates with NO imports — the host just calls exports.
//
// `array_sum` returns an i64, which JS surfaces as a BigInt; we compare via
// Number(). Usage: node run.mjs [path-to-wasm]  (default: ./heap.wasm)

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const wasmPath = process.argv[2] ?? join(here, "heap.wasm");
const bytes = readFileSync(wasmPath);

if (!WebAssembly.validate(bytes)) {
  console.error(`FAIL: ${wasmPath} is not a valid WebAssembly module`);
  process.exit(1);
}

// No import object needed — the runtime is linked in (no unresolved imports).
const { instance } = await WebAssembly.instantiate(bytes, {});
const got = Number(instance.exports.array_sum());
const want = 42;
const pass = got === want;
console.log(`${pass ? "ok  " : "FAIL"} array_sum() = ${got} (want ${want})`);
console.log(pass ? "heap on wasm works" : "heap on wasm BROKEN");
process.exit(pass ? 0 : 1);
