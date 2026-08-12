# kadraj

The HTML-aware image optimizer. Point it at a built static site and it:

1. finds every image the pages actually reference,
2. derives responsive AVIF variants (400/800/1200/1600px by default),
3. rewrites the markup: `<img>` becomes `<picture>` with an AVIF `srcset`,
   real `width`/`height` (no layout shift), `loading="lazy"` below the fold
   and `fetchpriority="high"` on the first image of each page.

Originals are never modified or deleted — everything kadraj does is
additive, and running it twice is a no-op. *Kadraj* is Turkish for
"framing", as in framing a shot properly.

```sh
kadraj ./dist
```

## Why

Framework image components (`astro:assets`, `next/image`) only see images
the developer imports. Images that arrive through CMS content, markdown, or
plain `<img>` tags ship at full size — a 2 MB PNG in a blog post stays a
2 MB PNG. kadraj works on the finished HTML, so it catches everything,
whatever framework produced it. Images already wrapped in `<picture>` or
carrying a `srcset` are left alone.

It is fast because it is Rust: [lol_html](https://github.com/cloudflare/lol-html)
(Cloudflare's streaming HTML rewriter) for the markup,
[ravif](https://github.com/kornelski/cavif-rs) (rav1e) for the encoding,
all cores in parallel. A 123 MB site with 1,900 pages takes ~14 seconds
cold and ~0.1 seconds when nothing changed.

## Install

```sh
cargo install kadraj
# or build from source
git clone https://github.com/productdevbook/kadraj && cd kadraj
cargo build --release   # needs nasm for rav1e's assembly
```

## Usage

```sh
kadraj <dist-dir> [--quality 50] [--widths 400,800,1200,1600] [--cache .kadraj-cache]
```

| Flag | Default | Meaning |
|---|---|---|
| `--quality` | `50` | AVIF quality, 1–100 |
| `--widths` | `400,800,1200,1600` | derivative widths; the largest also caps oversized sources |
| `--cache` | off | a directory that outlives the build. Build tools wipe their output on every run; with a cache, an unchanged image costs a file copy instead of an encode |
| `--og-manifest <file>` | off | writes, as JSON, what each page would need on a sharing card: title, description, site name, and the page's own biggest image |
| `--og-write` | off | also gives that image to pages naming none (`og:image`, `twitter:image`); needs `--site-url` |
| `--site-url <url>` | — | the site's address, because a sharing image must be absolute |

Derivative files land next to their sources, named
`photo.<hash>.<width>w.avif`. The hash covers the source bytes: unchanged
images are never re-encoded, changed images get new names — so derivatives
can be served with `Cache-Control: immutable`.

## Sharing cards

Social crawlers do not run scripts: they read four things out of the HTML
and give up. `--og-manifest` collects exactly those, for every page:

```json
[
  { "path": "/blog/kraft-bags/", "title": "Kraft bags — sizes and printing",
    "description": "How to choose…", "site": "MRT",
    "has_og_image": false,
    "image": { "src": "/img/bags.jpg", "width": 1600, "height": 1000, "remote": false } }
]
```

It also carries through what the page already says for machines: `og:type`,
and any `application/ld+json` verbatim. That is where a developer writes a
product's price, an article's author, an event's date — a better source
than anything guessed from the prose, and the right place to write it.

Two things it will not get wrong. The picture is the **biggest** one, not
the first — the first is usually a logo, and a logo shared as a photograph
tells a reader nothing. And a picture that appears on most of the site's
pages is the template's, not the page's: payment strips, author portraits
and mastheads are dropped, as is anything inside `<header>`, `<footer>`,
`<nav>` or `<aside>`.

Images served from somewhere else — a CDN, a bucket — are offered as they
are, with `remote: true` and no size. Measuring them would mean fetching
them, and a build that downloads the web fails when the web is slow.

With `--og-write` kadraj fills the tags itself for pages that name none. A
CMS that knows more (a cover image, a product price, a brand template) can
read the manifest instead and decide for itself.

## Vite and Astro plugins

Thin wrappers that run kadraj after the build. They spawn the `kadraj`
binary (from `PATH`, the `KADRAJ` environment variable, or the `binary`
option).

```js
// astro.config.mjs
import kadraj from "astro-kadraj"
export default { integrations: [kadraj({ quality: 50 })] }
```

```js
// vite.config.js
import kadraj from "vite-plugin-kadraj"
export default { plugins: [kadraj()] }
```

Both packages live in [`packages/`](packages/).

## What it does not do (yet)

Remote images (CDN/object-storage URLs) are skipped, WebP/JXL output and a
mode that emits image-proxy URLs instead of encoding are planned. See the
issues.

## License

MIT
