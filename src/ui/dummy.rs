use crate::event::{AppEvent, Event};

pub(crate) struct DummyUI {}

impl super::UI for DummyUI {
    fn new() -> color_eyre::Result<Self> {
        Ok(Self {})
    }

    fn set_log_level(_log_level: log::LevelFilter) {}

    async fn run(
        self,
        _tx: tokio::sync::mpsc::UnboundedSender<Event>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> color_eyre::Result<()> {
        while let Some(event) = rx.recv().await {
            if let Event::App(AppEvent::Done) = event {
                break;
            }
        }
        Ok(())
    }
}
