// Bundles the extension into a single out/extension.js so the published .vsix
// does not need to ship node_modules. `vscode` is provided by the host and is
// always external.
const esbuild = require("esbuild");

const production = process.argv.includes("--production");
const watch = process.argv.includes("--watch");

async function main() {
  const ctx = await esbuild.context({
    entryPoints: ["src/extension.ts"],
    bundle: true,
    format: "cjs",
    platform: "node",
    target: "node18",
    outfile: "out/extension.js",
    external: ["vscode"],
    minify: production,
    sourcemap: !production,
    sourcesContent: false,
    logLevel: "info",
  });

  if (watch) {
    await ctx.watch();
  } else {
    await ctx.rebuild();
    await ctx.dispose();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
