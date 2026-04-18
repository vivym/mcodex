use crate::legacy_core::config::Config;
#[cfg(target_os = "windows")]
use crate::legacy_core::windows_sandbox::WindowsSandboxLevelExt;
use codex_app_server_client::AppServerEvent;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::ServerNotification;
#[cfg(target_os = "windows")]
use codex_protocol::config_types::WindowsSandboxLevel;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Color;
use ratatui::widgets::Clear;
use ratatui::widgets::WidgetRef;

use codex_protocol::config_types::ForcedLoginMethod;

use crate::LoginStatus;
use crate::app_server_session::AppServerSession;
use crate::onboarding::auth::AuthModeWidget;
use crate::onboarding::auth::SignInOption;
use crate::onboarding::auth::SignInState;
use crate::onboarding::pooled_access_notice::PooledAccessNoticeOutcome;
use crate::onboarding::pooled_access_notice::PooledAccessNoticeWidget;
use crate::onboarding::trust_directory::TrustDirectorySelection;
use crate::onboarding::trust_directory::TrustDirectoryWidget;
use crate::onboarding::welcome::WelcomeWidget;
use crate::startup_access::StartupPromptDecision;
use crate::tui::FrameRequester;
use crate::tui::Tui;
use crate::tui::TuiEvent;
use color_eyre::eyre::Result;
use std::sync::Arc;
use std::sync::RwLock;

#[allow(clippy::large_enum_variant)]
enum Step {
    Welcome(WelcomeWidget),
    Auth(AuthModeWidget),
    PooledOnlyNotice(PooledAccessNoticeWidget),
    PooledPausedNotice(PooledAccessNoticeWidget),
    TrustDirectory(TrustDirectoryWidget),
}

pub(crate) trait KeyboardHandler {
    fn handle_key_event(&mut self, key_event: KeyEvent);
    fn handle_paste(&mut self, _pasted: String) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepState {
    Hidden,
    InProgress,
    Complete,
}

pub(crate) trait StepStateProvider {
    fn get_step_state(&self) -> StepState;
}

pub(crate) struct OnboardingScreen {
    request_frame: FrameRequester,
    steps: Vec<Step>,
    pending_auth_step: Option<AuthModeWidget>,
    is_done: bool,
    should_exit: bool,
}

pub(crate) struct OnboardingScreenArgs {
    pub show_trust_screen: bool,
    pub show_login_screen: bool,
    pub startup_prompt_decision: StartupPromptDecision,
    pub login_status: LoginStatus,
    pub app_server_request_handle: Option<AppServerRequestHandle>,
    pub config: Config,
}

pub(crate) struct OnboardingResult {
    pub directory_trust_decision: Option<TrustDirectorySelection>,
    pub should_exit: bool,
    pub reload_config: bool,
    pub login_flow_shown: bool,
}

impl OnboardingScreen {
    pub(crate) fn new(tui: &mut Tui, args: OnboardingScreenArgs) -> Self {
        Self::new_with_frame_requester(tui.frame_requester(), args)
    }

    pub(crate) fn new_with_frame_requester(
        request_frame: FrameRequester,
        args: OnboardingScreenArgs,
    ) -> Self {
        let OnboardingScreenArgs {
            show_trust_screen,
            show_login_screen,
            startup_prompt_decision,
            login_status,
            app_server_request_handle,
            config,
        } = args;
        let cwd = config.cwd.to_path_buf();
        let codex_home = config.codex_home.to_path_buf();
        let forced_login_method = config.forced_login_method;
        let auth_widget = if show_login_screen
            || matches!(
                startup_prompt_decision,
                StartupPromptDecision::PooledOnlyNotice
                    | StartupPromptDecision::PooledAccessPausedNotice
            ) {
            app_server_request_handle.map(|app_server_request_handle| {
                let highlighted_mode = match forced_login_method {
                    Some(ForcedLoginMethod::Api) => SignInOption::ApiKey,
                    _ => SignInOption::ChatGpt,
                };
                AuthModeWidget {
                    request_frame: request_frame.clone(),
                    highlighted_mode,
                    error: Arc::new(RwLock::new(None)),
                    sign_in_state: Arc::new(RwLock::new(SignInState::PickMode)),
                    login_status,
                    app_server_request_handle,
                    forced_login_method,
                    animations_enabled: config.animations,
                    animations_suppressed: std::cell::Cell::new(false),
                }
            })
        } else {
            None
        };
        let mut steps: Vec<Step> = Vec::new();
        steps.push(Step::Welcome(WelcomeWidget::new(
            !matches!(login_status, LoginStatus::NotAuthenticated),
            request_frame.clone(),
            config.animations,
        )));
        if matches!(
            startup_prompt_decision,
            StartupPromptDecision::PooledOnlyNotice
                | StartupPromptDecision::PooledAccessPausedNotice
        ) {
            match startup_prompt_decision {
                StartupPromptDecision::PooledOnlyNotice => {
                    steps.push(Step::PooledOnlyNotice(
                        PooledAccessNoticeWidget::pooled_only(config.animations),
                    ));
                }
                StartupPromptDecision::PooledAccessPausedNotice => {
                    steps.push(Step::PooledPausedNotice(
                        PooledAccessNoticeWidget::pooled_paused(config.animations),
                    ));
                }
                StartupPromptDecision::NeedsLogin | StartupPromptDecision::NoPrompt => {}
            }
        }
        let pending_auth_step = if show_login_screen
            && matches!(
                startup_prompt_decision,
                StartupPromptDecision::PooledOnlyNotice
                    | StartupPromptDecision::PooledAccessPausedNotice
            ) {
            auth_widget
        } else if show_login_screen {
            if let Some(auth_widget) = auth_widget {
                steps.push(Step::Auth(auth_widget));
            }
            None
        } else {
            auth_widget
        };
        #[cfg(target_os = "windows")]
        let show_windows_create_sandbox_hint =
            WindowsSandboxLevel::from_config(&config) == WindowsSandboxLevel::Disabled;
        #[cfg(not(target_os = "windows"))]
        let show_windows_create_sandbox_hint = false;
        let highlighted = TrustDirectorySelection::Trust;
        if show_trust_screen {
            steps.push(Step::TrustDirectory(TrustDirectoryWidget {
                cwd,
                codex_home,
                show_windows_create_sandbox_hint,
                should_quit: false,
                selection: None,
                highlighted,
                error: None,
            }))
        }
        // TODO: add git warning.
        Self {
            request_frame,
            steps,
            pending_auth_step,
            is_done: false,
            should_exit: false,
        }
    }

    fn current_steps_mut(&mut self) -> Vec<&mut Step> {
        let mut out: Vec<&mut Step> = Vec::new();
        for step in self.steps.iter_mut() {
            match step.get_step_state() {
                StepState::Hidden => continue,
                StepState::Complete => out.push(step),
                StepState::InProgress => {
                    out.push(step);
                    break;
                }
            }
        }
        out
    }

    fn current_steps(&self) -> Vec<&Step> {
        let mut out: Vec<&Step> = Vec::new();
        for step in self.steps.iter() {
            match step.get_step_state() {
                StepState::Hidden => continue,
                StepState::Complete => out.push(step),
                StepState::InProgress => {
                    out.push(step);
                    break;
                }
            }
        }
        out
    }

    fn should_suppress_animations(&self) -> bool {
        // Freeze the whole onboarding screen when auth is showing copyable login
        // material so terminal selection is not interrupted by redraws.
        self.current_steps().into_iter().any(|step| match step {
            Step::Auth(widget) => widget.should_suppress_animations(),
            Step::Welcome(_)
            | Step::PooledOnlyNotice(_)
            | Step::PooledPausedNotice(_)
            | Step::TrustDirectory(_) => false,
        })
    }

    fn is_auth_in_progress(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(step, Step::Auth(_)) && matches!(step.get_step_state(), StepState::InProgress)
        })
    }

    fn login_flow_shown(&self) -> bool {
        self.steps.iter().any(|step| matches!(step, Step::Auth(_)))
    }

    pub(crate) fn is_done(&self) -> bool {
        self.is_done
            || !self
                .steps
                .iter()
                .any(|step| matches!(step.get_step_state(), StepState::InProgress))
    }

    pub fn directory_trust_decision(&self) -> Option<TrustDirectorySelection> {
        self.steps
            .iter()
            .find_map(|step| {
                if let Step::TrustDirectory(TrustDirectoryWidget { selection, .. }) = step {
                    Some(*selection)
                } else {
                    None
                }
            })
            .flatten()
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    fn cancel_auth_if_active(&self) {
        for step in &self.steps {
            if let Step::Auth(widget) = step {
                widget.cancel_active_attempt();
            }
        }
    }

    fn auth_widget_mut(&mut self) -> Option<&mut AuthModeWidget> {
        self.steps.iter_mut().find_map(|step| match step {
            Step::Auth(widget) => Some(widget),
            Step::Welcome(_)
            | Step::PooledOnlyNotice(_)
            | Step::PooledPausedNotice(_)
            | Step::TrustDirectory(_) => None,
        })
    }

    fn reveal_pending_auth_step(&mut self) {
        let Some(auth_step) = self.pending_auth_step.take() else {
            return;
        };

        let insert_at = self
            .steps
            .iter()
            .rposition(|step| {
                matches!(
                    step,
                    Step::PooledOnlyNotice(_) | Step::PooledPausedNotice(_)
                )
            })
            .map_or(self.steps.len(), |index| index + 1);
        self.steps.insert(insert_at, Step::Auth(auth_step));
    }

    fn dismiss_startup_notice(&mut self) {
        self.steps.retain(|step| {
            !matches!(
                step,
                Step::PooledOnlyNotice(_) | Step::PooledPausedNotice(_)
            )
        });
    }

    #[cfg(test)]
    fn restore_pooled_paused_notice(&mut self, error: String) {
        self.restore_startup_notice_with_error(error);
    }

    fn active_startup_notice_mut(&mut self) -> Option<&mut PooledAccessNoticeWidget> {
        self.steps.iter_mut().find_map(|step| match step {
            Step::PooledOnlyNotice(widget) | Step::PooledPausedNotice(widget) => Some(widget),
            Step::Welcome(_) | Step::Auth(_) | Step::TrustDirectory(_) => None,
        })
    }

    fn restore_startup_notice_with_error(&mut self, error: String) {
        let Some(index) = self.steps.iter().position(|step| {
            matches!(
                step,
                Step::PooledOnlyNotice(_) | Step::PooledPausedNotice(_)
            )
        }) else {
            return;
        };

        let is_paused = matches!(self.steps[index], Step::PooledPausedNotice(_));
        self.steps[index] = if is_paused {
            Step::PooledPausedNotice(PooledAccessNoticeWidget::pooled_paused(
                /*animations_enabled*/ false,
            ))
        } else {
            Step::PooledOnlyNotice(PooledAccessNoticeWidget::pooled_only(
                /*animations_enabled*/ false,
            ))
        };
        if let Some(widget) = self.active_startup_notice_mut() {
            widget.set_error(Some(error));
        }
    }

    fn handle_app_server_notification(&mut self, notification: ServerNotification) {
        match notification {
            ServerNotification::AccountLoginCompleted(notification) => {
                if let Some(widget) = self.auth_widget_mut() {
                    widget.on_account_login_completed(notification);
                }
            }
            ServerNotification::AccountUpdated(notification) => {
                if let Some(widget) = self.auth_widget_mut() {
                    widget.on_account_updated(notification);
                }
            }
            _ => {}
        }
    }

    fn is_api_key_entry_active(&self) -> bool {
        self.steps.iter().any(|step| {
            if let Step::Auth(widget) = step {
                return widget
                    .sign_in_state
                    .read()
                    .is_ok_and(|g| matches!(&*g, SignInState::ApiKeyEntry(_)));
            }
            false
        })
    }
}

impl KeyboardHandler for OnboardingScreen {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        let is_api_key_entry_active = self.is_api_key_entry_active();
        let should_quit = match key_event {
            KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            } => true,
            KeyEvent {
                code: KeyCode::Char('q'),
                kind: KeyEventKind::Press,
                ..
            } => !is_api_key_entry_active,
            _ => false,
        };
        if should_quit {
            if self.is_auth_in_progress() {
                self.cancel_auth_if_active();
                // If the user cancels the auth menu, exit the app rather than
                // leave the user at a prompt in an unauthed state.
                self.should_exit = true;
            }
            self.is_done = true;
        } else {
            if let Some(Step::Welcome(widget)) = self
                .steps
                .iter_mut()
                .find(|step| matches!(step, Step::Welcome(_)))
            {
                widget.handle_key_event(key_event);
            }
            if let Some(active_step) = self.current_steps_mut().into_iter().last() {
                active_step.handle_key_event(key_event);
            }
            if self.steps.iter().any(|step| {
                if let Step::TrustDirectory(widget) = step {
                    widget.should_quit()
                } else {
                    false
                }
            }) {
                self.should_exit = true;
                self.is_done = true;
            }
        }
        self.request_frame.schedule_frame();
    }

    fn handle_paste(&mut self, pasted: String) {
        if pasted.is_empty() {
            return;
        }

        if let Some(active_step) = self.current_steps_mut().into_iter().last() {
            active_step.handle_paste(pasted);
        }
        self.request_frame.schedule_frame();
    }
}

impl WidgetRef for &OnboardingScreen {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let suppress_animations = self.should_suppress_animations();
        for step in self.current_steps() {
            match step {
                Step::Welcome(widget) => widget.set_animations_suppressed(suppress_animations),
                Step::Auth(widget) => widget.set_animations_suppressed(suppress_animations),
                Step::PooledOnlyNotice(_) | Step::PooledPausedNotice(_) => {}
                Step::TrustDirectory(_) => {}
            }
        }

        Clear.render(area, buf);
        // Render steps top-to-bottom, measuring each step's height dynamically.
        let mut y = area.y;
        let bottom = area.y.saturating_add(area.height);
        let width = area.width;

        // Helper to scan a temporary buffer and return number of used rows.
        fn used_rows(tmp: &Buffer, width: u16, height: u16) -> u16 {
            if width == 0 || height == 0 {
                return 0;
            }
            let mut last_non_empty: Option<u16> = None;
            for yy in 0..height {
                let mut any = false;
                for xx in 0..width {
                    let cell = &tmp[(xx, yy)];
                    let has_symbol = !cell.symbol().trim().is_empty();
                    let has_style = cell.fg != Color::Reset
                        || cell.bg != Color::Reset
                        || !cell.modifier.is_empty();
                    if has_symbol || has_style {
                        any = true;
                        break;
                    }
                }
                if any {
                    last_non_empty = Some(yy);
                }
            }
            last_non_empty.map(|v| v + 2).unwrap_or(0)
        }

        let mut i = 0usize;
        let current_steps = self.current_steps();

        while i < current_steps.len() && y < bottom {
            let step = &current_steps[i];
            let max_h = bottom.saturating_sub(y);
            if max_h == 0 || width == 0 {
                break;
            }
            let scratch_area = Rect::new(0, 0, width, max_h);
            let mut scratch = Buffer::empty(scratch_area);
            if let Step::Welcome(widget) = step {
                widget.update_layout_area(scratch_area);
            }
            step.render_ref(scratch_area, &mut scratch);
            let h = used_rows(&scratch, width, max_h).min(max_h);
            if h > 0 {
                let target = Rect {
                    x: area.x,
                    y,
                    width,
                    height: h,
                };
                Clear.render(target, buf);
                step.render_ref(target, buf);
                y = y.saturating_add(h);
            }
            i += 1;
        }
    }
}

impl KeyboardHandler for Step {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match self {
            Step::Welcome(widget) => widget.handle_key_event(key_event),
            Step::Auth(widget) => widget.handle_key_event(key_event),
            Step::PooledOnlyNotice(widget) | Step::PooledPausedNotice(widget) => {
                widget.handle_key_event(key_event)
            }
            Step::TrustDirectory(widget) => widget.handle_key_event(key_event),
        }
    }

    fn handle_paste(&mut self, pasted: String) {
        match self {
            Step::Welcome(_) => {}
            Step::Auth(widget) => widget.handle_paste(pasted),
            Step::PooledOnlyNotice(_) | Step::PooledPausedNotice(_) => {}
            Step::TrustDirectory(widget) => widget.handle_paste(pasted),
        }
    }
}

impl StepStateProvider for Step {
    fn get_step_state(&self) -> StepState {
        match self {
            Step::Welcome(w) => w.get_step_state(),
            Step::Auth(w) => w.get_step_state(),
            Step::PooledOnlyNotice(w) | Step::PooledPausedNotice(w) => {
                if w.outcome().is_some() {
                    StepState::Complete
                } else {
                    StepState::InProgress
                }
            }
            Step::TrustDirectory(w) => w.get_step_state(),
        }
    }
}

impl WidgetRef for Step {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        match self {
            Step::Welcome(widget) => {
                widget.render_ref(area, buf);
            }
            Step::Auth(widget) => {
                widget.render_ref(area, buf);
            }
            Step::PooledOnlyNotice(widget) | Step::PooledPausedNotice(widget) => {
                widget.render_ref(area, buf);
            }
            Step::TrustDirectory(widget) => {
                widget.render_ref(area, buf);
            }
        }
    }
}

pub(crate) async fn run_onboarding_app(
    args: OnboardingScreenArgs,
    mut app_server: Option<&mut AppServerSession>,
    tui: &mut Tui,
) -> Result<OnboardingResult> {
    use tokio_stream::StreamExt;

    let mut onboarding_screen = OnboardingScreen::new(tui, args);
    // One-time guard to fully clear the screen after ChatGPT login success message is shown
    let mut did_full_clear_after_success = false;
    let mut reload_config_after_notice = false;

    tui.draw(u16::MAX, |frame| {
        frame.render_widget_ref(&onboarding_screen, frame.area());
    })?;

    let tui_events = tui.event_stream();
    tokio::pin!(tui_events);

    while !onboarding_screen.is_done() {
        tokio::select! {
            event = tui_events.next() => {
                if let Some(event) = event {
                    match event {
                        TuiEvent::Key(key_event) => {
                            onboarding_screen.handle_key_event(key_event);
                            handle_startup_notice_outcome(
                                &mut onboarding_screen,
                                app_server.as_deref_mut(),
                                &mut reload_config_after_notice,
                            )
                            .await?;
                        }
                        TuiEvent::Paste(text) => {
                            onboarding_screen.handle_paste(text);
                        }
                        TuiEvent::Draw => {
                            if !did_full_clear_after_success
                                && onboarding_screen.steps.iter().any(|step| {
                                    if let Step::Auth(w) = step {
                                        w.sign_in_state.read().is_ok_and(|g| {
                                            matches!(&*g, super::auth::SignInState::ChatGptSuccessMessage)
                                        })
                                    } else {
                                        false
                                    }
                                })
                            {
                                // Reset any lingering SGR (underline/color) before clearing
                                let _ = ratatui::crossterm::execute!(
                                    std::io::stdout(),
                                    ratatui::crossterm::style::SetAttribute(
                                        ratatui::crossterm::style::Attribute::Reset
                                    ),
                                    ratatui::crossterm::style::SetAttribute(
                                        ratatui::crossterm::style::Attribute::NoUnderline
                                    ),
                                    ratatui::crossterm::style::SetForegroundColor(
                                        ratatui::crossterm::style::Color::Reset
                                    ),
                                    ratatui::crossterm::style::SetBackgroundColor(
                                        ratatui::crossterm::style::Color::Reset
                                    )
                                );
                                let _ = tui.terminal.clear();
                                did_full_clear_after_success = true;
                            }
                            let _ = tui.draw(u16::MAX, |frame| {
                                frame.render_widget_ref(&onboarding_screen, frame.area());
                            });
                        }
                    }
                }
            }
            event = async {
                match app_server.as_mut() {
                    Some(app_server) => app_server.next_event().await,
                    None => None,
                }
            }, if app_server.is_some() => {
                if let Some(event) = event {
                    match event {
                        AppServerEvent::ServerNotification(notification) => {
                            onboarding_screen.handle_app_server_notification(notification);
                        }
                        AppServerEvent::Disconnected { message } => {
                            return Err(color_eyre::eyre::eyre!(message));
                        }
                        AppServerEvent::Lagged { .. }
                        | AppServerEvent::ServerRequest(_) => {}
                    }
                }
            }
        }
    }
    Ok(OnboardingResult {
        directory_trust_decision: onboarding_screen.directory_trust_decision(),
        should_exit: onboarding_screen.should_exit(),
        reload_config: reload_config_after_notice,
        login_flow_shown: onboarding_screen.login_flow_shown(),
    })
}

async fn handle_startup_notice_outcome(
    onboarding_screen: &mut OnboardingScreen,
    app_server: Option<&mut AppServerSession>,
    reload_config_after_notice: &mut bool,
) -> Result<()> {
    let Some(outcome) = onboarding_screen
        .active_startup_notice_mut()
        .and_then(|widget| widget.outcome())
    else {
        return Ok(());
    };

    match outcome {
        PooledAccessNoticeOutcome::Continue => {
            onboarding_screen.dismiss_startup_notice();
        }
        PooledAccessNoticeOutcome::OpenLogin => {
            onboarding_screen.reveal_pending_auth_step();
        }
        PooledAccessNoticeOutcome::HideAndContinue => {
            let Some(app_server) = app_server else {
                tracing::warn!(
                    "unable to persist pooled startup notice preference without app server"
                );
                onboarding_screen.dismiss_startup_notice();
                return Ok(());
            };
            let is_remote = app_server.is_remote();
            if let Err(err) = app_server.write_hide_pooled_only_startup_notice(true).await {
                tracing::warn!(error = %err, "unable to persist pooled startup notice preference");
                onboarding_screen.dismiss_startup_notice();
                return Ok(());
            }
            if !is_remote {
                *reload_config_after_notice = true;
            }
            onboarding_screen.dismiss_startup_notice();
        }
        PooledAccessNoticeOutcome::ResumeAndContinue => {
            let Some(app_server) = app_server else {
                onboarding_screen.restore_startup_notice_with_error(
                    "unable to resume pooled startup".to_string(),
                );
                return Ok(());
            };
            if let Err(err) = app_server.resume_pooled_startup().await {
                onboarding_screen.restore_startup_notice_with_error(err.to_string());
            } else {
                onboarding_screen.dismiss_startup_notice();
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding::auth::AuthModeWidget;
    use crate::onboarding::auth::SignInState;
    use crate::onboarding::pooled_access_notice::PooledAccessNoticeWidget;
    use crate::onboarding::welcome::WelcomeWidget;
    use crate::startup_access::StartupPromptDecision;
    use crate::test_backend::VT100Backend;
    use crate::tui::FrameRequester;
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::Terminal as RatatuiTerminal;
    use tempfile::TempDir;

    use crate::LoginStatus;
    use crate::legacy_core::config::ConfigBuilder;

    async fn build_config(temp_dir: &TempDir) -> Result<Config> {
        ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .build()
            .await
            .map_err(Into::into)
    }

    fn step_names(screen: &OnboardingScreen) -> Vec<&'static str> {
        screen
            .current_steps()
            .into_iter()
            .map(|step| match step {
                Step::Welcome(_) => "welcome",
                Step::Auth(_) => "auth",
                Step::PooledOnlyNotice(_) => "pooled-only",
                Step::PooledPausedNotice(_) => "pooled-paused",
                Step::TrustDirectory(_) => "trust",
            })
            .collect()
    }

    fn render_to_string(screen: &OnboardingScreen) -> String {
        let mut terminal =
            RatatuiTerminal::new(VT100Backend::new(/*width*/ 72, /*height*/ 24)).expect("terminal");
        terminal
            .draw(|frame: &mut ratatui::Frame<'_>| {
                frame.render_widget_ref(screen, frame.area());
            })
            .expect("draw");
        format!("{}", terminal.backend())
    }

    fn auth_widget_mut(screen: &mut OnboardingScreen) -> &mut AuthModeWidget {
        screen
            .steps
            .iter_mut()
            .find_map(|step| match step {
                Step::Auth(widget) => Some(widget),
                Step::Welcome(_)
                | Step::PooledOnlyNotice(_)
                | Step::PooledPausedNotice(_)
                | Step::TrustDirectory(_) => None,
            })
            .expect("auth step")
    }

    #[tokio::test]
    async fn pooled_only_notice_starts_hidden_auth_and_reveals_it_with_l() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = build_config(&temp_dir).await?;
        let app_server = crate::start_embedded_app_server_for_picker(&config).await?;
        let request_frame = FrameRequester::test_dummy();
        let args = OnboardingScreenArgs {
            show_trust_screen: false,
            show_login_screen: false,
            startup_prompt_decision: StartupPromptDecision::PooledOnlyNotice,
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: Some(app_server.request_handle()),
            config,
        };

        let mut screen = OnboardingScreen::new_with_frame_requester(request_frame, args);
        assert_eq!(step_names(&screen), vec!["welcome", "pooled-only"]);
        assert_snapshot!(
            "pooled_only_notice_screen_initial",
            render_to_string(&screen)
        );

        screen.handle_key_event(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('l'),
            KeyModifiers::NONE,
        ));
        let mut reload_config = false;
        handle_startup_notice_outcome(&mut screen, None, &mut reload_config).await?;

        assert_eq!(step_names(&screen), vec!["welcome", "pooled-only", "auth"]);
        assert!(!reload_config);
        assert_snapshot!(
            "pooled_only_notice_screen_auth_revealed",
            render_to_string(&screen)
        );

        Ok(())
    }

    #[tokio::test]
    async fn pooled_only_notice_enter_dismisses_notice_before_trust_step() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = build_config(&temp_dir).await?;
        let request_frame = FrameRequester::test_dummy();
        let args = OnboardingScreenArgs {
            show_trust_screen: true,
            show_login_screen: false,
            startup_prompt_decision: StartupPromptDecision::PooledOnlyNotice,
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: None,
            config,
        };

        let mut screen = OnboardingScreen::new_with_frame_requester(request_frame, args);
        assert_eq!(step_names(&screen), vec!["welcome", "pooled-only"]);

        screen.handle_key_event(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            KeyModifiers::NONE,
        ));
        let mut reload_config = false;
        handle_startup_notice_outcome(&mut screen, None, &mut reload_config).await?;

        assert_eq!(step_names(&screen), vec!["welcome", "trust"]);
        assert!(!reload_config);

        Ok(())
    }

    #[tokio::test]
    async fn login_success_message_uses_mcodex_runtime_identity() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = build_config(&temp_dir).await?;
        let app_server = crate::start_embedded_app_server_for_picker(&config).await?;
        let request_frame = FrameRequester::test_dummy();
        let args = OnboardingScreenArgs {
            show_trust_screen: false,
            show_login_screen: true,
            startup_prompt_decision: StartupPromptDecision::NeedsLogin,
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: Some(app_server.request_handle()),
            config,
        };

        let mut screen = OnboardingScreen::new_with_frame_requester(request_frame, args);
        *auth_widget_mut(&mut screen).sign_in_state.write().unwrap() =
            SignInState::ChatGptSuccessMessage;

        let rendered = render_to_string(&screen);
        assert!(rendered.contains("grant mcodex"));
        assert!(rendered.contains("mcodex can make mistakes"));
        assert!(!rendered.contains("grant Codex"));
        assert!(!rendered.contains("Codex can make mistakes"));
        assert_snapshot!("needs_login_screen_chatgpt_success_message", rendered);

        Ok(())
    }

    #[tokio::test]
    async fn api_key_success_message_uses_mcodex_runtime_identity() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = build_config(&temp_dir).await?;
        let app_server = crate::start_embedded_app_server_for_picker(&config).await?;
        let request_frame = FrameRequester::test_dummy();
        let args = OnboardingScreenArgs {
            show_trust_screen: false,
            show_login_screen: true,
            startup_prompt_decision: StartupPromptDecision::NeedsLogin,
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: Some(app_server.request_handle()),
            config,
        };

        let mut screen = OnboardingScreen::new_with_frame_requester(request_frame, args);
        *auth_widget_mut(&mut screen).sign_in_state.write().unwrap() =
            SignInState::ApiKeyConfigured;

        let rendered = render_to_string(&screen);
        assert!(rendered.contains("mcodex will use usage-based billing with your API key."));
        assert!(!rendered.contains("Codex will use usage-based billing with your API key."));
        assert_snapshot!("needs_login_screen_api_key_configured", rendered);

        Ok(())
    }

    #[tokio::test]
    async fn pooled_only_notice_hide_failure_still_continues() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = build_config(&temp_dir).await?;
        let request_frame = FrameRequester::test_dummy();
        let args = OnboardingScreenArgs {
            show_trust_screen: false,
            show_login_screen: false,
            startup_prompt_decision: StartupPromptDecision::PooledOnlyNotice,
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: None,
            config,
        };

        let mut screen = OnboardingScreen::new_with_frame_requester(request_frame, args);
        screen.handle_key_event(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('n'),
            KeyModifiers::NONE,
        ));
        let mut reload_config = false;
        handle_startup_notice_outcome(&mut screen, None, &mut reload_config).await?;

        assert_eq!(step_names(&screen), vec!["welcome"]);
        assert!(screen.is_done());
        assert!(!reload_config);

        Ok(())
    }

    #[test]
    fn pooled_paused_resume_failure_keeps_notice_visible_with_inline_error() {
        let mut screen = OnboardingScreen {
            request_frame: FrameRequester::test_dummy(),
            steps: vec![
                Step::Welcome(WelcomeWidget::new(
                    /*is_logged_in*/ false,
                    crate::tui::FrameRequester::test_dummy(),
                    /*animations_enabled*/ false,
                )),
                Step::PooledPausedNotice(PooledAccessNoticeWidget::pooled_paused(
                    /*animations_enabled*/ false,
                )),
            ],
            pending_auth_step: None,
            is_done: false,
            should_exit: false,
        };

        screen.restore_pooled_paused_notice("resume failed".to_string());

        assert_eq!(step_names(&screen), vec!["welcome", "pooled-paused"]);
        assert_snapshot!(
            "pooled_paused_notice_inline_error",
            render_to_string(&screen)
        );
    }
}
