import { resolve } from "node:path"

import { spawnSync } from "node:child_process"

/** Runs kadraj on the build output after Vite finishes writing it. */
export default function kadraj(options = {}) {
  let outDir = "dist"
  let rootDir = process.cwd()
  return {
    name: "kadraj",
    apply: "build",
    configResolved(config) {
      outDir = config.build.outDir
      rootDir = config.root
    },
    closeBundle() {
      runKadraj(resolve(rootDir, outDir), options)
    },
  }
}

function runKadraj(dir, options = {}) {
  const bin = options.binary ?? process.env.KADRAJ ?? "kadraj"
  const args = [dir]
  if (options.quality !== undefined) args.push("--quality", String(options.quality))
  if (options.widths !== undefined) args.push("--widths", options.widths.join(","))
  const r = spawnSync(bin, args, { stdio: "inherit" })
  if (r.error && r.error.code === "ENOENT")
    throw new Error(
      "kadraj binary not found — `cargo install kadraj`, or point the KADRAJ env var or the plugin's `binary` option at it",
    )
  if (r.status !== 0) throw new Error(`kadraj exited with status ${r.status}`)
}
