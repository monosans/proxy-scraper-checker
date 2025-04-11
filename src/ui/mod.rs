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

cfg_if::cfg_if! {
    if #[cfg(all(feature="tui", feature="logger"))] {
        compile_error!("feature \"logger\" and feature \"tui\" cannot be enaled at the same time");
    } else if #[cfg(feature="tui" )] {
        mod tui;
        pub(crate) use self::tui::Tui as UIImpl;
    } else if #[cfg(feature = "logger")] {
        mod logger;
        pub(crate) use self::logger::LoggerUI as UIImpl;
    } else {
        mod dummy;
        pub(crate) use self::dummy::DummyUI as UIImpl;
    }
}
