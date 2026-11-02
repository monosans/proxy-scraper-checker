//! Runs the program the way people do: from a config file, to the end.

#![deny(
    warnings,
    ambiguous_negative_literals,
    dead_code_pub_in_binary,
    deprecated_in_future,
    deprecated_safe,
    deref_into_dyn_supertrait,
    ffi_unwind_calls,
    future_incompatible,
    impl_trait_redundant_captures,
    keyword_idents,
    let_underscore,
    macro_use_extern_crate,
    meta_variable_misuse,
    missing_copy_implementations,
    missing_docs,
    non_ascii_idents,
    nonstandard_style,
    raw_borrows_via_references,
    redundant_imports,
    redundant_lifetimes,
    refining_impl_trait,
    rust_2018_compatibility,
    rust_2018_idioms,
    rust_2021_compatibility,
    rust_2024_compatibility,
    single_use_lifetimes,
    trivial_casts,
    trivial_numeric_casts,
    unit_bindings,
    unsafe_code,
    unstable_features,
    unused,
    unused_import_braces,
    unused_lifetimes,
    unused_qualifications,
    variant_size_differences,
    clippy::all,
    clippy::pedantic,
    clippy::restriction,
    clippy::nursery,
    clippy::cargo
)]
#![allow(
    clippy::absolute_paths,
    clippy::allow_attributes_without_reason,
    clippy::arbitrary_source_item_ordering,
    clippy::as_conversions,
    clippy::blanket_clippy_restriction_lints,
    clippy::cast_precision_loss,
    clippy::cognitive_complexity,
    clippy::else_if_without_else,
    clippy::float_arithmetic,
    clippy::implicit_return,
    clippy::integer_division_remainder_used,
    clippy::iter_over_hash_type,
    clippy::min_ident_chars,
    clippy::missing_docs_in_private_items,
    clippy::mod_module_files,
    clippy::multiple_crate_versions,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::separated_literal_suffix,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    clippy::single_char_lifetime_names,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core,
    clippy::too_many_lines,
    clippy::unwrap_used
)]
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test is a crate of tests"
)]

use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

const PROGRAM: &str = env!("CARGO_BIN_EXE_proxy-scraper-checker");
const CONFIG_ENV: &str = "PROXY_SCRAPER_CHECKER_CONFIG";

struct Run {
    directory: PathBuf,
    config: PathBuf,
}

fn directory_for(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "proxy-scraper-checker-cli-{name}-{}",
        std::process::id()
    ))
}

impl Run {
    // A run in a directory of its own that scrapes `source` for HTTP proxies
    // and saves them to `output` without checking them.
    fn new(name: &str, source: &str, output: &str) -> Self {
        let directory = directory_for(name);
        drop(std::fs::remove_dir_all(&directory));
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.toml");
        std::fs::write(
            &config,
            format!(
                r#"
debug = false

[output]
path = '{output}'
sort_by_speed = false

[output.txt]
enabled = true

[output.json]
enabled = false
include_asn = false
include_geolocation = false

[checking]
check_url = ""
max_concurrent_checks = 8
timeout = 60.0
connect_timeout = 60.0
user_agent = "proxy-scraper-checker tests"

[scraping]
max_proxies_per_source = 0
timeout = 60.0
connect_timeout = 60.0
proxy = ""
user_agent = "proxy-scraper-checker tests"

[scraping.http]
enabled = true
urls = ['{source}']

[scraping.socks4]
enabled = false
urls = []

[scraping.socks5]
enabled = false
urls = []
"#
            ),
        )
        .unwrap();
        Self { directory, config }
    }

    // A source with one proxy, saved into the run's directory.
    fn with_one_proxy(name: &str) -> Self {
        let directory = directory_for(name);
        let run = Self::new(
            name,
            directory.join("source.txt").to_str().unwrap(),
            directory.join("out").to_str().unwrap(),
        );
        std::fs::write(run.directory.join("source.txt"), "1.1.1.1:80").unwrap();
        run
    }

    fn command(&self) -> Command {
        let mut command = Command::new(PROGRAM);
        command
            .env(CONFIG_ENV, &self.config)
            .current_dir(&self.directory)
            .stdin(Stdio::null());
        command
    }

    fn saved(&self, file: &str) -> String {
        std::fs::read_to_string(self.directory.join("out/proxies").join(file))
            .unwrap()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.directory));
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn succeeded(output: &Output) -> String {
    let stdout = text(&output.stdout);
    assert!(
        output.status.success(),
        "{}\n{stdout}\n{}",
        output.status,
        text(&output.stderr)
    );
    stdout
}

fn failed(output: &Output) -> String {
    let stderr = text(&output.stderr);
    assert!(!output.status.success(), "{}\n{stderr}", output.status);
    stderr
}

#[test]
fn a_run_saves_the_proxies_it_found_and_says_where() {
    let run = Run::with_one_proxy("saves");

    let stdout = succeeded(&run.command().output().unwrap());

    let out = std::path::absolute(run.directory.join("out")).unwrap();
    assert!(
        stdout.contains(&format!("Saved 1 proxy to {}", out.display())),
        "{stdout}"
    );
    assert!(stdout.contains("Thank you for using"), "{stdout}");
    assert_eq!(run.saved("all.txt"), "http://1.1.1.1:80");
    assert_eq!(run.saved("http.txt"), "1.1.1.1:80");
}

#[test]
fn an_empty_output_path_means_the_working_directory() {
    let run = Run::new("working-directory", "source.txt", "");
    std::fs::write(run.directory.join("source.txt"), "1.1.1.1:80").unwrap();

    let stdout = succeeded(&run.command().output().unwrap());

    assert!(stdout.contains("Saved 1 proxy to "), "{stdout}");
    let saved = run.directory.join("proxies/all.txt");
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "http://1.1.1.1:80");
}

#[test]
fn a_config_that_is_missing_or_wrong_is_reported() {
    let run = Run::with_one_proxy("bad-config");
    let missing = run.directory.join("missing.toml");

    let mut command = run.command();
    let stderr = failed(&command.env(CONFIG_ENV, &missing).output().unwrap());
    assert!(stderr.contains("failed to read file to string"), "{stderr}");

    let config = std::fs::read_to_string(&run.config).unwrap();
    std::fs::write(
        &run.config,
        config.replace("enabled = true", "enabled = false"),
    )
    .unwrap();
    let stderr = failed(&run.command().output().unwrap());
    assert!(
        stderr.contains("at least one of 'output.json' or 'output.txt'"),
        "{stderr}"
    );
}

#[test]
fn without_a_config_file_the_one_in_the_working_directory_is_used() {
    let run = Run::with_one_proxy("default-config");
    let mut command = run.command();
    command.env_remove(CONFIG_ENV);

    succeeded(&command.output().unwrap());

    assert_eq!(run.saved("all.txt"), "http://1.1.1.1:80");
}

// Under umask 277 the program can create directories, but nothing in them.
#[cfg(unix)]
fn without_write_access(run: &Run) -> Command {
    let mut command = Command::new("sh");
    command
        .args(["-c", "umask 277 && exec \"$0\"", PROGRAM])
        .env(CONFIG_ENV, &run.config)
        .current_dir(&run.directory)
        .stdin(Stdio::null());
    command
}

#[cfg(unix)]
#[test]
fn a_save_that_fails_fails_the_run_and_says_why() {
    let run = Run::with_one_proxy("save-fails");
    std::fs::create_dir_all(run.directory.join("out")).unwrap();

    let stderr = failed(&without_write_access(&run).output().unwrap());

    assert!(stderr.contains("failed to create file: "), "{stderr}");
    assert!(!run.directory.join("out/proxies").exists());
}

// Waits for the program to get as far as `line`, and returns everything it
// printed until then.
#[cfg(unix)]
fn wait_for(lines: &std::sync::mpsc::Receiver<String>, line: &str) -> String {
    let mut printed = String::new();
    while !printed.contains(line) {
        let next = lines
            .recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|e| format!("{e}: {printed}"))
            .unwrap();
        printed.push_str(&next);
        printed.push('\n');
    }
    printed
}

#[cfg(unix)]
#[test]
fn a_stop_signal_ends_the_run_with_what_it_has() {
    use std::io::BufRead as _;

    // A source that never answers, so the run would wait for a minute.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let source = std::thread::spawn(move || listener.accept().unwrap().0);
    let run = Run::new("stop-signal", &format!("http://{address}/"), "out");

    let mut child = run.command().stdout(Stdio::piped()).spawn().unwrap();
    let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let (tx, lines) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in stdout.lines() {
            tx.send(line.unwrap()).unwrap();
        }
    });

    // By then, it listens for the signal.
    wait_for(&lines, "Scraping 1 source");
    let source = source.join().unwrap();
    let kill = Command::new("sh")
        .args(["-c", "kill -TERM \"$0\"", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let status = child.wait().unwrap();
    reader.join().unwrap();
    drop(source);

    let printed: Vec<_> = lines.try_iter().collect();
    let printed = printed.join("\n");
    assert!(status.success(), "{status}\n{printed}");
    assert!(
        printed.contains("Received SIGTERM signal, exiting..."),
        "{printed}"
    );
    assert!(printed.contains("No proxies to save"), "{printed}");
}

// The program with a terminal of its own, as it has in a shell.
#[cfg(target_os = "linux")]
struct OnTerminal {
    child: std::process::Child,
    printed: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

// A test that fails must not leave the program behind.
#[cfg(target_os = "linux")]
impl Drop for OnTerminal {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

#[cfg(target_os = "linux")]
impl OnTerminal {
    // `script` runs `shell` on a terminal, which it copies its own standard
    // input to and the terminal's output from.
    fn start(run: &Run, shell: &str) -> Self {
        use std::io::Read as _;

        let mut child = Command::new("script")
            .args([
                "--quiet",
                "--return",
                "--command",
                &format!("stty cols 100 rows 30 && {shell}"),
                "/dev/null",
            ])
            .env(CONFIG_ENV, &run.config)
            .env("PROGRAM", PROGRAM)
            // Set but empty, which leaves the colors on.
            .env("NO_COLOR", "")
            .current_dir(&run.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let printed: std::sync::Arc<std::sync::Mutex<Vec<u8>>> =
            std::sync::Arc::default();
        let shared = std::sync::Arc::clone(&printed);
        let reader = std::thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            loop {
                let read = stdout.read(&mut chunk).unwrap();
                if read == 0 {
                    break;
                }
                let mut printed = shared.lock().unwrap();
                printed.extend_from_slice(chunk.get(..read).unwrap());
            }
        });
        Self { child, printed, reader: Some(reader) }
    }

    fn printed(&self) -> String {
        text(&self.printed.lock().unwrap())
    }

    #[cfg(feature = "tui")]
    fn wait_for(&self, shown: &str) {
        let started = std::time::Instant::now();
        while !self.printed().contains(shown) {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(30),
                "{}",
                self.printed()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[cfg(feature = "tui")]
    fn press(&mut self, keys: &str) {
        use std::io::Write as _;

        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(keys.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    fn finish(mut self) -> (std::process::ExitStatus, String) {
        let status = self.child.wait().unwrap();
        self.reader.take().unwrap().join().unwrap();
        (status, self.printed())
    }
}

#[cfg(all(target_os = "linux", feature = "tui"))]
#[test]
fn the_tui_shows_the_run_and_leaves_its_summary_behind() {
    let run = Run::with_one_proxy("tui");
    let mut terminal = OnTerminal::start(&run, "exec \"$PROGRAM\"");

    // The TUI only redraws what changed, so the words come one by one.
    terminal.wait_for("Finished");
    terminal.press("q");
    let (status, printed) = terminal.finish();

    assert!(status.success(), "{status}\n{printed}");
    // The last thing on the screen once the TUI is gone.
    let summary = printed.lines().rev().find(|line| !line.trim().is_empty());
    assert!(
        summary.is_some_and(|line| line.contains("Saved 1 proxy to ")),
        "{printed}"
    );
    assert_eq!(run.saved("all.txt"), "http://1.1.1.1:80");
}

#[cfg(all(target_os = "linux", feature = "tui"))]
#[test]
fn a_save_that_fails_closes_the_tui_and_says_why() {
    let run = Run::with_one_proxy("tui-save-fails");
    std::fs::create_dir_all(run.directory.join("out")).unwrap();
    let terminal = OnTerminal::start(&run, "umask 277 && exec \"$PROGRAM\"");

    let (status, printed) = terminal.finish();

    assert!(!status.success(), "{status}\n{printed}");
    assert!(printed.contains("failed to create file: "), "{printed}");
}

#[cfg(target_os = "linux")]
#[test]
fn logs_on_a_terminal_are_in_color() {
    let run = Run::with_one_proxy("colors");
    let terminal = OnTerminal::start(&run, "exec \"$PROGRAM\" < /dev/null");

    let (status, printed) = terminal.finish();

    assert!(status.success(), "{status}\n{printed}");
    assert!(printed.contains("\u{1b}["), "{printed}");
    assert!(printed.contains("Saved 1 proxy to "), "{printed}");
}
