#![allow(dead_code)]

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use crate::key_hint;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::render::Insets;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableExt as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PooledAccessNoticeKind {
    PooledOnly,
    PooledPaused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PooledAccessNoticeOutcome {
    Continue,
    OpenLogin,
    HideAndContinue,
    ResumeAndContinue,
}

pub(crate) struct PooledAccessNoticeWidget {
    kind: PooledAccessNoticeKind,
    outcome: Option<PooledAccessNoticeOutcome>,
    error: Option<String>,
}

impl PooledAccessNoticeWidget {
    pub(crate) fn pooled_only(_animations_enabled: bool) -> Self {
        Self {
            kind: PooledAccessNoticeKind::PooledOnly,
            outcome: None,
            error: None,
        }
    }

    pub(crate) fn pooled_paused(_animations_enabled: bool) -> Self {
        Self {
            kind: PooledAccessNoticeKind::PooledPaused,
            outcome: None,
            error: None,
        }
    }

    pub(crate) fn outcome(&self) -> Option<PooledAccessNoticeOutcome> {
        self.outcome
    }

    pub(crate) fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    fn title(&self) -> &'static str {
        match self.kind {
            PooledAccessNoticeKind::PooledOnly => "Pooled access is available",
            PooledAccessNoticeKind::PooledPaused => "Pooled access is paused",
        }
    }

    fn body(&self) -> &'static str {
        match self.kind {
            PooledAccessNoticeKind::PooledOnly => {
                "You can continue with pooled access or hand off to login."
            }
            PooledAccessNoticeKind::PooledPaused => {
                "Pooled access is paused for this startup. Resume it or hand off to login."
            }
        }
    }

    fn footer(&self) -> Line<'static> {
        match self.kind {
            PooledAccessNoticeKind::PooledOnly => Line::from(vec![
                "Press ".dim(),
                key_hint::plain(KeyCode::Enter).into(),
                " to continue, or press ".dim(),
                key_hint::plain(KeyCode::Char('l')).into(),
                " to log in".dim(),
            ]),
            PooledAccessNoticeKind::PooledPaused => Line::from(vec![
                "Press ".dim(),
                key_hint::plain(KeyCode::Enter).into(),
                " to resume, or press ".dim(),
                key_hint::plain(KeyCode::Char('l')).into(),
                " to log in".dim(),
            ]),
        }
    }
}

impl KeyboardHandler for PooledAccessNoticeWidget {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        match key_event.code {
            KeyCode::Enter => {
                self.outcome = Some(match self.kind {
                    PooledAccessNoticeKind::PooledOnly => PooledAccessNoticeOutcome::Continue,
                    PooledAccessNoticeKind::PooledPaused => {
                        PooledAccessNoticeOutcome::ResumeAndContinue
                    }
                });
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.outcome = Some(PooledAccessNoticeOutcome::OpenLogin);
            }
            _ => {}
        }
    }
}

impl WidgetRef for &PooledAccessNoticeWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let mut column = ColumnRenderable::new();
        column.push(Line::from(vec!["  ".into(), self.title().bold()]));
        column.push("");
        column.push(
            Paragraph::new(self.body().to_string())
                .wrap(Wrap { trim: true })
                .inset(Insets::tlbr(
                    /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
                )),
        );
        column.push("");
        column.push(self.footer().inset(Insets::tlbr(
            /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
        )));
        column.push("");
        if let Some(error) = &self.error {
            column.push(
                Paragraph::new(error.to_string())
                    .red()
                    .wrap(Wrap { trim: true })
                    .inset(Insets::tlbr(
                        /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
                    )),
            );
        }

        column.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_backend::VT100Backend;
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::Terminal;

    fn render_to_string(widget: &PooledAccessNoticeWidget) -> String {
        let mut terminal =
            Terminal::new(VT100Backend::new(/*width*/ 72, /*height*/ 14)).expect("terminal");
        terminal
            .draw(|f| (&widget).render_ref(f.area(), f.buffer_mut()))
            .expect("draw");
        format!("{}", terminal.backend())
    }

    #[test]
    fn pooled_only_notice_enter_marks_continue() {
        let mut widget = PooledAccessNoticeWidget::pooled_only(/*animations_enabled*/ false);
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::Continue));
    }

    #[test]
    fn pooled_only_notice_l_requests_login_handoff() {
        let mut widget = PooledAccessNoticeWidget::pooled_only(false);
        widget.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::OpenLogin));
    }

    #[test]
    fn pooled_paused_notice_enter_requests_resume() {
        let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
        widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            widget.outcome(),
            Some(PooledAccessNoticeOutcome::ResumeAndContinue)
        );
    }

    #[test]
    fn pooled_paused_notice_l_requests_login_handoff() {
        let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
        widget.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::OpenLogin));
    }

    #[test]
    fn pooled_paused_notice_shows_inline_error() {
        let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
        widget.set_error("resume failed".to_string());
        assert!(render_to_string(&widget).contains("resume failed"));
    }

    #[test]
    fn pooled_only_notice_renders_snapshot() {
        let widget = PooledAccessNoticeWidget::pooled_only(false);
        assert_snapshot!("pooled_only_notice", render_to_string(&widget));
    }

    #[test]
    fn pooled_paused_notice_renders_snapshot() {
        let widget = PooledAccessNoticeWidget::pooled_paused(false);
        assert_snapshot!("pooled_paused_notice", render_to_string(&widget));
    }
}
