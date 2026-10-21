use dioxus::prelude::*;
use recomphub_shared::{MediaRef, media_avif_path};

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
            let src = media_avif_path(filename, None);
            let set = srcset(filename, &m.widths);
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
