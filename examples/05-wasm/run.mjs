// Minimal hand-written JS loader for the Ruxen-compiled wasm module.
//
// No bindings framework, no wasm-bindgen — a wasm32-unknown-unknown reactor
// module exports plain functions over the C ABI (i32 in / i32 out), so the
// host just instantiates and calls them. This is the "you own the module,
// call its exports" convention (spec §5; ADR phase4-no-std-wasm decision #4).
//
// Usage: node run.mjs [path-to-wasm]   (default: ./add.wasm)

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const wasmPath = process.argv[2] ?? join(here, "add.wasm");

const bytes = readFileSync(wasmPath);

if (!WebAssembly.validate(bytes)) {
  console.error(`FAIL: ${wasmPath} is not a valid WebAssembly module`);
  process.exit(1);
}

const { instance } = await WebAssembly.instantiate(bytes);
const { add, mul, square } = instance.exports;

// Assert the exported Ruxen functions return the expected results.
const checks = [
  ["add(2, 3)", add(2, 3), 5],
  ["mul(6, 7)", mul(6, 7), 42],
  ["square(9)", square(9), 81],
  ["add(-4, 4)", add(-4, 4), 0],
];

let ok = true;
for (const [label, got, want] of checks) {
  const pass = got === want;
  ok &&= pass;
  console.log(`${pass ? "ok  " : "FAIL"} ${label} = ${got} (want ${want})`);
}

if (!ok) {
  console.error("FAIL: one or more exports returned the wrong result");
  process.exit(1);
}
console.log("all exports correct");
