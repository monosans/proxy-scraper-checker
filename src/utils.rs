use std::{fmt::Write as _, path::Path};

pub fn pretty_error(e: &crate::Error) -> compact_str::CompactString {
    let mut result = compact_str::CompactString::const_new("");
    for (i, cause) in e.chain().enumerate() {
        if i != 0 {
            result.push_str(" \u{2192} ");
        }
        write!(&mut result, "{cause}").unwrap();
    }
    result
}

// For steps that don't stop for each other, when two of them can fail.
pub fn keep_both_errors(first: crate::Result<()>, second: crate::Result<()>) -> crate::Result<()> {
    match (first, second) {
        (Err(first), Err(second)) => {
            Err(color_eyre::eyre::eyre!("{}; {}", pretty_error(&first), pretty_error(&second)))
        }
        (first, second) => first.and(second),
    }
}

// The tasks never get aborted, so a task that failed is one that panicked.
pub fn log_panic(task: &str, result: Result<(), tokio::task::JoinError>) {
    if let Err(e) = result {
        tracing::error!("{task} panicked: {}", pretty_error(&e.into()));
    }
}

// What a path in the container is, for someone looking from outside of it.
pub fn seen_from_outside(
    outside: &str,
    path: &Path,
    in_container: bool,
) -> compact_str::CompactString {
    if in_container {
        compact_str::format_compact!("{outside} ({} in container)", path.display())
    } else {
        compact_str::format_compact!("{}", path.display())
    }
}

pub fn grouped(number: usize) -> compact_str::CompactString {
    let mut buffer = itoa::Buffer::new();
    let digits = buffer.format(number);
    let mut grouped = compact_str::CompactString::default();
    for (remaining, digit) in (1..=digits.len()).rev().zip(digits.chars()) {
        grouped.push(digit);
        if remaining != 1 && remaining.saturating_sub(1).is_multiple_of(3) {
            grouped.push(',');
        }
    }
    grouped
}

pub const fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

pub fn clock(seconds: u64) -> compact_str::CompactString {
    let minutes = seconds.div_euclid(60);
    let hours = minutes.div_euclid(60);
    if hours == 0 {
        compact_str::format_compact!("{minutes}:{:02}", seconds.rem_euclid(60))
    } else {
        compact_str::format_compact!(
            "{hours}:{:02}:{:02}",
            minutes.rem_euclid(60),
            seconds.rem_euclid(60)
        )
    }
}

pub async fn is_container() -> bool {
    #[cfg(target_os = "linux")]
    {
        static CACHE: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();

        *CACHE
            .get_or_init(async || {
                tokio::fs::try_exists("/.dockerenv").await.unwrap_or(false)
                    || tokio::fs::try_exists("/run/.containerenv").await.unwrap_or(false)
            })
            .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
pub mod tests {
    use std::{io, path::Path, sync::Arc};

    use color_eyre::eyre::eyre;

    use super::{clock, grouped, is_container, keep_both_errors, log_panic, seen_from_outside};

    struct Writer(Arc<parking_lot::Mutex<Vec<u8>>>);

    #[expect(clippy::missing_trait_methods, reason = "the provided methods all go through write")]
    impl io::Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // What the code under test logs on this thread, debug messages included,
    // for as long as this is alive. Tasks of a current-thread runtime run on
    // it too; blocking ones do not.
    pub struct Logs {
        written: Arc<parking_lot::Mutex<Vec<u8>>>,
        _guard: tracing::dispatcher::DefaultGuard,
    }

    impl Logs {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.written.lock()).into_owned()
        }
    }

    // A callsite that no subscriber wants is skipped for good, and one that is
    // first reached while no test is capturing would be. So something always
    // wants them all, and then turns them away unless a test captures them.
    pub fn listen_everywhere() {
        static LISTENING: std::sync::Once = std::sync::Once::new();
        LISTENING.call_once(|| {
            let subscriber = tracing_subscriber::layer::SubscriberExt::with(
                tracing_subscriber::registry(),
                tracing_subscriber::filter::filter_fn(|_| false),
            );
            tracing::subscriber::set_global_default(subscriber).unwrap();
        });
    }

    pub fn capture_logs() -> Logs {
        listen_everywhere();
        let written = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let shared = Arc::clone(&written);
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || Writer(Arc::clone(&shared)))
            .finish();
        Logs { written, _guard: tracing::subscriber::set_default(subscriber) }
    }

    #[test]
    fn captured_logs_keep_everything_written() {
        use io::Write as _;

        let logs = capture_logs();
        let mut writer = Writer(Arc::clone(&logs.written));
        writer.write_all(b"first ").unwrap();
        writer.flush().unwrap();
        tracing::debug!("second");
        let text = logs.text();
        assert!(text.starts_with("first "), "{text}");
        assert!(text.contains("DEBUG"), "{text}");
        assert!(text.ends_with("second\n"), "{text}");
    }

    #[tokio::test]
    async fn a_panicked_task_is_logged_rather_than_passed_on() {
        let logs = capture_logs();

        let panicked = tokio::spawn(async {
            std::hint::black_box(None::<()>).unwrap();
        });
        log_panic("Test task", panicked.await);
        log_panic("Finished task", tokio::spawn(async {}).await);

        let text = logs.text();
        assert!(text.contains("ERROR"), "{text}");
        assert!(text.contains("Test task panicked: task "), "{text}");
        assert!(!text.contains("Finished task"), "{text}");
    }

    #[test]
    fn a_path_in_a_container_says_where_it_is_outside() {
        let path = Path::new("/home/app/out");
        assert_eq!(seen_from_outside("./out", path, false), "/home/app/out");
        assert_eq!(seen_from_outside("./out", path, true), "./out (/home/app/out in container)");
    }

    #[tokio::test]
    async fn the_container_check_gives_the_same_answer_every_time() {
        let first = is_container().await;
        assert_eq!(is_container().await, first);
    }

    #[test]
    fn both_failures_are_kept() {
        let error = keep_both_errors(Err(eyre!("first")), Err(eyre!("second")));
        assert_eq!(error.unwrap_err().to_string(), "first; second");
        drop(keep_both_errors(Ok(()), Err(eyre!("second"))).unwrap_err());
        drop(keep_both_errors(Err(eyre!("first")), Ok(())).unwrap_err());
        keep_both_errors(Ok(()), Ok(())).unwrap();
    }

    #[test]
    fn numbers_read_in_groups_of_three() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(12_480), "12,480");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn durations_grow_an_hours_field_only_when_needed() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(134), "2:14");
        assert_eq!(clock(3_725), "1:02:05");
    }
}
