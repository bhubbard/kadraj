//! What a page would want on a card when somebody shares it.
//!
//! Social crawlers do not run scripts and do not read a site's database;
//! they read four things out of the HTML and give up. So the pass that
//! already knows every page's images collects those four — the title, the
//! description, the site's name, and the picture worth showing — and
//! writes them down. A card can then be made from them by whoever knows
//! best: a CMS with the product's price, a static site with a template,
//! or nobody at all, in which case the page's own best image is already
//! the honest answer.
//!
//! The best image is the biggest one, not the first one. The first image
//! on a page is usually a logo, and a logo shared as a photograph tells
//! the reader nothing about what they are about to open.

use std::path::Path;

use serde::Serialize;

/// One page, as a sharing card would need it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Card {
    /// The address the page is served at.
    pub path: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub site: String,
    /// Whether the page already names its own sharing image.
    pub has_og_image: bool,
    /// The biggest image the page uses, when it uses one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<CardImage>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CardImage {
    pub src: String,
    /// Zero when the picture lives somewhere this pass cannot measure —
    /// a CDN, a bucket, another host. Its size is unknown, not absent.
    pub width: u32,
    pub height: u32,
    /// Whether it is served from somewhere else.
    pub remote: bool,
}

/// An image small enough to be furniture — a logo, an icon, an avatar —
/// is not what a page is about.
pub const MEANINGFUL_WIDTH: u32 = 600;

/// Reads what a card needs out of one page's HTML.
pub fn read(html: &str) -> Card {
    use std::cell::RefCell;

    let title = RefCell::new(String::new());
    let description = RefCell::new(String::new());
    let site = RefCell::new(String::new());
    let has_og_image = RefCell::new(false);
    let images = RefCell::new(Vec::<String>::new());
    let furniture = RefCell::new(Vec::<String>::new());

    let mut rewriter = lol_html::HtmlRewriter::new(
        lol_html::Settings::new()
            .append_element_content_handler(lol_html::text!("title", |t| {
                title.borrow_mut().push_str(t.as_str());
                Ok(())
            }))
            .append_element_content_handler(lol_html::element!("meta", |el| {
                let key = el
                    .get_attribute("property")
                    .or_else(|| el.get_attribute("name"))
                    .unwrap_or_default();
                let value = el.get_attribute("content").unwrap_or_default();
                match key.as_str() {
                    "og:image" | "twitter:image" if !value.trim().is_empty() => {
                        *has_og_image.borrow_mut() = true
                    }
                    "og:description" | "description" if description.borrow().is_empty() => {
                        *description.borrow_mut() = value
                    }
                    "og:site_name" if site.borrow().is_empty() => *site.borrow_mut() = value,
                    _ => {}
                }
                Ok(())
            }))
            // A picture in the header, the footer or the navigation belongs
            // to the site, not to the page: a payment-methods strip is wider
            // than most photographs and says nothing about what is here.
            .append_element_content_handler(lol_html::element!(
                "header img[src], footer img[src], nav img[src], aside img[src]",
                |el| {
                    if let Some(src) = el.get_attribute("src") {
                        furniture.borrow_mut().push(src);
                    }
                    Ok(())
                }
            ))
            .append_element_content_handler(lol_html::element!("img[src]", |el| {
                if let Some(src) = el.get_attribute("src") {
                    images.borrow_mut().push(src);
                }
                Ok(())
            })),
        |_: &[u8]| {},
    );
    // end() consumes the rewriter, which is what releases the handlers'
    // borrow of the cells below.
    let ok = rewriter.write(html.as_bytes()).is_ok() && rewriter.end().is_ok();
    if !ok {
        return Card {
            path: String::new(),
            title: String::new(),
            description: String::new(),
            site: String::new(),
            has_og_image: false,
            image: None,
        };
    }

    // Read, not moved: the handlers still hold a shared borrow of every
    // cell, and a shared borrow beside a read is no conflict.
    let mut description = description.borrow().trim().to_string();
    description.truncate(300);
    Card {
        path: String::new(),
        title: title.borrow().trim().to_string(),
        description,
        site: site.borrow().trim().to_string(),
        has_og_image: *has_og_image.borrow(),
        image: None,
    }
    .with_images(images.borrow().clone(), &furniture.borrow())
}

impl Card {
    /// Carries the page's image list until the caller, who knows each
    /// image's size, can pick the biggest.
    fn with_images(mut self, srcs: Vec<String>, furniture: &[String]) -> Self {
        let kept: Vec<String> = srcs
            .into_iter()
            .filter(|src| !furniture.contains(src))
            .collect();
        // Stored in the src field of a placeholder; `choose` replaces it.
        self.image = (!kept.is_empty()).then(|| CardImage {
            src: kept.join("\n"),
            width: 0,
            height: 0,
            remote: false,
        });
        self
    }

    /// The candidates this page still has, before one is chosen — what a
    /// caller counts to find the pictures that belong to every page.
    pub fn candidates(&self) -> Vec<String> {
        self.image
            .as_ref()
            .map(|c| c.src.split('\n').map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Replaces the candidate list with the biggest one that is big enough
    /// to be worth sharing. `sized` answers what a source is, in pixels.
    pub fn choose(
        &mut self,
        page: &Path,
        root: &Path,
        everywhere: &std::collections::HashSet<String>,
        sized: impl Fn(&Path) -> Option<(u32, u32)>,
    ) {
        let Some(candidates) = self.image.take() else {
            return;
        };
        let mut best: Option<(CardImage, u64)> = None;
        let mut first_remote: Option<CardImage> = None;
        for src in candidates.src.split('\n') {
            if everywhere.contains(src) {
                continue;
            }
            match crate::resolve(src, page, root) {
                Some(file) => {
                    let Some((w, h)) = sized(&file) else { continue };
                    if w < MEANINGFUL_WIDTH {
                        continue;
                    }
                    let area = u64::from(w) * u64::from(h);
                    if best.as_ref().is_none_or(|(_, seen)| area > *seen) {
                        best = Some((
                            CardImage {
                                src: src.to_string(),
                                width: w,
                                height: h,
                                remote: false,
                            },
                            area,
                        ));
                    }
                }
                // Somewhere else: a bucket, a CDN, another site. Measuring it
                // would mean fetching it, and a build that downloads the web
                // is a build that fails when the web is slow. It is offered
                // as it is, for whoever knows more than this pass does.
                None if src.starts_with("http") && first_remote.is_none() => {
                    first_remote = Some(CardImage {
                        src: src.to_string(),
                        width: 0,
                        height: 0,
                        remote: true,
                    });
                }
                None => {}
            }
        }
        self.image = best.map(|(image, _)| image).or(first_remote);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head>
        <title>Zeytinli Ekmek</title>
        <meta name="description" content="Tok tutan bir ekmek.">
        <meta property="og:site_name" content="Bir Mutfak">
        </head><body>
        <img src="/img/logo.png"><img src="/img/loaf.jpg">
        </body></html>"#;

    #[test]
    fn a_page_says_what_a_card_needs() {
        let card = read(PAGE);
        assert_eq!(card.title, "Zeytinli Ekmek");
        assert_eq!(card.description, "Tok tutan bir ekmek.");
        assert_eq!(card.site, "Bir Mutfak");
        assert!(!card.has_og_image);
    }

    #[test]
    fn a_page_that_named_its_own_image_is_left_alone() {
        let html = r#"<html><head><title>x</title>
            <meta property="og:image" content="https://site.example/a.png"></head><body></body></html>"#;
        assert!(read(html).has_og_image);
    }

    #[test]
    fn the_biggest_picture_wins_and_furniture_is_ignored() {
        let dir = std::env::temp_dir().join("kadraj-og-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("img")).unwrap();
        image::RgbaImage::from_pixel(200, 200, image::Rgba([1, 2, 3, 255]))
            .save(dir.join("img/logo.png"))
            .unwrap();
        image::RgbImage::from_pixel(1400, 900, image::Rgb([9, 8, 7]))
            .save(dir.join("img/loaf.jpg"))
            .unwrap();
        let root = dir.canonicalize().unwrap();

        let mut card = read(PAGE);
        card.choose(
            &root.join("index.html"),
            &root,
            &std::collections::HashSet::new(),
            |p| image::image_dimensions(p).ok(),
        );
        let picked = card.image.expect("a picture worth sharing");
        assert_eq!(picked.src, "/img/loaf.jpg");
        assert_eq!(picked.width, 1400);
    }

    #[test]
    fn a_picture_on_every_page_belongs_to_the_template() {
        let dir = std::env::temp_dir().join("kadraj-og-furniture");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("img")).unwrap();
        image::RgbImage::from_pixel(1920, 531, image::Rgb([1, 1, 1]))
            .save(dir.join("img/payments.jpg"))
            .unwrap();
        image::RgbImage::from_pixel(900, 900, image::Rgb([2, 2, 2]))
            .save(dir.join("img/dish.jpg"))
            .unwrap();
        let root = dir.canonicalize().unwrap();

        let html = r#"<html><head><title>t</title></head><body>
            <img src="/img/payments.jpg"><img src="/img/dish.jpg"></body></html>"#;
        let mut card = read(html);
        let everywhere = std::collections::HashSet::from(["/img/payments.jpg".to_string()]);
        card.choose(&root.join("index.html"), &root, &everywhere, |p| {
            image::image_dimensions(p).ok()
        });
        // The strip is wider, but it is on every page; the dish is this page's.
        assert_eq!(card.image.unwrap().src, "/img/dish.jpg");
    }

    #[test]
    fn a_footer_picture_is_never_a_candidate() {
        let html = r#"<html><head><title>t</title></head><body>
            <footer><img src="/img/payments.jpg"></footer></body></html>"#;
        assert!(read(html).candidates().is_empty());
    }

    #[test]
    fn a_page_of_only_small_images_offers_none() {
        let dir = std::env::temp_dir().join("kadraj-og-small");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("img")).unwrap();
        image::RgbaImage::from_pixel(200, 200, image::Rgba([1, 2, 3, 255]))
            .save(dir.join("img/logo.png"))
            .unwrap();
        let root = dir.canonicalize().unwrap();

        let html =
            r#"<html><head><title>t</title></head><body><img src="/img/logo.png"></body></html>"#;
        let mut card = read(html);
        card.choose(
            &root.join("index.html"),
            &root,
            &std::collections::HashSet::new(),
            |p| image::image_dimensions(p).ok(),
        );
        assert!(card.image.is_none());
    }
}
