use crate::event::Event;

pub(crate) struct LoggerUI {}

impl super::UI for LoggerUI {
    fn new() -> color_eyre::Result<Self> {
        env_logger::Builder::new().filter_level(log::LevelFilter::Debug).init();
        Ok(Self {})
    }

    fn set_log_level(log_level: log::LevelFilter) {
        log::set_max_level(log_level);
    }

    async fn run(
        self,
        _tx: tokio::sync::mpsc::UnboundedSender<Event>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> color_eyre::Result<()> {
        while rx.recv().await.is_some() {}
        Ok(())
    }
}
