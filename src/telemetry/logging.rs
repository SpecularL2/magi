use std::{
    env::current_dir,
    path::{Path, PathBuf},
};

use tracing::Level;
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{self, RollingFileAppender, Rotation},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use ansi_term::Colour::{Blue, Cyan, Purple, Red, Yellow};

/// Standard log file name prefix. This will be optionally appended with a timestamp
/// depending on the rotation strategy.
const LOG_FILE_NAME_PREFIX: &str = "magi.log";

/// Default log file rotation strategy. This can be overridden by the `logs_rotation` config.
const DEFAULT_ROTATION: &str = "daily";

/// Configure logging telemetry with a global handler.
pub fn init(
    verbose: bool,
    json_logs: bool,
    logs_dir: Option<String>,
    logs_rotation: Option<String>,
) -> Vec<WorkerGuard> {
    // If a directory is provided, log to file and stdout
    if let Some(dir) = logs_dir {
        let directory = PathBuf::from(dir);
        let rotation =
            get_rotation_strategy(&logs_rotation.unwrap_or_else(|| DEFAULT_ROTATION.into()));
        let appender = Some(get_rolling_file_appender(
            directory,
            rotation,
            LOG_FILE_NAME_PREFIX,
        ));
        return build_subscriber(verbose, json_logs, appender);
    }

    // If no directory is provided, log to stdout only
    build_subscriber(verbose, json_logs, None)
}

/// Subscriber Composer
///
/// Builds a subscriber with multiple layers into a [tracing](https://crates.io/crates/tracing) subscriber
/// and initializes it as the global default. This subscriber will log to stdout and optionally to a file.
pub fn build_subscriber(
    verbose: bool,
    json_logs: bool,
    appender: Option<RollingFileAppender>,
) -> Vec<WorkerGuard> {
    let mut guards = Vec::new();

    // Force the file logger to log at `debug` level
    let file_env_filter = EnvFilter::from("magi=debug,network=debug");
    let stdout_env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(match verbose {
            true => "magi=debug,network=debug".to_owned(),
            false => "magi=info,network=debug".to_owned(),
        })
    });

    let stdout_formatting_layer = AnsiTermLayer { verbose }.with_filter(stdout_env_filter);
    let subscriber = tracing_subscriber::registry();

    match (appender, json_logs) {
        (Some(appender), true) => {
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            guards.push(guard);
            subscriber
                .with(tracing_subscriber::fmt::layer().json())
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(non_blocking)
                        .with_filter(file_env_filter),
                )
                .init();
        }
        (Some(appender), false) => {
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            guards.push(guard);
            subscriber
                .with(stdout_formatting_layer)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(non_blocking)
                        .with_filter(file_env_filter),
                )
                .init();
        }
        (None, true) => {
            tracing_subscriber::fmt().json().init();
        }
        (None, false) => {
            subscriber.with(stdout_formatting_layer).init();
        }
    };

    guards
}

/// The AnsiVisitor
#[derive(Debug)]
pub struct AnsiVisitor;

impl tracing::field::Visit for AnsiVisitor {
    fn record_f64(&mut self, _: &tracing::field::Field, value: f64) {
        println!("{value}")
    }

    fn record_i64(&mut self, _: &tracing::field::Field, value: i64) {
        println!("{value}")
    }

    fn record_u64(&mut self, _: &tracing::field::Field, value: u64) {
        println!("{value}")
    }

    fn record_bool(&mut self, _: &tracing::field::Field, value: bool) {
        println!("{value}")
    }

    fn record_str(&mut self, _: &tracing::field::Field, value: &str) {
        println!("{value}")
    }

    fn record_error(
        &mut self,
        _: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        println!("{value}")
    }

    fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        println!("{value:?}")
    }
}

/// An Ansi Term layer for tracing
#[derive(Debug)]
pub struct AnsiTermLayer {
    verbose: bool,
}

impl<S> Layer<S> for AnsiTermLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Print the timestamp
        let utc: String = chrono::Utc::now().to_rfc2822();
        let strip_len = utc.find(|c| c == '+').unwrap_or(utc.len());
        let formatted_utc = utc[..strip_len].trim_end();
        print!("[{}] ", Cyan.paint(formatted_utc));

        // Print the level prefix
        match *event.metadata().level() {
            Level::ERROR => {
                print!("{}: ", Red.paint("ERROR"));
            }
            Level::WARN => {
                print!("{}: ", Yellow.paint("WARN"));
            }
            Level::INFO => {
                print!("{}: ", Blue.paint("INFO"));
            }
            Level::DEBUG => {
                print!("DEBUG: ");
            }
            Level::TRACE => {
                print!("{}: ", Purple.paint("TRACE"));
            }
        }

        if self.verbose {
            print!("{} ", Purple.paint(event.metadata().target()));

            let original_location = event
                .metadata()
                .name()
                .split(' ')
                .last()
                .unwrap_or_default();

            let relative_path = current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Remove common prefixes from the location and relative path
            let location_path = std::path::Path::new(original_location);
            let relative_path_path = std::path::Path::new(&relative_path);
            let common_prefix = location_path
                .ancestors()
                .collect::<Vec<&Path>>()
                .iter()
                .cloned()
                .rev()
                .zip(
                    relative_path_path
                        .ancestors()
                        .collect::<Vec<&Path>>()
                        .iter()
                        .cloned()
                        .rev(),
                )
                .take_while(|(a, b)| a == b)
                .last()
                .map(|(a, _)| a)
                .unwrap_or_else(|| std::path::Path::new(""));

            let location = location_path
                .strip_prefix(common_prefix)
                .unwrap_or(location_path)
                .to_str()
                .unwrap_or(original_location);

            let location = location.strip_prefix('/').unwrap_or(location);
            print!("at {} ", Cyan.paint(location.to_string()));
        }

        let mut visitor = AnsiVisitor;
        event.record(&mut visitor);
    }
}

/// Get the rotation strategy from the given string.
/// Defaults to rotating daily.
fn get_rotation_strategy(val: &str) -> Rotation {
    match val {
        "never" => Rotation::NEVER,
        "daily" => Rotation::DAILY,
        "hourly" => Rotation::HOURLY,
        "minutely" => Rotation::MINUTELY,
        _ => {
            eprintln!("Invalid log rotation strategy provided. Defaulting to rotating daily.");
            eprintln!("Valid rotation options are: 'never', 'daily', 'hourly', 'minutely'.");
            Rotation::DAILY
        }
    }
}

/// Get a rolling file appender for the given directory, rotation and file name prefix.
fn get_rolling_file_appender(
    directory: PathBuf,
    rotation: Rotation,
    file_name_prefix: &str,
) -> RollingFileAppender {
    match rotation {
        Rotation::NEVER => rolling::never(directory, file_name_prefix),
        Rotation::DAILY => rolling::daily(directory, file_name_prefix),
        Rotation::HOURLY => rolling::hourly(directory, file_name_prefix),
        Rotation::MINUTELY => rolling::minutely(directory, file_name_prefix),
    }
}
