use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use super::app::App;
use super::event::{AppEvent, EventHandler};
use super::ui;
use crate::state::NodeState;

pub async fn run_tui(state: Arc<NodeState>, node_id: String) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(state.clone(), node_id);
    let events = EventHandler::new(Duration::from_millis(250));

    let result = run_app(&mut terminal, &mut app, &events).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    events: &EventHandler,
) -> anyhow::Result<()> {
    while app.running {
        terminal.draw(|f| ui::draw(f, app))?;

        match events.next()? {
            AppEvent::Tick => {
                app.update().await;
            }
            AppEvent::Key(key) => match key.code {
                KeyCode::Char('q') => app.quit(),
                KeyCode::Tab => app.next_tab(),
                KeyCode::BackTab => app.prev_tab(),
                KeyCode::Left => app.prev_tab(),
                KeyCode::Right => app.next_tab(),
                KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                KeyCode::Char('1') => app.current_tab = super::app::Tab::Dashboard,
                KeyCode::Char('2') => app.current_tab = super::app::Tab::Network,
                KeyCode::Char('3') => app.current_tab = super::app::Tab::Validator,
                KeyCode::Char('4') => app.current_tab = super::app::Tab::DAG,
                KeyCode::Char('5') => app.current_tab = super::app::Tab::Logs,
                _ => {}
            },
            AppEvent::Quit => app.quit(),
        }
    }

    Ok(())
}
