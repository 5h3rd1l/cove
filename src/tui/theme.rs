use ratatui::style::Color;

use crate::config::AgentConfig;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ThemeMode {
    Auto,
    Dark,
    Light,
}

impl ThemeMode {
    pub(super) fn resolve(self) -> Theme {
        let detected_luma = if self == Self::Auto {
            terminal_light::luma().ok()
        } else {
            None
        };
        self.resolve_with_luma(detected_luma)
    }

    fn resolve_with_luma(self, detected_luma: Option<f32>) -> Theme {
        match self {
            Self::Light => Theme::light(),
            Self::Dark => Theme::dark(),
            Self::Auto if detected_luma.is_some_and(|luma| luma > 0.6) => Theme::light(),
            Self::Auto => Theme::dark(),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct Theme {
    pub(super) is_light: bool,
    pub(super) foreground: Color,
    pub(super) accent: Color,
    pub(super) warning: Color,
    pub(super) error: Color,
    pub(super) success: Color,
    pub(super) info: Color,
    pub(super) muted: Color,
    pub(super) secondary: Color,
    pub(super) panel_border: Color,
    pub(super) filter_selected_bg: Color,
    pub(super) selected_fg: Color,
    pub(super) selected_bg: Color,
    pub(super) key_fg: Color,
    pub(super) key_bg: Color,
    pub(super) user_accent: Color,
    pub(super) user_text: Color,
    pub(super) assistant_text: Color,
    pub(super) code_comment: Color,
    pub(super) code_string: Color,
    pub(super) code_literal: Color,
    pub(super) code_keyword: Color,
    pub(super) code_text: Color,
    pub(super) code_punctuation: Color,
    pub(super) match_fg: Color,
    pub(super) match_bg: Color,
    pub(super) age_new: Color,
    pub(super) age_recent: Color,
    pub(super) age_middle: Color,
    pub(super) age_old: Color,
    /// Accent colors handed out to workspaces in creation order.
    pub(super) ws_colors: [Color; 8],
}

impl Theme {
    pub(super) const fn dark() -> Self {
        Self {
            is_light: false,
            foreground: Color::Rgb(236, 239, 246),
            accent: Color::Rgb(140, 190, 255),
            warning: Color::Rgb(255, 206, 120),
            error: Color::Rgb(255, 122, 132),
            success: Color::Rgb(134, 226, 172),
            info: Color::Rgb(122, 214, 238),
            muted: Color::Rgb(152, 160, 180),
            secondary: Color::Rgb(196, 203, 220),
            panel_border: Color::Rgb(88, 98, 124),
            filter_selected_bg: Color::Reset,
            selected_fg: Color::Rgb(246, 248, 253),
            selected_bg: Color::Reset,
            key_fg: Color::Rgb(140, 190, 255),
            key_bg: Color::Reset,
            user_accent: Color::Rgb(140, 190, 255),
            user_text: Color::Rgb(196, 220, 255),
            assistant_text: Color::Rgb(226, 230, 240),
            code_comment: Color::Rgb(100, 160, 120),
            code_string: Color::Rgb(150, 220, 150),
            code_literal: Color::Rgb(210, 160, 255),
            code_keyword: Color::Rgb(120, 210, 255),
            code_text: Color::Rgb(220, 225, 230),
            code_punctuation: Color::Rgb(150, 158, 178),
            match_fg: Color::Black,
            match_bg: Color::Rgb(255, 206, 120),
            age_new: Color::Rgb(134, 226, 172),
            age_recent: Color::Rgb(190, 216, 140),
            age_middle: Color::Rgb(240, 190, 120),
            age_old: Color::Rgb(152, 160, 180),
            ws_colors: [
                Color::Rgb(240, 113, 120),
                Color::Rgb(130, 170, 255),
                Color::Rgb(160, 220, 120),
                Color::Rgb(255, 203, 107),
                Color::Rgb(199, 146, 234),
                Color::Rgb(110, 210, 235),
                Color::Rgb(247, 140, 108),
                Color::Rgb(255, 150, 200),
            ],
        }
    }

    pub(super) const fn light() -> Self {
        Self {
            is_light: true,
            foreground: Color::Reset,
            accent: Color::Rgb(40, 96, 210),
            warning: Color::Rgb(150, 90, 0),
            error: Color::Rgb(180, 35, 45),
            success: Color::Rgb(0, 120, 70),
            info: Color::Rgb(0, 92, 135),
            muted: Color::Rgb(100, 108, 128),
            secondary: Color::Rgb(60, 68, 90),
            panel_border: Color::Rgb(150, 158, 180),
            filter_selected_bg: Color::Reset,
            selected_fg: Color::Rgb(20, 24, 40),
            selected_bg: Color::Reset,
            key_fg: Color::Rgb(40, 96, 210),
            key_bg: Color::Reset,
            user_accent: Color::Rgb(40, 96, 210),
            user_text: Color::Rgb(30, 70, 160),
            assistant_text: Color::Reset,
            code_comment: Color::Rgb(30, 105, 50),
            code_string: Color::Rgb(15, 105, 55),
            code_literal: Color::Rgb(125, 55, 165),
            code_keyword: Color::Rgb(0, 85, 135),
            code_text: Color::Reset,
            code_punctuation: Color::Rgb(95, 100, 110),
            match_fg: Color::Black,
            match_bg: Color::Rgb(245, 205, 70),
            age_new: Color::Rgb(0, 120, 60),
            age_recent: Color::Rgb(125, 100, 0),
            age_middle: Color::Rgb(165, 85, 0),
            age_old: Color::Rgb(100, 80, 125),
            ws_colors: [
                Color::Rgb(190, 40, 55),
                Color::Rgb(30, 90, 200),
                Color::Rgb(30, 130, 40),
                Color::Rgb(170, 110, 0),
                Color::Rgb(125, 55, 165),
                Color::Rgb(0, 120, 150),
                Color::Rgb(190, 80, 30),
                Color::Rgb(190, 50, 120),
            ],
        }
    }

    /// Color of the workspace at this creation-order position.
    pub(super) fn ws_color(self, index: usize) -> Color {
        self.ws_colors[index % self.ws_colors.len()]
    }

    pub(super) fn agent_color(self, agent: &AgentConfig) -> Color {
        if self.is_light {
            agent.light_color
        } else {
            agent.color
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AGENTS;

    use super::*;

    #[test]
    fn automatic_mode_uses_terminal_luma_and_falls_back_to_dark() {
        assert_eq!(
            ThemeMode::Auto.resolve_with_luma(Some(0.61)),
            Theme::light()
        );
        assert_eq!(ThemeMode::Auto.resolve_with_luma(Some(0.60)), Theme::dark());
        assert_eq!(ThemeMode::Auto.resolve_with_luma(None), Theme::dark());
    }

    #[test]
    fn explicit_mode_ignores_detected_luma() {
        assert_eq!(
            ThemeMode::Light.resolve_with_luma(Some(0.0)),
            Theme::light()
        );
        assert_eq!(ThemeMode::Dark.resolve_with_luma(Some(1.0)), Theme::dark());
    }

    #[test]
    fn light_theme_replaces_white_agent_badges() {
        let cursor = &AGENTS["cursor"];

        assert_eq!(Theme::dark().agent_color(cursor), Color::Rgb(255, 255, 255));
        assert_eq!(Theme::light().agent_color(cursor), Color::Rgb(30, 30, 30));
    }
}
