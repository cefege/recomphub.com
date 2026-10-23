use dioxus::prelude::*;
use recomphub_shared::{MediaRef, media_avif_path};

#[cfg(feature = "desktop")]
use crate::components::SiteUrl;

/// `srcset` value listing every recorded AVIF width. The largest candidate is canonical.
pub fn srcset(filename: &str, widths: &[i32]) -> String {
    let largest = widths.iter().copied().max();
    widths
        .iter()
        .map(|width| {
            let url = media_avif_path(
                filename,
                (*width != largest.unwrap_or(*width)).then_some(*width),
            );
            format!("{url} {width}w")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(feature = "desktop")]
fn native_media_url(site_url: &str, path: String) -> String {
    format!("{site_url}{path}")
}

#[cfg(feature = "desktop")]
fn native_srcset(site_url: &str, filename: &str, widths: &[i32]) -> String {
    let largest = widths.iter().copied().max();
    widths
        .iter()
        .map(|width| {
            let path = media_avif_path(
                filename,
                (*width != largest.unwrap_or(*width)).then_some(*width),
            );
            format!("{site_url}{path} {width}w")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(feature = "desktop")]
fn native_media_paths(site_url: &str, filename: &str, widths: &[i32]) -> (String, String) {
    (
        native_media_url(site_url, media_avif_path(filename, None)),
        native_srcset(site_url, filename, widths),
    )
}

#[cfg(not(feature = "desktop"))]
fn web_media_paths(filename: &str, widths: &[i32]) -> (String, String) {
    (media_avif_path(filename, None), srcset(filename, widths))
}

#[component]
pub fn Picture(
    media: Option<MediaRef>,
    alt: String,
    initials: String,
    sizes: String,
    #[props(default)] eager: bool,
    #[props(default)] class: String,
) -> Element {
    #[cfg(feature = "desktop")]
    let site_url = use_context::<SiteUrl>().0;

    match media.filter(|m| m.ready && !m.widths.is_empty()) {
        Some(m) => {
            let Some(filename) = m.filename.as_deref() else {
                return rsx! {
                    div {
                        class: "{class} flex items-center justify-center bg-muted text-lg font-medium text-muted-foreground select-none",
                        role: "img",
                        "aria-label": "{alt}",
                        "{initials}"
                    }
                };
            };
            #[cfg(feature = "desktop")]
            let (src, set) = native_media_paths(&site_url, filename, &m.widths);
            #[cfg(not(feature = "desktop"))]
            let (src, set) = web_media_paths(filename, &m.widths);
            let (w, h) = intrinsic_size(&m).unwrap_or((480, 270));
            let loading = if eager { "eager" } else { "lazy" };
            rsx! {
                img {
                    class: "{class}",
                    src: "{src}",
                    srcset: "{set}",
                    sizes: "{sizes}",
                    width: "{w}",
                    height: "{h}",
                    alt: "{alt}",
                    loading: "{loading}",
                    decoding: "async",
                }
            }
        }
        None => rsx! {
            div {
                class: "{class} flex items-center justify-center bg-muted text-lg font-medium text-muted-foreground select-none",
                role: "img",
                "aria-label": "{alt}",
                "{initials}"
            }
        },
    }
}

/// Explicit `(width, height)` for layout, taken from the source aspect ratio at the largest width.
pub fn intrinsic_size(media: &MediaRef) -> Option<(i32, i32)> {
    let largest = media.widths.iter().copied().max()?;
    let (sw, sh) = (media.width?, media.height?);
    if sw <= 0 {
        return None;
    }
    let h = (largest as i64 * sh as i64 / sw as i64) as i32;
    Some((largest, h))
}
