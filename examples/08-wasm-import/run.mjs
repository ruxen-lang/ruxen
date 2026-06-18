// Host loader for the wasm host-import example (tier 4.09). Unlike the other
// examples, this module IMPORTS a function from the host — we supply it in the
// import object under the module name the lib block declared ("env"). i64 args
// arrive as BigInt. Usage: node run.mjs [path-to-wasm].
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const wasmPath = process.argv[2] ?? join(here, "import.wasm");
const bytes = readFileSync(wasmPath);

if (!WebAssembly.validate(bytes)) {
  console.error(`FAIL: ${wasmPath} is not a valid WebAssembly module`);
  process.exit(1);
}

// The host provides env.host_mul — the import the Ruxen `lib "env"` block declared.
const imports = { env: { host_mul: (a, b) => a * b } };
const { instance } = await WebAssembly.instantiate(bytes, imports);
const got = Number(instance.exports.compute());
const want = 42;
const pass = got === want;
console.log(`${pass ? "ok  " : "FAIL"} compute() = ${got} (want ${want})`);
console.log(pass ? "host import called: 6*7 in JS" : "host import BROKEN");
process.exit(pass ? 0 : 1);
