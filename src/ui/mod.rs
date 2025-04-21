use crate::event::Event;

pub(crate) trait UI {
    fn new() -> color_eyre::Result<Self>
    where
        Self: std::marker::Sized;

    fn set_log_level(log_level: log::LevelFilter);

    async fn run(
        self,
        tx: tokio::sync::mpsc::UnboundedSender<Event>,
        rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> color_eyre::Result<()>;
}

#[cfg(feature = "tui")]
mod tui;
#[cfg(feature = "tui")]
pub(crate) use self::tui::Tui as UIImpl;

#[cfg(not(feature = "tui"))]
mod logger;
#[cfg(not(feature = "tui"))]
pub(crate) use self::logger::LoggerUI as UIImpl;
