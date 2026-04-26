#![allow(dead_code)]

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
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
use crate::startup_access::StartupNoticeIssueKind;
use crate::startup_access::StartupNoticeIssueSource;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PooledAccessNoticeKind {
    PooledOnly,
    PooledPaused,
    DefaultPoolRequired {
        issue_kind: StartupNoticeIssueKind,
        issue_source: StartupNoticeIssueSource,
        candidate_pool_ids: Vec<String>,
    },
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

    pub(crate) fn default_pool_required(
        candidate_pool_ids: Vec<String>,
        animations_enabled: bool,
    ) -> Self {
        Self::default_pool_required_with_source(
            candidate_pool_ids,
            StartupNoticeIssueSource::None,
            animations_enabled,
        )
    }

    pub(crate) fn default_pool_required_with_source(
        candidate_pool_ids: Vec<String>,
        issue_source: StartupNoticeIssueSource,
        animations_enabled: bool,
    ) -> Self {
        let issue_kind = match issue_source {
            StartupNoticeIssueSource::None => StartupNoticeIssueKind::MultiplePoolsRequireDefault,
            StartupNoticeIssueSource::Override
            | StartupNoticeIssueSource::ConfigDefault
            | StartupNoticeIssueSource::PersistedSelection => {
                StartupNoticeIssueKind::InvalidExplicitDefault
            }
        };
        Self::default_pool_required_with_issue(
            candidate_pool_ids,
            issue_kind,
            issue_source,
            animations_enabled,
        )
    }

    pub(crate) fn default_pool_required_with_issue(
        candidate_pool_ids: Vec<String>,
        issue_kind: StartupNoticeIssueKind,
        issue_source: StartupNoticeIssueSource,
        _animations_enabled: bool,
    ) -> Self {
        Self {
            kind: PooledAccessNoticeKind::DefaultPoolRequired {
                issue_kind,
                issue_source,
                candidate_pool_ids,
            },
            outcome: None,
            error: None,
        }
    }

    pub(crate) fn outcome(&self) -> Option<PooledAccessNoticeOutcome> {
        self.outcome
    }

    pub(crate) fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    fn title(&self) -> &'static str {
        match self.kind {
            PooledAccessNoticeKind::PooledOnly => "Pooled access is available",
            PooledAccessNoticeKind::PooledPaused => "Pooled access is paused",
            PooledAccessNoticeKind::DefaultPoolRequired { .. } => "Choose a default account pool",
        }
    }

    fn body(&self) -> Vec<String> {
        match &self.kind {
            PooledAccessNoticeKind::PooledOnly => {
                vec!["You can continue with pooled access or hand off to login.".to_string()]
            }
            PooledAccessNoticeKind::PooledPaused => vec![
                "Pooled access is paused for this startup. Resume it or hand off to login."
                    .to_string(),
            ],
            PooledAccessNoticeKind::DefaultPoolRequired {
                issue_kind,
                issue_source,
                candidate_pool_ids,
            } => {
                let mut paragraphs = vec![match issue_kind {
                    StartupNoticeIssueKind::MultiplePoolsRequireDefault => {
                        "Startup found multiple visible account pools and needs a default before pooled access can continue.".to_string()
                    }
                    StartupNoticeIssueKind::InvalidExplicitDefault => match issue_source {
                        StartupNoticeIssueSource::None => {
                            "The default account pool is not available for startup.".to_string()
                        }
                        StartupNoticeIssueSource::PersistedSelection => {
                            "The saved default account pool is no longer available for startup.".to_string()
                        }
                        StartupNoticeIssueSource::ConfigDefault => {
                            "The configured default account pool is not available for startup.".to_string()
                        }
                        StartupNoticeIssueSource::Override => {
                            "The process-local account pool override is not available for startup.".to_string()
                        }
                    }
                }];
                if !candidate_pool_ids.is_empty() {
                    paragraphs.push(format!("Visible pools: {}", candidate_pool_ids.join(", ")));
                }
                paragraphs.push(match issue_kind {
                    StartupNoticeIssueKind::MultiplePoolsRequireDefault => {
                        "Run `mcodex accounts pool default set <POOL_ID>` to choose one of the visible pools.".to_string()
                    }
                    StartupNoticeIssueKind::InvalidExplicitDefault => match issue_source {
                        StartupNoticeIssueSource::None => {
                            "Run `mcodex accounts pool default set <POOL_ID>` to choose an available pool, or `mcodex accounts pool default clear` to remove the saved default.".to_string()
                        }
                        StartupNoticeIssueSource::PersistedSelection => {
                            "Run `mcodex accounts pool default set <POOL_ID>` to choose another pool, or `mcodex accounts pool default clear` to remove the saved default.".to_string()
                        }
                        StartupNoticeIssueSource::ConfigDefault => {
                            "Fix or remove `accounts.default_pool`, then restart startup or hand off to login.".to_string()
                        }
                        StartupNoticeIssueSource::Override => {
                            "Correct the process-local override, then restart startup or hand off to login.".to_string()
                        }
                    }
                });
                paragraphs
            }
        }
    }

    fn footer(&self) -> Line<'static> {
        match self.kind {
            PooledAccessNoticeKind::PooledOnly => Line::from(vec![
                "Press ".dim(),
                key_hint::plain(KeyCode::Enter).into(),
                " to continue, press ".dim(),
                key_hint::plain(KeyCode::Char('l')).into(),
                " to log in, or press ".dim(),
                key_hint::plain(KeyCode::Char('n')).into(),
                " to hide and continue".dim(),
            ]),
            PooledAccessNoticeKind::PooledPaused => Line::from(vec![
                "Press ".dim(),
                key_hint::plain(KeyCode::Enter).into(),
                " to resume, or press ".dim(),
                key_hint::plain(KeyCode::Char('l')).into(),
                " to log in".dim(),
            ]),
            PooledAccessNoticeKind::DefaultPoolRequired { .. } => Line::from(vec![
                "Press ".dim(),
                key_hint::plain(KeyCode::Enter).into(),
                " or ".dim(),
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
                    PooledAccessNoticeKind::DefaultPoolRequired { .. } => {
                        PooledAccessNoticeOutcome::OpenLogin
                    }
                });
            }
            KeyCode::Char('l') | KeyCode::Char('L')
                if !key_event.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.outcome = Some(PooledAccessNoticeOutcome::OpenLogin);
            }
            KeyCode::Char('n') | KeyCode::Char('N')
                if matches!(self.kind, PooledAccessNoticeKind::PooledOnly)
                    && !key_event.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
            {
                self.outcome = Some(PooledAccessNoticeOutcome::HideAndContinue);
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
        for paragraph in self.body() {
            column.push(
                Paragraph::new(paragraph)
                    .wrap(Wrap { trim: true })
                    .inset(Insets::tlbr(
                        /*top*/ 0, /*left*/ 2, /*bottom*/ 0, /*right*/ 0,
                    )),
            );
            column.push("");
        }
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
    fn ctrl_l_does_not_request_login_handoff() {
        let mut widget = PooledAccessNoticeWidget::pooled_only(false);
        widget.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert_eq!(widget.outcome(), None);
    }

    #[test]
    fn pooled_only_notice_n_requests_hide_and_continue() {
        let mut widget = PooledAccessNoticeWidget::pooled_only(false);
        widget.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(
            widget.outcome(),
            Some(PooledAccessNoticeOutcome::HideAndContinue)
        );
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
    fn pooled_notice_error_can_be_cleared() {
        let mut widget = PooledAccessNoticeWidget::pooled_only(false);
        widget.set_error(Some("resume failed".to_string()));
        assert!(render_to_string(&widget).contains("resume failed"));

        widget.set_error(None);
        assert!(!render_to_string(&widget).contains("resume failed"));
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

    #[test]
    fn pooled_default_selection_notice_renders_snapshot() {
        let widget = PooledAccessNoticeWidget::default_pool_required(
            vec!["team-main".to_string(), "team-other".to_string()],
            /*animations_enabled*/ false,
        );
        assert_snapshot!("pooled_default_selection_notice", render_to_string(&widget));
    }

    #[test]
    fn pooled_invalid_config_default_notice_renders_snapshot() {
        let widget = PooledAccessNoticeWidget::default_pool_required_with_source(
            vec!["team-main".to_string()],
            StartupNoticeIssueSource::ConfigDefault,
            /*animations_enabled*/ false,
        );
        assert_snapshot!(
            "pooled_invalid_config_default_notice",
            render_to_string(&widget)
        );
    }

    #[test]
    fn pooled_invalid_default_unknown_source_notice_does_not_claim_multiple_pools() {
        let widget = PooledAccessNoticeWidget::default_pool_required_with_issue(
            vec!["team-main".to_string()],
            StartupNoticeIssueKind::InvalidExplicitDefault,
            StartupNoticeIssueSource::None,
            /*animations_enabled*/ false,
        );
        let rendered = render_to_string(&widget);

        assert!(rendered.contains("default account pool is not available"));
        assert!(!rendered.contains("multiple visible account pools"));
        assert_snapshot!("pooled_invalid_default_unknown_source_notice", rendered);
    }

    #[test]
    fn pooled_paused_notice_renders_error_snapshot() {
        let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
        widget.set_error(Some("resume failed".to_string()));
        assert_snapshot!("pooled_paused_notice_error", render_to_string(&widget));
    }
}
