// kadraj — the HTML-aware image optimizer. Point it at a built site and it
// derives responsive AVIF variants for every image the pages actually use,
// then rewrites the markup to serve them. Inputs are never modified or
// removed; everything kadraj does is additive, so it is safe to re-run.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

mod og;

use anyhow::{Context, Result};
use fast_image_resize::images::Image;
use fast_image_resize::{PixelType, Resizer};
use rayon::prelude::*;
use walkdir::WalkDir;

struct Options {
    root: PathBuf,
    widths: Vec<u32>,
    quality: f32,
    /// Survives between builds. Build tools wipe their output directory on
    /// every run, which would throw the derivatives away and make every
    /// publish re-encode the world; a cache outside the output keeps an
    /// unchanged image an unchanged file copy.
    cache: Option<PathBuf>,
    /// Where to write what a sharing card would need, per page. Nothing is
    /// rendered here: kadraj knows what a page shows, not what a brand's
    /// card should look like.
    og_manifest: Option<PathBuf>,
    /// Whether to give a page the sharing image the manifest found for it.
    /// Off by default: a CMS usually knows better than the page does.
    og_write: bool,
    /// The site's own address, needed because a sharing image must be an
    /// absolute URL — crawlers do not resolve relative ones.
    site_url: Option<String>,
}

struct Derived {
    width: u32,
    height: u32,
    /// srcset entries: (site path, width)
    sources: Vec<(String, u32)>,
}

fn parse_args() -> Result<Options> {
    let mut root = None;
    let mut widths = vec![400, 800, 1200, 1600];
    let mut quality = 50.0;
    let mut cache = None;
    let mut og_manifest = None;
    let mut og_write = false;
    let mut site_url = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--quality" => quality = args.next().context("--quality needs a number")?.parse()?,
            "--cache" => cache = Some(PathBuf::from(args.next().context("--cache needs a path")?)),
            "--og-write" => og_write = true,
            "--site-url" => {
                site_url = Some(
                    args.next()
                        .context("--site-url needs an address like https://example.com")?
                        .trim_end_matches('/')
                        .to_string(),
                )
            }
            "--og-manifest" => {
                og_manifest = Some(PathBuf::from(
                    args.next().context("--og-manifest needs a path")?,
                ))
            }
            "--widths" => {
                widths = args
                    .next()
                    .context("--widths needs a list like 400,800,1600")?
                    .split(',')
                    .map(|w| w.trim().parse())
                    .collect::<Result<_, _>>()?;
                widths.sort_unstable();
            }
            "--help" | "-h" => {
                println!(
                    "kadraj <dist-dir> [--quality 50] [--widths 400,800,1200,1600] [--cache <dir>]\n\
                     Derives responsive AVIF variants for the images your built\n\
                     HTML uses and rewrites the markup to serve them. --cache\n\
                     keeps derivatives across builds whose output is wiped.\n\
                     --og-manifest writes, as json, what each page would need\n\
                     on a sharing card: its title, description and biggest image.\n\
                     --og-write --site-url <url> also gives that image to the\n\
                     pages that name none of their own."
                );
                std::process::exit(0);
            }
            other if root.is_none() && !other.starts_with('-') => root = Some(PathBuf::from(other)),
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    if let Some(dir) = &cache {
        std::fs::create_dir_all(dir).context("could not create the cache directory")?;
    }
    Ok(Options {
        root: root.context("usage: kadraj <dist-dir>")?.canonicalize()?,
        widths,
        quality,
        cache,
        og_manifest,
        og_write,
        site_url,
    })
}

fn main() -> Result<()> {
    let opt = parse_args()?;

    let htmls: Vec<PathBuf> = WalkDir::new(&opt.root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "html"))
        .map(|e| e.into_path())
        .collect();

    // Pass 1: read every page once and take everything from it. The
    // pictures it wants derived — except ones a <picture> already feeds or
    // that carry their own srcset, where the page made its own
    // arrangements — and, when asked, what a sharing card would need.
    let wants_cards = opt.og_manifest.is_some() || opt.og_write;
    let surveyed: Vec<(Vec<PathBuf>, Option<og::Card>)> = htmls
        .par_iter()
        .map(|page| {
            let Ok(html) = std::fs::read_to_string(page) else {
                return (Vec::new(), None);
            };
            let images = collect_imgs(&html, page, &opt.root).unwrap_or_default();
            let card = wants_cards.then(|| og::read(&html));
            (images, card)
        })
        .collect();

    let mut wanted: Vec<PathBuf> = surveyed
        .iter()
        .flat_map(|(images, _)| images.iter().cloned())
        .collect();
    wanted.sort();
    wanted.dedup();

    // Pass 2: derivatives. A derivative's name carries a hash of the source
    // bytes, so an existing file is always current and never re-encoded.
    let encoded = AtomicU64::new(0);
    let derived: Mutex<HashMap<PathBuf, Derived>> = Mutex::new(HashMap::new());
    wanted
        .par_iter()
        .for_each(|src| match derive(src, &opt, &encoded) {
            Ok(d) => {
                derived.lock().unwrap().insert(src.clone(), d);
            }
            Err(e) => eprintln!("skipped {}: {e}", src.display()),
        });
    let derived = derived.into_inner().unwrap();

    // Pass 3: markup. <img> becomes <picture><source avif srcset><img>.
    let rewritten: usize = htmls
        .par_iter()
        .map(|h| {
            rewrite(h, &opt.root, &derived)
                .map(usize::from)
                .unwrap_or(0)
        })
        .sum();

    if wants_cards {
        let cards = decide_cards(&htmls, surveyed, &opt, &derived);
        if let Some(path) = &opt.og_manifest {
            std::fs::write(path, serde_json::to_string_pretty(&cards)?)?;
        }
        println!("{} pages described for sharing", cards.len());
        if opt.og_write {
            let given = give_sharing_images(&cards, &opt)?;
            println!("{given} pages given a sharing image");
        }
    }

    println!(
        "{} images, {} encodes, {} pages rewritten",
        derived.len(),
        encoded.load(Ordering::Relaxed),
        rewritten,
    );
    Ok(())
}

/// What every page would need on a sharing card, in the order the pages
/// were found — shallow first, so a truncated list keeps the front pages.
///
/// Nothing is read here: pass 1 already has every page's facts. What is
/// left is the decision that needs the whole site at once — a picture on
/// most pages is the template's, not the page's.
fn decide_cards(
    htmls: &[PathBuf],
    surveyed: Vec<(Vec<PathBuf>, Option<og::Card>)>,
    opt: &Options,
    derived: &HashMap<PathBuf, Derived>,
) -> Vec<og::Card> {
    let mut read: Vec<(PathBuf, og::Card)> = htmls
        .iter()
        .cloned()
        .zip(surveyed)
        .filter_map(|(page, (_, card))| card.map(|card| (page, card)))
        .collect();

    let mut seen: HashMap<String, usize> = HashMap::new();
    for (_, card) in &read {
        for src in card.candidates() {
            *seen.entry(src).or_default() += 1;
        }
    }
    let everywhere: std::collections::HashSet<String> = if read.len() >= 8 {
        let most = read.len() / 3;
        seen.into_iter()
            .filter(|(_, count)| *count > most)
            .map(|(src, _)| src)
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut cards: Vec<og::Card> = read
        .par_iter_mut()
        .filter_map(|(page, card)| {
            // The sizes are already known from the derivative pass; only an
            // image nobody derived costs a read here.
            card.choose(page, &opt.root, &everywhere, |file| {
                derived
                    .get(file)
                    .map(|d| (d.width, d.height))
                    .or_else(|| image::image_dimensions(file).ok())
            });
            card.path = url_path(page.strip_prefix(&opt.root).ok()?);
            Some(card.clone())
        })
        .collect();
    cards.sort_by(|a, b| {
        (a.path.matches('/').count(), &a.path).cmp(&(b.path.matches('/').count(), &b.path))
    });
    cards
}

/// Writes og:image (and twitter:image) into the pages that named none.
///
/// Only where the manifest found something: a page with no picture worth
/// sharing is left as it is, for a CMS or a card renderer to answer.
fn give_sharing_images(cards: &[og::Card], opt: &Options) -> Result<usize> {
    let Some(site) = &opt.site_url else {
        eprintln!("--og-write needs --site-url; nothing written");
        return Ok(0);
    };
    let done: usize = cards
        .par_iter()
        .filter(|card| !card.has_og_image && card.image.is_some())
        .filter_map(|card| {
            let image = card.image.as_ref()?;
            let absolute = if image.remote {
                image.src.clone()
            } else {
                format!("{site}{}", image.src)
            };
            let file = page_file(&card.path, &opt.root);
            let html = std::fs::read_to_string(&file).ok()?;
            let at = html.find("</head>")?;
            let tags = format!(
                "<meta property=\"og:image\" content=\"{0}\">\
                 <meta name=\"twitter:image\" content=\"{0}\">\
                 <meta name=\"twitter:card\" content=\"summary_large_image\">",
                escape(&absolute)
            );
            let mut page = String::with_capacity(html.len() + tags.len());
            page.push_str(&html[..at]);
            page.push_str(&tags);
            page.push_str(&html[at..]);
            std::fs::write(&file, page).ok()?;
            Some(1)
        })
        .sum();
    Ok(done)
}

fn escape(url: &str) -> String {
    url.replace('&', "&amp;").replace('"', "&quot;")
}

/// The file behind a page's address — `url_path` in reverse.
fn page_file(path: &str, root: &Path) -> PathBuf {
    if path == "/" {
        root.join("index.html")
    } else if let Some(dir) = path.strip_suffix('/') {
        root.join(dir.trim_start_matches('/')).join("index.html")
    } else {
        root.join(format!("{}.html", path.trim_start_matches('/')))
    }
}

/// `blog/index.html` is the page at `/blog/`; `about.html` is `/about`.
fn url_path(relative: &Path) -> String {
    let file = relative.to_string_lossy().replace('\\', "/");
    if file == "index.html" {
        "/".to_string()
    } else if let Some(dir) = file.strip_suffix("/index.html") {
        format!("/{dir}/")
    } else {
        format!("/{}", file.strip_suffix(".html").unwrap_or(&file))
    }
}

fn is_raster(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp"
        )
    })
}

/// Turns a page's src into a file on disk; anything outside the site root,
/// remote, or non-raster resolves to nothing.
pub(crate) fn resolve(src: &str, page: &Path, root: &Path) -> Option<PathBuf> {
    if src.contains("://") || src.starts_with("data:") || src.starts_with("//") {
        return None;
    }
    let clean = src.split(['?', '#']).next()?;
    let path = if let Some(rest) = clean.strip_prefix('/') {
        root.join(rest)
    } else {
        page.parent()?.join(clean)
    };
    let path = path.canonicalize().ok()?;
    (path.starts_with(root) && is_raster(&path) && path.is_file()).then_some(path)
}

/// The images this page wants derived: local rasters from plain <img> tags.
fn collect_imgs(html: &str, page: &Path, root: &Path) -> Result<Vec<PathBuf>> {
    let found = Mutex::new(Vec::new());
    let skip = Mutex::new(HashSet::new());
    let mut rewriter = lol_html::HtmlRewriter::new(
        lol_html::Settings::new()
            .append_element_content_handler(lol_html::element!("picture img[src]", |el| {
                if let Some(src) = el.get_attribute("src") {
                    skip.lock().unwrap().insert(src);
                }
                Ok(())
            }))
            .append_element_content_handler(lol_html::element!("img[src]", |el| {
                if el.get_attribute("srcset").is_some() {
                    return Ok(());
                }
                if let Some(src) = el.get_attribute("src")
                    && !skip.lock().unwrap().contains(&src)
                    && let Some(p) = resolve(&src, page, root)
                {
                    found.lock().unwrap().push(p);
                }
                Ok(())
            })),
        |_: &[u8]| {},
    );
    rewriter.write(html.as_bytes())?;
    rewriter.end()?;
    Ok(found.into_inner().unwrap())
}

fn derive(src: &Path, opt: &Options, encoded: &AtomicU64) -> Result<Derived> {
    let bytes = std::fs::read(src)?;
    let hash = xxhash_rust::xxh3::xxh3_64(&bytes);
    let img = image::load_from_memory(&bytes).context("could not decode")?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());

    let cap = *opt.widths.last().unwrap_or(&1600);
    let mut widths: Vec<u32> = opt.widths.iter().copied().filter(|&x| x < w).collect();
    widths.push(w.min(cap));
    widths.dedup();

    let stem = src.file_stem().unwrap_or_default().to_string_lossy();
    let mut sources = Vec::new();
    for tw in widths {
        let th = (u64::from(h) * u64::from(tw) / u64::from(w)).max(1) as u32;
        let out = src.with_file_name(format!("{stem}.{hash:08x}.{tw}w.avif"));
        if !out.exists() {
            let cached = opt
                .cache
                .as_ref()
                .map(|dir| dir.join(format!("{hash:016x}.{tw}w.q{}.avif", opt.quality)));
            if let Some(hit) = cached.as_ref().filter(|c| c.is_file()) {
                std::fs::copy(hit, &out)?;
            } else {
                let resized = resize(&rgba, w, h, tw, th)?;
                let avif = ravif::Encoder::new()
                    .with_quality(opt.quality)
                    .with_speed(4)
                    .encode_rgba(ravif::Img::new(
                        rgb::FromSlice::as_rgba(resized.as_slice()),
                        tw as usize,
                        th as usize,
                    ))?;
                std::fs::write(&out, avif.avif_file)?;
                encoded.fetch_add(1, Ordering::Relaxed);
                if let Some(keep) = cached {
                    std::fs::copy(&out, keep).ok();
                }
            }
        }
        let site = format!("/{}", out.strip_prefix(&opt.root)?.to_string_lossy());
        sources.push((site, tw));
    }
    Ok(Derived {
        width: w,
        height: h,
        sources,
    })
}

fn resize(rgba: &image::RgbaImage, w: u32, h: u32, tw: u32, th: u32) -> Result<Vec<u8>> {
    if tw == w {
        return Ok(rgba.as_raw().clone());
    }
    let src = Image::from_vec_u8(w, h, rgba.as_raw().clone(), PixelType::U8x4)?;
    let mut dst = Image::new(tw, th, PixelType::U8x4);
    Resizer::new().resize(&src, &mut dst, None)?;
    Ok(dst.into_vec())
}

fn rewrite(page: &Path, root: &Path, derived: &HashMap<PathBuf, Derived>) -> Result<bool> {
    let html = std::fs::read_to_string(page)?;
    let mut out = Vec::with_capacity(html.len());
    // The first image on a page is usually its LCP: eager and prioritized,
    // everything after it lazy.
    let seen = std::cell::Cell::new(0u32);
    let in_picture = std::cell::Cell::new(false);
    let mut rewriter = lol_html::HtmlRewriter::new(
        lol_html::Settings::new()
            .append_element_content_handler(lol_html::element!("picture img[src]", |_el| {
                in_picture.set(true);
                Ok(())
            }))
            .append_element_content_handler(lol_html::element!("img[src]", |el| {
                if in_picture.replace(false) || el.get_attribute("srcset").is_some() {
                    return Ok(());
                }
                let Some(src) = el.get_attribute("src") else { return Ok(()) };
                let Some(path) = resolve(&src, page, root) else { return Ok(()) };
                let Some(d) = derived.get(&path) else { return Ok(()) };

                let srcset = d
                    .sources
                    .iter()
                    .map(|(u, w)| format!("{u} {w}w"))
                    .collect::<Vec<_>>()
                    .join(", ");
                el.before(
                    &format!(
                        r#"<picture><source type="image/avif" srcset="{srcset}" sizes="(max-width: 800px) 100vw, 800px">"#
                    ),
                    lol_html::html_content::ContentType::Html,
                );
                el.after("</picture>", lol_html::html_content::ContentType::Html);
                if el.get_attribute("width").is_none() {
                    el.set_attribute("width", &d.width.to_string()).ok();
                    el.set_attribute("height", &d.height.to_string()).ok();
                }
                let first = seen.get() == 0;
                seen.set(seen.get() + 1);
                if el.get_attribute("loading").is_none() {
                    el.set_attribute("loading", if first { "eager" } else { "lazy" }).ok();
                }
                if first {
                    el.set_attribute("fetchpriority", "high").ok();
                }
                Ok(())
            })),
        |c: &[u8]| out.extend_from_slice(c),
    );
    rewriter.write(html.as_bytes())?;
    rewriter.end()?;

    let changed = out != html.as_bytes();
    if changed {
        std::fs::write(page, out)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_stays_inside_the_root() {
        let dir = std::env::temp_dir().join("kadraj-test-resolve");
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img/a.png"), b"x").unwrap();
        let root = dir.canonicalize().unwrap();
        let page = root.join("index.html");

        assert!(resolve("/img/a.png", &page, &root).is_some());
        assert!(resolve("img/a.png", &page, &root).is_some());
        assert!(resolve("https://cdn.example/a.png", &page, &root).is_none());
        assert!(resolve("data:image/png;base64,xx", &page, &root).is_none());
        assert!(resolve("/../secret.png", &page, &root).is_none());
        assert!(resolve("/img/a.svg", &page, &root).is_none());
    }

    #[test]
    fn end_to_end_derives_and_rewrites() {
        let dir = std::env::temp_dir().join("kadraj-test-e2e");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("img")).unwrap();
        let px = image::RgbaImage::from_pixel(900, 600, image::Rgba([120, 90, 60, 255]));
        px.save(dir.join("img/photo.png")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            r#"<html><body><img src="/img/photo.png" alt="p"><picture><source srcset="x.avif"><img src="/img/photo.png"></picture></body></html>"#,
        )
        .unwrap();

        let root = dir.canonicalize().unwrap();
        let opt = Options {
            root: root.clone(),
            widths: vec![400, 800],
            quality: 50.0,
            cache: None,
            og_manifest: None,
            og_write: false,
            site_url: None,
        };
        let page = root.join("index.html");
        let html = std::fs::read_to_string(&page).unwrap();
        let wanted = collect_imgs(&html, &page, &root).unwrap();
        assert_eq!(wanted.len(), 1, "the picture-fed img must not be collected");

        let n = AtomicU64::new(0);
        let d = derive(&wanted[0], &opt, &n).unwrap();
        assert_eq!(d.width, 900);
        assert_eq!(d.sources.len(), 2); // 400 and 800 — the largest configured width caps it

        let mut map = HashMap::new();
        map.insert(wanted[0].clone(), d);
        assert!(rewrite(&root.join("index.html"), &root, &map).unwrap());
        let html = std::fs::read_to_string(root.join("index.html")).unwrap();
        assert!(html.contains(r#"<source type="image/avif""#));
        assert!(html.contains(r#"fetchpriority="high""#));
        assert!(html.contains(r#"width="900""#));
        // The already-arranged picture stayed untouched: still exactly one
        // avif source added.
        assert_eq!(html.matches("image/avif").count(), 1);
    }

    #[test]
    fn an_address_and_its_file_agree_both_ways() {
        let root = std::path::Path::new("/site");
        for file in ["index.html", "about.html", "blog/index.html"] {
            let path = url_path(std::path::Path::new(file));
            assert_eq!(page_file(&path, root), root.join(file), "{path}");
        }
    }

    #[test]
    fn the_cache_survives_a_wiped_output() {
        let dir = std::env::temp_dir().join("kadraj-test-cache");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("img")).unwrap();
        let px = image::RgbaImage::from_pixel(600, 400, image::Rgba([10, 200, 90, 255]));
        px.save(dir.join("img/a.png")).unwrap();

        let root = dir.canonicalize().unwrap();
        let opt = Options {
            root: root.clone(),
            widths: vec![400],
            quality: 50.0,
            cache: Some(root.join("cache")),
            og_manifest: None,
            og_write: false,
            site_url: None,
        };
        std::fs::create_dir_all(root.join("cache")).unwrap();
        let src = root.join("img/a.png");

        let n = AtomicU64::new(0);
        derive(&src, &opt, &n).unwrap();
        assert_eq!(n.load(Ordering::Relaxed), 1); // one 400w — the only configured width

        // The build tool wipes its output; the derivatives vanish with it.
        for e in std::fs::read_dir(root.join("img")).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "avif") {
                std::fs::remove_file(p).unwrap();
            }
        }

        derive(&src, &opt, &n).unwrap();
        assert_eq!(
            n.load(Ordering::Relaxed),
            1,
            "the second build must copy, not encode"
        );
    }
}
