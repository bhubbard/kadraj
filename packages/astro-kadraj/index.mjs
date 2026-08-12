import { fileURLToPath } from "node:url"

import { spawnSync } from "node:child_process"

/** Runs kadraj on the finished site after `astro build`. */
export default function kadraj(options = {}) {
  let site
  return {
    name: "kadraj",
    hooks: {
      // The site's address is in Astro's own config; a sharing image has
      // to be absolute, and asking for it twice is how the two disagree.
      "astro:config:done": ({ config }) => {
        site = config.site ? String(config.site).replace(/\/$/, "") : undefined
      },
      "astro:build:done": ({ dir }) => {
        runKadraj(fileURLToPath(dir), { siteUrl: site, ...options })
      },
    },
  }
}

function runKadraj(dir, options = {}) {
  const bin = options.binary ?? process.env.KADRAJ ?? "kadraj"
  const args = [dir]
  if (options.quality !== undefined) args.push("--quality", String(options.quality))
  if (options.widths !== undefined) args.push("--widths", options.widths.join(","))
  if (options.ogManifest) args.push("--og-manifest", options.ogManifest)
  if (options.siteUrl) args.push("--site-url", options.siteUrl)
  if (options.og) args.push("--og-write")
  const r = spawnSync(bin, args, { stdio: "inherit" })
  if (r.error && r.error.code === "ENOENT")
    throw new Error(
      "kadraj binary not found — `cargo install kadraj`, or point the KADRAJ env var or the plugin's `binary` option at it",
    )
  if (r.status !== 0) throw new Error(`kadraj exited with status ${r.status}`)
}
