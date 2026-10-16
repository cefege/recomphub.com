use dioxus::prelude::*;
use recomphub_shared::MediaRef;

/// URL of one AVIF variant under `/media`, as written by the worker's media pipeline.
pub fn avif_url(media: &MediaRef, width: i32) -> String {
    format!("/media/{}/{}-{}.avif", media.kind, media.hash, width)
}

/// `srcset` value listing every ready AVIF width.
pub fn srcset(media: &MediaRef) -> String {
    media
        .widths
        .iter()
        .map(|w| format!("{} {}w", avif_url(media, *w), w))
        .collect::<Vec<_>>()
        .join(", ")
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

/// AVIF image with `srcset`, `sizes`, explicit dimensions and lazy loading.
/// Media that is not ready renders a neutral placeholder with the initials.
#[component]
pub fn Picture(
    media: Option<MediaRef>,
    alt: String,
    initials: String,
    sizes: String,
    #[props(default)] eager: bool,
    #[props(default)] class: String,
) -> Element {
    match media.filter(|m| m.ready && !m.widths.is_empty()) {
        Some(m) => {
            let src = avif_url(&m, *m.widths.iter().max().unwrap_or(&480));
            let set = srcset(&m);
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
