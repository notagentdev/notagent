use std::sync::OnceLock;

use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Container, Line, component_ref};

use crate::config::get_bundled_interactive_asset_path;
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;

const BLOG_URL: &str = "https://mariozechner.at/posts/2026-04-08-ive-sold-out/";
const IMAGE_FILENAME: &str = "clankolas.png";

/// (`src/modes/interactive/assets/clankolas.png`, 539 053 bytes).
/// package directory at runtime; a standalone Rust binary has no package
/// directory, so the same bytes are compiled in. A file of that name next to
/// the binary still wins, which is what an npm-style install would place there.
const BUNDLED_IMAGE: &[u8] = include_bytes!("../../../../assets/clankolas.png");

/// `OnceLock` gives it the same "attempt exactly once" semantics.
fn load_image_base64() -> Option<&'static String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let bytes = std::fs::read(get_bundled_interactive_asset_path(IMAGE_FILENAME))
                .unwrap_or_else(|_| BUNDLED_IMAGE.to_vec());
            Some(base64_encode(&bytes))
        })
        .as_ref()
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The announcement panel.
pub struct EarendilAnnouncementComponent {
    container: Container,
}

impl Default for EarendilAnnouncementComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl EarendilAnnouncementComponent {
    /// New panel.
    pub fn new() -> Self {
        let theme_instance = theme();
        let border = || {
            component_ref(DynamicBorder::new(Some(std::rc::Rc::new(|text: &str| {
                theme().fg(ThemeColor::Accent, text)
            }))))
        };
        let mut container = Container::new();

        container.add_child(border());
        container.add_child(component_ref(Text::new(
            theme_instance
                .bold(&theme_instance.fg(ThemeColor::Accent, "notagent has joined Earendil")),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Muted, "Read the blog post:"),
            1,
            0,
        )));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::MdLink, BLOG_URL),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));

        if let Some(image_base64) = load_image_base64() {
            container.add_child(component_ref(Image::new(
                image_base64.clone(),
                "image/png",
                ImageTheme {
                    fallback_color: std::rc::Rc::new(|text: &str| {
                        theme().fg(ThemeColor::Muted, text)
                    }),
                },
                ImageOptions {
                    max_width_cells: Some(56),
                    filename: Some(IMAGE_FILENAME.to_string()),
                    ..Default::default()
                },
                None,
            )));
            container.add_child(component_ref(Spacer::new(1)));
        }

        container.add_child(border());
        Self { container }
    }
}

impl Component for EarendilAnnouncementComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
